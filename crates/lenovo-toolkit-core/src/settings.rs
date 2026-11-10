//! Cài đặt dạng JSON cùng vị trí/định dạng với bản C# (AbstractSettings):
//! `%LOCALAPPDATA%\LenovoLegionToolkit\<tệp>.json`, tên thuộc tính PascalCase, thụt lề.
//! Đọc-sửa-ghi trên `serde_json::Value` nên giữ nguyên các khóa lạ khi ghi.

use std::path::{Path, PathBuf};
use std::{fs, io};

use serde_json::{Map, Value};

pub const BALANCE_MODE_FILE: &str = "balancemode.json";
pub const AI_MODE_ENABLED: &str = "AIModeEnabled";

pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
    base.join("LenovoLegionToolkit")
}

pub fn file_path(name: &str) -> PathBuf {
    app_dir().join(name)
}

fn read_object(path: &Path) -> Map<String, Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| if let Value::Object(m) = v { Some(m) } else { None })
        .unwrap_or_default()
}

pub fn get_bool_at(path: &Path, key: &str) -> bool {
    read_object(path).get(key).and_then(Value::as_bool).unwrap_or(false)
}

pub fn set_bool_at(path: &Path, key: &str, value: bool) -> io::Result<()> {
    set_value_at(path, key, Value::Bool(value))
}

pub fn get_value_at(path: &Path, key: &str) -> Option<Value> {
    read_object(path).get(key).cloned()
}

pub fn set_value_at(path: &Path, key: &str, value: Value) -> io::Result<()> {
    let mut obj = read_object(path);
    obj.insert(key.to_string(), value);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(obj)).map_err(io::Error::other)?;
    fs::write(path, text)
}

/// Cờ riêng của bản Rust, lưu cạnh các tệp của bản C# trong cùng thư mục.
pub const APP_FILE: &str = "legion-rs.json";
pub const DISABLE_LENOVO_HOTKEYS: &str = "DisableLenovoHotkeys";
pub const DISABLE_VANTAGE: &str = "DisableVantage";
pub const DISABLE_LEGION_ZONE: &str = "DisableLegionZone";
pub const DISABLE_LEGION_SPACE: &str = "DisableLegionSpace";
pub const MINIMIZE_TO_TRAY: &str = "MinimizeToTray";
pub const MINIMIZE_ON_CLOSE: &str = "MinimizeOnClose";
pub const LANGUAGE: &str = "Language";
pub const POWER_SYNC_MODE: &str = "PowerModeMappingMode";
pub const RESET_BATTERY_ON_REBOOT: &str = "ResetBatteryOnSinceTimerOnReboot";
pub const GPU_OVERCLOCK_ENABLED: &str = "GpuOverclockEnabled";
pub const EXCLUDED_REFRESH_RATES: &str = "ExcludedRefreshRates";
/// Bật/tắt thông báo phím: công tắc chung rồi từng loại (mặc định bật).
pub const NOTIFY_ALL: &str = "NotifyAll";
pub const NOTIFY_POWER: &str = "NotifyPowerMode";
pub const NOTIFY_FNLOCK: &str = "NotifyFnLockCamera";
pub const NOTIFY_MIC: &str = "NotifyMicrophone";
pub const NOTIFY_TOUCHPAD: &str = "NotifyTouchpad";
pub const NOTIFY_BACKLIGHT: &str = "NotifyKeyboardBacklight";

pub fn excluded_refresh_rates_at(path: &Path) -> Vec<u32> {
    read_object(path)
        .get(EXCLUDED_REFRESH_RATES)
        .and_then(Value::as_array)
        .map(|arr| {
            let mut v: Vec<u32> = arr
                .iter()
                .filter_map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok()))
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        })
        .unwrap_or_default()
}

pub fn set_refresh_rate_excluded_at(path: &Path, hz: u32, excluded: bool) -> io::Result<()> {
    let mut list = excluded_refresh_rates_at(path);
    if excluded {
        if !list.contains(&hz) {
            list.push(hz);
            list.sort_unstable();
        }
    } else {
        list.retain(|&x| x != hz);
    }
    let arr = Value::Array(list.into_iter().map(|n| Value::Number(n.into())).collect());
    set_value_at(path, EXCLUDED_REFRESH_RATES, arr)
}

pub fn excluded_refresh_rates() -> Vec<u32> {
    excluded_refresh_rates_at(&file_path(APP_FILE))
}

pub fn set_refresh_rate_excluded(hz: u32, excluded: bool) -> io::Result<()> {
    set_refresh_rate_excluded_at(&file_path(APP_FILE), hz, excluded)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PowerSyncMode {
    #[default]
    WindowsPowerMode,
    WindowsPowerPlan,
    Disabled,
}

impl PowerSyncMode {
    pub const LABELS: [(Self, &'static str); 3] = [
        (Self::WindowsPowerMode, "Windows Power Mode"),
        (Self::WindowsPowerPlan, "Windows Power Plan"),
        (Self::Disabled, "Disabled"),
    ];

    pub fn label(self) -> &'static str {
        Self::LABELS.iter().find(|(m, _)| *m == self).map_or("Windows Power Mode", |(_, s)| s)
    }

    pub fn parse_label(s: &str) -> Option<Self> {
        Self::LABELS.iter().find(|(_, l)| *l == s).map(|(m, _)| *m)
    }
}

pub fn power_sync_mode() -> PowerSyncMode {
    read_object(&file_path(APP_FILE))
        .get(POWER_SYNC_MODE)
        .and_then(Value::as_str)
        .and_then(PowerSyncMode::parse_label)
        .unwrap_or(PowerSyncMode::WindowsPowerMode)
}

pub fn set_power_sync_mode(mode: PowerSyncMode) -> io::Result<()> {
    set_value_at(&file_path(APP_FILE), POWER_SYNC_MODE, Value::String(mode.label().to_string()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    Vi,
}

impl Lang {
    pub const LABELS: [(Self, &'static str); 2] = [(Self::En, "English"), (Self::Vi, "Tiếng Việt")];

    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Vi => "vi",
        }
    }

    pub fn from_code(s: &str) -> Self {
        if s.eq_ignore_ascii_case("vi") || s.eq_ignore_ascii_case("Tiếng Việt") {
            Self::Vi
        } else {
            Self::En
        }
    }

    pub fn label(self) -> &'static str {
        Self::LABELS.iter().find(|(l, _)| *l == self).map_or("English", |(_, s)| s)
    }

    pub fn parse_label(s: &str) -> Option<Self> {
        Self::LABELS.iter().find(|(_, l)| *l == s).map(|(lang, _)| *lang)
    }
}

pub fn lang_at(path: &Path) -> Lang {
    read_object(path)
        .get(LANGUAGE)
        .and_then(Value::as_str)
        .map(Lang::from_code)
        .unwrap_or(Lang::En)
}

pub fn set_lang_at(path: &Path, lang: Lang) -> io::Result<()> {
    set_value_at(path, LANGUAGE, Value::String(lang.code().to_string()))
}

pub fn lang() -> Lang {
    lang_at(&file_path(APP_FILE))
}

pub fn set_lang(lang: Lang) -> io::Result<()> {
    set_lang_at(&file_path(APP_FILE), lang)
}

pub fn get_opt_bool_at(path: &Path, key: &str) -> Option<bool> {
    read_object(path).get(key).and_then(Value::as_bool)
}

/// Đọc cờ; thiếu thì dùng giá trị mặc định.
pub fn flag(key: &str, default: bool) -> bool {
    get_opt_bool_at(&file_path(APP_FILE), key).unwrap_or(default)
}

pub fn set_flag(key: &str, value: bool) -> io::Result<()> {
    set_bool_at(&file_path(APP_FILE), key, value)
}

/// Danh sách thẻ điều khiển của máy lần chạy trước (mặt nạ bit), để khung hình đầu tiên dựng đúng bố cục ngay.
pub const LAYOUT_CACHE: &str = "DashboardLayoutCache";

pub fn layout_cache_at(path: &Path) -> Option<u32> {
    get_value_at(path, LAYOUT_CACHE)?.as_u64().and_then(|v| u32::try_from(v).ok())
}

pub fn set_layout_cache_at(path: &Path, mask: u32) -> io::Result<()> {
    set_value_at(path, LAYOUT_CACHE, Value::from(mask))
}

pub fn layout_cache() -> Option<u32> {
    layout_cache_at(&file_path(APP_FILE))
}

pub fn set_layout_cache(mask: u32) -> io::Result<()> {
    set_layout_cache_at(&file_path(APP_FILE), mask)
}

pub fn ai_mode_enabled() -> bool {
    get_bool_at(&file_path(BALANCE_MODE_FILE), AI_MODE_ENABLED)
}

pub fn set_ai_mode_enabled(value: bool) -> io::Result<()> {
    set_bool_at(&file_path(BALANCE_MODE_FILE), AI_MODE_ENABLED, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("legion-rs-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("sub").join("balancemode.json")
    }

    #[test]
    fn missing_file_reads_false_and_set_creates_it() {
        let p = temp_file("create");
        assert!(!get_bool_at(&p, AI_MODE_ENABLED));
        set_bool_at(&p, AI_MODE_ENABLED, true).unwrap();
        assert!(get_bool_at(&p, AI_MODE_ENABLED));
        set_bool_at(&p, AI_MODE_ENABLED, false).unwrap();
        assert!(!get_bool_at(&p, AI_MODE_ENABLED));
    }

    #[test]
    fn unknown_keys_are_preserved() {
        let p = temp_file("preserve");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, r#"{ "Other": 5, "AIModeEnabled": false }"#).unwrap();
        set_bool_at(&p, AI_MODE_ENABLED, true).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["Other"], 5);
        assert_eq!(v["AIModeEnabled"], true);
    }

    #[test]
    fn missing_key_distinguished_from_false() {
        let p = temp_file("optional");
        assert_eq!(get_opt_bool_at(&p, "X"), None);
        set_bool_at(&p, "X", false).unwrap();
        assert_eq!(get_opt_bool_at(&p, "X"), Some(false));
        assert_eq!(get_opt_bool_at(&p, "Y"), None);
    }

    #[test]
    fn layout_cache_roundtrips_and_keeps_other_keys() {
        let p = temp_file("layout");
        assert_eq!(layout_cache_at(&p), None);
        set_bool_at(&p, "Keep", true).unwrap();
        set_layout_cache_at(&p, 0x0002_03FF).unwrap();
        assert_eq!(layout_cache_at(&p), Some(0x0002_03FF));
        assert_eq!(get_opt_bool_at(&p, "Keep"), Some(true));
    }

    #[test]
    fn corrupt_file_reads_false() {
        let p = temp_file("corrupt");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "{ not json").unwrap();
        assert!(!get_bool_at(&p, AI_MODE_ENABLED));
    }

    #[test]
    fn language_round_trips_and_defaults_to_english() {
        let p = temp_file("lang");
        assert_eq!(lang_at(&p), Lang::En);
        set_lang_at(&p, Lang::Vi).unwrap();
        assert_eq!(lang_at(&p), Lang::Vi);
        assert_eq!(Lang::parse_label(Lang::Vi.label()), Some(Lang::Vi));
        assert_eq!(Lang::parse_label(Lang::En.label()), Some(Lang::En));
    }

    #[test]
    fn excluded_refresh_rates_round_trip() {
        let p = temp_file("hz");
        assert!(excluded_refresh_rates_at(&p).is_empty());
        set_refresh_rate_excluded_at(&p, 60, true).unwrap();
        assert_eq!(excluded_refresh_rates_at(&p), vec![60]);
        set_refresh_rate_excluded_at(&p, 144, true).unwrap();
        assert_eq!(excluded_refresh_rates_at(&p), vec![60, 144]);
        set_refresh_rate_excluded_at(&p, 60, false).unwrap();
        assert_eq!(excluded_refresh_rates_at(&p), vec![144]);
    }
}
