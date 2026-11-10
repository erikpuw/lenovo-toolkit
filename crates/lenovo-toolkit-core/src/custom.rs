//! Chế độ Custom (GodMode của bản C#) cho nền tảng Legion: giới hạn công suất CPU/GPU, giới hạn nhiệt,
//! quạt tối đa. Cơ chế như `GodModeController` (nhánh data-driven): đọc/ghi qua
//! `LENOVO_OTHER_METHOD GetFeatureValue/SetFeatureValue` với ID đã áp mặt nạ `0xFFFF00FF`, khoảng giá trị
//! và mặc định từ `LENOVO_CAPABILITY_DATA_01`, các bước rời rạc từ `LENOVO_DISCRETE_DATA`.
//!
//! Hai cấu hình: **Lenovo default** (giá trị mặc định của hãng, bị khóa) và **Custom** (người dùng chỉnh,
//! có cảnh báo). Mọi giá trị ghi xuống phần cứng đều được kiểm tra nằm trong khoảng/bước firmware báo.
//!
//! Chưa có: biểu đồ quạt (Fan_Set_Table), ép xung AMD (PBO/Curve Optimizer), NVAPI PCF, nền tảng
//! LegacyLegion/NonGaming, các cấu hình đặt tên do người dùng tạo.

use std::collections::HashMap;
use std::io;

use wmi::Variant;

use crate::wmi_util::{Wmi, variant_to_i32};

const OTHER: &str = "LENOVO_OTHER_METHOD";
/// Mặt nạ ID capability của nền tảng Legion (`GodModePlatformConfiguration.CapabilityIdMask`).
pub const MASK: u32 = 0xFFFF_00FF;
pub const FAN_FULL_SPEED_ID: u32 = 0x0402_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Cpu,
    Gpu,
}

#[derive(Debug, Clone, Copy)]
pub struct CapInfo {
    pub id: u32,
    pub title: &'static str,
    pub description: &'static str,
    /// Cảnh báo hiển thị khi chỉnh giá trị này ("" nếu không có).
    pub warning: &'static str,
    pub unit: &'static str,
    pub group: Group,
    /// Ghi lỗi thì bỏ qua (một số máy không có GPU rời/không hỗ trợ).
    pub fail_allowed: bool,
}

const fn cap(
    id: u32,
    title: &'static str,
    description: &'static str,
    warning: &'static str,
    unit: &'static str,
    group: Group,
    fail_allowed: bool,
) -> CapInfo {
    CapInfo { id, title, description, warning, unit, group, fail_allowed }
}

/// Danh sách capability của nền tảng Legion, thứ tự và nhãn theo `GodModeSettingsWindow`.
pub const CAPS: [CapInfo; 12] = [
    cap(0x0102_FF00, "Long Term Power Limit", "The maximum power the CPU is allowed to sustain indefinitely under long workloads.", "", "W", Group::Cpu, false),
    cap(0x0101_FF00, "Short Term Power Limit", "The maximum power the CPU is allowed to draw for brief bursts of high activity.", "", "W", Group::Cpu, false),
    cap(0x0103_FF00, "Peak Power Limit", "The absolute limit for instantaneous power spikes to protect the voltage regulator.", "Exceeding hardware limits may cause sudden system shutdowns or instability under instantaneous heavy loads.", "W", Group::Cpu, false),
    cap(0x0106_FF00, "Long Term Power Limit (Cross Loading)", "The sustained power limit applied to the CPU when the GPU is also under heavy load.", "", "W", Group::Cpu, false),
    cap(0x0107_FF00, "PL2 Tau", "How long the CPU may stay at the short term power limit.", "", "s", Group::Cpu, false),
    cap(0x0105_FF00, "APU sPPT Power Limit", "The Slow Package Power Tracking limit governing the slow-response power budget for AMD APU packages.", "", "W", Group::Cpu, false),
    cap(0x0104_FF00, "CPU Temperature Limit", "The target temperature threshold at which the CPU will begin thermal throttling.", "Higher temperature thresholds increase thermal stress and fan noise.", "°C", Group::Cpu, false),
    cap(0x0201_FF00, "GPU Power Boost", "Additional power the GPU may use on top of its base limit.", "", "W", Group::Gpu, true),
    cap(0x0202_FF00, "Configurable TGP", "The base sustained power limit allocated to the GPU, excluding dynamic boost headroom.", "", "W", Group::Gpu, true),
    cap(0x0203_FF00, "GPU Temperature Limit", "The target temperature threshold at which the GPU will begin thermal throttling.", "Higher temperature limits increase thermal stress on the graphics processor.", "°C", Group::Gpu, true),
    cap(0x0204_FF00, "Total Processing Power Target on AC", "The CPU power threshold that triggers dynamic power adjustments for the GPU while running on AC power.", "", "W", Group::Gpu, true),
    cap(0x020B_FF00, "GPU to CPU Dynamic Boost", "The maximum additional power the CPU is allowed to dynamically borrow from the GPU's budget.", "", "W", Group::Gpu, true),
];

pub const CUSTOM_MODE_WARNING: &str = "Custom Mode grants access to critical system parameters such as power limits and fan curves. Misconfiguration can lead to severe system instability, thermal throttling, or permanent hardware damage. Modifications made here may void your manufacturer warranty.";
pub const FAN_MAX_TITLE: &str = "Maximum fan speed";
pub const FAN_MAX_MESSAGE: &str = "Bypass all sensor-based fan curves and force both fans to run continuously at their maximum rated RPM.";
pub const FAN_MAX_WARNING: &str = "Prolonged operation at maximum speed increases fan wear and acoustic noise.";

pub fn cap_info(id: u32) -> Option<&'static CapInfo> {
    CAPS.iter().find(|c| c.id == id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    pub id: u32,
    pub default: i32,
    pub min: i32,
    pub max: i32,
    pub step: i32,
    /// Các giá trị cho phép khi firmware báo dạng rời rạc (đã sắp xếp); rỗng nếu dùng min/max/step.
    pub steps: Vec<i32>,
}

impl Range {
    /// Giá trị có hợp lệ theo firmware không.
    pub fn is_valid(&self, v: i32) -> bool {
        if !self.steps.is_empty() {
            return self.steps.contains(&v);
        }
        v >= self.min && v <= self.max && (self.step <= 1 || (v - self.min) % self.step == 0)
    }

    /// Đưa giá trị về giá trị hợp lệ gần nhất.
    pub fn snap(&self, v: i32) -> i32 {
        if !self.steps.is_empty() {
            return *self.steps.iter().min_by_key(|s| (i64::from(**s) - i64::from(v)).abs()).unwrap_or(&v);
        }
        let v = v.clamp(self.min, self.max);
        if self.step <= 1 {
            return v;
        }
        let off = (v - self.min + self.step / 2) / self.step * self.step;
        (self.min + off).clamp(self.min, self.max)
    }

    /// Giá trị kế tiếp theo hướng `dir` (+1/-1), dừng ở biên.
    pub fn next(&self, v: i32, dir: i32) -> i32 {
        let cur = self.snap(v);
        if !self.steps.is_empty() {
            let i = self.steps.iter().position(|s| *s == cur).unwrap_or(0) as i32;
            return self.steps[(i + dir.signum()).clamp(0, self.steps.len() as i32 - 1) as usize];
        }
        self.snap((cur + dir.signum() * self.step.max(1)).clamp(self.min, self.max))
    }
}

fn read_i32(row: &mut HashMap<String, Variant>, name: &str) -> Option<i32> {
    row.remove(name).and_then(|v| variant_to_i32(v).ok())
}

/// Khoảng giá trị và mặc định của các capability đã biết (bỏ qua mục không có bước nào).
pub fn read_ranges(w: &Wmi) -> io::Result<Vec<Range>> {
    let rows: Vec<HashMap<String, Variant>> = w.query("SELECT * FROM LENOVO_CAPABILITY_DATA_01")?;
    let discrete: Vec<HashMap<String, Variant>> = w.query("SELECT * FROM LENOVO_DISCRETE_DATA").unwrap_or_default();

    let mut steps: HashMap<u32, Vec<i32>> = HashMap::new();
    for mut r in discrete {
        if let (Some(id), Some(v)) = (read_i32(&mut r, "IDs"), read_i32(&mut r, "Value")) {
            steps.entry(id as u32).or_default().push(v);
        }
    }

    let mut result = Vec::new();
    for mut r in rows {
        let Some(id) = read_i32(&mut r, "IDs").map(|v| v as u32) else { continue };
        if cap_info(id).is_none() {
            continue;
        }
        let (default, min, max, step) = (
            read_i32(&mut r, "DefaultValue").unwrap_or(0),
            read_i32(&mut r, "MinValue").unwrap_or(0),
            read_i32(&mut r, "MaxValue").unwrap_or(0),
            read_i32(&mut r, "Step").unwrap_or(0),
        );
        let mut s = steps.remove(&id).unwrap_or_default();
        s.sort_unstable();
        s.dedup();
        if step == 0 && s.is_empty() {
            continue; // không chỉnh được
        }
        result.push(Range { id, default, min, max, step, steps: s });
    }
    // Thứ tự theo danh sách CAPS để giao diện ổn định.
    result.sort_by_key(|r| CAPS.iter().position(|c| c.id == r.id).unwrap_or(usize::MAX));
    Ok(result)
}

pub fn get_value(w: &Wmi, id: u32) -> io::Result<i32> {
    w.call_i32(OTHER, "GetFeatureValue", &[("IDs", (id & MASK) as i32)], "Value")
}

pub fn set_value(w: &Wmi, id: u32, value: i32) -> io::Result<()> {
    w.call(OTHER, "SetFeatureValue", &[("IDs", (id & MASK) as i32), ("value", value)])?;
    Ok(())
}

/// Giá trị mặc định của hãng cho từng chế độ nguồn (`LENOVO_DEFAULT_VALUE_IN_DIFFERENT_MODE_DATA`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeDefaults {
    /// Giá trị WMI của chế độ (enum + 1): 1 Quiet, 2 Balance, 3 Performance, 224 Extreme...
    pub mode: i32,
    pub cpu_long: i32,
    pub cpu_short: i32,
    pub cpu_peak: i32,
    pub cpu_cross: i32,
    pub apu_sppt: i32,
    pub cpu_temp: i32,
    pub gpu_boost: i32,
    pub gpu_ctgp: i32,
    pub gpu_temp: i32,
}

impl ModeDefaults {
    /// Giá trị mặc định của hãng cho capability `id` (nếu bảng này có).
    pub fn value_for(&self, id: u32) -> Option<i32> {
        Some(match id {
            0x0102_FF00 => self.cpu_long,
            0x0101_FF00 => self.cpu_short,
            0x0103_FF00 => self.cpu_peak,
            0x0106_FF00 => self.cpu_cross,
            0x0105_FF00 => self.apu_sppt,
            0x0104_FF00 => self.cpu_temp,
            0x0201_FF00 => self.gpu_boost,
            0x0202_FF00 => self.gpu_ctgp,
            0x0203_FF00 => self.gpu_temp,
            _ => return None,
        })
    }
}

pub fn read_mode_defaults(w: &Wmi) -> io::Result<Vec<ModeDefaults>> {
    let rows: Vec<HashMap<String, Variant>> = w.query("SELECT * FROM LENOVO_DEFAULT_VALUE_IN_DIFFERENT_MODE_DATA")?;
    Ok(rows
        .into_iter()
        .filter_map(|mut r| {
            Some(ModeDefaults {
                mode: read_i32(&mut r, "mode")?,
                cpu_long: read_i32(&mut r, "DefaultLongTermPowerlimit")?,
                cpu_short: read_i32(&mut r, "DefaultShortTermPowerlimit")?,
                cpu_peak: read_i32(&mut r, "DefaultPeakPowerLimit")?,
                cpu_cross: read_i32(&mut r, "DefaultCpuCrossLoading")?,
                apu_sppt: read_i32(&mut r, "DefaultAPUsPPTPowerLimit")?,
                cpu_temp: read_i32(&mut r, "DefaultTemperatueControl")?,
                gpu_boost: read_i32(&mut r, "Default_PPAB_Powerlimit")?,
                gpu_ctgp: read_i32(&mut r, "Default_cTGP_Powerlimit")?,
                gpu_temp: read_i32(&mut r, "DefaultTemperatueLimit")?,
            })
        })
        .collect())
}

/// Bộ giá trị cho chế độ Custom: id capability -> giá trị, và quạt tối đa.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Preset {
    pub values: HashMap<u32, i32>,
    pub fan_full_speed: bool,
}

/// Giá trị đang chạy trong firmware (đọc từng capability); mục không đọc được thì dùng mặc định.
/// Dùng để khởi tạo cấu hình Custom lần đầu, giữ nguyên những gì máy đang chạy.
pub fn current_preset(w: &Wmi, ranges: &[Range]) -> Preset {
    let values = ranges
        .iter()
        .map(|r| (r.id, r.snap(get_value(w, r.id).unwrap_or(r.default))))
        .collect();
    let fan_full_speed = get_value(w, FAN_FULL_SPEED_ID).is_ok_and(|v| v != 0);
    Preset { values, fan_full_speed }
}

/// Cấu hình **Lenovo default**: mặc định của hãng. Với mỗi capability dùng bảng mặc định của chế độ
/// Extreme (nếu máy hỗ trợ) hoặc Performance như `defaultPerformancePreset` của bản C#, nếu bảng đó
/// không có thì dùng `DefaultValue` trong dữ liệu capability. Quạt tối đa luôn tắt.
pub fn lenovo_default(ranges: &[Range], mode_defaults: &[ModeDefaults], supports_extreme: bool) -> Preset {
    let wanted_mode = if supports_extreme { 224 } else { 3 };
    let row = mode_defaults.iter().find(|d| d.mode == wanted_mode);
    let values = ranges
        .iter()
        .map(|r| {
            let v = row.and_then(|d| d.value_for(r.id)).unwrap_or(r.default);
            (r.id, r.snap(v))
        })
        .collect();
    Preset { values, fan_full_speed: false }
}

/// Kiểm tra bộ giá trị trước khi ghi: mọi giá trị phải hợp lệ theo firmware.
pub fn validate(preset: &Preset, ranges: &[Range]) -> Result<(), String> {
    for (id, v) in &preset.values {
        let r = ranges.iter().find(|r| r.id == *id).ok_or_else(|| format!("capability {id:#X} không được hỗ trợ"))?;
        if !r.is_valid(*v) {
            let name = cap_info(*id).map_or("giá trị", |c| c.title);
            return Err(format!("{name}: {v} nằm ngoài khoảng cho phép ({}..{}, bước {})", r.min, r.max, r.step));
        }
    }
    Ok(())
}

/// Ghi bộ giá trị xuống phần cứng. Trả về cảnh báo cho các mục được phép lỗi (GPU) mà ghi không thành.
pub fn apply(w: &Wmi, preset: &Preset, ranges: &[Range]) -> io::Result<Vec<String>> {
    validate(preset, ranges).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut warnings = Vec::new();
    for info in CAPS {
        let Some(v) = preset.values.get(&info.id) else { continue };
        if let Err(e) = set_value(w, info.id, *v) {
            if info.fail_allowed {
                warnings.push(format!("{}: {e}", info.title));
            } else {
                return Err(io::Error::other(format!("{}: {e}", info.title)));
            }
        }
    }
    // Luôn ghi trạng thái quạt tối đa (tắt khi không chọn) để chắc chắn không bị kẹt ở 100%.
    set_value(w, FAN_FULL_SPEED_ID, i32::from(preset.fan_full_speed))?;
    Ok(warnings)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Active {
    /// Cấu hình mặc định của Lenovo (khóa, không sửa được).
    Lenovo,
    /// Cấu hình người dùng chỉnh (có cảnh báo).
    Custom,
}

impl Active {
    pub const LABELS: [(Self, &'static str); 2] = [(Self::Lenovo, "Lenovo default (recommended)"), (Self::Custom, "Custom")];

    pub fn label(self) -> &'static str {
        Self::LABELS.iter().find(|(a, _)| *a == self).map_or("", |(_, l)| l)
    }

    pub fn parse_label(s: &str) -> Option<Self> {
        Self::LABELS.iter().find(|(_, l)| *l == s).map(|(a, _)| *a)
    }
}

/// Trạng thái chỉnh sửa trong cửa sổ Custom Mode: cấu hình Lenovo (khóa) và cấu hình người dùng.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editor {
    pub ranges: Vec<Range>,
    pub lenovo: Preset,
    pub user: Preset,
    pub active: Active,
}

impl Editor {
    pub fn new(ranges: Vec<Range>, lenovo: Preset, user: Option<Preset>, active: Active) -> Self {
        // Cấu hình người dùng mới bắt đầu từ mặc định Lenovo; giá trị đã lưu được đưa về giá trị hợp lệ.
        let mut user = user.unwrap_or_else(|| lenovo.clone());
        for r in &ranges {
            let v = user.values.get(&r.id).copied().unwrap_or_else(|| lenovo.values.get(&r.id).copied().unwrap_or(r.default));
            user.values.insert(r.id, r.snap(v));
        }
        user.values.retain(|id, _| ranges.iter().any(|r| r.id == *id));
        Self { ranges, lenovo, user, active }
    }

    pub fn current(&self) -> &Preset {
        match self.active {
            Active::Lenovo => &self.lenovo,
            Active::Custom => &self.user,
        }
    }

    /// Cấu hình Lenovo default bị khóa: không chỉnh được giá trị lẫn quạt tối đa.
    pub fn is_locked(&self) -> bool {
        self.active == Active::Lenovo
    }

    pub fn select(&mut self, active: Active) {
        self.active = active;
    }

    /// Tăng/giảm một giá trị theo bước của firmware; chỉ khi đang ở cấu hình Custom.
    pub fn tune(&mut self, id: u32, dir: i32) -> bool {
        if self.is_locked() {
            return false;
        }
        let Some(r) = self.ranges.iter().find(|r| r.id == id) else { return false };
        let cur = self.user.values.get(&id).copied().unwrap_or(r.default);
        let next = r.next(cur, dir);
        self.user.values.insert(id, next);
        next != cur
    }

    pub fn set_fan_full_speed(&mut self, on: bool) -> bool {
        if self.is_locked() {
            return false;
        }
        self.user.fan_full_speed = on;
        true
    }

    /// Đặt lại cấu hình người dùng về mặc định Lenovo.
    pub fn reset_user(&mut self) {
        self.user = self.lenovo.clone();
    }

    /// Có giá trị nào của cấu hình đang chọn khác mặc định Lenovo không (để hiện cảnh báo).
    pub fn differs_from_lenovo(&self, id: u32) -> bool {
        self.current().values.get(&id) != self.lenovo.values.get(&id)
    }
}

const SETTINGS_ACTIVE: &str = "CustomPreset";
const SETTINGS_VALUES: &str = "CustomValues";
const SETTINGS_FAN: &str = "CustomFanFullSpeed";

pub fn load_active() -> Active {
    let path = crate::settings::file_path(crate::settings::APP_FILE);
    match crate::settings::get_value_at(&path, SETTINGS_ACTIVE).as_ref().and_then(|v| v.as_str()) {
        Some("custom") => Active::Custom,
        _ => Active::Lenovo, // mặc định an toàn: cấu hình của Lenovo
    }
}

pub fn load_user() -> Option<Preset> {
    let path = crate::settings::file_path(crate::settings::APP_FILE);
    let values = crate::settings::get_value_at(&path, SETTINGS_VALUES)?;
    let values = values
        .as_object()?
        .iter()
        .filter_map(|(k, v)| Some((k.parse::<u32>().ok()?, i32::try_from(v.as_i64()?).ok()?)))
        .collect();
    let fan = crate::settings::get_value_at(&path, SETTINGS_FAN).and_then(|v| v.as_bool()).unwrap_or(false);
    Some(Preset { values, fan_full_speed: fan })
}

pub fn save(editor: &Editor) -> io::Result<()> {
    let path = crate::settings::file_path(crate::settings::APP_FILE);
    let active = if editor.active == Active::Custom { "custom" } else { "lenovo" };
    let values: serde_json::Map<String, serde_json::Value> =
        editor.user.values.iter().map(|(k, v)| (k.to_string(), serde_json::Value::from(*v))).collect();
    crate::settings::set_value_at(&path, SETTINGS_VALUES, serde_json::Value::Object(values))?;
    crate::settings::set_value_at(&path, SETTINGS_FAN, serde_json::Value::Bool(editor.user.fan_full_speed))?;
    crate::settings::set_value_at(&path, SETTINGS_ACTIVE, serde_json::Value::from(active))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_fixture() -> Editor {
        let ranges = vec![range(0x0102_FF00, 55, 5, 150, 5, &[]), range(0x0107_FF00, 56, 0, 0, 0, &[20, 56, 80])];
        let lenovo = Preset { values: HashMap::from([(0x0102_FF00, 55), (0x0107_FF00, 56)]), fan_full_speed: false };
        Editor::new(ranges, lenovo, None, Active::Lenovo)
    }

    #[test]
    fn lenovo_default_preset_is_locked() {
        let mut e = editor_fixture();
        assert!(e.is_locked());
        assert!(!e.tune(0x0102_FF00, 1));
        assert!(!e.set_fan_full_speed(true));
        assert_eq!(e.current().values[&0x0102_FF00], 55);
        assert!(!e.current().fan_full_speed);
    }

    #[test]
    fn custom_preset_edits_only_user_values_and_warns_on_difference() {
        let mut e = editor_fixture();
        e.select(Active::Custom);
        assert!(!e.differs_from_lenovo(0x0102_FF00));
        assert!(e.tune(0x0102_FF00, 1));
        assert_eq!(e.current().values[&0x0102_FF00], 60);
        assert!(e.differs_from_lenovo(0x0102_FF00));
        assert_eq!(e.lenovo.values[&0x0102_FF00], 55); // cấu hình Lenovo không bị đổi
        assert!(e.set_fan_full_speed(true));
        assert!(e.current().fan_full_speed);
        e.reset_user();
        assert_eq!(e.current().values[&0x0102_FF00], 55);
        assert!(!e.current().fan_full_speed);
    }

    #[test]
    fn tune_stops_at_firmware_bounds() {
        let mut e = editor_fixture();
        e.select(Active::Custom);
        for _ in 0..100 {
            e.tune(0x0102_FF00, 1);
        }
        assert_eq!(e.current().values[&0x0102_FF00], 150);
        assert!(!e.tune(0x0102_FF00, 1));
        assert!(e.tune(0x0107_FF00, 1));
        assert_eq!(e.current().values[&0x0107_FF00], 80);
        assert!(!e.tune(0xDEAD, 1)); // capability lạ
    }

    #[test]
    fn saved_user_values_are_snapped_and_unknown_ids_dropped() {
        let ranges = vec![range(0x0102_FF00, 55, 5, 150, 5, &[])];
        let lenovo = Preset { values: HashMap::from([(0x0102_FF00, 55)]), fan_full_speed: false };
        let saved = Preset { values: HashMap::from([(0x0102_FF00, 57), (0x0999_FF00, 3)]), fan_full_speed: true };
        let e = Editor::new(ranges, lenovo, Some(saved), Active::Custom);
        assert_eq!(e.user.values[&0x0102_FF00], 55);
        assert!(!e.user.values.contains_key(&0x0999_FF00));
        assert!(e.user.fan_full_speed);
    }

    #[test]
    fn active_labels_round_trip_and_default_is_lenovo() {
        for (a, l) in Active::LABELS {
            assert_eq!(Active::parse_label(l), Some(a));
        }
        assert_eq!(Active::parse_label("x"), None);
        assert!(Active::Lenovo.label().contains("recommended"));
    }

    fn range(id: u32, default: i32, min: i32, max: i32, step: i32, steps: &[i32]) -> Range {
        Range { id, default, min, max, step, steps: steps.to_vec() }
    }

    #[test]
    fn range_validation_and_snapping() {
        let r = range(0x0102_FF00, 55, 5, 150, 5, &[]);
        assert!(r.is_valid(55));
        assert!(!r.is_valid(56)); // sai bước
        assert!(!r.is_valid(155));
        assert_eq!(r.snap(57), 55);
        assert_eq!(r.snap(58), 60);
        assert_eq!(r.snap(1000), 150);
        assert_eq!(r.snap(-5), 5);
    }

    #[test]
    fn discrete_steps_only_allow_listed_values() {
        let r = range(0x0107_FF00, 56, 0, 0, 0, &[20, 40, 56, 80]);
        assert!(r.is_valid(56));
        assert!(!r.is_valid(50));
        assert_eq!(r.snap(50), 56);
        assert_eq!(r.snap(1), 20);
        assert_eq!(r.next(56, 1), 80);
        assert_eq!(r.next(80, 1), 80);
        assert_eq!(r.next(20, -1), 20);
    }

    #[test]
    fn next_moves_by_step_and_stops_at_bounds() {
        let r = range(0x0104_FF00, 95, 60, 100, 5, &[]);
        assert_eq!(r.next(95, 1), 100);
        assert_eq!(r.next(100, 1), 100);
        assert_eq!(r.next(60, -1), 60);
        assert_eq!(r.next(95, -1), 90);
    }

    #[test]
    fn lenovo_default_prefers_mode_table_then_capability_default() {
        let ranges = vec![range(0x0102_FF00, 40, 5, 150, 1, &[]), range(0x0107_FF00, 56, 0, 0, 0, &[20, 56, 80])];
        let table = ModeDefaults {
            mode: 224,
            cpu_long: 110,
            cpu_short: 0,
            cpu_peak: 0,
            cpu_cross: 0,
            apu_sppt: 0,
            cpu_temp: 0,
            gpu_boost: 0,
            gpu_ctgp: 0,
            gpu_temp: 0,
        };
        let p = lenovo_default(&ranges, &[table], true);
        assert_eq!(p.values[&0x0102_FF00], 110); // từ bảng Extreme
        assert_eq!(p.values[&0x0107_FF00], 56); // bảng không có -> DefaultValue
        assert!(!p.fan_full_speed);
        // Máy không hỗ trợ Extreme thì dùng dòng Performance (3); không có dòng thì dùng DefaultValue.
        assert_eq!(lenovo_default(&ranges, &[table], false).values[&0x0102_FF00], 40);
    }

    #[test]
    fn validate_rejects_out_of_range_before_writing() {
        let ranges = vec![range(0x0102_FF00, 55, 5, 150, 5, &[])];
        let mut p = Preset::default();
        p.values.insert(0x0102_FF00, 200);
        assert!(validate(&p, &ranges).unwrap_err().contains("ngoài khoảng"));
        p.values.insert(0x0102_FF00, 60);
        assert!(validate(&p, &ranges).is_ok());
        p.values.insert(0x0999_FF00, 1);
        assert!(validate(&p, &ranges).unwrap_err().contains("không được hỗ trợ"));
    }

    #[test]
    fn mask_clears_the_variant_byte() {
        assert_eq!(0x0102_FF00 & MASK, 0x0102_0000);
        assert_eq!(FAN_FULL_SPEED_ID & MASK, FAN_FULL_SPEED_ID);
        assert!(cap_info(0x0102_FF00).is_some());
        assert!(cap_info(0x1234).is_none());
    }

    #[test]
    fn every_cap_has_unique_id() {
        let mut ids: Vec<u32> = CAPS.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CAPS.len());
        assert!(CAPS.iter().filter(|c| !c.warning.is_empty()).count() >= 3);
    }
}
