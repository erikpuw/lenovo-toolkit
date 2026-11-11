//! Power mode (Quiet/Balance/Performance/...) qua WMI LENOVO_GAMEZONE_DATA
//! (port từ PowerModeFeature.cs + WMI.LenovoGameZoneData.cs).

use std::io;

use crate::compat::MachineInfo;
use crate::wmi_util::Wmi;

const CLASS: &str = "LENOVO_GAMEZONE_DATA";
const OTHER: &str = "LENOVO_OTHER_METHOD";
const CAP_GODMODE_FNQ: i32 = 0x0010_0000;
/// Giá trị WMI = giá trị enum C# + 1 (AbstractWmiFeature offset = 1).
const OFFSET: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerMode {
    Quiet,
    Balance,
    Performance,
    Extreme,
    GodMode,
}

impl PowerMode {
    const ALL: [Self; 5] = [Self::Quiet, Self::Balance, Self::Performance, Self::Extreme, Self::GodMode];

    const fn enum_value(self) -> u32 {
        match self {
            Self::Quiet => 0,
            Self::Balance => 1,
            Self::Performance => 2,
            Self::Extreme => 223,
            Self::GodMode => 254,
        }
    }

    pub fn to_wmi(self) -> u32 {
        self.enum_value() + OFFSET
    }

    pub fn from_wmi(raw: u32) -> Option<Self> {
        let v = raw.checked_sub(OFFSET)?;
        Self::ALL.into_iter().find(|m| m.enum_value() == v)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterStatus {
    Connected,
    ConnectedLowWattage,
    Disconnected,
}

/// Quy tắc của `Power.IsPowerAdapterConnectedAsync`: không đọc được thì coi như đang cắm;
/// cắm nhưng bộ sạc yếu hoặc sạc không bình thường thì `ConnectedLowWattage`.
pub fn adapter_from(adapter_online: bool, fit_for_oc: Option<bool>, charging_normally: Option<bool>) -> AdapterStatus {
    let good = fit_for_oc.unwrap_or(true) && charging_normally.unwrap_or(true);
    match (adapter_online, good) {
        (true, false) => AdapterStatus::ConnectedLowWattage,
        (true, true) => AdapterStatus::Connected,
        (false, _) => AdapterStatus::Disconnected,
    }
}

pub fn adapter_status_with(w: &Wmi) -> AdapterStatus {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

    let mut sps = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut sps) }.is_err() {
        return AdapterStatus::Connected;
    }
    if sps.ACLineStatus != 1 {
        return AdapterStatus::Disconnected;
    }
    let ask = |method: &str| -> Option<bool> { w.call_i32(CLASS, method, &[], "Data").ok().map(|v| v == 1) };
    adapter_from(true, ask("IsACFitForOC"), ask("GetPowerChargeMode"))
}

pub fn adapter_status() -> AdapterStatus {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

    let mut sps = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut sps) }.is_err() {
        return AdapterStatus::Connected;
    }
    if sps.ACLineStatus != 1 {
        return AdapterStatus::Disconnected;
    }
    let Ok(w) = Wmi::open() else {
        return AdapterStatus::Connected;
    };
    adapter_status_with(&w)
}

pub fn is_supported() -> io::Result<bool> {
    Ok(Wmi::open()?.call_i32(CLASS, "IsSupportSmartFan", &[], "Data")? > 0)
}

pub fn get_with(w: &Wmi) -> io::Result<PowerMode> {
    let raw = w.call_i32(CLASS, "GetSmartFanMode", &[], "Data")?;
    u32::try_from(raw)
        .ok()
        .and_then(PowerMode::from_wmi)
        .ok_or_else(|| io::Error::other(format!("giá trị power mode lạ: {raw}")))
}

pub fn get() -> io::Result<PowerMode> {
    get_with(&Wmi::open()?)
}

/// Ghi thẳng chế độ nguồn, không kiểm tra gì. Giao diện và dòng lệnh dùng `switch_with`.
pub fn set_with(w: &Wmi, mode: PowerMode) -> io::Result<()> {
    w.call(CLASS, "SetSmartFanMode", &[("Data", mode.to_wmi() as i32)])?;
    sync_windows_power_mode(mode);
    Ok(())
}

/// Chế độ chỉ chuyển được khi cắm sạc (bản C# từ chối khi máy chạy pin).
pub fn needs_ac(mode: PowerMode) -> bool {
    matches!(mode, PowerMode::Performance | PowerMode::Extreme | PowerMode::GodMode)
}

/// Chế độ trung gian để né lỗi firmware khi chuyển, như `PowerModeFeature.SetStateAsync` của bản C#:
/// BIOS từ J2CN chuyển Quiet→Performance phải qua Balance; BIOS từ K1CN rời Custom phải qua một chế độ khác trước.
pub fn intermediate(current: Option<PowerMode>, target: PowerMode, quiet_bug: bool, god_bug: bool) -> Option<PowerMode> {
    use PowerMode::*;
    match current {
        Some(Quiet) if quiet_bug && target == Performance => Some(Balance),
        Some(GodMode) if god_bug && target != GodMode => Some(match target {
            Quiet => Performance,
            Balance => Quiet,
            Performance => Balance,
            Extreme | GodMode => Extreme,
        }),
        _ => None,
    }
}

/// Đổi chế độ nguồn theo đúng quy tắc của bản C#: chỉ chế độ máy hỗ trợ, các chế độ hiệu năng cao phải cắm sạc,
/// và đi qua chế độ trung gian trên những BIOS bị lỗi chuyển chế độ.
pub fn switch_with(w: &Wmi, info: Option<&MachineInfo>, mode: PowerMode) -> io::Result<()> {
    if let Some(i) = info
        && !i.supported_power_modes.is_empty()
        && !i.supported_power_modes.contains(&mode)
    {
        return Err(io::Error::new(io::ErrorKind::Unsupported, format!("máy không hỗ trợ chế độ {mode:?}")));
    }
    if needs_ac(mode) && adapter_status_with(w) == AdapterStatus::Disconnected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("chế độ {mode:?} cần cắm sạc (máy đang chạy pin)"),
        ));
    }
    let (quiet_bug, god_bug) = info.map_or((false, false), |i| {
        (i.properties.has_quiet_to_performance_switching_bug, i.properties.has_god_mode_to_other_mode_switching_bug)
    });
    if let Some(step) = intermediate(get_with(w).ok(), mode, quiet_bug, god_bug) {
        w.call(CLASS, "SetSmartFanMode", &[("Data", step.to_wmi() as i32)])?;
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    set_with(w, mode)
}

/// Đọc trạng thái "Switch to Custom Mode with Fn+Q" (`CapabilityID.GodModeFnQSwitchable = 0x0010_0000`).
pub fn god_mode_fnq_get(w: &Wmi) -> io::Result<bool> {
    Ok(w.call_i32(OTHER, "GetFeatureValue", &[("IDs", CAP_GODMODE_FNQ)], "Value")? == 1)
}

/// Đặt trạng thái "Switch to Custom Mode with Fn+Q".
pub fn god_mode_fnq_set(w: &Wmi, on: bool) -> io::Result<()> {
    w.call(
        OTHER,
        "SetFeatureValue",
        &[("IDs", CAP_GODMODE_FNQ), ("Value", i32::from(on))],
    )?;
    Ok(())
}

/// Đồng bộ Windows Power Mode overlay (`powrprof.dll::PowerSetActiveOverlayScheme`) theo chế độ nguồn.
pub fn sync_windows_power_mode(mode: PowerMode) {
    use crate::settings::{self, PowerSyncMode};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::{GUID, s, w};

    if settings::power_sync_mode() != PowerSyncMode::WindowsPowerMode {
        return;
    }
    let guid = match mode {
        PowerMode::Quiet => GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a),
        PowerMode::Balance => GUID::zeroed(),
        PowerMode::Performance | PowerMode::Extreme | PowerMode::GodMode => {
            GUID::from_u128(0xded574b5_45a0_4f42_8737_46345c09c238)
        }
    };
    unsafe {
        if let Ok(dll) = LoadLibraryW(w!("powrprof.dll"))
            && let Some(proc) = GetProcAddress(dll, s!("PowerSetActiveOverlayScheme"))
        {
            let set_overlay: unsafe extern "system" fn(*const GUID) -> u32 = std::mem::transmute(proc);
            let _ = set_overlay(&guid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wmi_values_have_offset_one() {
        assert_eq!(PowerMode::Quiet.to_wmi(), 1);
        assert_eq!(PowerMode::Performance.to_wmi(), 3);
        assert_eq!(PowerMode::Extreme.to_wmi(), 224);
        assert_eq!(PowerMode::GodMode.to_wmi(), 255);
    }

    #[test]
    fn high_power_modes_need_ac_like_csharp() {
        assert!(needs_ac(PowerMode::Performance) && needs_ac(PowerMode::Extreme) && needs_ac(PowerMode::GodMode));
        assert!(!needs_ac(PowerMode::Quiet) && !needs_ac(PowerMode::Balance));
    }

    #[test]
    fn firmware_bug_routes_match_csharp() {
        use PowerMode::*;
        // Không có lỗi BIOS: không đi vòng.
        assert_eq!(intermediate(Some(Quiet), Performance, false, false), None);
        // J2CN+: Quiet → Performance đi qua Balance; các cặp khác thì không.
        assert_eq!(intermediate(Some(Quiet), Performance, true, false), Some(Balance));
        assert_eq!(intermediate(Some(Quiet), Balance, true, false), None);
        // K1CN+: rời Custom đi qua chế độ trung gian của bản C#.
        assert_eq!(intermediate(Some(GodMode), Quiet, false, true), Some(Performance));
        assert_eq!(intermediate(Some(GodMode), Balance, false, true), Some(Quiet));
        assert_eq!(intermediate(Some(GodMode), Performance, false, true), Some(Balance));
        assert_eq!(intermediate(Some(GodMode), Extreme, false, true), Some(Extreme));
        assert_eq!(intermediate(Some(GodMode), GodMode, false, true), None);
        assert_eq!(intermediate(None, Performance, true, true), None);
    }

    #[test]
    fn adapter_rules_match_csharp() {
        use AdapterStatus::*;
        assert_eq!(adapter_from(true, Some(true), Some(true)), Connected);
        assert_eq!(adapter_from(true, None, None), Connected);
        assert_eq!(adapter_from(true, Some(false), Some(true)), ConnectedLowWattage);
        assert_eq!(adapter_from(true, Some(true), Some(false)), ConnectedLowWattage);
        assert_eq!(adapter_from(false, Some(false), Some(false)), Disconnected);
    }

    #[test]
    fn from_wmi_roundtrip_and_rejects_unknown() {
        for m in PowerMode::ALL {
            assert_eq!(PowerMode::from_wmi(m.to_wmi()), Some(m));
        }
        assert_eq!(PowerMode::from_wmi(0), None);
        assert_eq!(PowerMode::from_wmi(50), None);
    }
}
