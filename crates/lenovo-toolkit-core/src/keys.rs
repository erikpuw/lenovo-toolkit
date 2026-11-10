//! Lắng nghe phím Fn đặc biệt qua EnergyDrv (port từ DriverKeyListener.cs).
//!
//! Cơ chế: tạo một event Windows, đăng ký với driver bằng IOCTL_KEY_WAIT_HANDLE;
//! mỗi khi có phím, driver báo event, ta đọc mặt nạ bit bằng IOCTL_KEY_VALUE.
//!
//! Lưu ý: chỉ nên có MỘT tiến trình đăng ký tại một thời điểm (đóng bản C# trước khi thử).
//! Chưa có: bỏ qua phím khi dịch vụ FnKeys của Lenovo còn bật (xem SoftwareDisabler).

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};

use crate::battery;
use crate::driver::Device;

const IOCTL_KEY_WAIT_HANDLE: u32 = 0x8310_20D8;
const IOCTL_KEY_VALUE: u32 = 0x8310_20CC;
const POLL_MS: u32 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    FnQ,
    FnF10,
    FnF4,
    FnF8,
    FnSpace,
}

const KEYS: [(Key, u32); 5] = [
    (Key::FnQ, 1),
    (Key::FnF10, 32),
    (Key::FnF4, 256),
    (Key::FnSpace, 4096),
    (Key::FnF8, 8192),
];

/// Giải mã mặt nạ bit thành các phím đang được báo (cờ [Flags] của bản C#).
pub fn decode(raw: u32) -> Vec<Key> {
    KEYS.iter().filter(|(_, bit)| raw & bit != 0).map(|(k, _)| *k).collect()
}

struct Event(HANDLE);
// HANDLE event dùng được từ mọi thread.
unsafe impl Send for Event {}
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub struct KeyListener {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl KeyListener {
    /// Bắt đầu lắng nghe; `on_key(raw, keys)` được gọi trên luồng riêng cho mỗi lần nhấn.
    pub fn start(mut on_key: impl FnMut(u32, Vec<Key>) + Send + 'static) -> io::Result<Self> {
        let dev = battery::open()?;
        let event = Event(
            unsafe { CreateEventW(None, true, false, None) }
                .map_err(|e| io::Error::other(format!("CreateEventW lỗi: {e}")))?,
        );

        // Buffer vào 16 byte như bản C#: 4 byte đầu là handle của event.
        let mut input = [0u8; 16];
        input[..4].copy_from_slice(&(event.0.0 as usize as u32).to_le_bytes());
        dev.ioctl_in(IOCTL_KEY_WAIT_HANDLE, &input)?;
        read_value(&dev)?; // xóa thanh ghi

        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let event = event; // bắt cả struct (Send), không chỉ trường HANDLE
            while !flag.load(Ordering::Relaxed) {
                if unsafe { WaitForSingleObject(event.0, POLL_MS) } != WAIT_OBJECT_0 {
                    continue;
                }
                if let Ok(raw) = read_value(&dev) {
                    on_key(raw, decode(raw));
                }
                unsafe {
                    let _ = ResetEvent(event.0);
                }
            }
        });
        Ok(Self { stop, thread: Some(thread) })
    }
}

fn read_value(dev: &Device) -> io::Result<u32> {
    dev.ioctl(IOCTL_KEY_VALUE, 0)
}

impl Drop for KeyListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_flags() {
        assert_eq!(decode(1), vec![Key::FnQ]);
        assert_eq!(decode(256 | 8192), vec![Key::FnF4, Key::FnF8]);
        assert_eq!(decode(0), vec![]);
        assert_eq!(decode(2), vec![]); // bit lạ bị bỏ qua
    }
}
