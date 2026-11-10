//! Chặn phần mềm quản lý phím/nguồn gốc của Lenovo để không tranh quyền với app
//! (port từ SoftwareDisabler/*.cs: FnKeys, Vantage, Legion Zone, Legion Space).
//!
//! Tắt: vô hiệu hóa scheduled task, dừng + vô hiệu hóa service, kết thúc tiến trình, vô hiệu hóa
//! driver đi kèm, tắt mục khởi động (Run + StartupApproved). Bật: làm ngược lại.
//!
//! Khác bản C#: scheduled task điều khiển bằng PowerShell (`Get-ScheduledTask`) thay cho TaskScheduler API;
//! không có sự kiện `OnRefreshed`.

use std::borrow::Cow;
use std::ffi::c_void;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::core::PCWSTR;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_BINARY};
use winreg::{RegKey, RegValue};

use crate::games::expand_env;
use crate::process::{self, Proc};
use crate::services::{Scm, ServiceInfo, StopResult};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_APPROVED_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const UWP_APPDATA_KEY: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Software {
    FnKeys,
    Vantage,
    LegionZone,
    LegionSpace,
}

impl Software {
    pub const ALL: [Self; 4] = [Self::FnKeys, Self::Vantage, Self::LegionZone, Self::LegionSpace];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "fnkeys" => Self::FnKeys,
            "vantage" => Self::Vantage,
            "legionzone" => Self::LegionZone,
            "legionspace" => Self::LegionSpace,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::FnKeys => "fnkeys",
            Self::Vantage => "vantage",
            Self::LegionZone => "legionzone",
            Self::LegionSpace => "legionspace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Enabled,
    Disabled,
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub status: Status,
    /// Service không dừng được: cần khởi động lại máy.
    pub not_stopped: Vec<String>,
    pub running_services: Vec<String>,
    pub running_processes: Vec<String>,
}

struct Spec {
    tasks: &'static [&'static str],
    services: &'static [&'static str],
    processes: &'static [&'static str],
    driver_prefixes: &'static [&'static str],
    driver_roots: Vec<String>,
    startup_names: &'static [&'static str],
    startup_roots: Vec<String>,
}

fn env_dir(var: &str) -> String {
    std::env::var(var).unwrap_or_default()
}

fn spec(sw: Software) -> Spec {
    let base = Spec {
        tasks: &[],
        services: &[],
        processes: &[],
        driver_prefixes: &[],
        driver_roots: Vec::new(),
        startup_names: &[],
        startup_roots: Vec::new(),
    };
    match sw {
        Software::FnKeys => Spec {
            services: &["LenovoFnAndFunctionKeys"],
            processes: &["LenovoUtilityUI", "LenovoUtilityService", "LenovoSmartKey"],
            ..base
        },
        Software::Vantage => Spec {
            tasks: &[
                "Lenovo\\BatteryGauge",
                "Lenovo\\ImController",
                "Lenovo\\ImController\\Plugins",
                "Lenovo\\ImController\\TimeBasedEvents",
                "Lenovo\\UDC",
                "Lenovo\\Vantage",
                "Lenovo\\Vantage\\Schedule",
            ],
            services: &["ImControllerService", "LenovoVantageService"],
            processes: &[
                "BGHelper",
                "Lenovo.Modern.ImController",
                "Lenovo.Vantage",
                "LenovoVantage",
                "QSHelper",
                "ScheduleEventAction",
            ],
            startup_names: &["LenovoVantageToolbar", "LenovoVantage"],
            startup_roots: vec![format!(r"{}\Lenovo\Vantage", env_dir("ProgramData"))],
            ..base
        },
        Software::LegionZone => Spec {
            services: &["LZService", "StreamingService"],
            processes: &[
                "BorderlessSpace",
                "ContextMenuInstaller",
                "DoudouAI",
                "LegionZone",
                "LZAgent",
                "LZInstall",
                "LZMain",
                "lzolhelp64",
                "LZService",
                "LZStrategy",
                "LZTray",
                "LZUpdate",
                "NvOcScanner",
                "StreamingDiagnosis",
                "StreamingHost",
                "StreamingService",
            ],
            driver_prefixes: &["AMDRyzenMasterDriver"],
            driver_roots: vec![format!(r"{}\Lenovo\LegionZone", env_dir("ProgramFiles(x86)"))],
            startup_roots: vec![
                format!(r"{}\Lenovo\LegionZone", env_dir("ProgramFiles(x86)")),
                format!(r"{}\Lenovo\LegionZone", env_dir("ProgramFiles")),
            ],
            ..base
        },
        Software::LegionSpace => Spec {
            services: &["DAService"],
            processes: &["Bino3D", "LegionGameWidget", "LegionSpace", "LegionSpaceComponent", "LegionSpaceToast", "LSDaemon"],
            ..base
        },
    }
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.is_char_boundary(prefix.len()) && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// Driver đi kèm chỉ tính khi thư mục cài đặt tồn tại (hoặc không khai báo thư mục).
fn active_driver_prefixes(spec: &Spec) -> &'static [&'static str] {
    if spec.driver_roots.is_empty() || spec.driver_roots.iter().any(|r| Path::new(r).is_dir()) {
        spec.driver_prefixes
    } else {
        &[]
    }
}

fn driver_names(spec: &Spec, all: &[ServiceInfo]) -> Vec<String> {
    let prefixes = active_driver_prefixes(spec);
    all.iter()
        .filter(|s| prefixes.iter().any(|p| starts_with_ci(&s.name, p)))
        .map(|s| s.name.clone())
        .collect()
}

fn all_service_names(spec: &Spec, all: &[ServiceInfo]) -> Vec<String> {
    spec.services.iter().map(|s| (*s).to_string()).chain(driver_names(spec, all)).collect()
}

fn exists(name: &str, all: &[ServiceInfo]) -> bool {
    all.iter().any(|s| s.name == name)
}

fn matches_process(spec: &Spec, p: &Proc) -> bool {
    spec.processes.iter().any(|n| starts_with_ci(&p.name, n))
}

/// Mô tả file thực thi ("FileDescription") từ tài nguyên phiên bản; `None` nếu không đọc được.
fn file_description(path: &str) -> Option<String> {
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let file = PCWSTR(wide.as_ptr());
    unsafe {
        let size = GetFileVersionInfoSizeW(file, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(file, None, size, data.as_mut_ptr().cast()).ok()?;

        let query = |sub: &str| -> Option<(*mut c_void, u32)> {
            let s: Vec<u16> = sub.encode_utf16().chain(Some(0)).collect();
            let (mut ptr, mut len) = (std::ptr::null_mut::<c_void>(), 0u32);
            VerQueryValueW(data.as_ptr().cast(), PCWSTR(s.as_ptr()), &mut ptr, &mut len)
                .as_bool()
                .then_some((ptr, len))
        };

        let (tp, tlen) = query("\\VarFileInfo\\Translation")?;
        if tlen < 4 {
            return None;
        }
        let t = std::slice::from_raw_parts(tp.cast::<u16>(), 2);
        let (vp, vlen) = query(&format!("\\StringFileInfo\\{:04x}{:04x}\\FileDescription", t[0], t[1]))?;
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(vp.cast::<u16>(), vlen as usize));
        Some(text.trim_end_matches('\0').to_string())
    }
}

/// `utility.exe` có mô tả "Lenovo Hotkeys" cũng là phần mềm phím của Lenovo (chỉ với FnKeys).
fn is_lenovo_hotkeys(p: &Proc) -> bool {
    p.name.eq_ignore_ascii_case("utility")
        && process::image_path(p.pid)
            .and_then(|path| file_description(&path))
            .is_some_and(|d| d.eq_ignore_ascii_case("Lenovo Hotkeys"))
}

fn running_processes(sw: Software, spec: &Spec, procs: &[Proc]) -> Vec<String> {
    let mut result: Vec<String> = procs.iter().filter(|p| matches_process(spec, p)).map(|p| p.name.clone()).collect();
    if sw == Software::FnKeys {
        result.extend(procs.iter().filter(|p| is_lenovo_hotkeys(p)).map(|p| p.name.clone()));
    }
    result
}

fn evaluate(sw: Software, spec: &Spec, scm: &Scm) -> io::Result<(Status, Vec<String>, Vec<String>)> {
    let all = scm.list()?;
    let names = all_service_names(spec, &all);
    let running_services: Vec<String> = names
        .iter()
        .filter(|n| all.iter().any(|s| &s.name == *n && s.active))
        .cloned()
        .collect();
    let running_procs = running_processes(sw, spec, &process::list()?);
    let status = if !running_services.is_empty() || !running_procs.is_empty() {
        Status::Enabled
    } else if names.iter().any(|n| exists(n, &all)) {
        Status::Disabled
    } else {
        Status::NotFound
    };
    Ok((status, running_services, running_procs))
}

pub fn status(sw: Software) -> io::Result<Status> {
    let scm = Scm::open_read()?;
    Ok(evaluate(sw, &spec(sw), &scm)?.0)
}

// ---- Scheduled task ----

/// Chạy một đoạn PowerShell ẩn cửa sổ, trả về stdout; lỗi nếu mã thoát khác 0.
pub(crate) fn powershell(script: &str) -> io::Result<String> {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(io::Error::other(format!("PowerShell lỗi: {}", String::from_utf8_lossy(&out.stderr).trim())))
    }
}

fn run_powershell(script: &str) -> io::Result<()> {
    powershell(script).map(drop)
}

fn set_tasks_enabled(spec: &Spec, enabled: bool) -> io::Result<()> {
    let verb = if enabled { "Enable-ScheduledTask" } else { "Disable-ScheduledTask" };
    for path in spec.tasks {
        let task_path = format!("\\{}\\", path.trim_matches('\\'));
        run_powershell(&format!(
            "$ErrorActionPreference='Stop'; Get-ScheduledTask -TaskPath '{task_path}' -ErrorAction SilentlyContinue | {verb} | Out-Null"
        ))?;
    }
    Ok(())
}

// ---- Service / driver ----

fn set_service(scm: &Scm, name: &str, enabled: bool, all: &[ServiceInfo], not_stopped: &mut Vec<String>) -> io::Result<()> {
    if !exists(name, all) {
        return Ok(());
    }
    scm.set_start_enabled(name, enabled)?;
    if enabled {
        let _ = scm.start(name); // có thể thiếu file thực thi: bỏ qua như bản C#
    } else if scm.stop(name)? == StopResult::NotStopped {
        not_stopped.push(name.to_string());
    }
    Ok(())
}

fn set_services(scm: &Scm, spec: &Spec, enabled: bool, not_stopped: &mut Vec<String>) -> io::Result<()> {
    let all = scm.list()?;
    for name in spec.services {
        set_service(scm, name, enabled, &all, not_stopped)?;
    }
    Ok(())
}

fn set_drivers(scm: &Scm, spec: &Spec, enabled: bool, not_stopped: &mut Vec<String>) -> io::Result<()> {
    let all = scm.list()?;
    for name in driver_names(spec, &all) {
        set_service(scm, &name, enabled, &all, not_stopped)?;
    }
    Ok(())
}

// ---- Tiến trình ----

fn wait_exit(pid: u32) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if process::list().is_ok_and(|l| !l.iter().any(|p| p.pid == pid)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn kill_processes(sw: Software, spec: &Spec) {
    let Ok(procs) = process::list() else { return };
    for p in &procs {
        if matches_process(spec, p) || (sw == Software::FnKeys && is_lenovo_hotkeys(p)) {
            if process::kill_tree(p.pid).is_ok() {
                wait_exit(p.pid);
            }
        }
    }
}

// ---- Mục khởi động (Run + StartupApproved) ----

fn normalize_path(path: &str) -> String {
    let mut r = expand_env(path);
    if let Some(rest) = r.strip_prefix(r"\??\") {
        r = rest.to_string();
    }
    if r.len() >= 12 && r[..12].eq_ignore_ascii_case(r"\SystemRoot\") {
        r = format!(r"{}\{}", env_dir("SystemRoot"), &r[12..]);
    }
    if r.starts_with(r"\\") {
        return r; // UNC: giữ nguyên
    }
    while r.contains(r"\\") {
        r = r.replace(r"\\", r"\");
    }
    r
}

fn extract_exe_path(command: &str) -> String {
    let value = command.trim();
    let path = if let Some(rest) = value.strip_prefix('"') {
        rest.find('"').filter(|&e| e > 0).map_or(value, |e| &rest[..e])
    } else {
        value.to_lowercase().find(".exe").map_or(value, |e| &value[..e + 4])
    };
    normalize_path(path)
}

fn roots(list: &[String]) -> Vec<String> {
    list.iter()
        .filter(|r| !r.trim().is_empty())
        .map(|r| format!("{}\\", normalize_path(r.trim()).trim_end_matches('\\')))
        .collect()
}

fn enabled_entry() -> Vec<u8> {
    vec![0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
}

fn disabled_entry(filetime: u64) -> Vec<u8> {
    let mut v = vec![0x03, 0, 0, 0];
    v.extend_from_slice(&filetime.to_le_bytes());
    v
}

fn now_filetime() -> u64 {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    (d.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(d.subsec_nanos() / 100)
}

fn startup_entry_names(hive: &RegKey, spec: &Spec) -> Vec<String> {
    let Ok(run) = hive.open_subkey_with_flags(RUN_KEY, KEY_READ) else { return Vec::new() };
    let value_names: Vec<String> = run.enum_values().filter_map(Result::ok).map(|(n, _)| n).collect();
    let roots = roots(&spec.startup_roots);
    let mut result: Vec<String> = Vec::new();
    let mut add = |n: &str| {
        if !result.iter().any(|r| r.eq_ignore_ascii_case(n)) {
            result.push(n.to_string());
        }
    };
    for name in spec.startup_names {
        if value_names.iter().any(|v| v.eq_ignore_ascii_case(name)) {
            add(name);
        }
    }
    for value_name in &value_names {
        let command: String = run.get_value(value_name).unwrap_or_default();
        if command.trim().is_empty() {
            continue;
        }
        let path = extract_exe_path(&command).to_lowercase();
        if roots.iter().any(|r| path.starts_with(&r.to_lowercase())) {
            add(value_name);
        }
    }
    result
}

fn set_startup_entries(spec: &Spec, enabled: bool) -> io::Result<()> {
    if spec.startup_names.is_empty() && spec.startup_roots.is_empty() {
        return Ok(());
    }
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let hive = RegKey::predef(hive);
        for name in startup_entry_names(&hive, spec) {
            let bytes = if enabled { enabled_entry() } else { disabled_entry(now_filetime()) };
            let (key, _) = hive
                .create_subkey_with_flags(STARTUP_APPROVED_RUN_KEY, KEY_WRITE)
                .map_err(|e| io::Error::other(format!("không mở được StartupApproved: {e}")))?;
            key.set_raw_value(&name, &RegValue { bytes: Cow::Owned(bytes), vtype: REG_BINARY })
                .map_err(|e| io::Error::other(format!("không đặt được mục khởi động {name}: {e}")))?;
        }
    }
    Ok(())
}

/// Ứng dụng UWP "LenovoUtility" tự khởi động: State = 2 (bật) / 1 (tắt).
fn set_uwp_startup(enabled: bool) -> io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(parent) = hkcu.open_subkey_with_flags(UWP_APPDATA_KEY, KEY_READ) else { return Ok(()) };
    let Some(sub) = parent.enum_keys().filter_map(Result::ok).find(|k| k.to_lowercase().contains("lenovoutility")) else {
        return Ok(());
    };
    let path = format!(r"{UWP_APPDATA_KEY}\{sub}\LenovoUtilityID");
    let Ok(key) = hkcu.open_subkey_with_flags(path, KEY_WRITE) else { return Ok(()) };
    key.set_value("State", &(if enabled { 2u32 } else { 1u32 }))
        .map_err(|e| io::Error::other(format!("không đặt được trạng thái LenovoUtility: {e}")))
}

// ---- Bật / tắt ----

pub fn disable(sw: Software) -> io::Result<Report> {
    let spec = spec(sw);
    let scm = Scm::open()?;
    let mut not_stopped = Vec::new();

    set_tasks_enabled(&spec, false)?;
    set_services(&scm, &spec, false, &mut not_stopped)?;
    kill_processes(sw, &spec);
    set_drivers(&scm, &spec, false, &mut not_stopped)?;
    set_startup_entries(&spec, false)?;
    if sw == Software::FnKeys {
        set_uwp_startup(false)?;
    }

    let (status, running_services, running_processes) = evaluate(sw, &spec, &scm)?;
    Ok(Report { status, not_stopped, running_services, running_processes })
}

pub fn enable(sw: Software) -> io::Result<()> {
    let spec = spec(sw);
    let scm = Scm::open()?;
    let mut ignored = Vec::new();

    set_tasks_enabled(&spec, true)?;
    set_services(&scm, &spec, true, &mut ignored)?;
    set_drivers(&scm, &spec, true, &mut ignored)?;
    set_startup_entries(&spec, true)?;
    if sw == Software::FnKeys {
        set_uwp_startup(true)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(name: &str, active: bool) -> ServiceInfo {
        ServiceInfo { name: name.into(), active }
    }

    #[test]
    fn startup_entry_bytes_match_windows_format() {
        assert_eq!(enabled_entry(), vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let d = disabled_entry(0x0102_0304_0506_0708);
        assert_eq!(d.len(), 12);
        assert_eq!(&d[..4], &[3, 0, 0, 0]);
        assert_eq!(&d[4..], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert!(now_filetime() > 130_000_000_000_000_000); // sau năm 2012
    }

    #[test]
    fn extracts_executable_from_run_commands() {
        assert_eq!(extract_exe_path(r#""C:\Program Files\Lenovo\app.exe" --silent"#), r"C:\Program Files\Lenovo\app.exe");
        assert_eq!(extract_exe_path(r"C:\Lenovo\app.EXE /x"), r"C:\Lenovo\app.EXE");
        assert_eq!(extract_exe_path(r"C:\\Lenovo\\a.exe"), r"C:\Lenovo\a.exe");
        assert_eq!(extract_exe_path(r"\\server\share\a.exe"), r"\\server\share\a.exe");
    }

    #[test]
    fn roots_are_normalized_with_trailing_backslash() {
        assert_eq!(roots(&[r"C:\A\B\".into(), "  ".into()]), vec![r"C:\A\B\".to_string()]);
    }

    #[test]
    fn driver_names_need_existing_package_root() {
        let mut s = spec(Software::LegionZone);
        let all = [svc("AMDRyzenMasterDriverV20", false), svc("other", true)];
        s.driver_roots = vec![r"Z:\không\tồn\tại".into()];
        assert!(driver_names(&s, &all).is_empty());
        s.driver_roots = vec![std::env::temp_dir().to_string_lossy().into_owned()];
        assert_eq!(driver_names(&s, &all), vec!["AMDRyzenMasterDriverV20".to_string()]);
        assert!(all_service_names(&s, &all).contains(&"LZService".to_string()));
    }

    #[test]
    fn process_matching_is_prefix_and_case_insensitive() {
        let s = spec(Software::FnKeys);
        let p = |n: &str| Proc { pid: 1, parent: 0, name: n.into() };
        assert!(matches_process(&s, &p("lenovoutilityui")));
        assert!(matches_process(&s, &p("LenovoUtilityService.Host")));
        assert!(!matches_process(&s, &p("explorer")));
        assert!(!starts_with_ci("ab", "abc"));
    }

    #[test]
    fn parses_software_names() {
        for sw in Software::ALL {
            assert_eq!(Software::parse(sw.name()), Some(sw));
        }
        assert_eq!(Software::parse("x"), None);
    }

    #[test]
    fn reads_file_description_of_system_binary() {
        let d = file_description(r"C:\Windows\System32\notepad.exe");
        assert!(d.is_some_and(|s| !s.is_empty()));
    }
}
