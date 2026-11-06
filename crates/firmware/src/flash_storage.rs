//! 片上非易失 Flash 配置存储管理器 (RP2350 On-Chip Flash NV Storage)
//!
//! 在 4MB Flash 最高扇区 (0x003F_F000) 划分 256 字节紧凑存储空间，
//! 支持 4 个独立配置 Profile 槽位与开机自启动槽位指定，具备 CRC16 校验与防损机制。
//! 采用直接内存映射读取 (Zero Overhead Read) 与 RP2350 BootROM RAM 函数写入，
//! 彻底规避 embassy-rp 0.10.0 HAL BOOTRAM_BASE 崩溃与内存未对齐隐患。

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

use arinc429_core::protocol::crc16_ccitt;

pub static FLASH_MGR: Mutex<CriticalSectionRawMutex, Option<FlashProfileManager>> =
    Mutex::new(None);

pub const FLASH_BASE: u32 = 0x1000_0000;
pub const FLASH_CAPACITY: usize = 4 * 1024 * 1024;
pub const SECTOR_SIZE: usize = 4096;
pub const PROFILE_SECTOR_OFFSET: u32 = (FLASH_CAPACITY - SECTOR_SIZE) as u32;
pub const MAX_PROFILE_SLOTS: usize = 4;
pub const SECTOR_PAGE_SIZE: usize = 256;
pub const PROFILE_MAGIC: u16 = 0x4239; // "B9"
pub const SECTOR_MAGIC: u32 = 0x5345_4354; // "SECT"
pub const PROFILE_RECORD_LEN: usize = 62;

/// 单槽位配置记录 (62 字节确定性紧凑结构)
#[derive(Clone, Copy)]
pub struct ProfileRecord {
    pub magic: u16,
    pub slot: u8,
    pub is_valid: u8,
    pub name: [u8; 32],
    pub directions: [u8; 12], // 0: Disabled, 1: Tx, 2: Rx
    pub speeds: [u8; 12],     // 0: Low (12.5k), 1: High (100k)
    pub crc16: u16,
}

impl Default for ProfileRecord {
    fn default() -> Self {
        Self {
            magic: PROFILE_MAGIC,
            slot: 0,
            is_valid: 0,
            name: [0; 32],
            directions: [0; 12], // 默认全部静默 Disabled
            speeds: [1; 12],     // 默认 HIGH (100k)
            crc16: 0,
        }
    }
}

impl ProfileRecord {
    /// 序列化为 62 字节确定性小端序字节数组 (安全零内存风险)
    pub fn to_bytes(&self) -> [u8; PROFILE_RECORD_LEN] {
        let mut buf = [0u8; PROFILE_RECORD_LEN];
        buf[0..2].copy_from_slice(&self.magic.to_le_bytes());
        buf[2] = self.slot;
        buf[3] = self.is_valid;
        buf[4..36].copy_from_slice(&self.name);
        buf[36..48].copy_from_slice(&self.directions);
        buf[48..60].copy_from_slice(&self.speeds);
        buf[60..62].copy_from_slice(&self.crc16.to_le_bytes());
        buf
    }

    /// 从 62 字节切片反序列化
    pub fn from_bytes(buf: &[u8; PROFILE_RECORD_LEN]) -> Self {
        let magic = u16::from_le_bytes([buf[0], buf[1]]);
        let slot = buf[2];
        let is_valid = buf[3];
        let mut name = [0u8; 32];
        name.copy_from_slice(&buf[4..36]);
        let mut directions = [0u8; 12];
        directions.copy_from_slice(&buf[36..48]);
        let mut speeds = [0u8; 12];
        speeds.copy_from_slice(&buf[48..60]);
        let crc16 = u16::from_le_bytes([buf[60], buf[61]]);
        Self {
            magic,
            slot,
            is_valid,
            name,
            directions,
            speeds,
            crc16,
        }
    }

    pub fn compute_crc(&self) -> u16 {
        let bytes = self.to_bytes();
        crc16_ccitt(&bytes[..PROFILE_RECORD_LEN - 2])
    }

    pub fn verify(&self) -> bool {
        self.magic == PROFILE_MAGIC && self.is_valid == 1 && self.compute_crc() == self.crc16
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashError {
    HalError,
    InvalidSlot(u8),
    EmptySlot,
}

/// RP2350 BootROM 函数指针表结构体
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
    pub fn new() -> Self {
        Self
    }

    /// 零开销内存映射读取 256 字节扇区缓冲页 (安全、非侵入、免关中断)
    pub fn read_page(&self, buf: &mut [u8; SECTOR_PAGE_SIZE]) {
        let flash_ptr = (FLASH_BASE + PROFILE_SECTOR_OFFSET) as *const u8;
        unsafe {
            core::ptr::copy_nonoverlapping(flash_ptr, buf.as_mut_ptr(), SECTOR_PAGE_SIZE);
        }
    }

    /// 关全局中断并在 SRAM 中安全执行 4KB 扇区擦除与 256 字节页写入
    pub fn write_page(&mut self, buf: &[u8; SECTOR_PAGE_SIZE]) -> Result<(), FlashError> {
        let ptrs = BootRomPointers::get();
        cortex_m::interrupt::free(|_| {
            unsafe {
                write_flash_inner_rom(&ptrs, PROFILE_SECTOR_OFFSET, buf.as_ptr(), SECTOR_PAGE_SIZE);
            }
        });
        Ok(())
    }

    pub fn load_profile(&mut self, slot: u8) -> Result<ProfileRecord, FlashError> {
        if slot as usize >= MAX_PROFILE_SLOTS {
            return Err(FlashError::InvalidSlot(slot));
        }

        let mut buf = [0u8; SECTOR_PAGE_SIZE];
        self.read_page(&mut buf);

        let offset = (slot as usize) * PROFILE_RECORD_LEN;
        let mut rec_bytes = [0u8; PROFILE_RECORD_LEN];
        rec_bytes.copy_from_slice(&buf[offset..offset + PROFILE_RECORD_LEN]);
        let record = ProfileRecord::from_bytes(&rec_bytes);

        if !record.verify() {
            return Err(FlashError::EmptySlot);
        }
        Ok(record)
    }

    pub fn save_profile(
        &mut self,
        slot: u8,
        name: &str,
        directions: &[u8; 12],
        speeds: &[u8; 12],
    ) -> Result<(), FlashError> {
        if slot as usize >= MAX_PROFILE_SLOTS {
            return Err(FlashError::InvalidSlot(slot));
        }

        let mut buf = [0u8; SECTOR_PAGE_SIZE];
        self.read_page(&mut buf);

        let mut rec = ProfileRecord {
            magic: PROFILE_MAGIC,
            slot,
            is_valid: 1,
            name: [0; 32],
            directions: *directions,
            speeds: *speeds,
            crc16: 0,
        };

        let nb = name.as_bytes();
        let copy_len = nb.len().min(31);
        rec.name[..copy_len].copy_from_slice(&nb[..copy_len]);
        rec.crc16 = rec.compute_crc();

        let offset = (slot as usize) * PROFILE_RECORD_LEN;
        buf[offset..offset + PROFILE_RECORD_LEN].copy_from_slice(&rec.to_bytes());

        self.write_page(&buf)?;
        Ok(())
    }

    pub fn set_boot_slot(&mut self, slot: u8) -> Result<(), FlashError> {
        if slot as usize >= MAX_PROFILE_SLOTS {
            return Err(FlashError::InvalidSlot(slot));
        }

        // 严格前置校验：必须确认目标槽位包含有效配置且通过 CRC16 校验，严禁将空槽位设为自启
        self.load_profile(slot)?;

        let mut buf = [0u8; SECTOR_PAGE_SIZE];
        self.read_page(&mut buf);

        // offset = 4 * 62 = 248 (严格 4 字节自然对齐边界)
        // [offset..offset+4]: SECTOR_MAGIC 严格 4 字节对齐
        // [offset+4]: boot slot 单字节
        let offset = MAX_PROFILE_SLOTS * PROFILE_RECORD_LEN;
        buf[offset..offset + 4].copy_from_slice(&SECTOR_MAGIC.to_le_bytes());
        buf[offset + 4] = slot;

        self.write_page(&buf)?;
        Ok(())
    }

    /// 清空开机自启动设置 (重置为上电默认静默基态)
    pub fn clear_boot_slot(&mut self) -> Result<(), FlashError> {
        let mut buf = [0u8; SECTOR_PAGE_SIZE];
        self.read_page(&mut buf);

        let offset = MAX_PROFILE_SLOTS * PROFILE_RECORD_LEN;
        buf[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes());
        buf[offset + 4] = 0xFF;

        self.write_page(&buf)?;
        Ok(())
    }

    pub fn get_boot_slot(&self) -> Option<u8> {
        let mut buf = [0u8; SECTOR_PAGE_SIZE];
        self.read_page(&mut buf);

        let offset = MAX_PROFILE_SLOTS * PROFILE_RECORD_LEN;
        let magic = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]);
        let slot = buf[offset + 4];
        if magic == SECTOR_MAGIC && (slot as usize) < MAX_PROFILE_SLOTS {
            Some(slot)
        } else {
            None
        }
    }
}
