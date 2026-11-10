//! Các tính năng bật/tắt qua WMI LENOVO_GAMEZONE_DATA
//! (WinKeyFeature, GSyncFeature, OverDriveGameZoneFeature, TouchpadLockWmiFeature,
//! IGPUModeGamezoneFeature của bản C#).

use std::io;

use crate::wmi_util::Wmi;

const CLASS: &str = "LENOVO_GAMEZONE_DATA";
const OTHER: &str = "LENOVO_OTHER_METHOD";
const CAP_IGPU_MODE: i32 = 0x0001_0000;
const CAP_GPU_STATUS: i32 = 0x0207_0000;

/// Chế độ GPU Working Mode (`HybridModeState` của bản C#).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HybridMode {
    On,
    OnIGPUOnly,
    OnAuto,
    Off,
}

impl HybridMode {
    pub const LABELS: [(Self, &'static str); 4] = [
        (Self::On, "Hybrid"),
        (Self::OnIGPUOnly, "Hybrid-iGPU Only"),
        (Self::OnAuto, "Hybrid-Auto"),
        (Self::Off, "dGPU"),
    ];

    pub fn label(self) -> &'static str {
        Self::LABELS.iter().find(|(m, _)| *m == self).map_or("Hybrid", |(_, s)| s)
    }

    pub fn parse_label(s: &str) -> Option<Self> {
        Self::LABELS.iter().find(|(_, l)| *l == s).map(|(m, _)| *m)
    }

    pub fn pack(gsync: i32, igpu: i32) -> Self {
        if gsync != 0 {
            Self::Off
        } else {
            match igpu {
                1 => Self::OnIGPUOnly,
                2 => Self::OnAuto,
                _ => Self::On,
            }
        }
    }

    pub fn unpack(self) -> (i32, i32) {
        match self {
            Self::On => (0, 0),
            Self::OnIGPUOnly => (0, 1),
            Self::OnAuto => (0, 2),
            Self::Off => (1, 0),
        }
    }
}

pub fn hybrid_supported_modes(supports_gsync: bool, supports_igpu: bool) -> Vec<HybridMode> {
    match (supports_gsync, supports_igpu) {
        (true, true) => vec![HybridMode::On, HybridMode::OnIGPUOnly, HybridMode::OnAuto, HybridMode::Off],
        (false, true) => vec![HybridMode::On, HybridMode::OnIGPUOnly, HybridMode::OnAuto],
        (true, false) => vec![HybridMode::On, HybridMode::Off],
        (false, false) => Vec::new(),
    }
}

pub fn hybrid_get(w: &Wmi, supports_gsync: bool, supports_igpu: bool) -> io::Result<HybridMode> {
    let gsync = if supports_gsync { get_with(w, Feature::GSync).unwrap_or(0) } else { 0 };
    let igpu = if supports_igpu {
        get_with(w, Feature::IGpuMode)
            .or_else(|_| w.call_i32(OTHER, "GetFeatureValue", &[("IDs", CAP_IGPU_MODE)], "Value"))
            .unwrap_or(0)
    } else {
        0
    };
    Ok(HybridMode::pack(gsync, igpu))
}

pub fn hybrid_set(w: &Wmi, supports_gsync: bool, supports_igpu: bool, mode: HybridMode) -> io::Result<()> {
    let (gsync, igpu) = mode.unpack();
    if supports_gsync && get_with(w, Feature::GSync).ok() != Some(gsync) {
        set_with(w, Feature::GSync, gsync)?;
    }
    if supports_igpu {
        let cur = get_with(w, Feature::IGpuMode)
            .or_else(|_| w.call_i32(OTHER, "GetFeatureValue", &[("IDs", CAP_IGPU_MODE)], "Value"))
            .ok();
        if cur != Some(igpu) && set_with(w, Feature::IGpuMode, igpu).is_err() {
            w.call(OTHER, "SetFeatureValue", &[("IDs", CAP_IGPU_MODE), ("Value", igpu)])?;
        }
    }
    Ok(())
}

pub fn gpu_oc_supported(w: &Wmi) -> bool {
    w.call_i32(CLASS, "IsSupportGpuOC", &[], "Data").is_ok_and(|v| v > 0)
}

pub fn deactivate_dgpu(w: &Wmi) -> io::Result<()> {
    let _ = w.call(OTHER, "SetFeatureValue", &[("IDs", CAP_GPU_STATUS), ("Value", 0)]);
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    WinKey,
    GSync,
    OverDrive,
    TouchpadLock,
    /// 0 = Default, 1 = IGPUOnly, 2 = Auto
    IGpuMode,
}

impl Feature {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "winkey" => Self::WinKey,
            "gsync" => Self::GSync,
            "overdrive" => Self::OverDrive,
            "touchpad" => Self::TouchpadLock,
            "igpu" => Self::IGpuMode,
            _ => return None,
        })
    }

    /// (is_support, get, set, tên tham số vào của set)
    fn methods(self) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            Self::WinKey => ("IsSupportDisableWinKey", "GetWinKeyStatus", "SetWinKeyStatus", "Data"),
            Self::GSync => ("IsSupportGSync", "GetGSyncStatus", "SetGSyncStatus", "Data"),
            Self::OverDrive => ("IsSupportOD", "GetODStatus", "SetODStatus", "Data"),
            Self::TouchpadLock => ("IsSupportDisableTP", "GetTPStatus", "SetTPStatus", "Data"),
            Self::IGpuMode => ("IsSupportIGPUMode", "GetIGPUModeStatus", "SetIGPUModeStatus", "mode"),
        }
    }

    pub fn max_value(self) -> i32 {
        if self == Self::IGpuMode { 2 } else { 1 }
    }
}

pub fn is_supported(feature: Feature) -> io::Result<bool> {
    is_supported_with(&Wmi::open()?, feature)
}

pub fn is_supported_with(w: &Wmi, feature: Feature) -> io::Result<bool> {
    let (support, ..) = feature.methods();
    Ok(w.call_i32(CLASS, support, &[], "Data")? > 0)
}

pub fn get(feature: Feature) -> io::Result<i32> {
    get_with(&Wmi::open()?, feature)
}

pub fn get_with(w: &Wmi, feature: Feature) -> io::Result<i32> {
    let (_, get, ..) = feature.methods();
    w.call_i32(CLASS, get, &[], "Data")
}

pub fn set(feature: Feature, value: i32) -> io::Result<()> {
    if !(0..=feature.max_value()).contains(&value) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{feature:?}: giá trị phải trong 0..={}", feature.max_value()),
        ));
    }
    set_with(&Wmi::open()?, feature, value)
}

pub fn set_with(w: &Wmi, feature: Feature, value: i32) -> io::Result<()> {
    if !(0..=feature.max_value()).contains(&value) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{feature:?}: giá trị phải trong 0..={}", feature.max_value()),
        ));
    }
    let (_, _, set, param) = feature.methods();
    w.call(CLASS, set, &[(param, value)])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_ranges() {
        assert_eq!(Feature::parse("igpu"), Some(Feature::IGpuMode));
        assert_eq!(Feature::parse("nope"), None);
        assert_eq!(Feature::IGpuMode.max_value(), 2);
        assert_eq!(Feature::WinKey.max_value(), 1);
    }

    #[test]
    fn set_rejects_out_of_range_before_touching_wmi() {
        assert!(set(Feature::GSync, 2).is_err());
        assert!(set(Feature::IGpuMode, -1).is_err());
    }
}
