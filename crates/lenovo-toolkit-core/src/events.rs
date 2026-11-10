//! Sự kiện đổi chế độ nguồn do firmware báo qua WMI (Fn+Q, nút vật lý...).
//! Port từ WMI.LenovoGameZoneSmartFanModeEvent / ThermalModeEvent và
//! PowerModeListener / ThermalModeListener của bản C#.
//!
//! Giới hạn: iterator của WMI chặn đến sự kiện kế tiếp, nên khi `EventListener` bị drop,
//! luồng nền chỉ thoát sau sự kiện tiếp theo (không chặn tiến trình thoát).
//! Chưa có: đồng bộ Windows power mode/plan khi đổi chế độ, cơ chế `SuppressNext`.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use crate::power::PowerMode;
use crate::wmi_util::{Wmi, variant_to_i32};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    SmartFanMode,
    ThermalMode,
}

impl Source {
    fn query(self) -> &'static str {
        match self {
            Self::SmartFanMode => "SELECT * FROM LENOVO_GAMEZONE_SMART_FAN_MODE_EVENT",
            Self::ThermalMode => "SELECT * FROM LENOVO_GAMEZONE_THERMAL_MODE_EVENT",
        }
    }
}

/// Giá trị `mode` của cả hai sự kiện trùng giá trị WMI của power mode (enum + 1).
pub fn mode_from_event(raw: i32) -> Option<PowerMode> {
    u32::try_from(raw).ok().and_then(PowerMode::from_wmi)
}

/// Sự kiện phím nóng đặc biệt từ `LENOVO_UTILITY_EVENT` (`PressTypeDataVal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialKey {
    /// `Fn+F9`: mở ứng dụng Lenovo Legion Toolkit.
    FnF9,
    /// `Fn+R`: chuyển đổi tần số quét màn hình.
    FnR,
    /// Firmware đã bật/tắt Fn Lock (chỉ báo trạng thái).
    FnLockOn,
    FnLockOff,
    /// Công tắc camera vật lý.
    CameraOn,
    CameraOff,
    /// Đèn bàn phím trắng đổi mức (phím Fn+Space).
    WhiteBacklightOff,
    WhiteBacklightLow,
    WhiteBacklightHigh,
}

pub fn special_key_from_event(raw: i32) -> Option<SpecialKey> {
    match raw {
        1 => Some(SpecialKey::FnF9),
        2 => Some(SpecialKey::FnLockOn),
        3 => Some(SpecialKey::FnLockOff),
        12 => Some(SpecialKey::CameraOn),
        13 => Some(SpecialKey::CameraOff),
        16 | 0x0041_002A => Some(SpecialKey::FnR),
        64 => Some(SpecialKey::WhiteBacklightOff),
        65 => Some(SpecialKey::WhiteBacklightLow),
        66 => Some(SpecialKey::WhiteBacklightHigh),
        _ => None,
    }
}

pub struct UtilityEventListener {
    stop: Arc<AtomicBool>,
}

impl UtilityEventListener {
    pub fn start(mut on_key: impl FnMut(SpecialKey) + Send + 'static) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<io::Result<()>>(1);

        thread::spawn(move || {
            let result = Wmi::open().and_then(|w| {
                w.watch(
                    "SELECT * FROM LENOVO_UTILITY_EVENT",
                    || {
                        let _ = ready_tx.send(Ok(()));
                    },
                    |mut props| {
                        if flag.load(Ordering::Relaxed) {
                            return false;
                        }
                        if let Some(raw) = props.remove("PressTypeDataVal").and_then(|v| variant_to_i32(v).ok())
                            && let Some(key) = special_key_from_event(raw)
                        {
                            on_key(key);
                        }
                        true
                    },
                )
            });
            if let Err(e) = result {
                let _ = ready_tx.send(Err(e));
            }
        });

        ready_rx
            .recv()
            .map_err(|_| io::Error::other("luồng sự kiện LENOVO_UTILITY_EVENT dừng bất ngờ"))??;
        Ok(Self { stop })
    }
}

impl Drop for UtilityEventListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub struct EventListener {
    stop: Arc<AtomicBool>,
}

impl EventListener {
    /// `on_change(raw, mode)` chạy trên luồng nền; `mode` là `None` nếu giá trị lạ.
    pub fn start(source: Source, mut on_change: impl FnMut(i32, Option<PowerMode>) + Send + 'static) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<io::Result<()>>(1);

        thread::spawn(move || {
            let result = Wmi::open().and_then(|w| {
                w.watch(
                    source.query(),
                    || {
                        let _ = ready_tx.send(Ok(()));
                    },
                    |mut props| {
                        if flag.load(Ordering::Relaxed) {
                            return false;
                        }
                        if let Some(raw) = props.remove("mode").and_then(|v| variant_to_i32(v).ok()) {
                            on_change(raw, mode_from_event(raw));
                        }
                        true
                    },
                )
            });
            if let Err(e) = result {
                // Nếu đã báo ready thì không còn ai nhận; nếu lỗi lúc đăng ký thì `start` nhận được.
                let _ = ready_tx.send(Err(e));
            }
        });

        ready_rx
            .recv()
            .map_err(|_| io::Error::other("luồng sự kiện WMI dừng bất ngờ"))??;
        Ok(Self { stop })
    }
}

impl Drop for EventListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_mode_maps_like_wmi_value() {
        assert_eq!(mode_from_event(1), Some(PowerMode::Quiet));
        assert_eq!(mode_from_event(2), Some(PowerMode::Balance));
        assert_eq!(mode_from_event(3), Some(PowerMode::Performance));
        assert_eq!(mode_from_event(224), Some(PowerMode::Extreme));
        assert_eq!(mode_from_event(255), Some(PowerMode::GodMode));
        assert_eq!(mode_from_event(0), None);
        assert_eq!(mode_from_event(-1), None);
    }

    #[test]
    fn special_key_maps_fn_f9_and_fn_r() {
        assert_eq!(special_key_from_event(1), Some(SpecialKey::FnF9));
        assert_eq!(special_key_from_event(16), Some(SpecialKey::FnR));
        assert_eq!(special_key_from_event(0x0041_002A), Some(SpecialKey::FnR));
        assert_eq!(special_key_from_event(32), None);
    }

    #[test]
    fn special_key_maps_state_reporting_keys() {
        assert_eq!(special_key_from_event(2), Some(SpecialKey::FnLockOn));
        assert_eq!(special_key_from_event(3), Some(SpecialKey::FnLockOff));
        assert_eq!(special_key_from_event(12), Some(SpecialKey::CameraOn));
        assert_eq!(special_key_from_event(13), Some(SpecialKey::CameraOff));
        assert_eq!(special_key_from_event(62), None); // Fn+F4 đi qua driver (keys.rs), không qua WMI
        assert_eq!(special_key_from_event(64), Some(SpecialKey::WhiteBacklightOff));
        assert_eq!(special_key_from_event(65), Some(SpecialKey::WhiteBacklightLow));
        assert_eq!(special_key_from_event(66), Some(SpecialKey::WhiteBacklightHigh));
    }
}
