//! Truy cập driver kernel của Lenovo qua DeviceIoControl.
//! Handle được đóng bằng RAII (`Drop`), không để rò như các handle WMI của bản C#.

use std::{io, ptr};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_READ_DATA, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_WRITE_DATA, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::PCWSTR;

pub const ENERGY_DRV: &str = r"\\.\EnergyDrv";
pub const IBM_PM_DRV: &str = r"\\.\IBMPmDrv";

pub const ACCESS_READ: u32 = FILE_READ_DATA.0;
pub const ACCESS_READ_WRITE: u32 = FILE_READ_DATA.0 | FILE_WRITE_DATA.0;

pub struct Device(HANDLE);

// HANDLE kernel dùng được từ mọi thread; DeviceIoControl là đồng bộ.
unsafe impl Send for Device {}
unsafe impl Sync for Device {}

impl Device {
    pub fn open(path: &str, access: u32) -> io::Result<Self> {
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|e| io::Error::other(format!("không mở được {path}: {e}")))?;
        Ok(Self(handle))
    }

    /// IOCTL với buffer vào/ra 4 byte (kiểu u32 như bản C#).
    pub fn ioctl(&self, code: u32, input: u32) -> io::Result<u32> {
        let mut out: u32 = 0;
        let mut returned: u32 = 0;
        unsafe {
            DeviceIoControl(
                self.0,
                code,
                Some(ptr::from_ref(&input).cast()),
                size_of::<u32>() as u32,
                Some(ptr::from_mut(&mut out).cast()),
                size_of::<u32>() as u32,
                Some(&mut returned),
                None,
            )
        }
        .map_err(|e| io::Error::other(format!("DeviceIoControl 0x{code:X} lỗi: {e}")))?;
        Ok(out)
    }
}

impl Device {
    /// IOCTL chỉ có buffer vào (không đọc kết quả), độ dài tùy ý.
    pub fn ioctl_in(&self, code: u32, input: &[u8]) -> io::Result<()> {
        let mut returned: u32 = 0;
        unsafe {
            DeviceIoControl(
                self.0,
                code,
                Some(input.as_ptr().cast()),
                input.len() as u32,
                None,
                0,
                Some(&mut returned),
                None,
            )
        }
        .map_err(|e| io::Error::other(format!("DeviceIoControl 0x{code:X} lỗi: {e}")))
    }

    /// IOCTL với buffer vào/ra tùy ý, trả về số byte thực tế nhận được.
    pub fn ioctl_buf(&self, code: u32, input: &[u8], output: &mut [u8]) -> io::Result<usize> {
        let mut returned: u32 = 0;
        unsafe {
            DeviceIoControl(
                self.0,
                code,
                (!input.is_empty()).then(|| input.as_ptr().cast()),
                input.len() as u32,
                (!output.is_empty()).then(|| output.as_mut_ptr().cast()),
                output.len() as u32,
                Some(&mut returned),
                None,
            )
        }
        .map_err(|e| io::Error::other(format!("DeviceIoControl 0x{code:X} lỗi: {e}")))?;
        Ok(returned as usize)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
