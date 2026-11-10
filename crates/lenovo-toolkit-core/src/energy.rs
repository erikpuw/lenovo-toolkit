//! Các tính năng qua EnergyDrv: khóa Fn, Always-on USB, sạc đêm, đèn bàn phím trắng
//! (FnLockFeature, AlwaysOnUSBFeature, BatteryNightChargeFeature,
//! OneLevelWhiteKeyboardBacklightFeature, WhiteKeyboardDriverBacklightFeature).
//!
//! `fn_lock_get/set` làm việc với trạng thái phần cứng thô; dùng `fn_lock_translate`
//! với `compat::MachineInfo::series` để đổi sang trạng thái người dùng thấy.
//! Chưa có: hàng đợi driver toàn cục (cooldown 20 ms) của `GlobalDriverLock`.

use std::io;

use crate::driver::Device;

const IOCTL_SETTINGS: u32 = 0x8310_20E8;
const IOCTL_NIGHT_CHARGE: u32 = 0x8310_2150;
const IOCTL_KEYBOARD: u32 = 0x8310_2144;

const fn bit(v: u32, n: u32) -> bool {
    v & (1 << n) != 0
}

fn send_all(dev: &Device, ioctl: u32, codes: &[u32]) -> io::Result<()> {
    for &c in codes {
        dev.ioctl(ioctl, c)?;
    }
    Ok(())
}

// ---- Fn lock (trạng thái phần cứng thô) ----
pub fn fn_lock_from_raw(raw: u32) -> bool {
    bit(raw, 10)
}
pub fn fn_lock_get(dev: &Device) -> io::Result<bool> {
    dev.ioctl(IOCTL_SETTINGS, 0x2).map(fn_lock_from_raw)
}
pub fn fn_lock_set(dev: &Device, on: bool) -> io::Result<()> {
    send_all(dev, IOCTL_SETTINGS, &[if on { 0xE } else { 0xF }])
}

/// Dòng máy mới (không phải Legion cũ/LOQ) báo trạng thái FnLock ngược với phần cứng thô.
pub fn fn_lock_translate(series: crate::compat::LegionSeries, hardware_on: bool) -> bool {
    use crate::compat::LegionSeries as S;
    if series as u8 <= S::LegionLegacy as u8 || series == S::Loq {
        hardware_on
    } else {
        !hardware_on
    }
}

// ---- Always-on USB ----
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlwaysOnUsb {
    Off,
    OnWhenSleeping,
    OnAlways,
}

impl AlwaysOnUsb {
    pub fn from_raw(raw: u32) -> Self {
        let s = raw.swap_bytes();
        match (bit(s, 31), bit(s, 23)) {
            (true, true) => Self::OnAlways,
            (true, false) => Self::OnWhenSleeping,
            _ => Self::Off,
        }
    }
    fn codes(self) -> [u32; 2] {
        match self {
            Self::Off => [0xB, 0x12],
            Self::OnWhenSleeping => [0xA, 0x12],
            Self::OnAlways => [0xA, 0x13],
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "off" => Self::Off,
            "sleeping" => Self::OnWhenSleeping,
            "always" => Self::OnAlways,
            _ => return None,
        })
    }
}
pub fn usb_get(dev: &Device) -> io::Result<AlwaysOnUsb> {
    dev.ioctl(IOCTL_SETTINGS, 0x2).map(AlwaysOnUsb::from_raw)
}
pub fn usb_set(dev: &Device, state: AlwaysOnUsb) -> io::Result<()> {
    send_all(dev, IOCTL_SETTINGS, &state.codes())
}

// ---- Sạc đêm ----
pub fn night_charge_from_raw(raw: u32) -> io::Result<bool> {
    if bit(raw, 0) {
        Ok(bit(raw, 4))
    } else {
        Err(io::Error::other(format!("trạng thái sạc đêm lạ: {raw:#b}")))
    }
}
pub fn night_charge_get(dev: &Device) -> io::Result<bool> {
    night_charge_from_raw(dev.ioctl(IOCTL_NIGHT_CHARGE, 0x11)?)
}
pub fn night_charge_set(dev: &Device, on: bool) -> io::Result<()> {
    send_all(dev, IOCTL_NIGHT_CHARGE, &[if on { 0x8000_0012 } else { 0x12 }])
}

// ---- Đèn bàn phím 1 mức ----
pub fn one_level_supported(dev: &Device) -> io::Result<bool> {
    Ok(dev.ioctl(IOCTL_SETTINGS, 0x2)? & 16 == 16)
}
pub fn one_level_get(dev: &Device) -> io::Result<bool> {
    Ok(dev.ioctl(IOCTL_SETTINGS, 0x2)? & 32 == 32)
}
pub fn one_level_set(dev: &Device, on: bool) -> io::Result<()> {
    send_all(dev, IOCTL_SETTINGS, &[if on { 0x8 } else { 0x9 }])
}

// ---- Đèn bàn phím trắng nhiều mức (Off=0, Low=1, High=2, Auto=3) ----
const READ_CAPABILITY: u32 = 0x1;
const READ_STATUS: u32 = 0x2;
const WRITE_STATUS: u32 = 0x3;
const VALID_MASK: u32 = 0x1;
const LEVEL_MASK: u32 = 0xF;
const LEVEL_SHIFT: u32 = 16;

const fn guard(capability: u32) -> u32 {
    (capability & !VALID_MASK) << 3
}

fn kbd_capability(dev: &Device) -> io::Result<u32> {
    let cap = dev.ioctl(IOCTL_KEYBOARD, READ_CAPABILITY)?;
    if cap & VALID_MASK != VALID_MASK {
        return Err(io::Error::other("capability đèn bàn phím không hợp lệ"));
    }
    Ok(cap)
}

/// Mức tối đa (0 = không hỗ trợ).
pub fn kbd_max_level(dev: &Device) -> io::Result<u32> {
    Ok(kbd_capability(dev)? >> 1)
}

pub fn kbd_get(dev: &Device) -> io::Result<u32> {
    let cap = kbd_capability(dev)?;
    let raw = dev.ioctl(IOCTL_KEYBOARD, guard(cap) | READ_STATUS)?;
    kbd_level_from_raw(raw)
}

pub fn kbd_level_from_raw(raw: u32) -> io::Result<u32> {
    if raw & VALID_MASK != VALID_MASK {
        return Err(io::Error::other("trạng thái đèn bàn phím không hợp lệ"));
    }
    Ok((raw & LEVEL_MASK) >> 1)
}

pub fn kbd_set(dev: &Device, level: u32) -> io::Result<()> {
    let cap = kbd_capability(dev)?;
    if level > (cap >> 1) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("mức tối đa là {}", cap >> 1)));
    }
    dev.ioctl(IOCTL_KEYBOARD, guard(cap) | (level << LEVEL_SHIFT) | WRITE_STATUS)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fn_lock_translation_by_series() {
        use crate::compat::LegionSeries as S;
        assert!(fn_lock_translate(S::Legion5, true));
        assert!(fn_lock_translate(S::LegionLegacy, true));
        assert!(fn_lock_translate(S::Loq, true));
        assert!(!fn_lock_translate(S::IdeaPad, true));
        assert!(fn_lock_translate(S::Unknown, false));
    }

    #[test]
    fn fn_lock_is_bit_10() {
        assert!(fn_lock_from_raw(1 << 10));
        assert!(!fn_lock_from_raw(0));
    }

    #[test]
    fn always_on_usb_decodes_after_byte_swap() {
        // bit 31 và 23 sau khi đảo byte => byte thấp 0x80 và 0x80 << 16 trong raw gốc
        let always = 0x0000_0080u32 | 0x0080_0000u32.swap_bytes();
        assert_eq!(AlwaysOnUsb::from_raw(always), AlwaysOnUsb::OnAlways);
        assert_eq!(AlwaysOnUsb::from_raw(0x80), AlwaysOnUsb::OnWhenSleeping);
        assert_eq!(AlwaysOnUsb::from_raw(0), AlwaysOnUsb::Off);
        assert_eq!(AlwaysOnUsb::OnAlways.codes(), [0xA, 0x13]);
    }

    #[test]
    fn night_charge_requires_valid_bit() {
        assert_eq!(night_charge_from_raw(0b1_0001).unwrap(), true);
        assert_eq!(night_charge_from_raw(0b0_0001).unwrap(), false);
        assert!(night_charge_from_raw(0b1_0000).is_err());
    }

    #[test]
    fn keyboard_guard_and_level() {
        assert_eq!(guard(0b111), 0b110 << 3);
        assert_eq!(kbd_level_from_raw(0b0111).unwrap(), 3);
        assert!(kbd_level_from_raw(0b0110).is_err());
    }
}
