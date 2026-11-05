//! 片上非易失 Flash 配置存储管理器 (RP2350 On-Chip Flash NV Profile Storage)
//!
//! 在 4MB QSPI Flash 最高 4KB 扇区 (0x003F_F000..0x0040_0000) 划分持久化存储空间，
//! 支持 4 个独立配置 Profile 槽位与开机自启动槽位指定，具备 CRC16 校验与防损机制。

use embassy_rp::flash::{Blocking, Error as FlashHalError, Flash, ERASE_SIZE};
use embassy_rp::peripherals::FLASH;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

use arinc429_core::protocol::crc16_ccitt;

pub static FLASH_MGR: Mutex<CriticalSectionRawMutex, Option<FlashProfileManager<'static>>> =
    Mutex::new(None);

pub const FLASH_CAPACITY: usize = 4 * 1024 * 1024; // 4MB Flash
pub const PROFILE_SECTOR_OFFSET: u32 = (FLASH_CAPACITY - ERASE_SIZE) as u32; // 0x003F_F000
pub const MAX_PROFILE_SLOTS: usize = 4;
pub const PROFILE_MAGIC: u32 = 0x5034_3239; // ASCII "P429"
pub const SECTOR_MAGIC: u32 = 0x5345_4354; // ASCII "SECT"

/// 单通道配置记录结构 (对齐为 36 字节)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ChannelConfigRecord {
    pub direction: u8,           // 0: Disabled, 1: Tx, 2: Rx
    pub high_speed: u8,          // 0: 12.5k (Low), 1: 100k (High)
    pub filter_mode: u8,         // 0: AcceptAll, 1: Whitelist, 2: Blacklist
    pub _reserved: u8,
    pub filter_bitmap: [u32; 8], // 256 位 Label 掩码
}

impl Default for ChannelConfigRecord {
    fn default() -> Self {
        Self {
            direction: 2, // 默认被动监听 Rx
            high_speed: 1, // 默认 100k
            filter_mode: 0, // 全放行
            _reserved: 0,
            filter_bitmap: [0; 8],
        }
    }
}

/// 单槽位配置结构体 (严格固定为 512 字节)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProfileRecord {
    pub magic: u32,
    pub version: u16,
    pub profile_id: u8,
    pub is_valid: u8,
    pub name: [u8; 32],
    pub channels: [ChannelConfigRecord; 12], // 12 * 36 = 432 字节
    pub crc16: u16,
    pub _padding: [u8; 38], // 填充至 512 字节
}

impl Default for ProfileRecord {
    fn default() -> Self {
        Self {
            magic: PROFILE_MAGIC,
            version: 1,
            profile_id: 0,
            is_valid: 0,
            name: [0; 32],
            channels: [ChannelConfigRecord::default(); 12],
            crc16: 0,
            _padding: [0; 38],
        }
    }
}

impl ProfileRecord {
    /// 计算该 Profile 记录的 CRC-16 校验值
    pub fn compute_crc(&self) -> u16 {
        let slice = unsafe {
            core::slice::from_raw_parts(
                self as *const _ as *const u8,
                core::mem::size_of::<ProfileRecord>() - 40, // 覆盖至 channels 结束
            )
        };
        crc16_ccitt(slice)
    }

    /// 校验记录合法性与 CRC
    pub fn verify(&self) -> bool {
        self.magic == PROFILE_MAGIC && self.is_valid == 1 && self.compute_crc() == self.crc16
    }
}

/// 4096 字节扇区全景物理映射
#[repr(C)]
pub struct FlashSectorLayout {
    pub profiles: [ProfileRecord; MAX_PROFILE_SLOTS], // 4 * 512 = 2048 字节
    pub boot_slot: u8,                                // 0..3 (0xFF 表示未设定)
    pub sector_magic: u32,                           // SECTOR_MAGIC
    pub _padding: [u8; ERASE_SIZE - 2048 - 5],
}

#[derive(Debug, Clone, Copy)]
pub enum FlashError {
    HalError,
    InvalidSlot(u8),
    CorruptedData,
    EmptySlot,
}

impl From<FlashHalError> for FlashError {
    fn from(_: FlashHalError) -> Self {
        FlashError::HalError
    }
}

/// 非易失存储管理控制器
pub struct FlashProfileManager<'d> {
    flash: Flash<'d, FLASH, Blocking, FLASH_CAPACITY>,
}

impl<'d> FlashProfileManager<'d> {
    pub fn new(flash: Flash<'d, FLASH, Blocking, FLASH_CAPACITY>) -> Self {
        Self { flash }
    }

    /// 读取整块扇区进入内存
    fn read_sector(&mut self, buf: &mut [u8; ERASE_SIZE]) -> Result<(), FlashError> {
        self.flash.blocking_read(PROFILE_SECTOR_OFFSET, buf)?;
        Ok(())
    }

    /// 将修改后的整块扇区写回 Flash (带 Erase 擦除)
    fn write_sector(&mut self, buf: &[u8; ERASE_SIZE]) -> Result<(), FlashError> {
        self.flash
            .blocking_erase(PROFILE_SECTOR_OFFSET, PROFILE_SECTOR_OFFSET + ERASE_SIZE as u32)?;
        self.flash.blocking_write(PROFILE_SECTOR_OFFSET, buf)?;
        Ok(())
    }

    /// 读取指定槽位的 Profile
    pub fn load_profile(&mut self, slot: u8) -> Result<ProfileRecord, FlashError> {
        if slot as usize >= MAX_PROFILE_SLOTS {
            return Err(FlashError::InvalidSlot(slot));
        }

        let mut sector_buf = [0u8; ERASE_SIZE];
        self.read_sector(&mut sector_buf)?;

        let offset = (slot as usize) * core::mem::size_of::<ProfileRecord>();
        let record = unsafe {
            core::ptr::read_unaligned(
                sector_buf[offset..].as_ptr() as *const ProfileRecord
            )
        };

        if !record.verify() {
            return Err(FlashError::EmptySlot);
        }

        Ok(record)
    }

    /// 保存当前配置至指定槽位
    pub fn save_profile(
        &mut self,
        slot: u8,
        name: &str,
        channels: &[ChannelConfigRecord; 12],
    ) -> Result<(), FlashError> {
        if slot as usize >= MAX_PROFILE_SLOTS {
            return Err(FlashError::InvalidSlot(slot));
        }

        let mut sector_buf = [0u8; ERASE_SIZE];
        let _ = self.read_sector(&mut sector_buf);

        let mut record = ProfileRecord::default();
        record.magic = PROFILE_MAGIC;
        record.version = 1;
        record.profile_id = slot;
        record.is_valid = 1;
        record.channels = *channels;

        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(31);
        record.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
        record.name[copy_len] = 0; // null-terminate

        record.crc16 = record.compute_crc();

        let offset = (slot as usize) * core::mem::size_of::<ProfileRecord>();
        unsafe {
            core::ptr::copy_nonoverlapping(
                &record as *const _ as *const u8,
                sector_buf[offset..].as_mut_ptr(),
                core::mem::size_of::<ProfileRecord>(),
            );
        }

        self.write_sector(&sector_buf)?;
        Ok(())
    }

    /// 设置开机默认加载的槽位 (0..3)
    pub fn set_boot_slot(&mut self, slot: u8) -> Result<(), FlashError> {
        if slot as usize >= MAX_PROFILE_SLOTS {
            return Err(FlashError::InvalidSlot(slot));
        }

        let mut sector_buf = [0u8; ERASE_SIZE];
        let _ = self.read_sector(&mut sector_buf);

        // boot_slot 紧随 4 个 Profile 之后 (偏移 2048)
        let boot_slot_offset = MAX_PROFILE_SLOTS * core::mem::size_of::<ProfileRecord>();
        sector_buf[boot_slot_offset] = slot;

        // 写入扇区 magic
        let magic_bytes = SECTOR_MAGIC.to_le_bytes();
        sector_buf[boot_slot_offset + 1..boot_slot_offset + 5].copy_from_slice(&magic_bytes);

        self.write_sector(&sector_buf)?;
        Ok(())
    }

    /// 获取当前设定的开机自启动槽位
    pub fn get_boot_slot(&mut self) -> Option<u8> {
        let mut sector_buf = [0u8; ERASE_SIZE];
        if self.read_sector(&mut sector_buf).is_err() {
            return None;
        }

        let boot_slot_offset = MAX_PROFILE_SLOTS * core::mem::size_of::<ProfileRecord>();
        let magic = u32::from_le_bytes([
            sector_buf[boot_slot_offset + 1],
            sector_buf[boot_slot_offset + 2],
            sector_buf[boot_slot_offset + 3],
            sector_buf[boot_slot_offset + 4],
        ]);

        if magic == SECTOR_MAGIC {
            let slot = sector_buf[boot_slot_offset];
            if (slot as usize) < MAX_PROFILE_SLOTS {
                return Some(slot);
            }
        }
        None
    }
}
