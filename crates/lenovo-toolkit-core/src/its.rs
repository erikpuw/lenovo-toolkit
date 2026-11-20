//! Chế độ nhiệt & quạt chính hãng ITS (Intelligent Thermal Solution / DYTC) cho các dòng
//! Lenovo Non-Gaming (ThinkBook, IdeaPad, Yoga, XiaoXin, Lenovo Slim, Motobook, Lenovo V)
//! đi hoàn toàn qua driver ký số chính hãng `\\.\EnergyDrv` (`IOCTL_ENERGY_SMART_POWER`).
//!
//! Port an toàn từ `ITSModeFeatureDriver.cs` của LenovoLegionToolkit:
//! - Luôn kiểm tra bit khả năng (`DYTC_CMD_GET_FUNCTION_CAPABILITIES` + `DYTC_RESPONSE_VALID_BIT`)
//!   từ firmware trước khi cho phép gửi bất kỳ lệnh ghi nào.
//! - Chế độ Geek (Full-Speed) chỉ được phép bật khi firmware quảng bá bit `0x0002_0001` VÀ đang cắm sạc AC,
//!   tự động rollback (`DYTC_CMD_DISABLE_FULL_SPEED`) nếu có lỗi.

use std::io;
use std::thread;
use std::time::{Duration, Instant};

use crate::driver::Device;
use crate::power::{self, WinPowerMode};

/// Mã IOCTL chính hãng của `EnergyDrv.sys` cho lệnh DYTC Smart Power / ITS.
pub const IOCTL_ENERGY_SMART_POWER: u32 = 0x8310_213C;

const DYTC_CMD_GET_MODE: u32 = 2;
const DYTC_CMD_GET_FUNCTION_CAPABILITIES: u32 = 3;
const DYTC_CMD_GET_EXTENDED_CAPABILITIES: u32 = 10;
const DYTC_CMD_RESET: u32 = 0x0000_01FF;
const DYTC_CMD_SET_FUNCTION: u32 = 0x1;

const DYTC_RESPONSE_VALID_BIT: u32 = 0x1;
const DYTC_SET_FUNCTION_ENABLE_BIT: u32 = 0x0010_0000;
const DYTC_SET_FUNCTION_SHIFT: u32 = 12;
const DYTC_SET_MODE_PARAMETER_SHIFT: u32 = 16;
const DYTC_GET_FUNCTION_SHIFT: u32 = 8;
const DYTC_GET_MODE_PARAMETER_SHIFT: u32 = 12;
const DYTC_FUNCTION_CAPABILITY_SHIFT: u32 = 16;

const DYTC_MODE_PARAMETER_PERFORMANCE: u32 = 2;
const DYTC_MODE_PARAMETER_COOL: u32 = 3;
const DYTC_MODE_PARAMETER_INTELLIGENT_OR_DISABLE: u32 = 0xF;

const DYTC_FULL_SPEED_CAPABILITY_MASK: u32 = 0x0002_0001;
const DYTC_CMD_DISABLE_FULL_SPEED: u32 = 0x000F_100B;
const DYTC_CMD_ENABLE_FULL_SPEED: u32 = 0x001F_100B;
const DYTC_DEFAULT_FUNCTION_READBACK: u32 = 0x0001_F001;
const GEEK_ENABLE_REPEAT_COUNT: usize = 2;
const STATE_TIMEOUT: Duration = Duration::from_millis(3000);
const POLL_STEP: Duration = Duration::from_millis(200);

const FN_FIRMWARE_DEFAULT: u32 = 0;
const FN_MANUAL: u32 = 3;
const FN_UNKNOWN4: u32 = 4;
const FN_INTELLIGENT_COOLING: u32 = 5;
const FN_AUTO_6: u32 = 6;
const FN_AUTO_7: u32 = 7;
const FN_AUTO_8: u32 = 8;
const FN_ITS_MODE_CONTROL: u32 = 11;
const FN_AUTO_13: u32 = 13;
const FN_AUTO_15: u32 = 15;

const REQUIRED_BASE_MODE_CAPABILITIES: u32 = DYTC_RESPONSE_VALID_BIT
    | (1u32 << (DYTC_FUNCTION_CAPABILITY_SHIFT + FN_INTELLIGENT_COOLING))
    | (1u32 << (DYTC_FUNCTION_CAPABILITY_SHIFT + FN_ITS_MODE_CONTROL));

const DISABLE_ORDER_BEFORE_SWITCH: [u32; 6] = [
    FN_AUTO_15,
    FN_AUTO_8,
    FN_AUTO_7,
    FN_AUTO_6,
    FN_INTELLIGENT_COOLING,
    FN_ITS_MODE_CONTROL,
];

/// Các chế độ nhiệt ITS (Intelligent Thermal Solution) cho dòng Non-Gaming Lenovo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItsMode {
    /// Intelligent Cooling (tự động cân bằng).
    Auto,
    /// Battery Saving / Cool (yên tĩnh, mát mẻ, tiết kiệm pin).
    Cool,
    /// Extreme Performance (hiệu năng tối đa).
    Performance,
    /// Geek / Full-Speed (chỉ mở khi firmware hỗ trợ và đang cắm sạc AC).
    Geek,
}

impl ItsMode {
    pub const fn to_win_power_mode(self) -> WinPowerMode {
        match self {
            Self::Cool => WinPowerMode::BestPowerEfficiency,
            Self::Auto => WinPowerMode::Balanced,
            Self::Performance | Self::Geek => WinPowerMode::BestPerformance,
        }
    }
}

pub const fn build_set_function_cmd(function: u32, mode_param: u32, enable: bool) -> u32 {
    DYTC_CMD_SET_FUNCTION
        | (function << DYTC_SET_FUNCTION_SHIFT)
        | (mode_param << DYTC_SET_MODE_PARAMETER_SHIFT)
        | if enable { DYTC_SET_FUNCTION_ENABLE_BIT } else { 0 }
}

pub const fn build_disable_function_cmd(function: u32) -> u32 {
    build_set_function_cmd(function, DYTC_MODE_PARAMETER_INTELLIGENT_OR_DISABLE, false)
}

const fn current_function(raw: u32) -> u32 {
    (raw >> DYTC_GET_FUNCTION_SHIFT) & 0xF
}

const fn is_auto_mode_function(function: u32, mode_param: u32) -> bool {
    matches!(
        function,
        FN_FIRMWARE_DEFAULT | FN_INTELLIGENT_COOLING | FN_AUTO_6 | FN_AUTO_7 | FN_AUTO_8 | FN_AUTO_15
    ) || (function == FN_MANUAL && mode_param <= 3)
        || (function == FN_AUTO_13 && matches!(mode_param, 1 | 2))
}

pub const fn is_full_speed_compatible_readback(raw: u32) -> bool {
    raw == DYTC_DEFAULT_FUNCTION_READBACK
        || ((raw & DYTC_RESPONSE_VALID_BIT) != 0
            && current_function(raw) == FN_ITS_MODE_CONTROL
            && ((raw >> DYTC_GET_MODE_PARAMETER_SHIFT) & 0xF) == DYTC_MODE_PARAMETER_PERFORMANCE)
}

/// Giải mã phản hồi từ `DYTC_CMD_GET_MODE`.
pub const fn decode_mode(raw: u32, geek_active: bool, ac_connected: bool) -> Option<ItsMode> {
    if (raw & DYTC_RESPONSE_VALID_BIT) == 0 {
        return None;
    }
    let function = current_function(raw);
    let mode_param = (raw >> DYTC_GET_MODE_PARAMETER_SHIFT) & 0xF;

    if geek_active && ac_connected && is_full_speed_compatible_readback(raw) {
        Some(ItsMode::Geek)
    } else if function == FN_ITS_MODE_CONTROL && mode_param == DYTC_MODE_PARAMETER_COOL {
        Some(ItsMode::Cool)
    } else if function == FN_ITS_MODE_CONTROL && mode_param == DYTC_MODE_PARAMETER_PERFORMANCE {
        Some(ItsMode::Performance)
    } else if is_auto_mode_function(function, mode_param) || (function == FN_ITS_MODE_CONTROL && (raw >> 16) == 1) {
        Some(ItsMode::Auto)
    } else {
        None
    }
}

pub const fn has_base_capabilities(caps: u32) -> bool {
    (caps & REQUIRED_BASE_MODE_CAPABILITIES) == REQUIRED_BASE_MODE_CAPABILITIES
}

pub const fn has_geek_capability(ext_caps: u32) -> bool {
    (ext_caps & DYTC_FULL_SPEED_CAPABILITY_MASK) == DYTC_FULL_SPEED_CAPABILITY_MASK
}

fn send_cmd(dev: &Device, cmd: u32) -> io::Result<u32> {
    dev.ioctl(IOCTL_ENERGY_SMART_POWER, cmd)
}

/// Kiểm tra an toàn: firmware có thực sự hỗ trợ DYTC ITS qua `EnergyDrv` hay không.
pub fn is_supported(dev: &Device) -> bool {
    let Ok(caps) = send_cmd(dev, DYTC_CMD_GET_FUNCTION_CAPABILITIES) else {
        return false;
    };
    if !has_base_capabilities(caps) {
        return false;
    }
    let Ok(raw) = send_cmd(dev, DYTC_CMD_GET_MODE) else {
        return false;
    };
    (raw & DYTC_RESPONSE_VALID_BIT) != 0
}

/// Kiểm tra firmware có quảng bá chế độ Geek (Full Speed) hay không.
pub fn is_geek_advertised(dev: &Device) -> bool {
    send_cmd(dev, DYTC_CMD_GET_EXTENDED_CAPABILITIES).is_ok_and(has_geek_capability)
}

/// Danh sách các chế độ ITS được firmware máy này hỗ trợ.
pub fn supported_modes(dev: &Device) -> Vec<ItsMode> {
    if !is_supported(dev) {
        return Vec::new();
    }
    if is_geek_advertised(dev) {
        vec![ItsMode::Auto, ItsMode::Cool, ItsMode::Performance, ItsMode::Geek]
    } else {
        vec![ItsMode::Auto, ItsMode::Cool, ItsMode::Performance]
    }
}

/// Đọc chế độ ITS hiện tại từ `EnergyDrv`.
pub fn get(dev: &Device, geek_active: bool, ac_connected: bool) -> io::Result<Option<ItsMode>> {
    let raw = send_cmd(dev, DYTC_CMD_GET_MODE)?;
    Ok(decode_mode(raw, geek_active, ac_connected))
}

fn wait_for_mode(dev: &Device, expected: ItsMode, ac_connected: bool) -> bool {
    let start = Instant::now();
    loop {
        if let Ok(raw) = send_cmd(dev, DYTC_CMD_GET_MODE) {
            let ok = if expected == ItsMode::Geek {
                is_full_speed_compatible_readback(raw)
            } else {
                decode_mode(raw, false, ac_connected) == Some(expected)
            };
            if ok {
                return true;
            }
        }
        if start.elapsed() >= STATE_TIMEOUT {
            return false;
        }
        thread::sleep(POLL_STEP);
    }
}

/// Đổi chế độ nhiệt ITS an toàn qua `EnergyDrv` theo đúng trình tự của Lenovo.
pub fn set(dev: &Device, mode: ItsMode, ac_connected: bool) -> io::Result<()> {
    if !is_supported(dev) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "máy không hỗ trợ chế độ nhiệt ITS qua EnergyDrv",
        ));
    }

    let geek_supported = is_geek_advertised(dev);
    if mode == ItsMode::Geek {
        if !geek_supported {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "firmware không hỗ trợ chế độ Geek (Full Speed)",
            ));
        }
        if !ac_connected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "chế độ Geek yêu cầu đang cắm sạc AC",
            ));
        }
    }

    if geek_supported {
        let _ = send_cmd(dev, DYTC_CMD_DISABLE_FULL_SPEED);
    }

    let reset_required = if mode == ItsMode::Auto {
        send_cmd(dev, DYTC_CMD_GET_MODE)
            .ok()
            .is_some_and(|raw| !matches!(current_function(raw), FN_MANUAL | FN_UNKNOWN4))
    } else {
        false
    };

    for func in DISABLE_ORDER_BEFORE_SWITCH {
        send_cmd(dev, build_disable_function_cmd(func))?;
    }

    if reset_required {
        send_cmd(dev, DYTC_CMD_RESET)?;
    }

    let target_cmd = match mode {
        ItsMode::Auto => build_set_function_cmd(FN_INTELLIGENT_COOLING, DYTC_MODE_PARAMETER_INTELLIGENT_OR_DISABLE, true),
        ItsMode::Cool => build_set_function_cmd(FN_ITS_MODE_CONTROL, DYTC_MODE_PARAMETER_COOL, true),
        ItsMode::Performance | ItsMode::Geek => {
            build_set_function_cmd(FN_ITS_MODE_CONTROL, DYTC_MODE_PARAMETER_PERFORMANCE, true)
        }
    };
    send_cmd(dev, target_cmd)?;

    let base_state = if mode == ItsMode::Geek { ItsMode::Performance } else { mode };
    if !wait_for_mode(dev, base_state, ac_connected) {
        return Err(io::Error::other(format!("firmware không xác nhận chuyển sang chế độ {base_state:?}")));
    }

    if mode == ItsMode::Geek {
        for _ in 0..GEEK_ENABLE_REPEAT_COUNT {
            if let Err(e) = send_cmd(dev, DYTC_CMD_ENABLE_FULL_SPEED) {
                let _ = send_cmd(dev, DYTC_CMD_DISABLE_FULL_SPEED);
                return Err(e);
            }
        }
        if !wait_for_mode(dev, ItsMode::Geek, ac_connected) {
            let _ = send_cmd(dev, DYTC_CMD_DISABLE_FULL_SPEED);
            return Err(io::Error::other("firmware không xác nhận bật Geek / Full Speed"));
        }
    }

    power::sync_windows_power_mode_overlay(mode.to_win_power_mode());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_masks_validate_strictly() {
        assert!(!has_base_capabilities(0));
        assert!(!has_base_capabilities(DYTC_RESPONSE_VALID_BIT));
        assert!(has_base_capabilities(REQUIRED_BASE_MODE_CAPABILITIES));
        assert!(!has_geek_capability(0x0000_0001));
        assert!(has_geek_capability(0x0002_0001));
    }

    #[test]
    fn command_encoding_matches_dytc_spec() {
        assert_eq!(build_set_function_cmd(FN_ITS_MODE_CONTROL, DYTC_MODE_PARAMETER_COOL, true), 0x0013_B001);
        assert_eq!(
            build_set_function_cmd(FN_ITS_MODE_CONTROL, DYTC_MODE_PARAMETER_PERFORMANCE, true),
            0x0012_B001
        );
        assert_eq!(
            build_set_function_cmd(FN_INTELLIGENT_COOLING, DYTC_MODE_PARAMETER_INTELLIGENT_OR_DISABLE, true),
            0x001F_5001
        );
        assert_eq!(build_disable_function_cmd(FN_ITS_MODE_CONTROL), 0x000F_B001);
    }

    #[test]
    fn decode_mode_covers_all_states_and_invalid_bit() {
        // Bit 0 == 0 -> không hợp lệ
        assert_eq!(decode_mode(0x0000_3B00, false, true), None);
        // Cool: function=11 (0xB), mode=3, valid=1
        assert_eq!(decode_mode(0x0000_3B01, false, true), Some(ItsMode::Cool));
        // Performance: function=11 (0xB), mode=2, valid=1
        assert_eq!(decode_mode(0x0000_2B01, false, true), Some(ItsMode::Performance));
        // Geek chỉ trả về khi geek_active && ac_connected
        assert_eq!(decode_mode(0x0000_2B01, true, true), Some(ItsMode::Geek));
        assert_eq!(decode_mode(0x0000_2B01, true, false), Some(ItsMode::Performance));
        // Intelligent Cooling (Auto): function=5, valid=1
        assert_eq!(decode_mode(0x0000_F501, false, true), Some(ItsMode::Auto));
    }
}
