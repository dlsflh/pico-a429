//! 片上非易失 Flash 配置存储管理器 (RP2350 On-Chip Flash NV Storage)
//!
//! 在 4MB Flash 最高扇区 (0x003F_F000) 划分 256 字节紧凑存储空间，
//! 支持 4 个独立配置 Profile 槽位与开机自启动槽位指定，具备 CRC16 校验与防损机制。

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

/// 单槽位配置记录 (46 字节紧凑结构)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProfileRecord {
    pub magic: u16,
    pub slot: u8,
    pub is_valid: u8,
    pub name: [u8; 16],
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
            name: [0; 16],
            directions: [2; 12], // 默认 RX
            speeds: [1; 12],     // 默认 HIGH
            crc16: 0,
        }
    }
}

impl ProfileRecord {
    pub fn compute_crc(&self) -> u16 {
        let len = core::mem::size_of::<ProfileRecord>() - 2;
        let slice = unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, len) };
        crc16_ccitt(slice)
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

        let offset = (slot as usize) * core::mem::size_of::<ProfileRecord>();
        let record = unsafe {
            core::ptr::read_unaligned(buf[offset..].as_ptr() as *const ProfileRecord)
        };

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
            name: [0; 16],
            directions: *directions,
            speeds: *speeds,
            crc16: 0,
        };

        let nb = name.as_bytes();
        let copy_len = nb.len().min(15);
        rec.name[..copy_len].copy_from_slice(&nb[..copy_len]);
        rec.crc16 = rec.compute_crc();

        let offset = (slot as usize) * core::mem::size_of::<ProfileRecord>();
        unsafe {
            core::ptr::copy_nonoverlapping(
                &rec as *const _ as *const u8,
                buf[offset..].as_mut_ptr(),
                core::mem::size_of::<ProfileRecord>(),
            );
        }

        self.write_page(&buf)?;
        Ok(())
    }

    pub fn set_boot_slot(&mut self, slot: u8) -> Result<(), FlashError> {
        if slot as usize >= MAX_PROFILE_SLOTS {
            return Err(FlashError::InvalidSlot(slot));
        }

        let mut buf = [0u8; SECTOR_PAGE_SIZE];
        let _ = self.read_page(&mut buf);

        let offset = MAX_PROFILE_SLOTS * core::mem::size_of::<ProfileRecord>();
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

        let offset = MAX_PROFILE_SLOTS * core::mem::size_of::<ProfileRecord>();
        let magic = u32::from_le_bytes([buf[offset + 1], buf[offset + 2], buf[offset + 3], buf[offset + 4]]);
        if magic == SECTOR_MAGIC && (buf[offset] as usize) < MAX_PROFILE_SLOTS {
            Some(buf[offset])
        } else {
            None
        }
    }
}
