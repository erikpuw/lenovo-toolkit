//! Logic thuần của giao diện (nhãn hiển thị, định dạng), tách khỏi `main.rs` để test không bị dính
//! manifest `requireAdministrator` của file chạy. Nhãn lấy từ Resource.resx của bản gốc.

// Khay hệ thống nằm trong lib để test (kể cả đo kích thước tooltip) chạy được: bin có manifest admin nên `test = false`.
pub mod icons;
pub mod tray;

use icons::IconKind;
use lenovo_toolkit_core::autorun::AutorunState;
use lenovo_toolkit_core::battery::{BatteryDetail, BatteryState};
use lenovo_toolkit_core::energy::AlwaysOnUsb;
use lenovo_toolkit_core::events::SpecialKey;
use lenovo_toolkit_core::gamezone::HybridMode;
use lenovo_toolkit_core::power::PowerMode;
use lenovo_toolkit_core::settings::{self, Lang, PowerSyncMode};
use tray::Toast;

pub fn tr(lang: Lang, en: &'static str, vi: &'static str) -> &'static str {
    match lang {
        Lang::En => en,
        Lang::Vi => vi,
    }
}

/// WMI trả 0x80041003 (WBEM_E_ACCESS_DENIED) khi thiếu quyền admin.
pub fn explain(e: impl std::fmt::Display) -> String {
    let s = e.to_string();
    if s.contains("0x80041003") {
        "WMI từ chối truy cập: cần chạy bằng quyền admin".into()
    } else {
        s
    }
}

/// Giá trị thiếu hiển thị là "-" như bản gốc.
pub const DASH: &str = "-";

pub const APP_NAME: &str = "Lenovo Toolkit";
/// Lấy từ `Cargo.toml` (một nguồn duy nhất, cũng được nhúng vào thông tin phiên bản của file .exe).
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Tên hiển thị kèm phiên bản, ví dụ "Lenovo Toolkit v1.0.1" (tiêu đề cửa sổ, khay hệ thống).
pub fn app_title() -> String {
    format!("{APP_NAME} v{APP_VERSION}")
}

/// Thông báo cho máy không thuộc nhóm gaming (ThinkBook, IdeaPad, Slim, Yoga...): các điều khiển ghi bị ẩn.
pub fn unsupported_notice(lang: Lang, model: &str) -> String {
    match lang {
        Lang::En => format!(
            "{model}: this device uses ITS (Intelligent Thermal Solution) instead of Legion power modes, which {APP_NAME} does not support yet. Controls that write to hardware are hidden; sensors stay read-only. {APP_NAME} supports Legion, LOQ and IdeaPad Gaming."
        ),
        Lang::Vi => format!(
            "{model}: thiết bị này dùng ITS (Intelligent Thermal Solution) thay cho chế độ nguồn của Legion nên {APP_NAME} chưa hỗ trợ. Các điều khiển ghi xuống phần cứng được ẩn, cảm biến chỉ để xem. {APP_NAME} hỗ trợ Legion, LOQ và IdeaPad Gaming."
        ),
    }
}

fn on_off(on: bool, lang: Lang) -> &'static str {
    if on { tr(lang, "On", "Bật") } else { tr(lang, "Off", "Tắt") }
}

fn state_toast(flag: &'static str, icon: IconKind, name: &str, on: bool, lang: Lang) -> Toast {
    Toast { flag, icon, text: format!("{name}: {}", on_off(on, lang)), on }
}

/// Thông báo cho phím chỉ báo trạng thái qua WMI (Fn Lock, camera, đèn bàn phím). `None` với phím có việc riêng.
pub fn key_toast(key: SpecialKey, lang: Lang) -> Option<Toast> {
    use SpecialKey::*;
    Some(match key {
        FnLockOn | FnLockOff => state_toast(settings::NOTIFY_FNLOCK, IconKind::Keyboard, "Fn Lock", key == FnLockOn, lang),
        CameraOn | CameraOff => state_toast(settings::NOTIFY_FNLOCK, IconKind::Camera, "Camera", key == CameraOn, lang),
        WhiteBacklightOff => backlight_toast(0, tr(lang, "Off", "Tắt"), lang),
        WhiteBacklightLow => backlight_toast(1, tr(lang, "Low", "Thấp"), lang),
        WhiteBacklightHigh => backlight_toast(2, tr(lang, "High", "Cao"), lang),
        FnF9 | FnR => return None,
    })
}

/// Touchpad bật khi `GetTPStatus` = 0 (khóa tắt), giống `NotifyTouchpadLockAsync` của bản gốc.
pub fn touchpad_on(tp_status: i32) -> bool {
    tp_status == 0
}

pub fn touchpad_toast(on: bool, lang: Lang) -> Toast {
    state_toast(settings::NOTIFY_TOUCHPAD, IconKind::Touchpad, "Touchpad", on, lang)
}

/// `level` 0 là tắt; `label` là nhãn mức đèn (Tắt/Thấp/Cao).
pub fn backlight_toast(level: u32, label: &str, lang: Lang) -> Toast {
    Toast {
        flag: settings::NOTIFY_BACKLIGHT,
        icon: IconKind::Backlight,
        text: format!("{}: {label}", tr(lang, "Keyboard backlight", "Đèn bàn phím")),
        on: level > 0,
    }
}

pub fn mic_toast(on: bool, lang: Lang) -> Toast {
    Toast {
        flag: settings::NOTIFY_MIC,
        icon: IconKind::Mic,
        text: format!("{}: {}", tr(lang, "Microphone", "Micro"), on_off(on, lang)),
        on,
    }
}

pub fn power_toast(m: PowerMode, lang: Lang) -> Toast {
    Toast {
        flag: settings::NOTIFY_POWER,
        icon: IconKind::Power,
        text: format!("{}: {}", tr(lang, "Power mode", "Chế độ nguồn"), power_label_tr(m, lang)),
        on: true,
    }
}

#[cfg(test)]
mod app_info_tests {
    use super::*;

    #[test]
    fn toasts_cover_state_keys_and_skip_action_keys() {
        let t = key_toast(SpecialKey::FnLockOn, Lang::En).unwrap();
        assert_eq!((t.text.as_str(), t.on), ("Fn Lock: On", true));
        let t = key_toast(SpecialKey::CameraOff, Lang::Vi).unwrap();
        assert_eq!((t.text.as_str(), t.on), ("Camera: Tắt", false));
        let t = key_toast(SpecialKey::WhiteBacklightHigh, Lang::Vi).unwrap();
        assert_eq!((t.text.as_str(), t.on), ("Đèn bàn phím: Cao", true));
        assert!(!key_toast(SpecialKey::WhiteBacklightOff, Lang::En).unwrap().on);
        // Mỗi loại thông báo có cờ riêng để bật/tắt trong Settings.
        assert_eq!(key_toast(SpecialKey::CameraOn, Lang::En).unwrap().flag, settings::NOTIFY_FNLOCK);
        assert_eq!(key_toast(SpecialKey::WhiteBacklightLow, Lang::En).unwrap().flag, settings::NOTIFY_BACKLIGHT);
        assert_eq!(touchpad_toast(true, Lang::En).flag, settings::NOTIFY_TOUCHPAD);
        assert_eq!(mic_toast(true, Lang::En).flag, settings::NOTIFY_MIC);
        assert_eq!(power_toast(PowerMode::Quiet, Lang::En).flag, settings::NOTIFY_POWER);
        assert!(key_toast(SpecialKey::FnR, Lang::En).is_none());
        // GetTPStatus: 0 = khóa tắt = touchpad đang dùng; 1 = đã khóa.
        assert!(touchpad_on(0) && !touchpad_on(1));
        assert_eq!(touchpad_toast(false, Lang::En).text, "Touchpad: Off");
        assert_eq!(mic_toast(false, Lang::Vi).text, "Micro: Tắt");
        assert!(mic_toast(true, Lang::En).on && !mic_toast(false, Lang::En).on); // tắt thì icon gạch chéo
        assert_eq!(power_toast(PowerMode::Balance, Lang::En).text, "Power mode: Balance");
    }

    #[test]
    fn unsupported_notice_names_the_device_and_scope() {
        let n = unsupported_notice(Lang::En, "ThinkBook 16");
        assert!(n.contains("ThinkBook 16") && n.contains("ITS") && n.contains("Legion, LOQ"));
        assert!(unsupported_notice(Lang::Vi, "X").contains("chưa hỗ trợ"));
    }


    #[test]
    fn title_has_name_and_version() {
        assert_eq!(APP_VERSION, "1.0.1");
        assert_eq!(app_title(), "Lenovo Toolkit v1.0.1");
    }
}

pub fn temp(v: i32) -> String {
    if v < 0 { DASH.into() } else { format!("{v}°C") }
}

pub fn rpm(v: i32) -> String {
    if v < 0 { DASH.into() } else { format!("{v} RPM") }
}

pub fn percent(v: Option<u32>) -> String {
    v.map_or_else(|| DASH.into(), |p| format!("{p}%"))
}

pub fn power_label(m: PowerMode) -> &'static str {
    power_label_tr(m, Lang::En)
}

pub fn power_label_tr(m: PowerMode, lang: Lang) -> &'static str {
    match m {
        PowerMode::Quiet => tr(lang, "Quiet", "Yên tĩnh (Quiet)"),
        PowerMode::Balance => tr(lang, "Balance", "Cân bằng (Balance)"),
        PowerMode::Performance => tr(lang, "Performance", "Hiệu năng (Performance)"),
        PowerMode::Extreme => tr(lang, "Extreme", "Cực đại (Extreme)"),
        PowerMode::GodMode => tr(lang, "Custom", "Tùy chỉnh (Custom)"),
    }
}

pub fn parse_power_label(s: &str) -> Option<PowerMode> {
    [PowerMode::Quiet, PowerMode::Balance, PowerMode::Performance, PowerMode::Extreme, PowerMode::GodMode]
        .into_iter()
        .find(|m| power_label_tr(*m, Lang::En) == s || power_label_tr(*m, Lang::Vi) == s)
}

pub const BATTERY_LABELS: [(BatteryState, &str); 3] = [
    (BatteryState::Conservation, "Conservation"),
    (BatteryState::Normal, "Normal"),
    (BatteryState::RapidCharge, "Rapid Charge"),
];

pub fn battery_label(s: BatteryState) -> &'static str {
    battery_label_tr(s, Lang::En)
}

pub fn battery_label_tr(s: BatteryState, lang: Lang) -> &'static str {
    match s {
        BatteryState::Conservation => tr(lang, "Conservation", "Bảo vệ pin (80%)"),
        BatteryState::Normal => tr(lang, "Normal", "Sạc thường (100%)"),
        BatteryState::RapidCharge => tr(lang, "Rapid Charge", "Sạc nhanh"),
    }
}

pub fn battery_options(lang: Lang) -> Vec<String> {
    [BatteryState::Conservation, BatteryState::Normal, BatteryState::RapidCharge]
        .into_iter()
        .map(|b| battery_label_tr(b, lang).to_string())
        .collect()
}

pub fn parse_battery_label(s: &str) -> Option<BatteryState> {
    [BatteryState::Conservation, BatteryState::Normal, BatteryState::RapidCharge]
        .into_iter()
        .find(|b| battery_label_tr(*b, Lang::En) == s || battery_label_tr(*b, Lang::Vi) == s)
}

pub const USB_LABELS: [(AlwaysOnUsb, &str); 3] = [
    (AlwaysOnUsb::Off, "Off"),
    (AlwaysOnUsb::OnWhenSleeping, "On, when sleeping"),
    (AlwaysOnUsb::OnAlways, "On, always"),
];

pub fn usb_label(s: AlwaysOnUsb) -> &'static str {
    usb_label_tr(s, Lang::En)
}

pub fn usb_label_tr(s: AlwaysOnUsb, lang: Lang) -> &'static str {
    match s {
        AlwaysOnUsb::Off => tr(lang, "Off", "Tắt"),
        AlwaysOnUsb::OnWhenSleeping => tr(lang, "On, when sleeping", "Bật khi máy ngủ"),
        AlwaysOnUsb::OnAlways => tr(lang, "On, always", "Luôn bật"),
    }
}

pub fn usb_options(lang: Lang) -> Vec<String> {
    [AlwaysOnUsb::Off, AlwaysOnUsb::OnWhenSleeping, AlwaysOnUsb::OnAlways]
        .into_iter()
        .map(|u| usb_label_tr(u, lang).to_string())
        .collect()
}

pub fn parse_usb_label(s: &str) -> Option<AlwaysOnUsb> {
    [AlwaysOnUsb::Off, AlwaysOnUsb::OnWhenSleeping, AlwaysOnUsb::OnAlways]
        .into_iter()
        .find(|u| usb_label_tr(*u, Lang::En) == s || usb_label_tr(*u, Lang::Vi) == s)
}

/// Nhãn mức đèn bàn phím: 0 Off, 1 Low, 2 High, 3 Auto.
pub const KBD_LABELS: [&str; 4] = ["Off", "Low", "High", "Auto"];
pub const KBD_LABELS_VI: [&str; 4] = ["Tắt", "Thấp", "Cao", "Tự động"];

pub fn kbd_label_tr(level: u32, lang: Lang) -> &'static str {
    let table = match lang {
        Lang::En => &KBD_LABELS,
        Lang::Vi => &KBD_LABELS_VI,
    };
    table.get(level as usize).copied().unwrap_or(DASH)
}

pub fn kbd_options(max_level: u32) -> Vec<String> {
    kbd_options_tr(max_level, Lang::En)
}

pub fn kbd_options_tr(max_level: u32, lang: Lang) -> Vec<String> {
    let table = match lang {
        Lang::En => &KBD_LABELS,
        Lang::Vi => &KBD_LABELS_VI,
    };
    table.iter().take(max_level as usize + 1).map(|s| (*s).to_string()).collect()
}

pub fn parse_kbd_label(s: &str) -> Option<u32> {
    KBD_LABELS
        .iter()
        .position(|l| *l == s)
        .or_else(|| KBD_LABELS_VI.iter().position(|l| *l == s))
        .map(|i| i as u32)
}

pub fn hybrid_label_tr(m: HybridMode, lang: Lang) -> &'static str {
    match m {
        HybridMode::On => tr(lang, "Hybrid", "Hybrid (Tự động chuyển)"),
        HybridMode::OnIGPUOnly => tr(lang, "Hybrid-iGPU Only", "Chỉ iGPU (Tiết kiệm pin)"),
        HybridMode::OnAuto => tr(lang, "Hybrid-Auto", "Hybrid-Tự động ngắt dGPU"),
        HybridMode::Off => tr(lang, "dGPU", "Chỉ card rời (dGPU)"),
    }
}

pub fn parse_hybrid_label(s: &str) -> Option<HybridMode> {
    [HybridMode::On, HybridMode::OnIGPUOnly, HybridMode::OnAuto, HybridMode::Off]
        .into_iter()
        .find(|m| hybrid_label_tr(*m, Lang::En) == s || hybrid_label_tr(*m, Lang::Vi) == s)
}

pub fn autorun_label_tr(s: AutorunState, lang: Lang) -> &'static str {
    match s {
        AutorunState::Disabled => tr(lang, "Disabled", "Tắt"),
        AutorunState::Enabled => tr(lang, "Enabled", "Bật"),
        AutorunState::EnabledDelayed => tr(lang, "Enabled (delayed)", "Bật (trễ 30 giây)"),
    }
}

pub fn autorun_options(lang: Lang) -> Vec<String> {
    [AutorunState::Disabled, AutorunState::Enabled, AutorunState::EnabledDelayed]
        .into_iter()
        .map(|a| autorun_label_tr(a, lang).to_string())
        .collect()
}

pub fn parse_autorun_label(s: &str) -> Option<AutorunState> {
    [AutorunState::Disabled, AutorunState::Enabled, AutorunState::EnabledDelayed]
        .into_iter()
        .find(|a| autorun_label_tr(*a, Lang::En) == s || autorun_label_tr(*a, Lang::Vi) == s)
}

pub fn powersync_label_tr(m: PowerSyncMode, lang: Lang) -> &'static str {
    match m {
        PowerSyncMode::WindowsPowerMode => tr(lang, "Windows Power Mode", "Chế độ nguồn Windows"),
        PowerSyncMode::WindowsPowerPlan => tr(lang, "Windows Power Plan", "Gói nguồn (Power Plan)"),
        PowerSyncMode::Disabled => tr(lang, "Disabled", "Tắt"),
    }
}

pub fn powersync_options(lang: Lang) -> Vec<String> {
    [PowerSyncMode::WindowsPowerMode, PowerSyncMode::WindowsPowerPlan, PowerSyncMode::Disabled]
        .into_iter()
        .map(|m| powersync_label_tr(m, lang).to_string())
        .collect()
}

pub fn parse_powersync_label(s: &str) -> Option<PowerSyncMode> {
    [PowerSyncMode::WindowsPowerMode, PowerSyncMode::WindowsPowerPlan, PowerSyncMode::Disabled]
        .into_iter()
        .find(|m| powersync_label_tr(*m, Lang::En) == s || powersync_label_tr(*m, Lang::Vi) == s)
}

pub fn custom_cap_tr(id: u32, lang: Lang) -> Option<(&'static str, &'static str, &'static str)> {
    let info = lenovo_toolkit_core::custom::cap_info(id)?;
    if lang == Lang::En {
        return Some((info.title, info.description, info.warning));
    }
    Some(match id {
        0x0102_FF00 => (
            "Giới hạn công suất dài hạn CPU (PL1)",
            "Công suất tối đa CPU được phép duy trì liên tục khi tải nặng kéo dài.",
            "",
        ),
        0x0101_FF00 => (
            "Giới hạn công suất ngắn hạn CPU (PL2)",
            "Công suất tối đa CPU được phép rút trong các đợt tăng tốc ngắn.",
            "",
        ),
        0x0103_FF00 => (
            "Giới hạn công suất đỉnh CPU (Peak)",
            "Ngưỡng công suất tức thời tối đa để bảo vệ mạch cấp nguồn.",
            "Đặt quá cao có thể gây sập nguồn đột ngột hoặc mất ổn định khi tải nặng tức thời.",
        ),
        0x0106_FF00 => (
            "Giới hạn công suất CPU khi tải chéo (Cross Loading)",
            "Giới hạn công suất duy trì của CPU khi GPU cũng đang chạy tải nặng.",
            "",
        ),
        0x0107_FF00 => (
            "Thời gian duy trì PL2 (PL2 Tau)",
            "Thời gian tối đa CPU được phép giữ ở mức công suất ngắn hạn PL2.",
            "",
        ),
        0x0105_FF00 => (
            "Giới hạn công suất APU sPPT",
            "Ngưỡng công suất phản hồi chậm dành cho APU AMD.",
            "",
        ),
        0x0104_FF00 => (
            "Giới hạn nhiệt độ CPU",
            "Ngưỡng nhiệt độ mục tiêu mà tại đó CPU bắt đầu giảm xung để hạ nhiệt.",
            "Ngưỡng nhiệt cao hơn sẽ làm tăng nhiệt lượng và tiếng ồn quạt.",
        ),
        0x0201_FF00 => (
            "Tăng cường công suất GPU (Dynamic Boost)",
            "Công suất bổ sung mà GPU được phép dùng thêm trên mức cơ bản.",
            "",
        ),
        0x0202_FF00 => (
            "Công suất GPU cơ bản (Configurable TGP)",
            "Mức công suất nền tảng cấp cho GPU (chưa tính phần Dynamic Boost).",
            "",
        ),
        0x0203_FF00 => (
            "Giới hạn nhiệt độ GPU",
            "Ngưỡng nhiệt độ mục tiêu mà tại đó GPU bắt đầu giảm xung để hạ nhiệt.",
            "Ngưỡng nhiệt cao hơn sẽ làm tăng tải nhiệt lên nhân đồ họa.",
        ),
        0x0204_FF00 => (
            "Tổng công suất mục tiêu khi cắm sạc (AC)",
            "Ngưỡng công suất CPU kích hoạt điều chỉnh điện năng động cho GPU khi cắm sạc.",
            "",
        ),
        0x020B_FF00 => (
            "Công suất mượn từ GPU sang CPU",
            "Công suất tối đa mà CPU được phép mượn động từ ngân sách điện của GPU.",
            "",
        ),
        _ => (info.title, info.description, info.warning),
    })
}

pub fn ram_text(used_gb: f64, total_gb: f64) -> String {
    format!("{used_gb:.1}/{total_gb:.1} GB")
}

pub fn watt_signed(mw: Option<i32>) -> String {
    match mw {
        None => DASH.into(),
        Some(0) => "0.00 W".into(),
        Some(v) => format!("{:+.2} W", f64::from(v) / 1000.0),
    }
}

pub fn wh_text(mwh: u32) -> String {
    if mwh == 0 {
        DASH.into()
    } else {
        format!("{:.2} Wh", f64::from(mwh) / 1000.0)
    }
}

pub fn temp_f64(c: Option<f64>) -> String {
    c.map_or_else(|| DASH.into(), |v| format!("{v:.1} °C"))
}

pub fn battery_status_line(d: &BatteryDetail, lang: Lang) -> String {
    if d.ac_online {
        if d.is_charging || d.rate_mw.is_some_and(|r| r > 0) {
            tr(lang, "Connected, charging...", "Đã cắm sạc, đang sạc...").into()
        } else {
            tr(lang, "Connected, not charging", "Đã cắm sạc, không sạc").into()
        }
    } else if let Some(secs) = d.life_remaining_secs {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        match (lang, hours > 0) {
            (Lang::En, true) => format!("Estimated time remaining: {hours}h {mins}m"),
            (Lang::En, false) => format!("Estimated time remaining: {mins}m"),
            (Lang::Vi, true) => format!("Thời lượng ước tính còn lại: {hours} giờ {mins} phút"),
            (Lang::Vi, false) => format!("Thời lượng ước tính còn lại: {mins} phút"),
        }
    } else {
        tr(lang, "Discharging (estimating time...)", "Đang xả pin (đang ước tính...)").into()
    }
}

/// Tạo danh sách hàng thông tin trang Battery theo đúng thứ tự và nhãn của `BatteryPage.xaml` gốc.
pub fn build_battery_rows(d: &BatteryDetail, lang: Lang) -> Vec<(&'static str, &'static str, String)> {
    let mut rows = Vec::with_capacity(11);
    if d.temperature_c.is_some() {
        rows.push((
            tr(lang, "Battery temperature", "Nhiệt độ pin"),
            tr(lang, "Current temperature of the battery.", "Nhiệt độ hiện tại của viên pin."),
            temp_f64(d.temperature_c),
        ));
    }
    rows.push((
        tr(lang, "Discharge rate", "Công suất sạc / xả"),
        tr(
            lang,
            "Rate at which battery is charged (+), or discharged (-).",
            "Tốc độ nạp điện (+) hoặc xả điện (-) hiện tại của pin.",
        ),
        watt_signed(d.rate_mw),
    ));
    rows.push((
        tr(lang, "Minimum discharge rate", "Công suất tối thiểu"),
        tr(
            lang,
            "Minimum rate at which battery is charged (+), or discharged (-).",
            "Mức công suất sạc (+) hoặc xả (-) thấp nhất ghi nhận trong phiên.",
        ),
        watt_signed(d.min_rate_mw),
    ));
    rows.push((
        tr(lang, "Maximum discharge rate", "Công suất tối đa"),
        tr(
            lang,
            "Maximum rate at which battery is charged (+), or discharged (-).",
            "Mức công suất sạc (+) hoặc xả (-) cao nhất ghi nhận trong phiên.",
        ),
        watt_signed(d.max_rate_mw),
    ));
    rows.push((
        tr(lang, "Current capacity", "Dung lượng hiện tại"),
        tr(lang, "Current battery capacity.", "Mức năng lượng hiện đang tích trữ trong pin."),
        wh_text(d.current_capacity_mwh),
    ));
    rows.push((
        tr(lang, "Full charge capacity", "Dung lượng khi sạc đầy"),
        tr(
            lang,
            "Battery capacity when fully charged.",
            "Dung lượng tối đa thực tế khi sạc đầy 100%.",
        ),
        wh_text(d.full_charge_capacity_mwh),
    ));
    rows.push((
        tr(lang, "Design capacity", "Dung lượng thiết kế"),
        tr(
            lang,
            "Battery capacity as designed.",
            "Dung lượng gốc của nhà sản xuất khi xuất xưởng.",
        ),
        wh_text(d.design_capacity_mwh),
    ));
    rows.push((
        tr(lang, "Battery health", "Độ bền pin (Health)"),
        tr(
            lang,
            "Battery maximum charge capacity.",
            "Tỷ lệ dung lượng sạc đầy thực tế so với dung lượng thiết kế.",
        ),
        d.health_percent()
            .map_or_else(|| DASH.into(), |h| format!("{h:.2} %")),
    ));
    rows.push((
        tr(lang, "Cycle count", "Số chu kỳ sạc (Cycle count)"),
        tr(
            lang,
            "Number of charge/discharge cycles.",
            "Tổng số chu kỳ nạp/xả của viên pin.",
        ),
        d.cycle_count.to_string(),
    ));
    if let Some(mfg) = &d.manufacture_date {
        rows.push((
            tr(lang, "Manufacture date", "Ngày sản xuất"),
            tr(
                lang,
                "When was the battery manufactured.",
                "Ngày viên pin được sản xuất tại nhà máy.",
            ),
            mfg.clone(),
        ));
    }
    if let Some(first) = &d.first_use_date {
        rows.push((
            tr(lang, "First use date", "Ngày sử dụng đầu tiên"),
            tr(
                lang,
                "When was the battery first used.",
                "Ngày viên pin được kích hoạt sử dụng lần đầu.",
            ),
            first.clone(),
        ));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_round_trip() {
        for m in [PowerMode::Quiet, PowerMode::Balance, PowerMode::Performance, PowerMode::Extreme, PowerMode::GodMode] {
            assert_eq!(parse_power_label(power_label(m)), Some(m));
        }
        assert_eq!(power_label(PowerMode::GodMode), "Custom");
        for (b, l) in BATTERY_LABELS {
            assert_eq!(parse_battery_label(l), Some(b));
            assert_eq!(battery_label(b), l);
        }
        for (u, l) in USB_LABELS {
            assert_eq!(parse_usb_label(l), Some(u));
            assert_eq!(usb_label(u), l);
        }
        assert_eq!(parse_power_label("bogus"), None);
    }

    #[test]
    fn keyboard_levels() {
        assert_eq!(kbd_options(0), vec!["Off"]);
        assert_eq!(kbd_options(2), vec!["Off", "Low", "High"]);
        assert_eq!(kbd_options(3).len(), 4);
        assert_eq!(parse_kbd_label("High"), Some(2));
        assert_eq!(parse_kbd_label("x"), None);
    }

    #[test]
    fn formatting() {
        assert_eq!(temp(-1), "-");
        assert_eq!(temp(49), "49°C");
        assert_eq!(rpm(1900), "1900 RPM");
        assert_eq!(rpm(-1), "-");
        assert_eq!(percent(Some(78)), "78%");
        assert_eq!(percent(None), "-");
        assert_eq!(ram_text(10.84, 15.7), "10.8/15.7 GB");
    }

    #[test]
    fn access_denied_is_explained() {
        assert!(explain("HRESULT Call failed with: 0x80041003").contains("quyền admin"));
        assert_eq!(explain("lỗi khác"), "lỗi khác");
    }

    #[test]
    fn formats_battery_details_and_rows() {
        use lenovo_toolkit_core::battery::BatteryDetail;
        assert_eq!(watt_signed(Some(15_230)), "+15.23 W");
        assert_eq!(watt_signed(Some(-12_500)), "-12.50 W");
        assert_eq!(watt_signed(Some(0)), "0.00 W");
        assert_eq!(watt_signed(None), "-");
        assert_eq!(wh_text(75_400), "75.40 Wh");
        assert_eq!(wh_text(0), "-");
        assert_eq!(temp_f64(Some(31.42)), "31.4 °C");
        assert_eq!(temp_f64(None), "-");

        let d = BatteryDetail {
            ac_online: true,
            is_charging: false,
            is_low_battery: false,
            percent: 80,
            life_remaining_secs: None,
            rate_mw: Some(0),
            min_rate_mw: Some(0),
            max_rate_mw: Some(0),
            current_capacity_mwh: 64_000,
            full_charge_capacity_mwh: 78_000,
            design_capacity_mwh: 80_000,
            cycle_count: 42,
            temperature_c: Some(29.5),
            manufacture_date: Some("2024-03-12".into()),
            first_use_date: Some("2024-06-01".into()),
        };
        assert_eq!(battery_status_line(&d, Lang::En), "Connected, not charging");
        assert_eq!(battery_status_line(&d, Lang::Vi), "Đã cắm sạc, không sạc");
        let rows = build_battery_rows(&d, Lang::En);
        assert_eq!(rows.len(), 11);
        assert_eq!(rows[0].0, "Battery temperature");
        assert_eq!(rows[0].2, "29.5 °C");
        assert_eq!(rows[7].0, "Battery health");
        assert_eq!(rows[7].2, "97.50 %");
        assert_eq!(rows[8].0, "Cycle count");
        assert_eq!(rows[8].2, "42");

        let rows_vi = build_battery_rows(&d, Lang::Vi);
        assert_eq!(rows_vi[0].0, "Nhiệt độ pin");
        assert_eq!(rows_vi[7].0, "Độ bền pin (Health)");
    }
}
