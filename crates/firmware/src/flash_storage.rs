//! 片上非易失 Flash 配置存储管理器 (RP2350 On-Chip Flash NV Storage)
//!
//! 在 4MB Flash 最高扇区 (0x003F_F000) 划分存储空间，
//! 支持 4 个独立配置 Profile 槽位与开机自启动槽位指定，完整持久化 12 通道配置与 LabelFilter。
//! 采用直接内存映射读取 (Zero Overhead Read) 与 RP2350 BootROM RAM 函数写入，
//! 彻底规避 embassy-rp 0.10.0 HAL BOOTRAM_BASE 崩溃与内存未对齐隐患。

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

use arinc429_core::protocol::{ProfileRecord, PROFILE_MAGIC, PROFILE_RECORD_LEN};

pub static FLASH_MGR: Mutex<CriticalSectionRawMutex, Option<FlashProfileManager>> =
    Mutex::new(None);

pub const FLASH_BASE: u32 = 0x1000_0000;
pub const FLASH_CAPACITY: usize = 4 * 1024 * 1024;
pub const SECTOR_SIZE: usize = 4096;
pub const PROFILE_SECTOR_OFFSET: u32 = (FLASH_CAPACITY - SECTOR_SIZE) as u32;
pub const MAX_PROFILE_SLOTS: usize = 4;
pub const SECTOR_MAGIC: u32 = 0x5345_4354; // "SECT"

/// 存储扇区已使用总长度：4 个 512 字节 Profile 记录 (2048B) + 1 个 256 字节元数据页 (256B) = 2304 字节 (9 个 256B 页)
pub const SECTOR_STORAGE_LEN: usize = (MAX_PROFILE_SLOTS * PROFILE_RECORD_LEN) + 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashError {
    InvalidSlot(u8),
    EmptySlot,
    CrcMismatch,
}

/// RP2350 BootROM 内部常驻 Flash 驱动函数指针表 (零代码段搬运、零 Flash 占用)
#[repr(C)]
struct BootRomPointers {
    connect_internal_flash: unsafe extern "C" fn() -> (),
    flash_exit_xip: unsafe extern "C" fn() -> (),
    flash_range_erase: unsafe extern "C" fn(addr: u32, count: usize, block_size: u32, block_cmd: u8) -> (),
    flash_range_program: unsafe extern "C" fn(addr: u32, data: *const u8, count: usize) -> (),
    flash_flush_cache: unsafe extern "C" fn() -> (),
    flash_enter_cmd_xip: unsafe extern "C" fn() -> (),
}

impl BootRomPointers {
    #[inline(always)]
    fn get() -> Self {
        Self {
            connect_internal_flash: embassy_rp::rom_data::connect_internal_flash::ptr(),
            flash_exit_xip: embassy_rp::rom_data::flash_exit_xip::ptr(),
            flash_range_erase: embassy_rp::rom_data::flash_range_erase::ptr(),
            flash_range_program: embassy_rp::rom_data::flash_range_program::ptr(),
            flash_flush_cache: embassy_rp::rom_data::flash_flush_cache::ptr(),
            flash_enter_cmd_xip: embassy_rp::rom_data::flash_enter_cmd_xip::ptr(),
        }
    }
}

/// 放置在 SRAM (.data.ram_func) 中的安全 Flash 擦写底层执行函数
/// 保证在 XIP 关闭期间，指令完全从 SRAM 读取，绝不触发 Flash 指令取指
#[inline(never)]
#[unsafe(link_section = ".data.ram_func")]
unsafe fn write_flash_inner_rom(
    ptrs: &BootRomPointers,
    offset: u32,
    data: *const u8,
    data_len: usize,
) {
    (ptrs.connect_internal_flash)();
    (ptrs.flash_exit_xip)();
    (ptrs.flash_range_erase)(offset, SECTOR_SIZE, 1 << 31, 0);
    (ptrs.flash_range_program)(offset, data, data_len);
    (ptrs.flash_flush_cache)();
    (ptrs.flash_enter_cmd_xip)();
}

pub struct FlashProfileManager;

impl FlashProfileManager {
    pub const fn new() -> Self {
        Self
    }

    /// 零开销内存映射读取扇区存储缓冲 (安全、非侵入、免关中断)
    pub fn read_storage(&self, buf: &mut [u8; SECTOR_STORAGE_LEN]) {
        let flash_ptr = (FLASH_BASE + PROFILE_SECTOR_OFFSET) as *const u8;
        unsafe {
            core::ptr::copy_nonoverlapping(flash_ptr, buf.as_mut_ptr(), SECTOR_STORAGE_LEN);
        }
    }

    /// 关全局中断并在 SRAM 中安全执行 4KB 扇区擦除与多页写入
    pub fn write_storage(&mut self, buf: &[u8; SECTOR_STORAGE_LEN]) -> Result<(), FlashError> {
        let ptrs = BootRomPointers::get();
        cortex_m::interrupt::free(|_| {
            unsafe {
                write_flash_inner_rom(&ptrs, PROFILE_SECTOR_OFFSET, buf.as_ptr(), SECTOR_STORAGE_LEN);
            }
        });
        Ok(())
    }

    /// 零拷贝直接从 Flash 内存映射区读取单槽位 ProfileRecord (仅消耗 512B 局部栈空间)
    pub fn load_profile(&mut self, slot: u8) -> Result<ProfileRecord, FlashError> {
        if slot as usize >= MAX_PROFILE_SLOTS {
            return Err(FlashError::InvalidSlot(slot));
        }

        let offset = (slot as usize) * PROFILE_RECORD_LEN;
        let flash_ptr = (FLASH_BASE + PROFILE_SECTOR_OFFSET + offset as u32) as *const u8;
        let mut rec_bytes = [0u8; PROFILE_RECORD_LEN];
        unsafe {
            core::ptr::copy_nonoverlapping(flash_ptr, rec_bytes.as_mut_ptr(), PROFILE_RECORD_LEN);
        }
        let record = ProfileRecord::from_bytes(&rec_bytes);

        if !record.verify() {
            return Err(FlashError::EmptySlot);
        }
        Ok(record)
    }

    /// 完整保存包含 12 通道方向、速率与 LabelFilter 规则的 Profile
    pub fn save_profile(
        &mut self,
        slot: u8,
        name: &[u8; 32],
        directions: &[u8; 12],
        speeds: &[u8; 12],
        filter_modes: &[u8; 12],
        filter_bitmaps: &[[u32; 8]; 12],
    ) -> Result<(), FlashError> {
        if slot as usize >= MAX_PROFILE_SLOTS {
            return Err(FlashError::InvalidSlot(slot));
        }

        static mut WRITE_BUF: [u8; SECTOR_STORAGE_LEN] = [0u8; SECTOR_STORAGE_LEN];
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(WRITE_BUF) };
        self.read_storage(buf);

        let mut rec = ProfileRecord {
            magic: PROFILE_MAGIC,
            slot,
            is_valid: 1,
            name: *name,
            directions: *directions,
            speeds: *speeds,
            filter_modes: *filter_modes,
            filter_bitmaps: *filter_bitmaps,
            crc16: 0,
        };

        rec.crc16 = rec.compute_crc();

        let offset = (slot as usize) * PROFILE_RECORD_LEN;
        buf[offset..offset + PROFILE_RECORD_LEN].copy_from_slice(&rec.to_bytes());

        self.write_storage(buf)?;
        Ok(())
    }

    pub fn set_boot_slot(&mut self, slot: u8) -> Result<(), FlashError> {
        if slot as usize >= MAX_PROFILE_SLOTS {
            return Err(FlashError::InvalidSlot(slot));
        }

        // 严格前置校验：必须确认目标槽位包含有效配置且通过 CRC16 校验，严禁将空槽位设为自启
        self.load_profile(slot)?;

        static mut WRITE_BUF: [u8; SECTOR_STORAGE_LEN] = [0u8; SECTOR_STORAGE_LEN];
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(WRITE_BUF) };
        self.read_storage(buf);

        let offset = MAX_PROFILE_SLOTS * PROFILE_RECORD_LEN;
        buf[offset..offset + 4].copy_from_slice(&SECTOR_MAGIC.to_le_bytes());
        buf[offset + 4] = slot;

        self.write_storage(buf)?;
        Ok(())
    }

    /// 清空开机自启动设置 (重置为上电默认静默基态)
    pub fn clear_boot_slot(&mut self) -> Result<(), FlashError> {
        static mut WRITE_BUF: [u8; SECTOR_STORAGE_LEN] = [0u8; SECTOR_STORAGE_LEN];
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(WRITE_BUF) };
        self.read_storage(buf);

        let offset = MAX_PROFILE_SLOTS * PROFILE_RECORD_LEN;
        buf[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes());
        buf[offset + 4] = 0xFF;

        self.write_storage(buf)?;
        Ok(())
    }

    /// 清空指定槽位的配置 Profile (将其完全归零并清除 CRC)
    pub fn clear_slot(&mut self, slot: u8) -> Result<(), FlashError> {
        if slot as usize >= MAX_PROFILE_SLOTS {
            return Err(FlashError::InvalidSlot(slot));
        }

        static mut WRITE_BUF: [u8; SECTOR_STORAGE_LEN] = [0u8; SECTOR_STORAGE_LEN];
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(WRITE_BUF) };
        self.read_storage(buf);

        let offset = (slot as usize) * PROFILE_RECORD_LEN;
        buf[offset..offset + PROFILE_RECORD_LEN].fill(0);

        // 若当前自启动槽位恰为此槽位，同步清除自启动
        let boot_offset = MAX_PROFILE_SLOTS * PROFILE_RECORD_LEN;
        let boot_magic = u32::from_le_bytes([
            buf[boot_offset],
            buf[boot_offset + 1],
            buf[boot_offset + 2],
            buf[boot_offset + 3],
        ]);
        if boot_magic == SECTOR_MAGIC && buf[boot_offset + 4] == slot {
            buf[boot_offset..boot_offset + 4].fill(0);
            buf[boot_offset + 4] = 0xFF;
        }

        self.write_storage(buf)?;
        Ok(())
    }

    /// 清空所有 4 个槽位与自启动元数据 (重置整个存储扇区为全 0 静默基态)
    pub fn clear_all(&mut self) -> Result<(), FlashError> {
        static mut WRITE_BUF: [u8; SECTOR_STORAGE_LEN] = [0u8; SECTOR_STORAGE_LEN];
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(WRITE_BUF) };
        buf.fill(0);
        let boot_offset = MAX_PROFILE_SLOTS * PROFILE_RECORD_LEN;
        buf[boot_offset + 4] = 0xFF;

        self.write_storage(buf)?;
        Ok(())
    }

    /// 仅读取 5 字节自启动元数据，零额外栈开销
    pub fn get_boot_slot(&self) -> Option<u8> {
        let offset = MAX_PROFILE_SLOTS * PROFILE_RECORD_LEN;
        let flash_ptr = (FLASH_BASE + PROFILE_SECTOR_OFFSET + offset as u32) as *const u8;
        let mut meta = [0u8; 5];
        unsafe {
            core::ptr::copy_nonoverlapping(flash_ptr, meta.as_mut_ptr(), 5);
        }
        let magic = u32::from_le_bytes([meta[0], meta[1], meta[2], meta[3]]);
        let slot = meta[4];
        if magic == SECTOR_MAGIC && (slot as usize) < MAX_PROFILE_SLOTS {
            Some(slot)
        } else {
            None
        }
    }
}
