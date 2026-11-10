//! Tự chạy cùng Windows bằng Scheduled Task đăng nhập với quyền cao nhất (port từ System/Autorun.cs).
//!
//! Task chạy thẳng `<exe> --minimized` (không qua rundll32/ShellExecute: đường đó làm UAC hiện một mục
//! tạm chớp trên taskbar mỗi lần đăng nhập). App ở chế độ `--minimized` không tạo cửa sổ nào hiện ra.
//! Dùng tên task riêng (không trùng bản C#) để không ghi đè autorun của bản C# đang cài.
//! Khác bản C#: điều khiển bằng PowerShell `ScheduledTasks` thay cho TaskScheduler API.

use std::io;
use std::path::Path;

use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

use crate::disabler::powershell;

const TASK_NAME: &str = "LenovoToolkit_Autorun";
/// Tên task trước khi đổi tên ứng dụng: vẫn được nhận ra là đang bật, và bị xóa khi bật/tắt lại.
const LEGACY_TASK_NAME: &str = "LegionToolkitRust_Autorun";
const DELAY: &str = "PT30S";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutorunState {
    Disabled,
    Enabled,
    EnabledDelayed,
}

impl AutorunState {
    pub const LABELS: [(Self, &'static str); 3] = [
        (Self::Disabled, "Disabled"),
        (Self::Enabled, "Enabled"),
        (Self::EnabledDelayed, "Enabled (delayed)"),
    ];

    pub fn label(self) -> &'static str {
        Self::LABELS.iter().find(|(s, _)| *s == self).map_or("", |(_, l)| l)
    }

    pub fn parse_label(s: &str) -> Option<Self> {
        Self::LABELS.iter().find(|(_, l)| *l == s).map(|(st, _)| *st)
    }
}

/// UAC tắt hẳn (EnableLUA = 0): hạ RunLevel xuống thường như bản C#.
fn uac_disabled() -> bool {
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System", KEY_READ)
        .and_then(|k| k.get_value::<u32, _>("EnableLUA"))
        .is_ok_and(|v| v == 0)
}

/// Nháy đơn trong chuỗi PowerShell được nhân đôi.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn parse_state(stdout: &str) -> AutorunState {
    match stdout.trim() {
        "enabled" => AutorunState::Enabled,
        "delayed" => AutorunState::EnabledDelayed,
        _ => AutorunState::Disabled,
    }
}

fn register_script(exe: &str, delayed: bool, run_level: &str) -> String {
    let dir = Path::new(exe).parent().and_then(Path::to_str).unwrap_or_default();
    let delay = if delayed { format!("$t.Delay = '{DELAY}'; ") } else { String::new() };
    format!(
        "$ErrorActionPreference='Stop'; \
         $user=[Security.Principal.WindowsIdentity]::GetCurrent().Name; \
         $exe={exe}; \
         $a=New-ScheduledTaskAction -Execute $exe -Argument '--minimized' -WorkingDirectory {dir}; \
         $t=New-ScheduledTaskTrigger -AtLogOn -User $user; {delay}\
         $p=New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel {run_level}; \
         $s=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero); \
         Register-ScheduledTask -TaskName '{TASK_NAME}' -Action $a -Trigger $t -Principal $p -Settings $s -Force | Out-Null; \
         Unregister-ScheduledTask -TaskName '{LEGACY_TASK_NAME}' -Confirm:$false -ErrorAction SilentlyContinue",
        exe = ps_quote(exe),
        dir = ps_quote(dir),
    )
}

pub fn state() -> io::Result<AutorunState> {
    let out = powershell(&format!(
        "$t=Get-ScheduledTask -TaskName '{TASK_NAME}' -ErrorAction SilentlyContinue; \
         if(-not $t){{$t=Get-ScheduledTask -TaskName '{LEGACY_TASK_NAME}' -ErrorAction SilentlyContinue}}; \
         if(-not $t){{'disabled'}} elseif($t.Triggers[0].Delay){{'delayed'}} else {{'enabled'}}"
    ))?;
    Ok(parse_state(&out))
}

/// Cần quyền admin để đăng ký task chạy với quyền cao nhất.
pub fn set(state: AutorunState) -> io::Result<()> {
    if state == AutorunState::Disabled {
        return powershell(&format!(
            "Unregister-ScheduledTask -TaskName '{TASK_NAME}' -Confirm:$false -ErrorAction SilentlyContinue; \
             Unregister-ScheduledTask -TaskName '{LEGACY_TASK_NAME}' -Confirm:$false -ErrorAction SilentlyContinue"
        ))
        .map(drop);
    }
    let exe = std::env::current_exe()?;
    let run_level = if uac_disabled() { "Limited" } else { "Highest" };
    powershell(&register_script(&exe.to_string_lossy(), state == AutorunState::EnabledDelayed, run_level)).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_round_trip() {
        for (s, l) in AutorunState::LABELS {
            assert_eq!(AutorunState::parse_label(l), Some(s));
            assert_eq!(s.label(), l);
        }
        assert_eq!(AutorunState::parse_label("x"), None);
    }

    #[test]
    fn state_parsing() {
        assert_eq!(parse_state("enabled\r\n"), AutorunState::Enabled);
        assert_eq!(parse_state("delayed"), AutorunState::EnabledDelayed);
        assert_eq!(parse_state(""), AutorunState::Disabled);
        assert_eq!(parse_state("garbage"), AutorunState::Disabled);
    }

    #[test]
    fn quoting_doubles_single_quotes() {
        assert_eq!(ps_quote(r"C:\O'Neil\app.exe"), r"'C:\O''Neil\app.exe'");
    }

    #[test]
    fn script_contains_required_pieces() {
        let s = register_script(r"C:\Tools\lenovo-toolkit.exe", true, "Highest");
        // Chạy thẳng exe: đi qua rundll32/ShellExecute sẽ kéo theo cửa sổ UAC tạm hiện chớp trên taskbar.
        assert!(!s.contains("rundll32") && !s.contains("ShellExec_RunDLL"));
        assert!(s.contains("-Execute $exe -Argument '--minimized'"));
        assert!(s.contains("-RunLevel Highest"));
        assert!(s.contains("$t.Delay = 'PT30S'"));
        assert!(s.contains(r"'C:\Tools'"));
        assert!(!register_script(r"C:\a.exe", false, "Limited").contains("Delay"));
        // Đăng ký task mới thì xóa task cũ để không chạy hai lần.
        assert!(s.contains("Register-ScheduledTask -TaskName 'LenovoToolkit_Autorun'"));
        assert!(s.contains("Unregister-ScheduledTask -TaskName 'LegionToolkitRust_Autorun'"));
    }

    #[test]
    fn reading_state_does_not_require_admin() {
        // Không tạo task trong test; chỉ kiểm tra việc truy vấn chạy được.
        assert!(state().is_ok());
    }
}
