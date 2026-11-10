//! Nhận diện máy và khả năng hỗ trợ (port từ Utils/Compatibility.cs).
//!
//! Chưa có: chế độ fake machine information (fake_mi.json), `FeatureData` từ
//! LENOVO_CAPABILITY_DATA_00/flags, `SupportsAlwaysOnAc`, in thông tin máy dạng cây.
//! `is_amd_device` lấy kết quả khớp cuối cùng thay cho RegexOptions.RightToLeft.

use std::io;

use regex::Regex;
use serde::Deserialize;
use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

use crate::driver::{ACCESS_READ, Device, IBM_PM_DRV};
use crate::gamezone::{self, Feature};
use crate::power::PowerMode;
use crate::wmi_util::Wmi;

const ALLOWED_VENDORS: [&str; 2] = ["LENOVO", "MOTOROLA"];

const ALLOWED_MODELS_PREFIX: &[&str] = &[
    // Legion Go
    "8APU1",
    // Worldwide variants
    "17ACH", "17ARH", "17ITH", "17IMH", "17ITL",
    "16ACH", "16ADR", "16AFR", "16AHP", "16APH", "16ARH", "16ARP", "16ARX", "16IAH", "16IAX", "16IMH",
    "16IRH", "16IRX", "16ITH",
    "18IAX", "NX",
    "15ACH", "15AHP", "15ALC", "15IPH", "15AKP", "15APH", "15ARH", "15ARP", "15ASH", "15IAH", "15IAX",
    "15IHU", "15IMH", "15IRH", "15IRX", "15ITH",
    "14AHP", "14APH", "14IRP", "14AKP", "14IRH", "14IAP",
    // Chinese variants
    "G5000", "R9000", "R7000", "Y9000", "Y7000",
    // Limited compatibility
    "17IR", "15IR", "15IC", "15IK",
    // ThinkBooks
    "ThinkBook",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegionSeries {
    Legion5 = 0,
    LegionPro5 = 1,
    LegionSlim5 = 2,
    Legion7 = 3,
    LegionPro7 = 4,
    Legion9 = 5,
    LegionGo = 6,
    LenovoSlim = 7,
    LegionLegacy = 8,
    IdeaPad = 9,
    IdeaPadGaming = 10,
    Loq = 11,
    Yoga = 12,
    ThinkBook = 13,
    Motorola = 14,
    Unknown = 255,
}

impl LegionSeries {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Legion5 => "Legion 5",
            Self::LegionPro5 => "Legion Pro 5",
            Self::LegionSlim5 => "Legion Slim 5",
            Self::Legion7 => "Legion 7",
            Self::LegionPro7 => "Legion Pro 7",
            Self::Legion9 => "Legion 9",
            Self::LegionGo => "Legion Go",
            Self::LenovoSlim => "Lenovo Slim",
            Self::LegionLegacy => "Legion Legacy",
            Self::IdeaPad => "IdeaPad",
            Self::IdeaPadGaming => "IdeaPad Gaming",
            Self::Loq => "LOQ",
            Self::Yoga => "Yoga",
            Self::ThinkBook => "ThinkBook",
            Self::Motorola => "Motorola",
            Self::Unknown => "",
        }
    }
}

use LegionSeries as S;

const MACHINE_TYPES: [(LegionSeries, &[&str]); 7] = [
    (S::Legion5, &["83F0", "83F1", "83M0", "83NX", "83N2", "83LY", "83DG", "83EW", "83EG", "83JJ", "82RC", "82RB",
        "82TB", "83EF", "82RE", "82RD", "83Q7", "83Q6", "83RW", "83VK"]),
    (S::LegionSlim5, &["83DH", "83EX", "82Y5", "82Y9", "82YA", "83D6"]),
    (S::LegionPro5, &["83LT", "83F3", "83DF", "83F2", "83LU", "82WM", "83NN", "82WK", "82JQ"]),
    (S::Legion7, &["83KY", "83FD", "82UH", "82TD", "82N6", "83V9"]),
    (S::LegionPro7, &["83RU", "83F5", "83DE", "82WR", "82WQ", "82WS"]),
    (S::Legion9, &["83G0", "83EY"]),
    (S::LegionGo, &["83E1"]),
];

const MODEL_KEYWORDS: [(&str, LegionSeries); 10] = [
    ("LOQ", S::Loq),
    ("IdeaPad Gaming", S::IdeaPadGaming),
    ("IdeaPad", S::IdeaPad),
    ("XiaoXin", S::IdeaPad),
    ("YOGA", S::Yoga),
    ("Lenovo Slim", S::LenovoSlim),
    ("ThinkBook", S::ThinkBook),
    ("Legion", S::LegionLegacy),
    ("Motorola", S::Motorola),
    ("Motobook", S::Motorola),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GodModePlatform {
    LegacyLegion,
    Legion,
    NonGaming,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiosVersion {
    pub prefix: String,
    pub version: Option<i32>,
}

impl BiosVersion {
    fn new(prefix: &str, version: Option<i32>) -> Self {
        Self { prefix: prefix.into(), version }
    }

    pub fn is_higher_or_equal_than(&self, other: &Self) -> bool {
        if !self.prefix.eq_ignore_ascii_case(&other.prefix) {
            return false;
        }
        match (self.version, other.version) {
            (Some(a), Some(b)) => a >= b,
            _ => true,
        }
    }

    pub fn is_lower_than(&self, other: &Self) -> bool {
        if !self.prefix.eq_ignore_ascii_case(&other.prefix) {
            return false;
        }
        match (self.version, other.version) {
            (Some(a), Some(b)) => a < b,
            _ => true,
        }
    }
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

pub fn legion_series(model: &str, machine_type: &str) -> LegionSeries {
    for (series, types) in MACHINE_TYPES {
        if types.contains(&machine_type) {
            return series;
        }
    }
    MODEL_KEYWORDS
        .iter()
        .find(|(kw, _)| contains_ci(model, kw))
        .map_or(S::Unknown, |(_, s)| *s)
}

pub fn is_legion(series: LegionSeries) -> bool {
    matches!(
        series,
        S::Legion5 | S::LegionPro5 | S::LenovoSlim | S::LegionSlim5 | S::Legion7 | S::LegionPro7 | S::Legion9
            | S::LegionGo | S::Loq | S::LegionLegacy | S::IdeaPadGaming
    )
}

fn parse_digits(b: &[u8], start: usize, max_len: usize) -> i32 {
    let end = (start + max_len).min(b.len());
    let mut n = 0i32;
    for &c in &b[start..end] {
        if !c.is_ascii_digit() {
            break;
        }
        n = n.saturating_mul(10).saturating_add(i32::from(c - b'0'));
    }
    n
}

/// Thế hệ máy suy ra từ tên model (ba cách theo thứ tự như bản C#).
pub fn machine_generation(model: &str) -> i32 {
    let b = model.as_bytes();

    // 1) 1-2 chữ số đứng ngay sau 3 chữ cái: "(?<=[A-Z]{3})\d{1,2}"
    for i in 3..b.len() {
        if b[i].is_ascii_digit() && b[i - 3..i].iter().all(u8::is_ascii_alphabetic) {
            return parse_digits(b, i, 2);
        }
    }
    // 2) "g\d+"
    for i in 0..b.len().saturating_sub(1) {
        if (b[i] == b'g' || b[i] == b'G') && b[i + 1].is_ascii_digit() {
            return parse_digits(b, i + 1, usize::MAX - i - 1);
        }
    }
    // 3) cụm số đúng 1-2 chữ số, bỏ 14..=18
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i - start <= 2 {
                let v = parse_digits(b, start, 2);
                if !(14..=18).contains(&v) {
                    return v;
                }
            }
        } else {
            i += 1;
        }
    }
    0
}

pub fn is_chinese_model(model: &str) -> bool {
    ["R7000", "R9000", "Y7000", "Y9000"].iter().any(|m| model.contains(m))
}

pub fn is_amd_device(model: &str) -> bool {
    if model.is_empty() {
        return false;
    }
    let re = Regex::new(r"(?:[AI][A-Z]{2}(?:\d+|\b))|(?:\bR\d{4})").expect("regex hợp lệ");
    re.find_iter(&model.to_uppercase())
        .last()
        .is_some_and(|m| m.as_str().starts_with(['A', 'R']))
}

pub fn supports_its_mode(model: &str) -> bool {
    if contains_ci(model, "IdeaPad Gaming") {
        return false;
    }
    ["IdeaPad", "ThinkBook", "Lenovo Slim", "Motobook", "XiaoXin", "YOGA"]
        .iter()
        .any(|k| contains_ci(model, k))
}

/// Nhóm thiết bị quyết định bộ điều khiển nào dùng được.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    /// Legion, LOQ, IdeaPad Gaming: chế độ nguồn WMI SmartFan, quạt WMI, Custom Mode (Quiet..Custom).
    Gaming,
    /// ThinkBook, IdeaPad, Slim, Yoga, XiaoXin, Motobook: dùng ITS (Intelligent Thermal Solution) qua
    /// driver với các chế độ khác (Auto/Cool/Performance/Geek); chưa được hỗ trợ ở phiên bản này.
    NonGaming,
}

pub fn device_class(model: &str) -> DeviceClass {
    if supports_its_mode(model) { DeviceClass::NonGaming } else { DeviceClass::Gaming }
}

impl MachineInfo {
    pub fn class(&self) -> DeviceClass {
        device_class(&self.model)
    }
}

/// Parse chuỗi BIOS dạng "GKCN54WW": 4 ký tự đầu [A-Z0-9] và cặp chữ số đầu tiên.
pub fn parse_bios(raw: &str) -> (Option<BiosVersion>, Option<String>) {
    let s = raw.trim();
    let prefix: String = s
        .chars()
        .take(4)
        .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        .collect();
    let prefix = if prefix.len() == 4 { prefix } else { String::new() };
    let b = s.as_bytes();
    let version = (0..b.len().saturating_sub(1))
        .find(|&i| b[i].is_ascii_digit() && b[i + 1].is_ascii_digit())
        .map(|i| parse_digits(b, i, 2));
    match version {
        Some(v) => (Some(BiosVersion { prefix, version: Some(v) }), Some(s.to_string())),
        None => (None, None),
    }
}

fn supports_extreme_mode(modes: &[PowerMode], smart_fan: i32, zone: i32) -> bool {
    modes.contains(&PowerMode::Extreme) && (matches!(smart_fan, 6..=8) || matches!(zone, 3..=5))
}

fn supports_god_mode_v1(smart_fan: i32, zone: i32, bios: Option<&BiosVersion>) -> bool {
    let affected = [
        BiosVersion::new("G9CN", Some(24)),
        BiosVersion::new("GKCN", Some(46)),
        BiosVersion::new("H1CN", Some(39)),
        BiosVersion::new("HACN", Some(31)),
        BiosVersion::new("HHCN", Some(20)),
    ];
    if affected.iter().any(|a| bios.is_some_and(|b| b.is_lower_than(a))) {
        return false;
    }
    matches!(smart_fan, 4 | 5) || matches!(zone, 1 | 2)
}

fn supports_god_mode_v3(smart_fan: i32, zone: i32, gen_: i32, model: &str, machine_type: &str) -> bool {
    let series = legion_series(model, machine_type);
    let affected_series = matches!(series, S::Legion5 | S::Legion7);
    let affected_model = ["Legion 5", "Legion 7", "Legion Pro 5 16IAX10H", "LOQ", "Y7000", "R7000"]
        .iter()
        .any(|m| model.contains(m));
    let supported = matches!(smart_fan, 8 | 9) || matches!(zone, 5 | 6);
    (affected_series || affected_model) && supported && gen_ >= 10
}

#[allow(clippy::too_many_arguments)]
pub fn god_mode_platform(
    modes: &[PowerMode],
    smart_fan: i32,
    zone: i32,
    gen_: i32,
    model: &str,
    machine_type: &str,
    bios: Option<&BiosVersion>,
) -> Option<GodModePlatform> {
    if supports_its_mode(model) {
        return Some(GodModePlatform::NonGaming);
    }
    if !modes.contains(&PowerMode::GodMode) {
        return None;
    }
    if supports_god_mode_v1(smart_fan, zone, bios) {
        return Some(GodModePlatform::LegacyLegion);
    }
    if supports_god_mode_v3(smart_fan, zone, gen_, model, machine_type)
        || matches!(smart_fan, 8 | 9)
        || matches!(zone, 5 | 6)
        || matches!(smart_fan, 6 | 7)
        || matches!(zone, 3 | 4)
    {
        return Some(GodModePlatform::Legion);
    }
    None
}

fn has_quiet_to_performance_bug(bios: Option<&BiosVersion>) -> bool {
    bios.is_some_and(|b| b.is_higher_or_equal_than(&BiosVersion::new("J2CN", None)))
}

fn has_god_mode_to_other_bug(bios: Option<&BiosVersion>) -> bool {
    bios.is_some_and(|b| b.is_higher_or_equal_than(&BiosVersion::new("K1CN", None)))
}

fn is_excluded_from_lenovo_lighting(bios: Option<&BiosVersion>, gen_: i32, series: LegionSeries) -> bool {
    if series == S::Legion7 && gen_ == 6 {
        return true; // RGB do firmware điều khiển
    }
    bios.is_some_and(|b| b.is_lower_than(&BiosVersion::new("GKCN", Some(54))))
}

fn is_excluded_from_panel_logo_lighting(machine_type: &str, model: &str) -> bool {
    const EXCLUDED: [(&str, &str); 11] = [
        ("82JH", "15ITH6H"), ("82JK", "15ITH6"), ("82JM", "17ITH6H"), ("82JN", "17ITH6"), ("82JU", "15ACH6H"),
        ("82JW", "15ACH6"), ("82JY", "17ACH6H"), ("82K0", "17ACH6"), ("82K1", "15IHU6"), ("82K2", "15ACH6"),
        ("82NW", "15ACH6A"),
    ];
    EXCLUDED.iter().any(|(mt, m)| machine_type.contains(mt) && model.contains(m))
}

fn has_alternative_full_spectrum_layout(machine_type: &str) -> bool {
    matches!(machine_type, "83G0" | "83AG")
}

/// Overdrive không hỗ trợ trên Legion 7/Pro 7 từ thế hệ 10.
pub fn is_overdrive_supported(series: LegionSeries, gen_: i32) -> bool {
    !matches!(series, S::Legion7 | S::LegionPro7) || gen_ < 10
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Properties {
    pub supports_extreme_mode: bool,
    pub god_mode_platform: Option<GodModePlatform>,
    pub supports_gsync: bool,
    pub supports_igpu_mode: bool,
    pub supports_ai_mode: bool,
    pub supports_boot_logo_change: bool,
    pub supports_its_mode: bool,
    pub has_quiet_to_performance_switching_bug: bool,
    pub has_god_mode_to_other_mode_switching_bug: bool,
    pub is_excluded_from_lenovo_lighting: bool,
    pub is_excluded_from_panel_logo_lenovo_lighting: bool,
    pub has_alternative_full_spectrum_layout: bool,
    pub is_amd_device: bool,
    pub is_chinese_model: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineInfo {
    pub vendor: String,
    pub machine_type: String,
    pub model: String,
    pub serial_number: String,
    pub generation: i32,
    pub series: LegionSeries,
    pub bios: Option<BiosVersion>,
    pub bios_raw: Option<String>,
    pub supported_power_modes: Vec<PowerMode>,
    pub smart_fan_version: i32,
    pub legion_zone_version: i32,
    pub properties: Properties,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct ProductRow {
    Vendor: Option<String>,
    Name: Option<String>,
    Version: Option<String>,
    IdentifyingNumber: Option<String>,
}

const OTHER: &str = "LENOVO_OTHER_METHOD";
const CAP_SUPPORTED_POWER_MODES: i32 = 0x0007_0000;
const CAP_LEGION_ZONE_VERSION: i32 = 0x0009_0000;

fn supported_power_modes(w: &Wmi) -> Vec<PowerMode> {
    let from_bits = |v: i32, bits: &[(u32, PowerMode)]| -> Vec<PowerMode> {
        bits.iter().filter(|(bit, _)| v & (1 << bit) != 0).map(|(_, m)| *m).collect()
    };
    if let Ok(v) = w.call_i32(OTHER, "GetFeatureValue", &[("IDs", CAP_SUPPORTED_POWER_MODES)], "Value") {
        return from_bits(v, &[
            (0, PowerMode::Quiet), (1, PowerMode::Balance), (2, PowerMode::Performance),
            (4, PowerMode::Extreme), (16, PowerMode::GodMode),
        ]);
    }
    if let Ok(v) = w.call_i32(OTHER, "GetSupportThermalMode", &[], "mode") {
        return from_bits(v, &[
            (0, PowerMode::Quiet), (1, PowerMode::Balance), (2, PowerMode::Performance), (16, PowerMode::GodMode),
        ]);
    }
    Vec::new()
}

fn legion_zone_version(w: &Wmi) -> i32 {
    w.call_i32(OTHER, "GetFeatureValue", &[("IDs", CAP_LEGION_ZONE_VERSION)], "Value")
        .or_else(|_| w.call_i32(OTHER, "Get_Support_LegionZone_Version", &[], "Version"))
        .unwrap_or(-1)
}

fn read_bios_reg() -> (String, String, String, String) {
    let Ok(k) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"HARDWARE\DESCRIPTION\System\BIOS", KEY_READ)
    else {
        return (String::new(), String::new(), String::new(), String::new());
    };
    let get = |name| k.get_value::<String, _>(name).unwrap_or_default().trim().to_string();
    let vendor = get("SystemManufacturer");
    let machine_type = get("SystemProductName");
    let model = {
        let v = get("SystemVersion");
        if v.is_empty() { get("SystemFamily") } else { v }
    };
    let bios_raw = get("BIOSVersion");
    (vendor, machine_type, model, bios_raw)
}

/// Truy vấn phần cứng thật rồi tính toàn bộ thông tin máy.
pub fn detect() -> io::Result<MachineInfo> {
    detect_with(&Wmi::open()?)
}

/// Truy vấn thông tin máy bằng kết nối `Wmi` có sẵn và ưu tiên đọc Registry để khởi động tức thì.
pub fn detect_with(w: &Wmi) -> io::Result<MachineInfo> {
    let (mut vendor, mut machine_type, mut model, bios_raw_reg) = read_bios_reg();
    let mut serial_number = String::new();
    if vendor.is_empty() || machine_type.is_empty() || model.is_empty() {
        let cim = Wmi::open_cimv2()?;
        let row = cim
            .query::<ProductRow>("SELECT Vendor, Name, Version, IdentifyingNumber FROM Win32_ComputerSystemProduct")?
            .into_iter()
            .next()
            .ok_or_else(|| io::Error::other("không đọc được Win32_ComputerSystemProduct"))?;
        vendor = row.Vendor.unwrap_or_default();
        machine_type = row.Name.unwrap_or_default();
        model = row.Version.unwrap_or_default();
        serial_number = row.IdentifyingNumber.unwrap_or_default();
    }

    let generation = machine_generation(&model);
    let series = legion_series(&model, &machine_type);
    let (bios, bios_raw) = parse_bios(&bios_raw_reg);
    let modes = supported_power_modes(w);
    let smart_fan = w.call_i32("LENOVO_GAMEZONE_DATA", "IsSupportSmartFan", &[], "Data").unwrap_or(-1);
    let zone = legion_zone_version(w);

    let properties = Properties {
        supports_extreme_mode: supports_extreme_mode(&modes, smart_fan, zone),
        god_mode_platform: god_mode_platform(&modes, smart_fan, zone, generation, &model, &machine_type, bios.as_ref()),
        supports_gsync: gamezone::is_supported_with(w, Feature::GSync).unwrap_or(false),
        supports_igpu_mode: gamezone::is_supported_with(w, Feature::IGpuMode).unwrap_or(false)
            || w.call_i32(OTHER, "GetFeatureValue", &[("IDs", 0x0001_0000)], "Value").is_ok(),
        supports_ai_mode: w.call_i32("LENOVO_GAMEZONE_DATA", "GetIntelligentSubMode", &[], "Data").is_ok(),
        supports_boot_logo_change: true,
        supports_its_mode: supports_its_mode(&model),
        has_quiet_to_performance_switching_bug: has_quiet_to_performance_bug(bios.as_ref()),
        has_god_mode_to_other_mode_switching_bug: has_god_mode_to_other_bug(bios.as_ref()),
        is_excluded_from_lenovo_lighting: is_excluded_from_lenovo_lighting(bios.as_ref(), generation, series),
        is_excluded_from_panel_logo_lenovo_lighting: is_excluded_from_panel_logo_lighting(&machine_type, &model),
        has_alternative_full_spectrum_layout: has_alternative_full_spectrum_layout(&machine_type),
        is_amd_device: is_amd_device(&model),
        is_chinese_model: is_chinese_model(&model),
    };

    Ok(MachineInfo {
        vendor,
        machine_type,
        model,
        serial_number,
        generation,
        series,
        bios,
        bios_raw,
        supported_power_modes: modes,
        smart_fan_version: smart_fan,
        legion_zone_version: zone,
        properties,
    })
}

/// Máy có tương thích không: GameZone WMI tồn tại, hoặc (hãng + model cho phép), hoặc có IBMPmDrv.
pub fn is_compatible(info: &MachineInfo) -> io::Result<bool> {
    let basic = Wmi::open()?
        .query::<std::collections::HashMap<String, wmi::Variant>>("SELECT __Path FROM LENOVO_GAMEZONE_DATA")
        .map(|r| !r.is_empty())
        .unwrap_or(false);
    let vendor_ok = ALLOWED_VENDORS.iter().any(|v| v.eq_ignore_ascii_case(&info.vendor));
    let model_ok = ALLOWED_MODELS_PREFIX.iter().any(|p| contains_ci(&info.model, p));
    let has_pm_device = Device::open(IBM_PM_DRV, ACCESS_READ).is_ok();
    Ok(basic || (vendor_ok && model_ok) || has_pm_device)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_rules() {
        assert_eq!(machine_generation("Legion 5 15ARH7H"), 7);
        assert_eq!(machine_generation("Legion Pro 7 16IRX9H"), 9);
        assert_eq!(machine_generation("Legion 7 16ACHg6"), 6);
        assert_eq!(machine_generation("LOQ 15IRH8"), 8);
        assert_eq!(machine_generation("Legion"), 0);
        assert_eq!(machine_generation("Legion Pro 5 16IAX10H"), 10);
    }

    #[test]
    fn series_from_machine_type_then_keyword() {
        assert_eq!(legion_series("whatever", "83DG"), LegionSeries::Legion5);
        assert_eq!(legion_series("Legion Go 8APU1", "83E1"), LegionSeries::LegionGo);
        assert_eq!(legion_series("LOQ 15IRH8", "82XV"), LegionSeries::Loq);
        assert_eq!(legion_series("IdeaPad Gaming 3", "82K1"), LegionSeries::IdeaPadGaming);
        assert_eq!(legion_series("IdeaPad 5", "xxxx"), LegionSeries::IdeaPad);
        assert_eq!(legion_series("Foo", "xxxx"), LegionSeries::Unknown);
    }

    #[test]
    fn amd_chinese_its() {
        assert!(is_amd_device("Legion 5 15ARH7H"));
        assert!(!is_amd_device("Legion 5 15IRX9"));
        assert!(is_amd_device("Legion R9000P"));
        assert!(!is_amd_device(""));
        assert!(is_chinese_model("Legion Y9000P"));
        assert!(supports_its_mode("ThinkBook 16"));
        assert!(!supports_its_mode("IdeaPad Gaming 3"));
        assert!(!supports_its_mode("Legion 5"));
    }

    #[test]
    fn gaming_and_non_gaming_lines_are_separated() {
        for m in ["Legion Y7000 IRX9", "LOQ 15IRX9", "Legion Pro 5 16IRX9", "IdeaPad Gaming 3 15ACH6"] {
            assert_eq!(device_class(m), DeviceClass::Gaming, "{m}");
        }
        for m in ["ThinkBook 16 G6", "IdeaPad Slim 5", "Lenovo Slim 7", "Yoga Pro 9", "XiaoXin Pro 16", "Motobook 60"] {
            assert_eq!(device_class(m), DeviceClass::NonGaming, "{m}");
        }
    }

    #[test]
    fn bios_parsing_and_comparison() {
        let (v, raw) = parse_bios("GKCN54WW ");
        let v = v.unwrap();
        assert_eq!(v, BiosVersion { prefix: "GKCN".into(), version: Some(54) });
        assert_eq!(raw.as_deref(), Some("GKCN54WW"));
        assert!(v.is_higher_or_equal_than(&BiosVersion::new("GKCN", Some(54))));
        assert!(!v.is_lower_than(&BiosVersion::new("GKCN", Some(54))));
        assert!(v.is_lower_than(&BiosVersion::new("GKCN", Some(60))));
        assert!(!v.is_lower_than(&BiosVersion::new("H1CN", Some(60)))); // khác prefix
        assert_eq!(parse_bios("nodigits"), (None, None));
    }

    #[test]
    fn god_mode_platform_rules() {
        let modes = [PowerMode::Quiet, PowerMode::Balance, PowerMode::Performance, PowerMode::GodMode];
        assert_eq!(god_mode_platform(&modes, 9, -1, 10, "Legion 5 16IRX10", "83F0", None), Some(GodModePlatform::Legion));
        assert_eq!(god_mode_platform(&modes, 4, -1, 6, "Legion 5 15ARH05H", "82B1", None), Some(GodModePlatform::LegacyLegion));
        assert_eq!(god_mode_platform(&[PowerMode::Quiet], 9, -1, 10, "Legion 5", "83F0", None), None);
        assert_eq!(god_mode_platform(&[], 0, 0, 0, "ThinkBook 16", "21XX", None), Some(GodModePlatform::NonGaming));
        // BIOS thấp hơn mốc → bỏ qua nhánh V1
        let old = BiosVersion::new("G9CN", Some(20));
        assert_eq!(god_mode_platform(&modes, 4, -1, 6, "Legion", "xx", Some(&old)), None);
    }

    #[test]
    fn extreme_requires_mode_and_version() {
        let with = [PowerMode::Extreme];
        assert!(supports_extreme_mode(&with, 7, -1));
        assert!(supports_extreme_mode(&with, -1, 4));
        assert!(!supports_extreme_mode(&with, 4, 1));
        assert!(!supports_extreme_mode(&[], 7, 4));
    }

    #[test]
    fn lighting_and_layout_exclusions() {
        assert!(is_excluded_from_lenovo_lighting(None, 6, LegionSeries::Legion7));
        let low = BiosVersion::new("GKCN", Some(50));
        assert!(is_excluded_from_lenovo_lighting(Some(&low), 8, LegionSeries::Legion5));
        assert!(!is_excluded_from_lenovo_lighting(None, 8, LegionSeries::Legion5));
        assert!(is_excluded_from_panel_logo_lighting("82JH", "Legion 5 15ITH6H"));
        assert!(!is_excluded_from_panel_logo_lighting("83DG", "Legion 5 15ITH6H"));
        assert!(has_alternative_full_spectrum_layout("83G0"));
        assert!(!is_overdrive_supported(LegionSeries::Legion7, 10));
        assert!(is_overdrive_supported(LegionSeries::Legion7, 8));
        assert!(is_overdrive_supported(LegionSeries::Loq, 10));
    }
}
