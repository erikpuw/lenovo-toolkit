#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Giao diện Slint gọn nhẹ kèm System Tray Win32 và chống phình RAM.
//! - Một luồng worker duy nhất sở hữu kết nối WMI được tái sử dụng (`Option<Wmi>`).
//! - Khi cửa sổ ẩn dưới System Tray (`--minimized` hoặc đóng/thu nhỏ), worker tạm dừng hoàn toàn
//!   việc đọc cảm biến và cập nhật UI, đồng thời gọi `EmptyWorkingSet` để thu hồi RAM.
//! - Khi mở cửa sổ: chỉ đọc chi tiết pin (`battery::detail`) khi đang ở tab Battery (`page == 1`).


use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use lenovo_toolkit_core::audio;
use lenovo_toolkit_core::autorun::{self, AutorunState};
use lenovo_toolkit_core::battery::{self, BatteryDetail, BatteryState, RateTracker};
use lenovo_toolkit_core::compat::{self, DeviceClass, GodModePlatform, MachineInfo};
use lenovo_toolkit_core::custom::{self, Editor};
use lenovo_toolkit_core::disabler::{self, Software, Status};
use lenovo_toolkit_core::display;
use lenovo_toolkit_core::driver::Device;
use lenovo_toolkit_core::events::{EventListener, Source, SpecialKey, UtilityEventListener};
use lenovo_toolkit_core::keys::{Key, KeyListener};
use lenovo_toolkit_core::fan::{self, Profile};
use lenovo_toolkit_core::gamezone::{self, Feature};
use lenovo_toolkit_core::lighting::{self, Light};
use lenovo_toolkit_core::power::{self, AdapterStatus, PowerMode};
use lenovo_toolkit_core::settings::{self, Lang, PowerSyncMode};
use lenovo_toolkit_core::sysstat::{self, CpuMeter};
use lenovo_toolkit_core::wmi_util::Wmi;
use lenovo_toolkit_core::{ai, energy};
use lenovo_toolkit::{
    DASH, autorun_label_tr, autorun_options, battery_label_tr, battery_options,
    battery_status_line, build_battery_rows, custom_cap_tr, explain, hybrid_label_tr,
    kbd_label_tr, kbd_options_tr, parse_autorun_label, parse_battery_label, parse_hybrid_label,
    parse_kbd_label, parse_power_label, parse_powersync_label, parse_usb_label, percent,
    power_label_tr, powersync_label_tr, powersync_options, ram_text, rpm, temp, tr, unsupported_notice,
    usb_label_tr, usb_options,
};
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, VecModel};
use lenovo_toolkit::tray::{self, TrayEvent};

slint::include_modules!();

const POLL: Duration = Duration::from_secs(2);
const IDLE_POLL: Duration = Duration::from_secs(10);
/// Số vòng poll giữa hai lần đọc lại điều khiển / kiểm tra AI Engine (10 giây).
const ITEMS_EVERY: u32 = 5;

enum Cmd {
    Combo(String, String),
    Toggle(String, bool),
    Ai(bool),
    CustomSelect(String),
    CustomTune(i32, i32),
    CustomFan(bool),
    CustomApply,
    CustomReset,
    /// Mở cửa sổ Custom: nạp lại cấu hình đã lưu, bỏ các thay đổi chưa áp dụng.
    CustomReload,
    /// Mở cửa sổ "Về thiết bị của bạn" khi bấm vào tên máy ở góc phải trên.
    DeviceInfoOpen,
    /// Người dùng chuyển tab (0 = Dashboard, 1 = Battery, 2 = Settings).
    SetPage(i32),
    /// Hiện hoặc ẩn cửa sổ xuống System Tray.
    SetVisible(bool),
    /// Đặt nhanh chế độ nguồn từ menu chuột phải ở System Tray.
    QuickPower(PowerMode),
    /// Tổ hợp `Fn+R`: chuyển đổi giữa các mức tần số quét không bị loại trừ.
    CycleRefreshRate,
    /// Rê chuột vào biểu tượng khay: đọc nhanh số liệu cho tooltip (không đi qua `apply`).
    Tooltip,
    /// Có sự kiện từ firmware (Fn+Q...) hoặc cần đọc lại ngay.
    Refresh,
}

// ---- Mô hình dữ liệu gửi sang UI (Send, chuyển thành kiểu Slint trong luồng UI) ----

struct DeviceInfoSnap {
    header: String,
    os: String,
    serial: String,
    serial_masked: String,
    model: String,
    mtm: String,
    bios: String,
    series: String,
    generation: String,
    power_modes: String,
    capabilities: String,
    cpu_name: String,
    cpu_sub: String,
    gpus: Vec<(String, String, String)>,
    rams: Vec<(String, String, String)>,
    mb_name: String,
    mb_vendor: String,
    drives: Vec<(String, String, String, f32)>,
    displays: Vec<(String, String, String)>,
    bat_charge: String,
    bat_health: String,
    bat_health_frac: f32,
    bat_design: String,
    bat_full: String,
    bat_cycles: String,
    bat_date: String,
    nets: Vec<(String, String, String)>,
}

#[derive(Clone)]
struct ItemSnap {
    id: String,
    icon: &'static str,
    title: String,
    message: String,
    hint: &'static str,
    kind: i32,
    options: Vec<String>,
    value: String,
    on: bool,
}

struct TuneSnap {
    id: i32,
    title: &'static str,
    description: &'static str,
    warning: &'static str,
    value_text: String,
    can_dec: bool,
    can_inc: bool,
    changed: bool,
}

struct CustomSnap {
    rows: Vec<TuneSnap>,
    locked: bool,
    active: String,
    fan_on: bool,
    note: String,
}

struct RowSnap {
    label: &'static str,
    value: String,
    fraction: f32,
}

struct SectionSnap {
    title: &'static str,
    subtitle: String,
    rows: Vec<RowSnap>,
}

#[derive(Default)]
struct Snapshot {
    vi: bool,
    model: String,
    status: String,
    /// Thông báo thiết bị không được hỗ trợ (rỗng với thiết bị gaming).
    notice: String,
    ai_enabled: bool,
    sensor_sections: Vec<SectionSnap>,
    /// Power, Graphics, Display, Other; `None` nếu chưa đọc lại ở vòng này.
    groups: Option<[Vec<ItemSnap>; 4]>,
    /// Trang Settings: App Behavior, Power, Smart Keys, Software Control, Notifications.
    settings_items: Option<[Vec<ItemSnap>; 5]>,
    custom: Option<CustomSnap>,
    device_info: Option<DeviceInfoSnap>,
    battery_percent: String,
    battery_status: String,
    battery_warning: String,
    battery_rows: Option<Vec<(&'static str, &'static str, String)>>,
}

#[derive(Default, Clone, Copy)]
struct Support {
    night: bool,
    usb: bool,
    logo: bool,
    ports: bool,
    touchpad: bool,
    fnlock: bool,
    winkey: bool,
    overdrive: bool,
    gpu_oc: bool,
    godmode_fnq: bool,
    kbd_max: u32,
}

impl Support {
    /// Mười cờ ở bit 0..9, `kbd_max` ở bit 16 trở lên.
    fn to_mask(self) -> u32 {
        let flags = [
            self.night, self.usb, self.logo, self.ports, self.touchpad, self.fnlock, self.winkey, self.overdrive,
            self.gpu_oc, self.godmode_fnq,
        ];
        flags.iter().enumerate().fold(self.kbd_max << 16, |m, (i, on)| m | (u32::from(*on) << i))
    }

    fn from_mask(m: u32) -> Self {
        let bit = |i: u32| (m >> i) & 1 == 1;
        Self {
            night: bit(0),
            usb: bit(1),
            logo: bit(2),
            ports: bit(3),
            touchpad: bit(4),
            fnlock: bit(5),
            winkey: bit(6),
            overdrive: bit(7),
            gpu_oc: bit(8),
            godmode_fnq: bit(9),
            kbd_max: m >> 16,
        }
    }

    /// Bố cục tạm khi chưa dò phần cứng: danh sách của lần chạy trước, lần đầu thì hiện đủ mọi thẻ.
    fn placeholder() -> Self {
        settings::layout_cache().map_or_else(|| Self::from_mask(0x0002_03FF), Self::from_mask)
    }
}

fn fraction(value: i32, max: i32) -> f32 {
    if value < 0 || max <= 0 { 0.0 } else { (value as f32 / max as f32).clamp(0.0, 1.0) }
}

fn detect_support(wmi: Option<&Wmi>, dev: Option<&Device>, info: Option<&MachineInfo>) -> Support {
    let mut s = Support::default();
    if let Some(d) = dev {
        s.night = energy::night_charge_get(d).is_ok();
        s.usb = energy::usb_get(d).is_ok();
        s.fnlock = energy::fn_lock_get(d).is_ok();
        s.kbd_max = energy::kbd_max_level(d).unwrap_or(0);
    }
    let lighting_excluded = info.is_some_and(|i| i.properties.is_excluded_from_lenovo_lighting);
    let logo_excluded = info.is_some_and(|i| i.properties.is_excluded_from_panel_logo_lenovo_lighting);
    if let Some(w) = wmi {
        s.ports = !lighting_excluded && lighting::is_supported(w, Light::Ports).unwrap_or(false);
        s.logo = !lighting_excluded && !logo_excluded && lighting::is_supported(w, Light::Logo).unwrap_or(false);
        s.touchpad = gamezone::is_supported_with(w, Feature::TouchpadLock).unwrap_or(false);
        s.winkey = gamezone::is_supported_with(w, Feature::WinKey).unwrap_or(false);
        s.overdrive = gamezone::is_supported_with(w, Feature::OverDrive).unwrap_or(false)
            && info.is_none_or(|i| compat::is_overdrive_supported(i.series, i.generation));
        s.gpu_oc = gamezone::gpu_oc_supported(w);
        s.godmode_fnq = power::god_mode_fnq_get(w).is_ok();
    }
    s
}

fn toggle(id: impl Into<String>, icon: &'static str, title: impl Into<String>, message: impl Into<String>, on: bool) -> ItemSnap {
    ItemSnap {
        id: id.into(),
        icon,
        title: title.into(),
        message: message.into(),
        hint: "",
        kind: 1,
        options: Vec::new(),
        value: String::new(),
        on,
    }
}

fn combo(
    id: impl Into<String>,
    icon: &'static str,
    title: impl Into<String>,
    message: impl Into<String>,
    options: Vec<String>,
    value: String,
) -> ItemSnap {
    ItemSnap {
        id: id.into(),
        icon,
        title: title.into(),
        message: message.into(),
        hint: "",
        kind: 0,
        options,
        value,
        on: false,
    }
}

/// Hiện thông báo phím nếu công tắc "Thông báo phím" đang bật.
fn notify(toast: tray::Toast) {
    if settings::flag(settings::NOTIFY_ALL, true) && settings::flag(toast.flag, true) {
        tray::show_toast(toast);
    }
}

/// Trang Settings: App Behavior, Power, Smart Keys (Fn+R / Fn+F9), và Software Control.
#[allow(clippy::too_many_arguments)]
fn build_settings(
    lang: Lang,
    wmi: Option<&Wmi>,
    sup: &Support,
    autorun: AutorunState,
    min_to_tray: bool,
    min_on_close: bool,
    vantage: Option<Status>,
    legion_zone: Option<Status>,
    legion_space: Option<Status>,
    hotkeys: Option<Status>,
) -> [Vec<ItemSnap>; 5] {
    let app = vec![
        combo(
            "lang",
            "\u{F2B7}",
            tr(lang, "Language", "Ngôn ngữ"),
            tr(lang, "Choose interface display language.", "Chọn ngôn ngữ hiển thị của giao diện."),
            Lang::LABELS.iter().map(|(_, l)| (*l).to_string()).collect(),
            lang.label().to_string(),
        ),
        combo(
            "autorun",
            "\u{E768}",
            tr(lang, "Autorun", "Khởi động cùng Windows"),
            tr(
                lang,
                "Start hidden in the system tray when you log in to Windows.",
                "Tự động chạy ẩn dưới khay hệ thống khi đăng nhập Windows.",
            ),
            autorun_options(lang),
            autorun_label_tr(autorun, lang).to_string(),
        ),
        toggle(
            "min-to-tray",
            "\u{E740}",
            tr(lang, "Minimize to tray", "Thu nhỏ xuống khay hệ thống"),
            tr(
                lang,
                "Hide the window to the system tray and reclaim memory when minimized.",
                "Ẩn cửa sổ xuống khay hệ thống và giải phóng RAM khi bấm thu nhỏ.",
            ),
            min_to_tray,
        ),
        toggle(
            "min-on-close",
            "\u{E8BB}",
            tr(lang, "Minimize on close", "Ẩn xuống khay khi đóng (X)"),
            tr(
                lang,
                "Keep running in the system tray when clicking the window close (X) button.",
                "Tiếp tục chạy ngầm dưới khay hệ thống khi bấm nút đóng cửa sổ (X).",
            ),
            min_on_close,
        ),
    ];

    // Mục "Thông báo" riêng: công tắc chung rồi từng loại (icon trùng với icon hiện trong thông báo).
    let notify_toggle = |id: &str, icon: &'static str, title: &'static str, message: &'static str, flag: &str| {
        toggle(id, icon, title, message, settings::flag(flag, true))
    };
    let notify_items = vec![
        notify_toggle(
            "notify-all",
            "\u{EA8F}",
            tr(lang, "Key notifications", "Thông báo phím"),
            tr(
                lang,
                "Show a short on-screen message when you press these keys. Hidden while a game is fullscreen.",
                "Hiện thông báo ngắn trên màn hình khi bấm các phím bên dưới. Tự ẩn khi game toàn màn hình.",
            ),
            settings::NOTIFY_ALL,
        ),
        notify_toggle(
            "notify-power",
            "\u{E945}",
            tr(lang, "Power mode", "Chế độ nguồn"),
            tr(lang, "Fn+Q and power mode changes.", "Khi bấm Fn+Q hoặc đổi chế độ nguồn."),
            settings::NOTIFY_POWER,
        ),
        notify_toggle(
            "notify-fnlock",
            "\u{E765}",
            tr(lang, "Fn Lock / Camera", "Fn Lock / Camera"),
            tr(lang, "Fn Lock and camera switch state.", "Trạng thái Fn Lock và công tắc camera."),
            settings::NOTIFY_FNLOCK,
        ),
        notify_toggle(
            "notify-mic",
            "\u{E720}",
            tr(lang, "Microphone", "Micro"),
            tr(lang, "Fn+F4 microphone mute.", "Khi bấm Fn+F4 tắt/bật mic."),
            settings::NOTIFY_MIC,
        ),
        notify_toggle(
            "notify-touchpad",
            "\u{EFA5}",
            tr(lang, "Touchpad", "Touchpad"),
            tr(lang, "Fn+F10 touchpad lock.", "Khi bấm Fn+F10 khóa touchpad."),
            settings::NOTIFY_TOUCHPAD,
        ),
        notify_toggle(
            "notify-backlight",
            "\u{E706}",
            tr(lang, "Keyboard backlight", "Đèn bàn phím"),
            tr(lang, "Fn+Space backlight level.", "Khi bấm Fn+Space đổi mức đèn."),
            settings::NOTIFY_BACKLIGHT,
        ),
    ];

    let mut power_settings = Vec::new();
    if sup.godmode_fnq
        && let Some(w) = wmi
    {
        power_settings.push(toggle(
            "godmode-fnq",
            "\u{EC4A}",
            tr(lang, "Switch to Custom Mode with Fn+Q", "Chuyển sang Custom Mode bằng Fn+Q"),
            tr(
                lang,
                "Allow quick switching to Custom Mode with Fn+Q.",
                "Cho phép bấm tổ hợp Fn+Q để chuyển nhanh sang chế độ Custom.",
            ),
            power::god_mode_fnq_get(w).unwrap_or(false),
        ));
    }
    let sync_mode = settings::power_sync_mode();
    power_settings.push(combo(
        "powersync",
        "\u{E945}",
        tr(lang, "Power Mode Synchronization", "Đồng bộ chế độ nguồn Windows"),
        tr(
            lang,
            "Automatically change Windows Power Plan or Windows Power Mode when changing Power Modes.",
            "Tự động đổi Windows Power Mode (Tiết kiệm / Cân bằng / Hiệu suất cao) khi chuyển chế độ nguồn.",
        ),
        powersync_options(lang),
        powersync_label_tr(sync_mode, lang).to_string(),
    ));
    power_settings.push(toggle(
        "reset-battery-timer",
        "\u{E83E}",
        tr(
            lang,
            "Reset \"On battery since\" at startup",
            "Đặt lại bộ đếm \"Dùng pin từ\" khi khởi động",
        ),
        tr(
            lang,
            "Resets the counter for \"On battery since\" in the battery section when the system reboots.",
            "Đặt lại thời gian dùng pin trong mục Pin mỗi khi khởi động lại máy.",
        ),
        settings::flag(settings::RESET_BATTERY_ON_REBOOT, false),
    ));

    let mut software = Vec::new();
    let restart_hint = tr(
        lang,
        "Restart is recommended after changing this option.",
        "Nên khởi động lại máy sau khi thay đổi tùy chọn này.",
    );
    let entries = [
        (
            vantage,
            "disable-vantage",
            tr(
                lang,
                "Disable Lenovo Vantage and ImController",
                "Vô hiệu hóa Lenovo Vantage và ImController",
            ),
            tr(
                lang,
                "Disable Lenovo Vantage and ImController without uninstalling them.",
                "Tắt dịch vụ Lenovo Vantage và ImController chạy nền mà không cần gỡ cài đặt.",
            ),
        ),
        (
            legion_zone,
            "disable-legionzone",
            tr(lang, "Disable Legion Zone", "Vô hiệu hóa Legion Zone"),
            tr(
                lang,
                "Disable Lenovo Legion Zone and its background services without uninstalling it.",
                "Tắt Lenovo Legion Zone và dịch vụ chạy nền mà không cần gỡ cài đặt.",
            ),
        ),
        (
            legion_space,
            "disable-legionspace",
            tr(lang, "Disable Legion Space", "Vô hiệu hóa Legion Space"),
            tr(
                lang,
                "Disable Lenovo Legion Space and its background services without uninstalling it.",
                "Tắt Lenovo Legion Space và dịch vụ chạy nền mà không cần gỡ cài đặt.",
            ),
        ),
        (
            hotkeys,
            "disable-fnkeys",
            tr(lang, "Disable Lenovo Hotkeys", "Vô hiệu hóa Lenovo Hotkeys"),
            tr(
                lang,
                "Disable Lenovo Hotkeys and its service without uninstalling it, this app will handle the Fn shortcuts instead.",
                "Tắt dịch vụ Lenovo Hotkeys mà không cần gỡ cài đặt, ứng dụng này sẽ xử lý phím tắt Fn thay thế.",
            ),
        ),
    ];
    for (st, id, title, msg) in entries {
        if let Some(s) = st.filter(|s| *s != Status::NotFound) {
            let mut t = toggle(id, "\u{E711}", title, msg, s == Status::Disabled);
            t.hint = restart_hint;
            software.push(t);
        }
    }

    // Smart Keys: loại trừ mức tần số quét khi bấm Fn+R (tích chọn 60 Hz để giữ cố định 144 Hz)
    let mut smartkeys = Vec::new();
    let excluded = settings::excluded_refresh_rates();
    if let Ok((_, rates)) = display::refresh_rates() {
        for hz in rates {
            let title = match lang {
                Lang::En => format!("Exclude {hz} Hz from Fn+R"),
                Lang::Vi => format!("Loại trừ {hz} Hz khi bấm Fn+R"),
            };
            let msg = match lang {
                Lang::En => format!(
                    "When enabled, pressing Fn+R skips {hz} Hz (tick 60 Hz to keep the screen locked at your high refresh rate)."
                ),
                Lang::Vi => format!(
                    "Khi bật, bấm tổ hợp Fn+R sẽ bỏ qua mức {hz} Hz (bật mục 60 Hz nếu muốn giữ cố định 144 Hz)."
                ),
            };
            let mut item = toggle(format!("exclude-hz-{hz}"), "\u{E7F4}", title, msg, excluded.contains(&hz));
            item.hint = tr(
                lang,
                "Fn+F9 also opens Lenovo Toolkit from the system tray.",
                "Phím tắt Fn+F9 cũng được hỗ trợ để mở nhanh cửa sổ ứng dụng từ khay hệ thống.",
            );
            smartkeys.push(item);
        }
    }
    [app, power_settings, smartkeys, software, notify_items]
}

/// Dựng 4 nhóm điều khiển (Power, Graphics, Display, Other) theo luồng dọc từ trên xuống.
fn build_groups(
    lang: Lang,
    wmi: Option<&Wmi>,
    dev: Option<&Device>,
    info: Option<&MachineInfo>,
    sup: &Support,
    dgpu_awake: bool,
    // Khung hình đầu: chưa có kết nối phần cứng nhưng vẫn dựng đủ thẻ tĩnh, giá trị là "-" hoặc "đang đọc".
    loading: bool,
) -> [Vec<ItemSnap>; 4] {
    // Máy không thuộc nhóm gaming (ITS): không dựng điều khiển ghi, tránh ghi bừa xuống firmware.
    if info.is_some_and(|i| i.class() == DeviceClass::NonGaming) {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }
    let mut power_items = Vec::new();
    let mut graphics = Vec::new();
    let mut display = Vec::new();
    let mut other = Vec::new();

    // 1. Power
    let modes: Vec<PowerMode> = info
        .map(|i| i.supported_power_modes.clone())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| vec![PowerMode::Quiet, PowerMode::Balance, PowerMode::Performance]);
    let current_mode = wmi.and_then(|w| power::get_with(w).ok());
    let mut pm = combo(
        "powermode",
        "\u{E945}",
        tr(lang, "Power Mode", "Chế độ nguồn"),
        tr(
            lang,
            "Change performance mode. Performance mode can also be changed with Fn+Q.",
            "Chuyển chế độ hiệu năng. Có thể bấm tổ hợp phím Fn+Q để đổi nhanh.",
        ),
        modes.iter().map(|m| power_label_tr(*m, lang).to_string()).collect(),
        current_mode.map_or_else(|| DASH.to_string(), |m| power_label_tr(m, lang).to_string()),
    );
    let ai_gear = current_mode == Some(PowerMode::Balance) && info.is_some_and(|i| i.properties.supports_ai_mode);
    let custom_gear = current_mode == Some(PowerMode::GodMode)
        && info.is_some_and(|i| i.properties.god_mode_platform == Some(GodModePlatform::Legion));
    if ai_gear || custom_gear {
        pm.kind = 2;
    }
    power_items.push(pm);

    if dev.is_some() || loading {
        power_items.push(combo(
            "batterymode",
            "\u{E83E}",
            tr(lang, "Battery Mode", "Chế độ pin"),
            tr(lang, "Choose how the battery is charged.", "Chọn chế độ sạc và bảo vệ tuổi thọ pin."),
            battery_options(lang),
            dev.and_then(|d| battery::get(d).ok()).map_or_else(|| DASH.to_string(), |b| battery_label_tr(b, lang).to_string()),
        ));
    }
    if sup.night && (dev.is_some() || loading) {
        power_items.push(toggle(
            "nightcharge",
            "\u{E708}",
            tr(lang, "Overnight Battery Charging", "Sạc pin qua đêm"),
            tr(
                lang,
                "When enabled, this device will charge to 80% when plugged in overnight and finish charging to 100% by the time you use this device in the morning.",
                "Khi bật, máy giữ mức sạc 80% ban đêm và tự sạc đầy 100% trước khi bạn dùng vào buổi sáng.",
            ),
            dev.and_then(|d| energy::night_charge_get(d).ok()).unwrap_or(false),
        ));
    }
    if sup.usb && (dev.is_some() || loading) {
        power_items.push(combo(
            "usb",
            "\u{E88E}",
            tr(lang, "Always on USB", "Luôn bật sạc USB"),
            tr(
                lang,
                "Charge USB devices, when the laptop is off, sleeping or hibernated.",
                "Cấp nguồn sạc cho thiết bị USB ngay cả khi tắt máy, ngủ hoặc ngủ đông.",
            ),
            usb_options(lang),
            dev.and_then(|d| energy::usb_get(d).ok()).map_or_else(|| DASH.to_string(), |u| usb_label_tr(u, lang).to_string()),
        ));
    }

    // 2. Graphics (GPU Working Mode, Discrete GPU, Overclock GPU)
    if (wmi.is_some() && info.is_some()) || loading {
        // Chưa biết máy hỗ trợ gì (khung hình đầu): cứ hiện đủ lựa chọn, sau khi dò xong sẽ thu lại cho đúng.
        let gsync_sup = info.is_none_or(|i| i.properties.supports_gsync);
        let igpu_sup = info.is_none_or(|i| i.properties.supports_igpu_mode);
        let hmodes = gamezone::hybrid_supported_modes(gsync_sup, igpu_sup);
        if !hmodes.is_empty() {
            let cur_hybrid = wmi.and_then(|w| gamezone::hybrid_get(w, gsync_sup, igpu_sup).ok());
            let mut gm = combo(
                "hybridmode",
                "\u{E7F4}",
                tr(lang, "GPU Working Mode", "Chế độ hoạt động GPU"),
                tr(
                    lang,
                    "Select GPU operating mode based on your computer's usage and power conditions.",
                    "Chọn chế độ vận hành giữa card tích hợp (iGPU) và card rời (dGPU).",
                ),
                hmodes.iter().map(|m| hybrid_label_tr(*m, lang).to_string()).collect(),
                cur_hybrid.map_or_else(|| DASH.to_string(), |m| hybrid_label_tr(m, lang).to_string()),
            );
            gm.hint = tr(
                lang,
                "Switching modes may require restart.",
                "Chuyển chế độ Hybrid / dGPU có thể cần khởi động lại máy.",
            );
            graphics.push(gm);
        }
        let active_str = tr(lang, "Active", "Đang hoạt động");
        let deact_str = tr(lang, "Deactivate", "Ngắt card rời");
        let off_str = tr(lang, "Powered Off", "Đã tắt nguồn");
        let reading_str = tr(lang, "Reading...", "Đang đọc thông tin...");
        let dgpu_state = if loading {
            reading_str
        } else if dgpu_awake {
            active_str
        } else {
            off_str
        };
        let dgpu_opts = if loading {
            vec![reading_str.to_string()]
        } else if dgpu_awake {
            vec![active_str.to_string(), deact_str.to_string()]
        } else {
            vec![off_str.to_string()]
        };
        graphics.push(combo(
            "dgpu",
            "\u{E950}",
            tr(lang, "Discrete GPU", "Card đồ họa rời (dGPU)"),
            tr(
                lang,
                "Disable discrete GPU, if it is active unnecessarily.",
                "Trạng thái nguồn của card đồ họa rời. Chọn \"Ngắt card rời\" khi không dùng.",
            ),
            dgpu_opts,
            dgpu_state.to_string(),
        ));
        if sup.gpu_oc {
            graphics.push(toggle(
                "gpu-oc",
                "\u{E945}",
                tr(lang, "Overclock GPU", "Ép xung GPU"),
                tr(
                    lang,
                    "Increase performance by overclocking discrete GPU.",
                    "Tăng hiệu năng xử lý đồ họa bằng cách ép xung card rời.",
                ),
                settings::flag(settings::GPU_OVERCLOCK_ENABLED, false),
            ));
        }
    }

    // 3. Display (Refresh Rate + Over Drive)
    if let Ok((cur_hz, rates)) = display::refresh_rates()
        && !rates.is_empty()
    {
        display.push(combo(
            "refreshrate",
            "\u{E7F4}",
            tr(lang, "Refresh Rate", "Tần số quét màn hình"),
            tr(
                lang,
                "Change refresh rate of the built-in display. You can also use Fn+R shortcut.",
                "Đổi tần số quét của màn hình tích hợp. Có thể bấm tổ hợp phím Fn+R để chuyển nhanh.",
            ),
            rates.iter().map(|hz| format!("{hz} Hz")).collect(),
            format!("{cur_hz} Hz"),
        ));
    }
    if sup.overdrive {
        display.push(toggle(
            "overdrive",
            "\u{E706}",
            tr(lang, "Over Drive", "Tăng tốc màn hình (Over Drive)"),
            tr(
                lang,
                "Improve response time of the built-in display.",
                "Giảm thời gian phản hồi điểm ảnh của màn hình tích hợp.",
            ),
            wmi.and_then(|w| gamezone::get_with(w, Feature::OverDrive).ok()).is_some_and(|v| v == 1),
        ));
    }

    // 4. Other
    if sup.kbd_max >= 1 && (dev.is_some() || loading) {
        other.push(combo(
            "kbd",
            "\u{E765}",
            tr(lang, "Keyboard backlight", "Đèn nền bàn phím"),
            tr(
                lang,
                "You can change brightness with Fn+Space.",
                "Có thể đổi nhanh độ sáng đèn phím bằng tổ hợp Fn+Space.",
            ),
            kbd_options_tr(sup.kbd_max, lang),
            dev.and_then(|d| energy::kbd_get(d).ok()).map_or_else(|| DASH.to_string(), |l| kbd_label_tr(l, lang).to_string()),
        ));
    }
    if sup.logo || sup.ports {
        let on = |light| wmi.and_then(|w| lighting::get(w, light).ok()).is_some_and(|v| v == 1);
        if sup.logo {
            other.push(toggle(
                "logo",
                "\u{E706}",
                tr(lang, "Panel logo backlight", "Đèn logo nắp máy"),
                tr(
                    lang,
                    "Turn on or off the backlight on the lid of the laptop.",
                    "Bật hoặc tắt đèn biểu tượng trên mặt lưng màn hình.",
                ),
                on(Light::Logo),
            ));
        }
        if sup.ports {
            other.push(toggle(
                "ports",
                "\u{E88E}",
                tr(lang, "Ports backlight", "Đèn cổng kết nối sau máy"),
                tr(
                    lang,
                    "Turn on or off backlight of the ports on the back of the laptop.",
                    "Bật hoặc tắt đèn chiếu sáng các cổng kết nối phía sau máy.",
                ),
                on(Light::Ports),
            ));
        }
    }
    if sup.touchpad {
        other.push(toggle(
            "touchpad",
            "\u{E772}",
            tr(lang, "Touchpad Lock", "Khóa Touchpad"),
            tr(lang, "Disable touchpad.", "Vô hiệu hóa bàn di chuột cảm ứng."),
            wmi.and_then(|w| gamezone::get_with(w, Feature::TouchpadLock).ok()).is_some_and(|v| v == 1),
        ));
    }
    if sup.fnlock && ((dev.is_some() && info.is_some()) || loading) {
        other.push(toggle(
            "fnlock",
            "\u{E765}",
            tr(lang, "Fn Lock", "Khóa phím Fn"),
            tr(
                lang,
                "Use secondary functions of F1-F12 keys without holding Fn key.",
                "Sử dụng chức năng phụ của dãy phím F1-F12 mà không cần giữ phím Fn.",
            ),
            dev.zip(info)
                .and_then(|(d, i)| energy::fn_lock_get(d).ok().map(|hw| energy::fn_lock_translate(i.series, hw)))
                .unwrap_or(false),
        ));
    }
    if sup.winkey {
        other.push(toggle(
            "winkey",
            "\u{E765}",
            tr(lang, "Windows Key Lock", "Khóa phím Windows"),
            tr(
                lang,
                "Only works on built-in keyboard.",
                "Khóa phím Windows để tránh bấm nhầm khi chơi game (chỉ áp dụng cho bàn phím máy).",
            ),
            wmi.and_then(|w| gamezone::get_with(w, Feature::WinKey).ok()).is_some_and(|v| v == 1),
        ));
    }

    [power_items, graphics, display, other]
}

fn row(label: &'static str, value: impl Into<String>, fraction: f32) -> RowSnap {
    RowSnap { label, value: value.into(), fraction }
}

struct Worker {
    lang: Lang,
    wmi: Option<Wmi>,
    dev: Option<Device>,
    info: Option<MachineInfo>,
    profile: Option<&'static Profile>,
    fan_max: (i32, i32),
    cpu: CpuMeter,
    cpu_name: String,
    gpu_name: String,
    support: Option<Support>,
    support_without_info: bool,
    autorun: Option<AutorunState>,
    custom: Option<Editor>,
    custom_note: String,
    custom_applied: bool,
    hw_overview: Option<sysstat::HardwareOverview>,
    /// Kết quả đọc phần cứng chạy nền từ lúc khởi động (cũng cấp tên CPU/GPU cho bảng cảm biến).
    hw_rx: Option<mpsc::Receiver<sysstat::HardwareOverview>>,
    device_info_due: bool,
    op_list_cache: Option<Vec<(String, i32)>>,
    rate_tracker: RateTracker,
    visible: bool,
    page: i32,
    last_error: String,
    tick: u32,
    items_due: bool,
    settings_due: bool,
}

impl Worker {
    /// Áp dụng lại các cờ "chặn phần mềm Lenovo" khi khởi động (như bản C#): chỉ khi phần mềm đang chạy.
    fn enforce_software_flags() -> String {
        let wanted = [
            (Software::FnKeys, settings::DISABLE_LENOVO_HOTKEYS, true),
            (Software::Vantage, settings::DISABLE_VANTAGE, false),
            (Software::LegionZone, settings::DISABLE_LEGION_ZONE, false),
            (Software::LegionSpace, settings::DISABLE_LEGION_SPACE, false),
        ];
        let mut errors = Vec::new();
        for (sw, key, default) in wanted {
            if settings::flag(key, default) && disabler::status(sw).is_ok_and(|s| s == Status::Enabled) {
                match disabler::disable(sw) {
                    Ok(r) if r.status == Status::Enabled => errors.push(format!("{} may not have been disabled", sw.name())),
                    Ok(_) => {}
                    Err(e) => errors.push(format!("{}: {}", sw.name(), explain(e))),
                }
            }
        }
        errors.join(" · ")
    }

    fn new(start_visible: bool, hw_rx: Option<mpsc::Receiver<sysstat::HardwareOverview>>) -> Self {
        let mut cpu = CpuMeter::new();
        let _ = cpu.sample(); // mồi mẫu đầu tiên để lần mở cửa sổ có ngay % CPU
        let wmi = Wmi::open().ok();
        let info = wmi.as_ref().and_then(|w| compat::detect_with(w).ok());
        Self {
            lang: settings::lang(),
            wmi,
            autorun: None,
            dev: battery::open().ok(),
            info,
            profile: None,
            fan_max: (0, 0),
            cpu,
            cpu_name: String::new(),
            gpu_name: String::new(),
            support: None,
            support_without_info: false,
            custom: None,
            custom_note: String::new(),
            custom_applied: false,
            hw_overview: None,
            hw_rx,
            device_info_due: false,
            op_list_cache: None,
            rate_tracker: RateTracker::default(),
            visible: start_visible,
            page: 0,
            last_error: String::new(),
            tick: 0,
            items_due: true,
            settings_due: true,
        }
    }

    /// Lấy kết quả đọc phần cứng nền; `wait` thì chờ nếu chưa xong (thay vì đọc lại lần nữa).
    /// Lần đầu có kết quả cũng điền tên CPU/GPU cho bảng cảm biến.
    fn hw(&mut self, wait: bool) -> Option<&sysstat::HardwareOverview> {
        if self.hw_overview.is_none() {
            let got = self.hw_rx.as_ref().and_then(|rx| if wait { rx.recv().ok() } else { rx.try_recv().ok() });
            let hw = match got {
                Some(hw) => hw,
                None if wait => sysstat::hardware_overview(), // luồng nền không có/đã chết
                None => return None,
            };
            self.cpu_name = sysstat::short_cpu_name(&hw.cpu_full_name);
            self.gpu_name = hw
                .gpus
                .iter()
                .find(|g| !g.is_shared)
                .or(hw.gpus.first())
                .map(|g| sysstat::short_gpu_name(&g.name))
                .unwrap_or_default();
            self.hw_overview = Some(hw);
        }
        self.hw_overview.as_ref()
    }

    /// Thiết bị thuộc nhóm gaming (hoặc chưa rõ): chỉ khi đó mới ghi xuống phần cứng.
    fn gaming(&self) -> bool {
        self.info.as_ref().is_none_or(|i| i.class() == DeviceClass::Gaming)
    }

    /// Thông báo "chưa hỗ trợ" cho máy không gaming; rỗng nếu là máy gaming.
    fn unsupported_message(&self) -> String {
        self.info
            .as_ref()
            .filter(|i| i.class() == DeviceClass::NonGaming)
            .map(|i| unsupported_notice(self.lang, &i.model))
            .unwrap_or_default()
    }

    /// Đọc nhanh số liệu cho tooltip khay (chỉ chạy khi rê chuột vào biểu tượng) rồi gửi cho cửa sổ tooltip.
    fn show_tooltip(&mut self) {
        let lang = self.lang;
        let _ = self.ensure_wmi();
        if self.profile.is_none()
            && let Some(w) = self.wmi.as_ref()
        {
            self.profile = fan::detect(w).ok();
        }
        let sensors = match (self.wmi.as_ref(), self.profile) {
            (Some(w), Some(p)) => fan::read(w, p).ok(),
            _ => None,
        };
        // % CPU cần hai mẫu cách nhau: lấy mẫu mồi rồi đọc lại sau một khoảng ngắn.
        let _ = self.cpu.sample();
        thread::sleep(Duration::from_millis(250));
        let cpu_util = self.cpu.sample();

        let (cpu_temp, ec_gpu_temp, cpu_fan, gpu_fan) =
            sensors.map_or((-1, -1, -1, -1), |x| (x.cpu_temp, x.gpu_temp, x.cpu_fan_rpm, x.gpu_fan_rpm));
        let nv = sysstat::gpu_stats();
        let gpu_temp = if ec_gpu_temp > 0 { ec_gpu_temp } else { nv.temp_c.unwrap_or(-1) };
        let mode = self
            .wmi
            .as_ref()
            .and_then(|w| power::get_with(w).ok())
            .map_or_else(|| DASH.to_string(), |m| power_label_tr(m, lang).to_string());
        let mem = sysstat::memory().ok();
        let bat = sysstat::battery().ok();

        let mut lines = vec![lenovo_toolkit::app_title(), format!("{}: {mode}", tr(lang, "Power mode", "Chế độ nguồn"))];
        if self.gaming() {
            lines.push(format!("CPU:  {} · {} · {}", percent(cpu_util), temp(cpu_temp), rpm(cpu_fan)));
            lines.push(format!("GPU:  {} · {} · {}", percent(nv.utilization), temp(gpu_temp), rpm(gpu_fan)));
        } else {
            lines.push(format!("CPU:  {}", percent(cpu_util)));
            lines.push(format!("GPU:  {} · {}", percent(nv.utilization), temp(gpu_temp)));
        }
        lines.push(format!(
            "RAM:  {}",
            mem.map_or_else(|| DASH.to_string(), |m| format!("{} ({}%)", ram_text(m.used_gb, m.total_gb), m.load_percent))
        ));
        lines.push(match bat {
            Some(b) => {
                let state = if b.ac_online && b.charging {
                    tr(lang, "charging", "đang sạc")
                } else if b.ac_online {
                    "AC"
                } else {
                    tr(lang, "on battery", "dùng pin")
                };
                format!("{}:  {} · {state}", tr(lang, "Battery", "Pin"), percent(b.percent.map(u32::from)))
            }
            None => format!("{}:  {DASH}", tr(lang, "Battery", "Pin")),
        });
        tray::show_tooltip(lines);
    }

    fn ensure_wmi(&mut self) -> Option<&Wmi> {
        if self.wmi.is_none() {
            self.wmi = Wmi::open().ok();
        }
        self.wmi.as_ref()
    }

    fn apply(&mut self, cmd: Cmd) {
        self.items_due = true;
        let result = match cmd {
            Cmd::Refresh => Ok(()),
            Cmd::SetPage(p) => {
                self.page = p;
                if p == 2 {
                    self.settings_due = true;
                }
                Ok(())
            }
            Cmd::SetVisible(v) => {
                self.visible = v;
                if v {
                    let _ = self.cpu.sample();
                }
                Ok(())
            }
            Cmd::Tooltip => Ok(()),
            Cmd::QuickPower(_) if !self.gaming() => Err(self.unsupported_message()),
            Cmd::QuickPower(mode) => {
                if let Some(w) = self.ensure_wmi() {
                    power::set_with(w, mode).map_err(explain)
                } else {
                    power::set(mode).map_err(explain)
                }
            }
            Cmd::CycleRefreshRate => {
                // Chỉ xử lý Fn+R khi dịch vụ Lenovo Hotkeys đã tắt hoặc không có trên máy
                if disabler::status(Software::FnKeys).is_ok_and(|s| s == Status::Enabled) {
                    Ok(())
                } else {
                    match display::refresh_rates() {
                        Ok((cur, all)) => {
                            let excluded = settings::excluded_refresh_rates();
                            if let Some(next_hz) = display::next_refresh_rate(&all, &excluded, cur) {
                                display::set_refresh_rate(next_hz).map_err(explain)
                            } else {
                                Ok(())
                            }
                        }
                        Err(e) => Err(explain(e)),
                    }
                }
            }
            Cmd::Ai(on) => ai::set_enabled(on)
                .and_then(|()| {
                    if on {
                        if let Some(w) = self.wmi.as_ref() {
                            ai::refresh_with(w, &mut self.op_list_cache).map(drop)
                        } else {
                            ai::refresh().map(drop)
                        }
                    } else {
                        ai::disable_if_needed().map(drop)
                    }
                })
                .map_err(explain),
            Cmd::Combo(id, value) => self.combo(&id, &value),
            Cmd::Toggle(id, on) => self.toggle(&id, on),
            Cmd::CustomSelect(label) => {
                if let (Some(e), Some(a)) = (self.custom.as_mut(), custom::Active::parse_label(&label)) {
                    e.select(a);
                    self.custom_note.clear();
                }
                Ok(())
            }
            Cmd::CustomTune(id, dir) => {
                if let Some(e) = self.custom.as_mut() {
                    e.tune(id as u32, dir);
                }
                Ok(())
            }
            Cmd::CustomFan(on) => {
                if let Some(e) = self.custom.as_mut() {
                    e.set_fan_full_speed(on);
                }
                Ok(())
            }
            Cmd::CustomReset => {
                if let Some(e) = self.custom.as_mut() {
                    e.reset_user();
                    self.custom_note.clear();
                }
                Ok(())
            }
            Cmd::CustomReload => {
                self.custom = None;
                self.custom_note.clear();
                self.ensure_custom();
                Ok(())
            }
            Cmd::CustomApply => self.apply_custom(),
            Cmd::DeviceInfoOpen => {
                self.device_info_due = true;
                Ok(())
            }
        };
        self.last_error = result.err().map(|e| format!("Error: {e}")).unwrap_or_default();
    }

    fn build_editor(w: &Wmi, info: &MachineInfo) -> Option<Editor> {
        if info.properties.god_mode_platform != Some(GodModePlatform::Legion) {
            return None;
        }
        let ranges = custom::read_ranges(w).ok().filter(|r| !r.is_empty())?;
        let defaults = custom::read_mode_defaults(w).unwrap_or_default();
        let lenovo = custom::lenovo_default(&ranges, &defaults, info.properties.supports_extreme_mode);
        let user = custom::load_user().unwrap_or_else(|| custom::current_preset(w, &ranges));
        Some(Editor::new(ranges, lenovo, Some(user), custom::load_active()))
    }

    fn ensure_custom(&mut self) {
        if self.custom.is_none() {
            let _ = self.ensure_wmi();
            if let (Some(w), Some(info)) = (self.wmi.as_ref(), self.info.as_ref()) {
                self.custom = Self::build_editor(w, info);
            }
        }
    }

    fn apply_custom(&mut self) -> Result<(), String> {
        let editor = self.custom.as_ref().ok_or("Custom mode data is not available on this device")?;
        custom::save(editor).map_err(|e| e.to_string())?;
        let cur = editor.current().clone();
        let ranges = editor.ranges.clone();
        let w = self.ensure_wmi().ok_or("WMI connection unavailable")?;
        if power::get_with(w).ok() != Some(PowerMode::GodMode) {
            self.custom_note = tr(
                self.lang,
                "Saved. It will be applied when Custom power mode is active.",
                "Đã lưu. Thông số sẽ tự áp dụng khi chuyển sang chế độ Custom.",
            )
            .into();
            return Ok(());
        }
        let warnings = custom::apply(w, &cur, &ranges).map_err(explain)?;
        self.custom_note = if warnings.is_empty() {
            tr(self.lang, "Applied.", "Đã áp dụng.").into()
        } else {
            format!("Applied. Not supported by this device: {}", warnings.join("; "))
        };
        Ok(())
    }

    fn apply_saved_custom_on_entry(&mut self) {
        let _ = self.ensure_wmi();
        let mode_is_custom = self
            .wmi
            .as_ref()
            .and_then(|w| power::get_with(w).ok())
            .is_some_and(|m| m == PowerMode::GodMode);
        if !mode_is_custom {
            self.custom_applied = false;
            return;
        }
        if self.custom_applied {
            return;
        }
        let Some((w, info)) = self.wmi.as_ref().zip(self.info.as_ref()) else { return };
        let Some(editor) = Self::build_editor(w, info) else { return };
        self.custom_applied = true;
        if let Err(e) = custom::apply(w, editor.current(), &editor.ranges) {
            self.last_error = format!("Custom mode: {}", explain(e));
        }
    }

    fn custom_snap(&self) -> Option<CustomSnap> {
        let e = self.custom.as_ref()?;
        let cur = e.current();
        let rows = e
            .ranges
            .iter()
            .filter_map(|r| {
                let info = custom::cap_info(r.id)?;
                let (title, description, warning) = custom_cap_tr(r.id, self.lang).unwrap_or((
                    info.title,
                    info.description,
                    info.warning,
                ));
                let v = cur.values.get(&r.id).copied().unwrap_or(r.default);
                Some(TuneSnap {
                    id: r.id as i32,
                    title,
                    description,
                    warning,
                    value_text: format!("{v} {}", info.unit),
                    can_dec: r.next(v, -1) != v,
                    can_inc: r.next(v, 1) != v,
                    changed: e.differs_from_lenovo(r.id),
                })
            })
            .collect();
        Some(CustomSnap {
            rows,
            locked: e.is_locked(),
            active: e.active.label().to_string(),
            fan_on: cur.fan_full_speed,
            note: self.custom_note.clone(),
        })
    }

    fn build_device_info(&mut self) -> DeviceInfoSnap {
        let lang = self.lang;
        let hw = self.hw(true).cloned().unwrap_or_default();
        let info = self.info.as_ref();

        let vendor = info.map_or("LENOVO", |i| i.vendor.as_str());
        let model = info.map_or("Legion", |i| i.model.as_str());
        let mtm = info.map_or(DASH, |i| i.machine_type.as_str());
        let bios = info
            .and_then(|i| i.bios_raw.as_deref())
            .filter(|s| !s.is_empty())
            .unwrap_or(DASH);
        let serial = if !hw.serial_number.is_empty() {
            hw.serial_number.clone()
        } else {
            info.map(|i| i.serial_number.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| DASH.to_string())
        };
        let serial_masked = if serial == DASH {
            DASH.to_string()
        } else {
            "*".repeat(serial.len().max(8))
        };

        let series = info.map(|i| i.series.label()).unwrap_or("").to_string();
        let generation = info
            .filter(|i| i.generation > 0)
            .map(|i| {
                match lang {
                    Lang::En => format!("Gen {}", i.generation),
                    Lang::Vi => format!("Thế hệ {}", i.generation),
                }
            })
            .unwrap_or_default();

        let modes = info
            .map(|i| &i.supported_power_modes)
            .filter(|m| !m.is_empty())
            .map(|m| {
                m.iter()
                    .map(|p| power_label_tr(*p, lang))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| DASH.to_string());

        let mut caps = Vec::new();
        if let Some(i) = info {
            if i.properties.supports_gsync {
                caps.push("G-Sync".to_string());
            }
            if i.properties.supports_igpu_mode {
                caps.push("iGPU Mode".to_string());
            }
            if i.properties.supports_ai_mode {
                caps.push("AI Mode".to_string());
            }
            if i.properties.supports_extreme_mode {
                caps.push("Extreme Mode".to_string());
            }
            if i.properties.god_mode_platform.is_some() {
                caps.push("Custom Mode (God Mode)".to_string());
            }
            if i.properties.supports_boot_logo_change {
                caps.push("Boot Logo Customization".to_string());
            }
            if i.legion_zone_version > 0 {
                caps.push(format!("Legion Zone v{}", i.legion_zone_version));
            }
        }
        if let Some(sup) = self.support {
            if sup.overdrive {
                caps.push("Screen Overdrive".to_string());
            }
            if sup.godmode_fnq {
                caps.push("Fn+Q Custom Mode Switching".to_string());
            }
            if sup.gpu_oc {
                caps.push("GPU Overclocking".to_string());
            }
        }

        let cpu_sub = if hw.cpu_cores > 0 && hw.cpu_threads > 0 {
            match lang {
                Lang::En => format!("{} Cores / {} Threads", hw.cpu_cores, hw.cpu_threads),
                Lang::Vi => format!("{} Nhân / {} Luồng", hw.cpu_cores, hw.cpu_threads),
            }
        } else {
            String::new()
        };

        let gpus = hw
            .gpus
            .iter()
            .map(|g| {
                let sub = if g.is_shared {
                    tr(lang, "Shared / Dynamic", "Chia sẻ / Động").to_string()
                } else if let Some(v) = g.vram_gb {
                    format!("{v:.1} GB")
                } else {
                    tr(lang, "Dedicated GPU", "Card đồ họa rời").to_string()
                };
                (g.name.clone(), String::new(), sub)
            })
            .collect();

        let rams = hw
            .rams
            .iter()
            .map(|r| {
                let title = format!("{:.0} GB  {}", r.capacity_gb, r.generation);
                let right = if r.speed_mhz > 0 {
                    format!("{} MHz", r.speed_mhz)
                } else {
                    String::new()
                };
                let slot_str = match lang {
                    Lang::En => format!("Slot {}", r.slot),
                    Lang::Vi => format!("Khe cắm {}", r.slot),
                };
                let sub = if r.manufacturer.is_empty() {
                    slot_str
                } else {
                    format!("{slot_str} • {}", r.manufacturer)
                };
                (title, right, sub)
            })
            .collect();

        let drives = hw
            .drives
            .iter()
            .map(|d| {
                let title = if d.label.is_empty() {
                    d.name.clone()
                } else {
                    format!("{} {}", d.name, d.label)
                };
                let usage = match lang {
                    Lang::En => format!("{:.1} GB Used / {:.1} GB Total", d.used_gb, d.total_gb),
                    Lang::Vi => format!("{:.1} GB Đã dùng / {:.1} GB Tổng", d.used_gb, d.total_gb),
                };
                let frac = if d.total_gb > 0.0 {
                    (d.used_gb / d.total_gb).clamp(0.0, 1.0) as f32
                } else {
                    0.0
                };
                (title, usage, d.model.clone(), frac)
            })
            .collect();

        let displays = hw
            .displays
            .iter()
            .map(|m| {
                let title = match lang {
                    Lang::En => format!("Display {}: {}", m.index, m.name),
                    Lang::Vi => format!("Màn hình {}: {}", m.index, m.name),
                };
                let sub = format!("{} × {}  @  {} Hz", m.width, m.height, m.hz);
                (title, String::new(), sub)
            })
            .collect();

        let nets = hw
            .networks
            .iter()
            .map(|n| (n.name.clone(), String::new(), n.mac.clone()))
            .collect();

        let (bat_charge, bat_health, bat_health_frac, bat_design, bat_full, bat_cycles, bat_date) =
            match battery::detail(self.dev.as_ref(), &mut self.rate_tracker) {
                Ok(d) => {
                    let h = d.health_percent().unwrap_or(0.0);
                    (
                        format!("{:.1}%", f64::from(d.percent)),
                        format!("{h:.1}%"),
                        (h / 100.0).clamp(0.0, 1.0) as f32,
                        format!("{:.1} Wh", f64::from(d.design_capacity_mwh) / 1000.0),
                        format!("{:.1} Wh", f64::from(d.full_charge_capacity_mwh) / 1000.0),
                        d.cycle_count.to_string(),
                        d.manufacture_date.unwrap_or_else(|| DASH.to_string()),
                    )
                }
                Err(_) => (
                    DASH.into(),
                    DASH.into(),
                    0.0,
                    DASH.into(),
                    DASH.into(),
                    DASH.into(),
                    DASH.into(),
                ),
            };

        DeviceInfoSnap {
            header: format!("{vendor} {model}").to_uppercase(),
            os: hw.os_name,
            serial,
            serial_masked,
            model: model.to_string(),
            mtm: mtm.to_string(),
            bios: bios.to_string(),
            series,
            generation,
            power_modes: modes,
            capabilities: caps.join(", "),
            cpu_name: hw.cpu_full_name,
            cpu_sub,
            gpus,
            rams,
            mb_name: if hw.motherboard_product.is_empty() {
                DASH.to_string()
            } else {
                hw.motherboard_product
            },
            mb_vendor: hw.motherboard_manufacturer,
            drives,
            displays,
            bat_charge,
            bat_health,
            bat_health_frac,
            bat_design,
            bat_full,
            bat_cycles,
            bat_date,
            nets,
        }
    }

    fn dev(&self) -> Result<&Device, String> {
        self.dev.as_ref().ok_or_else(|| "cannot open EnergyDrv driver".to_string())
    }

    fn combo(&mut self, id: &str, value: &str) -> Result<(), String> {
        let bad = || format!("invalid value '{value}' for {id}");
        match id {
            "lang" => {
                let l = Lang::parse_label(value).ok_or_else(bad)?;
                settings::set_lang(l).map_err(|e| e.to_string())?;
                self.lang = l;
                self.settings_due = true;
                if self.hw_overview.is_some() {
                    self.device_info_due = true;
                }
                Ok(())
            }
            "autorun" => {
                let state = parse_autorun_label(value).ok_or_else(bad)?;
                autorun::set(state).map_err(explain)?;
                self.autorun = Some(autorun::state().unwrap_or(state));
                self.settings_due = true;
                Ok(())
            }
            "powersync" => {
                let m = parse_powersync_label(value).ok_or_else(bad)?;
                settings::set_power_sync_mode(m).map_err(|e| e.to_string())?;
                if m == PowerSyncMode::WindowsPowerMode
                    && let Some(cur) = self.wmi.as_ref().and_then(|w| power::get_with(w).ok())
                {
                    power::sync_windows_power_mode(cur);
                }
                self.settings_due = true;
                Ok(())
            }
            "powermode" => {
                let mode = parse_power_label(value).ok_or_else(bad)?;
                if self.info.as_ref().is_some_and(|i| !i.supported_power_modes.contains(&mode)) {
                    return Err(format!("power mode {value} is not supported on this device"));
                }
                if let Some(w) = self.ensure_wmi() {
                    power::set_with(w, mode).map_err(explain)
                } else {
                    power::set(mode).map_err(explain)
                }
            }
            "hybridmode" => {
                let mode = parse_hybrid_label(value).ok_or_else(bad)?;
                let (gsync, igpu) = self
                    .info
                    .as_ref()
                    .map(|i| (i.properties.supports_gsync, i.properties.supports_igpu_mode))
                    .unwrap_or((false, false));
                let w = self.ensure_wmi().ok_or("WMI unavailable")?;
                gamezone::hybrid_set(w, gsync, igpu, mode).map_err(explain)
            }
            "dgpu" => {
                if value == "Deactivate" || value == "Ngắt card rời" {
                    let w = self.ensure_wmi().ok_or("WMI unavailable")?;
                    gamezone::deactivate_dgpu(w).map_err(explain)?;
                }
                Ok(())
            }
            "batterymode" => {
                let state: BatteryState = parse_battery_label(value).ok_or_else(bad)?;
                battery::set(self.dev()?, state).map(drop).map_err(explain)
            }
            "usb" => energy::usb_set(self.dev()?, parse_usb_label(value).ok_or_else(bad)?).map_err(explain),
            "kbd" => energy::kbd_set(self.dev()?, parse_kbd_label(value).ok_or_else(bad)?).map_err(explain),
            "refreshrate" => {
                let hz: u32 = value
                    .trim_end_matches("Hz")
                    .trim()
                    .parse()
                    .map_err(|_| bad())?;
                display::set_refresh_rate(hz).map_err(explain)
            }
            _ => Err(format!("unknown control {id}")),
        }
    }

    fn toggle(&mut self, id: &str, on: bool) -> Result<(), String> {
        let software = |sw: Software, key: &str, name: &str| -> Result<(), String> {
            settings::set_flag(key, on).map_err(|e| e.to_string())?;
            if on {
                let r = disabler::disable(sw).map_err(explain)?;
                if r.status == Status::Enabled {
                    return Err(format!("{name} may not have been disabled correctly"));
                }
                Ok(())
            } else {
                disabler::enable(sw).map_err(explain)
            }
        };
        let res = match id {
            "min-to-tray" => settings::set_flag(settings::MINIMIZE_TO_TRAY, on).map_err(|e| e.to_string()),
            "min-on-close" => settings::set_flag(settings::MINIMIZE_ON_CLOSE, on).map_err(|e| e.to_string()),
            "notify-all" => settings::set_flag(settings::NOTIFY_ALL, on).map_err(|e| e.to_string()),
            "notify-power" => settings::set_flag(settings::NOTIFY_POWER, on).map_err(|e| e.to_string()),
            "notify-fnlock" => settings::set_flag(settings::NOTIFY_FNLOCK, on).map_err(|e| e.to_string()),
            "notify-mic" => settings::set_flag(settings::NOTIFY_MIC, on).map_err(|e| e.to_string()),
            "notify-touchpad" => settings::set_flag(settings::NOTIFY_TOUCHPAD, on).map_err(|e| e.to_string()),
            "notify-backlight" => settings::set_flag(settings::NOTIFY_BACKLIGHT, on).map_err(|e| e.to_string()),
            "reset-battery-timer" => settings::set_flag(settings::RESET_BATTERY_ON_REBOOT, on).map_err(|e| e.to_string()),
            "godmode-fnq" => {
                let w = self.ensure_wmi().ok_or("WMI unavailable")?;
                power::god_mode_fnq_set(w, on).map_err(explain)
            }
            "gpu-oc" => settings::set_flag(settings::GPU_OVERCLOCK_ENABLED, on).map_err(|e| e.to_string()),
            "disable-vantage" => software(Software::Vantage, settings::DISABLE_VANTAGE, "Lenovo Vantage and/or ImController"),
            "disable-legionzone" => software(Software::LegionZone, settings::DISABLE_LEGION_ZONE, "Legion Zone"),
            "disable-legionspace" => software(Software::LegionSpace, settings::DISABLE_LEGION_SPACE, "Legion Space"),
            "disable-fnkeys" => software(Software::FnKeys, settings::DISABLE_LENOVO_HOTKEYS, "Lenovo Hotkeys"),
            "nightcharge" => energy::night_charge_set(self.dev()?, on).map_err(explain),
            "overdrive" => {
                let w = self.ensure_wmi().ok_or("WMI unavailable")?;
                gamezone::set_with(w, Feature::OverDrive, i32::from(on)).map_err(explain)
            }
            "logo" => {
                let w = self.ensure_wmi().ok_or("WMI unavailable")?;
                lighting::set(w, Light::Logo, i32::from(on)).map_err(explain)
            }
            "ports" => {
                let w = self.ensure_wmi().ok_or("WMI unavailable")?;
                lighting::set(w, Light::Ports, i32::from(on)).map_err(explain)
            }
            "touchpad" => {
                let w = self.ensure_wmi().ok_or("WMI unavailable")?;
                gamezone::set_with(w, Feature::TouchpadLock, i32::from(on)).map_err(explain)
            }
            "winkey" => {
                let w = self.ensure_wmi().ok_or("WMI unavailable")?;
                gamezone::set_with(w, Feature::WinKey, i32::from(on)).map_err(explain)
            }
            "fnlock" => {
                let series = self.info.as_ref().map(|i| i.series).ok_or("machine information unavailable")?;
                energy::fn_lock_set(self.dev()?, energy::fn_lock_translate(series, on)).map_err(explain)
            }
            _ if id.starts_with("exclude-hz-") => {
                let hz: u32 = id["exclude-hz-".len()..]
                    .parse()
                    .map_err(|_| format!("invalid refresh rate id {id}"))?;
                settings::set_refresh_rate_excluded(hz, on).map_err(|e| e.to_string())?;
                self.settings_due = true;
                // Nếu người dùng vừa loại trừ mức đang dùng và chỉ còn 1 mức khả dụng (ví dụ giữ cố định 144 Hz), tự chuyển sang mức đó luôn
                if on
                    && let Ok((cur, all)) = display::refresh_rates()
                    && cur == hz
                {
                    let excluded = settings::excluded_refresh_rates();
                    if let Some(next_hz) = display::next_refresh_rate(&all, &excluded, cur) {
                        let _ = display::set_refresh_rate(next_hz);
                    }
                }
                Ok(())
            }
            _ => Err(format!("unknown control {id}")),
        };
        if matches!(
            id,
            "min-to-tray"
                | "min-on-close"
                | "reset-battery-timer"
                | "godmode-fnq"
                | "disable-vantage"
                | "disable-legionzone"
                | "disable-legionspace"
                | "disable-fnkeys"
        ) {
            self.settings_due = true;
        }
        res
    }

    /// Xử lý nền nhẹ khi cửa sổ đang ẩn dưới System Tray (chỉ kiểm tra AI Engine và Custom Mode).
    fn background_tick(&mut self) {
        self.apply_saved_custom_on_entry();
        if self.info.as_ref().is_some_and(|i| i.properties.supports_ai_mode) && ai::is_enabled() {
            let _ = self.ensure_wmi();
            if let Some(w) = self.wmi.as_ref() {
                let _ = ai::refresh_with(w, &mut self.op_list_cache);
            }
        }
    }

    fn snapshot(&mut self) -> Snapshot {
        let lang = self.lang;
        if self.dev.is_none() {
            self.dev = battery::open().ok();
        }
        let _ = self.ensure_wmi();
        if self.info.is_none() {
            self.info = self.wmi.as_ref().and_then(|w| compat::detect_with(w).ok());
        }

        // AI Engine: chỉ kiểm tra mỗi ITEMS_EVERY vòng (10 giây) hoặc khi vừa thao tác, dùng lại kết nối WMI.
        // Thẻ điều khiển và AI chỉ thuộc trang Dashboard: ở trang Pin/Settings không tốn hàng chục truy vấn WMI.
        let _ = self.hw(false);
        let items_tick = self.items_due || self.tick.is_multiple_of(ITEMS_EVERY);
        let check_items = items_tick && self.page == 0;
        self.items_due = false; // mọi lệnh kế tiếp (đổi trang, đổi cài đặt...) tự bật lại
        if check_items
            && self.last_error.is_empty()
            && self.info.as_ref().is_some_and(|i| i.properties.supports_ai_mode)
            && ai::is_enabled()
            && let Some(w) = self.wmi.as_ref()
            && let Err(e) = ai::refresh_with(w, &mut self.op_list_cache)
        {
            self.last_error = format!("AI: {}", explain(e));
        }

        self.apply_saved_custom_on_entry();

        let mut problems = Vec::new();
        if !self.last_error.is_empty() {
            problems.push(self.last_error.clone());
        }

        // Chỉ đọc cảm biến WMI khi đang ở trang Dashboard (page == 0) để giảm tải tối đa.
        let readings = if self.page == 0 {
            match self.wmi.as_ref() {
                Some(w) => {
                    if self.profile.is_none() {
                        self.profile = fan::detect(w).ok();
                        if let Some(p) = self.profile {
                            self.fan_max = fan::max_speeds(w, p).unwrap_or((0, 0));
                        }
                    }
                    match self.profile.map(|p| fan::read(w, p)) {
                        Some(Ok(x)) => Some(x),
                        Some(Err(e)) => {
                            problems.push(format!("sensors: {}", explain(e)));
                            self.wmi = None; // kết nối WMI lỗi -> mở lại ở vòng sau
                            None
                        }
                        None => {
                            problems.push("sensors: no fan profile detected".into());
                            None
                        }
                    }
                }
                None => {
                    problems.push("WMI: connection unavailable (run as Administrator)".into());
                    None
                }
            }
        } else {
            None
        };

        let cpu_util = if self.page == 0 { self.cpu.sample() } else { None };
        let mem = if self.page == 0 { sysstat::memory().ok() } else { None };
        let bat = sysstat::battery().ok();

        let (cpu_temp, ec_gpu_temp, pch_temp) = readings.map_or((-1, -1, -1), |x| (x.cpu_temp, x.gpu_temp, x.pch_temp));
        let (cpu_fan, gpu_fan) = readings.map_or((-1, -1), |x| (x.cpu_fan_rpm, x.gpu_fan_rpm));

        // Đọc % tải và nhiệt độ GPU NVIDIA qua nvapi64.dll (nhẹ, < 0.05ms, chỉ khi ở trang Home)
        let nv_gpu = if self.page == 0 { sysstat::gpu_stats() } else { sysstat::GpuStats::default() };
        let gpu_temp = if ec_gpu_temp > 0 {
            ec_gpu_temp
        } else {
            nv_gpu.temp_c.unwrap_or(-1)
        };
        let gpu_util = nv_gpu.utilization.or_else(|| (gpu_temp > 0).then_some(0));
        let dgpu_awake = gpu_temp > 0 || nv_gpu.utilization.is_some();

        // Bảng cảm biến gộp gọn 3 phần (CPU, GPU, System), đồng bộ 3 dòng mỗi cột.
        let mut sys_rows = vec![row(
            "RAM",
            mem.map_or_else(|| DASH.to_string(), |m| format!("{} ({}%)", ram_text(m.used_gb, m.total_gb), m.load_percent)),
            mem.map_or(0.0, |m| m.load_percent as f32 / 100.0),
        )];
        if pch_temp >= 0 {
            sys_rows.push(row(
                tr(lang, "PCH Temp", "Nhiệt độ PCH"),
                temp(pch_temp),
                fraction(pch_temp, 100),
            ));
        }
        sys_rows.push(row(
            tr(lang, "Battery", "Pin"),
            match bat {
                Some(b) => {
                    let st = if b.ac_online && b.charging {
                        tr(lang, "Charging", "Đang sạc")
                    } else if b.ac_online {
                        "AC"
                    } else {
                        tr(lang, "Battery", "Dùng pin")
                    };
                    format!("{} · {st}", percent(b.percent.map(u32::from)))
                }
                None => DASH.to_string(),
            },
            bat.and_then(|b| b.percent).map_or(0.0, |p| f32::from(p) / 100.0),
        ));

        let sensor_sections = vec![
            SectionSnap {
                title: "CPU",
                subtitle: self.cpu_name.clone(),
                rows: vec![
                    row(
                        tr(lang, "Utilization", "Sử dụng"),
                        percent(cpu_util),
                        cpu_util.map_or(0.0, |p| p as f32 / 100.0),
                    ),
                    row(tr(lang, "Temperature", "Nhiệt độ"), temp(cpu_temp), fraction(cpu_temp, 100)),
                    row(
                        tr(lang, "Fan Speed", "Tốc độ quạt"),
                        rpm(cpu_fan),
                        fraction(cpu_fan, self.fan_max.0),
                    ),
                ],
            },
            SectionSnap {
                title: "GPU",
                subtitle: self.gpu_name.clone(),
                rows: vec![
                    row(
                        tr(lang, "Utilization", "Sử dụng"),
                        percent(gpu_util),
                        gpu_util.map_or(0.0, |p| p as f32 / 100.0),
                    ),
                    row(tr(lang, "Temperature", "Nhiệt độ"), temp(gpu_temp), fraction(gpu_temp, 100)),
                    row(
                        tr(lang, "Fan Speed", "Tốc độ quạt"),
                        rpm(gpu_fan),
                        fraction(gpu_fan, self.fan_max.1),
                    ),
                ],
            },
            SectionSnap {
                title: tr(lang, "System", "Hệ thống"),
                subtitle: tr(lang, "Memory & Board", "Bộ nhớ & Bo mạch").into(),
                rows: sys_rows,
            },
        ];

        // Trang Battery (page == 1): đọc chi tiết IOCTL từ driver pin Windows + Lenovo EnergyDrv.
        let (battery_percent, battery_status, battery_warning, battery_rows) = if self.page == 1 {
            match battery::detail(self.dev.as_ref(), &mut self.rate_tracker) {
                Ok(d) => {
                    let adapter = self
                        .wmi
                        .as_ref()
                        .map_or(AdapterStatus::Connected, power::adapter_status_with);
                    let mut warns = Vec::new();
                    if d.is_low_battery {
                        warns.push(tr(lang, "Battery level low", "Pin yếu"));
                    }
                    if adapter == AdapterStatus::ConnectedLowWattage {
                        warns.push(tr(
                            lang,
                            "Low wattage charger connected",
                            "Đang cắm bộ sạc công suất thấp",
                        ));
                    }
                    (
                        format!("{}%", d.percent),
                        battery_status_line(&d, lang),
                        warns.join(" · "),
                        Some(build_battery_rows(&d, lang)),
                    )
                }
                Err(_) => (
                    percent(bat.and_then(|b| b.percent).map(u32::from)),
                    match bat {
                        Some(b) if b.ac_online && b.charging => tr(lang, "Connected, charging...", "Đã cắm sạc, đang sạc...").into(),
                        Some(b) if b.ac_online => tr(lang, "Connected, not charging", "Đã cắm sạc, không sạc").into(),
                        Some(_) => tr(lang, "Discharging", "Đang dùng pin").into(),
                        None => DASH.into(),
                    },
                    String::new(),
                    None,
                ),
            }
        } else {
            (
                percent(bat.and_then(|b| b.percent).map(u32::from)),
                match bat {
                    Some(b) if b.ac_online && b.charging => tr(lang, "Connected, charging...", "Đã cắm sạc, đang sạc...").into(),
                    Some(b) if b.ac_online => tr(lang, "Connected, not charging", "Đã cắm sạc, không sạc").into(),
                    Some(_) => tr(lang, "Discharging", "Đang dùng pin").into(),
                    None => DASH.into(),
                },
                String::new(),
                None,
            )
        };

        if self.support.is_none() || (self.support_without_info && self.info.is_some()) {
            self.support_without_info = self.info.is_none();
            let sup = detect_support(self.wmi.as_ref(), self.dev.as_ref(), self.info.as_ref());
            // Chỉ lưu khi đã dò được thật (có kết nối) và khác lần trước, để lần mở sau dựng đúng bố cục ngay.
            if self.wmi.is_some() && settings::layout_cache() != Some(sup.to_mask()) {
                let _ = settings::set_layout_cache(sup.to_mask());
            }
            self.support = Some(sup);
        }
        let sup = self.support.unwrap_or_default();

        let groups = if check_items {
            Some(build_groups(
                lang,
                self.wmi.as_ref(),
                self.dev.as_ref(),
                self.info.as_ref(),
                &sup,
                dgpu_awake,
                false,
            ))
        } else {
            None
        };

        // Chỉ truy vấn schtasks / dịch vụ nền (Settings) khi mở trang Settings hoặc vừa sửa cài đặt.
        let settings_items = (self.settings_due || (self.page == 2 && items_tick)).then(|| {
            self.settings_due = false;
            let ar = *self
                .autorun
                .get_or_insert_with(|| autorun::state().unwrap_or(AutorunState::Disabled));
            let status = |sw| disabler::status(sw).ok();
            build_settings(
                lang,
                self.wmi.as_ref(),
                &sup,
                ar,
                settings::flag(settings::MINIMIZE_TO_TRAY, true),
                settings::flag(settings::MINIMIZE_ON_CLOSE, true),
                status(Software::Vantage),
                status(Software::LegionZone),
                status(Software::LegionSpace),
                status(Software::FnKeys),
            )
        });
        let device_info = self.device_info_due.then(|| {
            self.device_info_due = false;
            self.build_device_info()
        });
        self.tick = self.tick.wrapping_add(1);

        Snapshot {
            vi: lang == Lang::Vi,
            custom: self.custom_snap(),
            device_info,
            settings_items,
            model: self.info.as_ref().map_or_else(|| "Lenovo".to_string(), |i| i.model.clone()),
            status: problems.join(" · "),
            notice: self.unsupported_message(),
            ai_enabled: ai::is_enabled(),
            sensor_sections,
            groups,
            battery_percent,
            battery_status,
            battery_warning,
            battery_rows,
        }
    }
}

// ---- Chuyển Snapshot sang kiểu Slint (chạy trong luồng UI) ----

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn to_items(v: Vec<ItemSnap>) -> ModelRc<Item> {
    model(
        v.into_iter()
            .map(|i| Item {
                id: i.id.into(),
                icon: i.icon.into(),
                title: i.title.into(),
                message: i.message.into(),
                hint: i.hint.into(),
                kind: i.kind,
                options: model(i.options.into_iter().map(Into::into).collect()),
                value: i.value.into(),
                on: i.on,
            })
            .collect(),
    )
}

fn to_sub_items(v: Vec<(String, String, String)>) -> ModelRc<SubItem> {
    model(
        v.into_iter()
            .map(|(title, right, sub)| SubItem {
                title: title.into(),
                right: right.into(),
                sub: sub.into(),
            })
            .collect(),
    )
}

fn push(ui: &AppWindow, s: Snapshot) {
    ui.set_vi(s.vi);
    ui.set_model_name(s.model.into());
    ui.set_status(s.status.into());
    ui.set_device_notice(s.notice.into());
    ui.set_ai_enabled(s.ai_enabled);
    ui.set_battery_percent(s.battery_percent.into());
    ui.set_battery_status(s.battery_status.into());
    ui.set_battery_warning(s.battery_warning.into());
    if let Some(rows) = s.battery_rows {
        ui.set_battery_rows(model(
            rows.into_iter()
                .map(|(title, message, value)| InfoRow {
                    title: title.into(),
                    message: message.into(),
                    value: value.into(),
                })
                .collect(),
        ));
    }
    ui.set_sensor_sections(model(
        s.sensor_sections
            .into_iter()
            .map(|sec| SectionData {
                title: sec.title.into(),
                subtitle: sec.subtitle.into(),
                rows: model(
                    sec.rows
                        .into_iter()
                        .map(|r| SensorRow {
                            label: r.label.into(),
                            value: r.value.into(),
                            fraction: r.fraction,
                        })
                        .collect(),
                ),
            })
            .collect(),
    ));
    if let Some(c) = s.custom {
        ui.set_custom_rows(model(
            c.rows
                .into_iter()
                .map(|r| TuneRow {
                    id: r.id,
                    title: r.title.into(),
                    description: r.description.into(),
                    warning: r.warning.into(),
                    value_text: r.value_text.into(),
                    can_dec: r.can_dec,
                    can_inc: r.can_inc,
                    changed: r.changed,
                })
                .collect(),
        ));
        ui.set_custom_locked(c.locked);
        ui.set_custom_active(c.active.into());
        ui.set_custom_fan_on(c.fan_on);
        ui.set_custom_note(c.note.into());
    }
    if let Some(d) = s.device_info {
        ui.set_device_info(DeviceInfoData {
            header: d.header.into(),
            os: d.os.into(),
            serial: d.serial.into(),
            serial_masked: d.serial_masked.into(),
            model: d.model.into(),
            mtm: d.mtm.into(),
            bios: d.bios.into(),
            series: d.series.into(),
            generation: d.generation.into(),
            power_modes: d.power_modes.into(),
            capabilities: d.capabilities.into(),
            cpu_name: d.cpu_name.into(),
            cpu_sub: d.cpu_sub.into(),
            gpus: to_sub_items(d.gpus),
            rams: to_sub_items(d.rams),
            mb_name: d.mb_name.into(),
            mb_vendor: d.mb_vendor.into(),
            drives: model(
                d.drives
                    .into_iter()
                    .map(|(title, usage, m, fraction)| DriveItem {
                        title: title.into(),
                        usage: usage.into(),
                        model: m.into(),
                        fraction,
                    })
                    .collect(),
            ),
            displays: to_sub_items(d.displays),
            bat_charge: d.bat_charge.into(),
            bat_health: d.bat_health.into(),
            bat_health_frac: d.bat_health_frac,
            bat_design: d.bat_design.into(),
            bat_full: d.bat_full.into(),
            bat_cycles: d.bat_cycles.into(),
            bat_date: d.bat_date.into(),
            nets: to_sub_items(d.nets),
        });
    }
    if let Some([app, power_settings, smartkeys, software, notify_items]) = s.settings_items {
        ui.set_settings_notify_items(to_items(notify_items));
        ui.set_settings_app_items(to_items(app));
        ui.set_settings_power_items(to_items(power_settings));
        ui.set_settings_smartkey_items(to_items(smartkeys));
        ui.set_settings_software_items(to_items(software));
    }
    if let Some([power, graphics, display, other]) = s.groups {
        ui.set_power_items(to_items(power));
        ui.set_graphics_items(to_items(graphics));
        ui.set_display_items(to_items(display));
        ui.set_other_items(to_items(other));
    }
}

fn hide_window_to_tray(ui: &AppWindow, tx: &mpsc::Sender<Cmd>) {
    let _ = tx.send(Cmd::SetVisible(false));
    let _ = ui.hide();
    tray::trim_working_set();
}

fn show_window_from_tray(ui: &AppWindow, tx: &mpsc::Sender<Cmd>) {
    let _ = ui.show();
    ui.window().set_minimized(false);
    let _ = tx.send(Cmd::SetVisible(true));
}

/// Khung hình đầu tiên không cần chạm phần cứng: nhãn và bố cục hiện ngay, số liệu là "-" rồi được điền dần.
fn skeleton(lang: Lang) -> Snapshot {
    let sensors = |title: &'static str, subtitle: String, labels: Vec<&'static str>| SectionSnap {
        title,
        subtitle,
        rows: labels.into_iter().map(|l| row(l, DASH, 0.0)).collect(),
    };
    let (util, temp_l, fan) = (
        tr(lang, "Utilization", "Sử dụng"),
        tr(lang, "Temperature", "Nhiệt độ"),
        tr(lang, "Fan Speed", "Tốc độ quạt"),
    );
    // Nhãn trang Pin lấy từ chính hàm dựng hàng thật; chỉ thay giá trị bằng "-".
    let blank = BatteryDetail {
        ac_online: false,
        is_charging: false,
        is_low_battery: false,
        percent: 0,
        life_remaining_secs: None,
        rate_mw: Some(0),
        min_rate_mw: Some(0),
        max_rate_mw: Some(0),
        current_capacity_mwh: 0,
        full_charge_capacity_mwh: 0,
        design_capacity_mwh: 0,
        cycle_count: 0,
        temperature_c: Some(0.0),
        manufacture_date: Some(String::new()),
        first_use_date: Some(String::new()),
    };
    Snapshot {
        vi: lang == Lang::Vi,
        model: "Lenovo".into(),
        sensor_sections: vec![
            sensors("CPU", String::new(), vec![util, temp_l, fan]),
            sensors("GPU", String::new(), vec![util, temp_l, fan]),
            sensors(tr(lang, "System", "Hệ thống"), tr(lang, "Memory & Board", "Bộ nhớ & Bo mạch").into(), vec!["RAM", tr(lang, "Battery", "Pin")]),
        ],
        groups: Some(build_groups(lang, None, None, None, &Support::placeholder(), false, true)),
        battery_percent: DASH.into(),
        battery_status: DASH.into(),
        battery_rows: Some(build_battery_rows(&blank, lang).into_iter().map(|(t, m, _)| (t, m, DASH.to_string())).collect()),
        ..Default::default()
    }
}

fn run_worker(rx: mpsc::Receiver<Cmd>, tx: mpsc::Sender<Cmd>, ui: slint::Weak<AppWindow>, start_visible: bool) {
    if start_visible {
        let lang = settings::lang();
        let _ = slint::invoke_from_event_loop({
            let ui = ui.clone();
            move || {
                if let Some(ui) = ui.upgrade() {
                    push(&ui, skeleton(lang));
                }
            }
        });
    }
    // Đọc phần cứng (CPU/GPU/RAM/ổ đĩa...) ở luồng nền ngay từ đầu để hộp "Về thiết bị" mở là có sẵn.
    let (hw_tx, hw_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = hw_tx.send(sysstat::hardware_overview());
    });
    let mut w = Worker::new(start_visible, Some(hw_rx));
    if start_visible {
        // Đẩy khung hình đầu tiên ngay lập tức trước khi chạy các tác vụ kiểm tra dịch vụ nền.
        w.settings_due = false;
        let snap = w.snapshot();
        w.settings_due = true;
        let _ = slint::invoke_from_event_loop({
            let ui = ui.clone();
            let tx = tx.clone();
            move || {
                if let Some(ui) = ui.upgrade() {
                    if ui.window().is_minimized() && settings::flag(settings::MINIMIZE_TO_TRAY, true) {
                        hide_window_to_tray(&ui, &tx);
                    } else {
                        push(&ui, snap);
                    }
                }
            }
        });
    } else {
        tray::trim_working_set();
    }
    // Chỉ chặn phần mềm Lenovo trên máy gaming: máy ITS (ThinkBook, IdeaPad...) chưa được app xử lý phím.
    if w.gaming() {
        w.last_error = Worker::enforce_software_flags();
    }

    loop {
        let timeout = if w.visible { POLL } else { IDLE_POLL };
        match rx.recv_timeout(timeout) {
            Ok(Cmd::Tooltip) => {
                w.show_tooltip();
                continue;
            }
            Ok(cmd) => w.apply(cmd),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if !w.visible {
            w.background_tick();
            continue;
        }
        let snap = w.snapshot();
        let sent = slint::invoke_from_event_loop({
            let ui = ui.clone();
            let tx = tx.clone();
            move || {
                if let Some(ui) = ui.upgrade() {
                    if ui.window().is_minimized() && settings::flag(settings::MINIMIZE_TO_TRAY, true) {
                        hide_window_to_tray(&ui, &tx);
                    } else {
                        push(&ui, snap);
                    }
                }
            }
        });
        if sent.is_err() {
            return;
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let start_minimized = std::env::args().any(|a| a == "--minimized");
    let ui = AppWindow::new()?;
    ui.set_app_title(lenovo_toolkit::app_title().into());
    let (tx, rx) = mpsc::channel::<Cmd>();

    ui.on_page_changed({
        let tx = tx.clone();
        move |p| {
            let _ = tx.send(Cmd::SetPage(p));
        }
    });
    ui.on_combo({
        let tx = tx.clone();
        move |id, value| {
            let _ = tx.send(Cmd::Combo(id.to_string(), value.to_string()));
        }
    });
    ui.on_toggle({
        let tx = tx.clone();
        move |id, on| {
            let _ = tx.send(Cmd::Toggle(id.to_string(), on));
        }
    });
    ui.on_set_ai({
        let tx = tx.clone();
        move |on| {
            let _ = tx.send(Cmd::Ai(on));
        }
    });
    ui.on_custom_select({
        let tx = tx.clone();
        move |label| {
            let _ = tx.send(Cmd::CustomSelect(label.to_string()));
        }
    });
    ui.on_custom_tune({
        let tx = tx.clone();
        move |id, dir| {
            let _ = tx.send(Cmd::CustomTune(id, dir));
        }
    });
    ui.on_custom_fan({
        let tx = tx.clone();
        move |on| {
            let _ = tx.send(Cmd::CustomFan(on));
        }
    });
    ui.on_custom_apply({
        let tx = tx.clone();
        move || {
            let _ = tx.send(Cmd::CustomApply);
        }
    });
    ui.on_custom_reset({
        let tx = tx.clone();
        move || {
            let _ = tx.send(Cmd::CustomReset);
        }
    });
    ui.on_custom_open({
        let tx = tx.clone();
        move || {
            let _ = tx.send(Cmd::CustomReload);
        }
    });
    ui.on_device_info_open({
        let tx = tx.clone();
        move || {
            let _ = tx.send(Cmd::DeviceInfoOpen);
        }
    });

    // Bấm nút X: nếu bật MinimizeOnClose (mặc định bật) thì ẩn xuống System Tray và thu hồi RAM.
    ui.window().on_close_requested({
        let weak = ui.as_weak();
        let tx = tx.clone();
        move || {
            if settings::flag(settings::MINIMIZE_ON_CLOSE, true) {
                if let Some(ui) = weak.upgrade() {
                    hide_window_to_tray(&ui, &tx);
                }
                CloseRequestResponse::KeepWindowShown
            } else {
                let _ = slint::quit_event_loop();
                CloseRequestResponse::HideWindow
            }
        }
    });

    // Khởi tạo biểu tượng System Tray Win32
    tray::spawn_tray({
        let weak = ui.as_weak();
        let tx = tx.clone();
        move |ev| match ev {
            TrayEvent::Open => {
                let weak = weak.clone();
                let tx = tx.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        show_window_from_tray(&ui, &tx);
                    }
                });
            }
            TrayEvent::SetPower(mode) => {
                let _ = tx.send(Cmd::QuickPower(mode));
            }
            TrayEvent::Hover => {
                let _ = tx.send(Cmd::Tooltip);
            }
            TrayEvent::OpenPage(page) => {
                let weak = weak.clone();
                let tx = tx.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        show_window_from_tray(&ui, &tx);
                        ui.set_page(page);
                        let _ = tx.send(Cmd::SetPage(page));
                    }
                });
            }
            TrayEvent::Quit => {
                let _ = slint::quit_event_loop();
            }
        }
    });

    // Fn+Q do firmware xử lý và báo qua WMI: làm mới trạng thái ngay khi có sự kiện.
    let _events = EventListener::start(Source::SmartFanMode, {
        let tx = tx.clone();
        move |_, mode| {
            let _ = tx.send(Cmd::Refresh);
            if let Some(m) = mode {
                notify(lenovo_toolkit::power_toast(m, settings::lang()));
            }
        }
    })
    .ok();

    // Fn+F9 (mở ứng dụng) và Fn+R (đổi tần số quét màn hình) qua WMI LENOVO_UTILITY_EVENT.
    let _utility_events = UtilityEventListener::start({
        let weak = ui.as_weak();
        let tx = tx.clone();
        move |key| match key {
            SpecialKey::FnF9 => {
                let weak = weak.clone();
                let tx = tx.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        show_window_from_tray(&ui, &tx);
                    }
                });
            }
            SpecialKey::FnR => {
                let _ = tx.send(Cmd::CycleRefreshRate);
            }
            other => {
                if let Some(toast) = lenovo_toolkit::key_toast(other, settings::lang()) {
                    notify(toast);
                }
            }
        }
    })
    .ok();

    // Fn+F4 (mic), Fn+F10 (touchpad), Fn+Space (đèn bàn phím) đến từ driver EnergyDrv như bản gốc (DriverKeyListener).
    // Fn+Q đã có sự kiện WMI ở trên nên bỏ qua ở đây.
    let _driver_keys = KeyListener::start(|_, keys| {
        for key in keys {
            match key {
                Key::FnF4 => {
                    // COM cần luồng riêng; không chặn luồng đọc driver.
                    thread::spawn(|| {
                        if let Ok(Some(on)) = audio::toggle_microphone() {
                            notify(lenovo_toolkit::mic_toast(on, settings::lang()));
                        }
                    });
                }
                Key::FnF10 => {
                    thread::spawn(|| {
                        // Trạng thái khóa touchpad lấy từ WMI GameZone (GetTPStatus) như bản gốc, không phải registry.
                        if let Ok(status) = gamezone::get(Feature::TouchpadLock) {
                            notify(lenovo_toolkit::touchpad_toast(lenovo_toolkit::touchpad_on(status), settings::lang()));
                        }
                    });
                }
                Key::FnSpace => {
                    thread::spawn(|| {
                        let lang = settings::lang();
                        if let Ok(level) = battery::open().and_then(|d| energy::kbd_get(&d)) {
                            notify(lenovo_toolkit::backlight_toast(level, kbd_label_tr(level, lang), lang));
                        }
                    });
                }
                Key::FnQ | Key::FnF8 => {}
            }
        }
    })
    .ok();

    let weak = ui.as_weak();
    thread::spawn(move || run_worker(rx, tx, weak, !start_minimized));

    // Nếu khởi động thường thì mở cửa sổ; nếu `--minimized` (Autorun) thì ẩn hoàn toàn dưới System Tray.
    if !start_minimized {
        ui.show()?;
    } else {
        tray::trim_working_set();
    }

    slint::run_event_loop_until_quit()
}
