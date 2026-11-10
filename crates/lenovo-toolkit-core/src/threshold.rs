//! Ngưỡng sạc pin: registry PWRMGRV + IBMPmDrv (port từ ChargeThresholdFeature.cs).

use std::io;

use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE};

use crate::driver::{ACCESS_READ, Device, IBM_PM_DRV};

const DATA_PATH: &str = r"SOFTWARE\WOW6432Node\Lenovo\PWRMGRV\ConfKeys\Data";
const BARCODE: &str = "Barcode Number";
const START_PCT: &str = "ChargeStartPercentage";
const STOP_PCT: &str = "ChargeStopPercentage";
const START_CTL: &str = "ChargeStartControl";
const STOP_CTL: &str = "ChargeStopControl";

const IOCTL_MODE: u32 = 0x22_261C;
const IOCTL_START: u32 = 0x22_2630;
const IOCTL_STOP: u32 = 0x22_2638;

const MIN: u32 = 2;
const MAX: u32 = 99;
const MODE_AUTOMATIC: u32 = 0;
const MODE_ENABLED: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChargeThreshold {
    pub enabled: bool,
    pub start: u32,
    pub stop: u32,
}

impl ChargeThreshold {
    /// Kiểm tra khoảng 2..=99 và start < stop trước khi ghi xuống firmware.
    pub fn new(enabled: bool, start: u32, stop: u32) -> io::Result<Self> {
        if !(MIN..=MAX).contains(&start) || !(MIN..=MAX).contains(&stop) || start >= stop {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("ngưỡng không hợp lệ: cần {MIN}..={MAX} và start < stop"),
            ));
        }
        Ok(Self { enabled, start, stop })
    }
}

pub fn open() -> io::Result<Device> {
    Device::open(IBM_PM_DRV, ACCESS_READ)
}

fn to_driver_request(index: u32, value: u32) -> u32 {
    ((index & 0x3) << 8) | value
}

/// Trả về (chỉ số pin, đường dẫn key) của các pin hỗ trợ ngưỡng sạc.
fn batteries() -> io::Result<Vec<(u32, String)>> {
    let data = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(DATA_PATH, KEY_READ)
        .map_err(|e| io::Error::new(e.kind(), format!("máy không hỗ trợ ngưỡng sạc (thiếu HKLM\\{DATA_PATH}): {e}")))?;
    let mut result = Vec::new();
    for name in data.enum_keys().flatten() {
        let Some(rest) = name
            .get(..7)
            .filter(|p| p.eq_ignore_ascii_case("Battery"))
            .map(|_| &name[7..])
        else {
            continue;
        };
        let Ok(index) = rest.parse::<u32>() else { continue };
        if !(1..=2).contains(&index) {
            continue;
        }
        let Ok(sub) = data.open_subkey_with_flags(&name, KEY_READ) else { continue };
        let barcode: String = sub.get_value(BARCODE).unwrap_or_default();
        if barcode.trim().is_empty() {
            continue;
        }
        let key = format!(r"{DATA_PATH}\{barcode}");
        let has_value = RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(&key, KEY_READ)
            .is_ok_and(|k| k.get_raw_value(START_PCT).is_ok());
        if has_value {
            result.push((index, key));
        }
    }
    if result.is_empty() {
        return Err(io::Error::other("không có pin nào hỗ trợ ngưỡng sạc"));
    }
    Ok(result)
}

pub fn get() -> io::Result<ChargeThreshold> {
    let (_, key) = batteries()?.swap_remove(0);
    let k = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(key, KEY_READ)?;
    let pct = |name| k.get_value::<u32, _>(name).unwrap_or(0).clamp(MIN, MAX);
    let ctl = |name| k.get_value::<u32, _>(name).unwrap_or(0);
    Ok(ChargeThreshold {
        enabled: ctl(START_CTL) != 0 || ctl(STOP_CTL) != 0,
        start: pct(START_PCT),
        stop: pct(STOP_PCT),
    })
}

/// Cần quyền admin (ghi HKLM). Ghi registry trước rồi gửi IOCTL, giống bản C#.
pub fn set(dev: &Device, state: ChargeThreshold) -> io::Result<()> {
    let batteries = batteries()?;
    for (_, key) in &batteries {
        let k = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(key, KEY_READ | KEY_WRITE)?;
        k.set_value(START_PCT, &state.start)?;
        k.set_value(STOP_PCT, &state.stop)?;
        let ctl = u32::from(state.enabled);
        k.set_value(START_CTL, &ctl)?;
        k.set_value(STOP_CTL, &ctl)?;
    }
    for (index, _) in &batteries {
        let i = *index;
        if state.enabled {
            send(dev, IOCTL_STOP, to_driver_request(i, state.stop))?;
            send(dev, IOCTL_START, to_driver_request(i, state.start))?;
            send(dev, IOCTL_MODE, to_driver_request(i, MODE_ENABLED))?;
        } else {
            send(dev, IOCTL_STOP, to_driver_request(i, 0))?;
            send(dev, IOCTL_START, to_driver_request(i, 0))?;
            send(dev, IOCTL_MODE, MODE_AUTOMATIC)?;
        }
    }
    Ok(())
}

fn send(dev: &Device, code: u32, value: u32) -> io::Result<()> {
    let result = dev.ioctl(code, value)?;
    if (result as i32) < 0 {
        return Err(io::Error::other(format!(
            "driver từ chối: code={code:08X} value={value} result={result:08X}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_request_packs_index_into_bits_8_9() {
        assert_eq!(to_driver_request(1, 80), 0x150);
        assert_eq!(to_driver_request(2, 60), 0x23C);
        assert_eq!(to_driver_request(5, 0), 0x100); // chỉ lấy 2 bit thấp của index
    }

    #[test]
    fn threshold_validation() {
        assert!(ChargeThreshold::new(true, 50, 80).is_ok());
        assert!(ChargeThreshold::new(true, 80, 50).is_err());
        assert!(ChargeThreshold::new(true, 1, 80).is_err());
        assert!(ChargeThreshold::new(true, 50, 100).is_err());
    }
}
