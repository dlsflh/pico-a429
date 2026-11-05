//! 片上非易失 Flash 配置存储管理器 (RP2350 On-Chip Flash NV Storage)
//!
//! 在 4MB Flash 最高扇区 (0x003F_F000) 划分 256 字节紧凑存储空间，
//! 支持 4 个独立配置 Profile 槽位与开机自启动槽位指定，具备 CRC16 校验与防损机制。
//! 采用确定性定长安全字节序列化，彻底消除 `unsafe` 裸指针强转与内存未对齐隐患。

use embassy_rp::flash::{Blocking, Error as FlashHalError, Flash, ERASE_SIZE};
use embassy_rp::peripherals::FLASH;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

use arinc429_core::protocol::crc16_ccitt;

pub static FLASH_MGR: Mutex<CriticalSectionRawMutex, Option<FlashProfileManager<'static>>> =
    Mutex::new(None);

pub const FLASH_CAPACITY: usize = 4 * 1024 * 1024;
pub const PROFILE_SECTOR_OFFSET: u32 = (FLASH_CAPACITY - ERASE_SIZE) as u32;
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
            directions: [2; 12], // 默认 RX
            speeds: [1; 12],     // 默认 HIGH
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

#[derive(Debug, Clone, Copy)]
pub enum FlashError {
    HalError,
    InvalidSlot(u8),
    EmptySlot,
}

impl From<FlashHalError> for FlashError {
    fn from(_: FlashHalError) -> Self {
        FlashError::HalError
    }
}

pub struct FlashProfileManager<'d> {
    flash: Flash<'d, FLASH, Blocking, FLASH_CAPACITY>,
}

impl<'d> FlashProfileManager<'d> {
    pub fn new(flash: Flash<'d, FLASH, Blocking, FLASH_CAPACITY>) -> Self {
        Self { flash }
    }

    fn read_page(&mut self, buf: &mut [u8; SECTOR_PAGE_SIZE]) -> Result<(), FlashError> {
        self.flash.blocking_read(PROFILE_SECTOR_OFFSET, buf)?;
        Ok(())
    }

    fn write_page(&mut self, buf: &[u8; SECTOR_PAGE_SIZE]) -> Result<(), FlashError> {
        self.flash
            .blocking_erase(PROFILE_SECTOR_OFFSET, PROFILE_SECTOR_OFFSET + ERASE_SIZE as u32)?;
        self.flash.blocking_write(PROFILE_SECTOR_OFFSET, buf)?;
        Ok(())
    }

    pub fn load_profile(&mut self, slot: u8) -> Result<ProfileRecord, FlashError> {
        if slot as usize >= MAX_PROFILE_SLOTS {
            return Err(FlashError::InvalidSlot(slot));
        }

        let mut buf = [0u8; SECTOR_PAGE_SIZE];
        self.read_page(&mut buf)?;

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
        let _ = self.read_page(&mut buf);

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

        let mut buf = [0u8; SECTOR_PAGE_SIZE];
        let _ = self.read_page(&mut buf);

        let offset = MAX_PROFILE_SLOTS * PROFILE_RECORD_LEN;
        buf[offset] = slot;
        buf[offset + 1..offset + 5].copy_from_slice(&SECTOR_MAGIC.to_le_bytes());

        self.write_page(&buf)?;
        Ok(())
    }

    pub fn get_boot_slot(&mut self) -> Option<u8> {
        let mut buf = [0u8; SECTOR_PAGE_SIZE];
        if self.read_page(&mut buf).is_err() {
            return None;
        }

        let offset = MAX_PROFILE_SLOTS * PROFILE_RECORD_LEN;
        let magic = u32::from_le_bytes([
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
            buf[offset + 4],
        ]);
        if magic == SECTOR_MAGIC && (buf[offset] as usize) < MAX_PROFILE_SLOTS {
            Some(buf[offset])
        } else {
            None
        }
    }
}
