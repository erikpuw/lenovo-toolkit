//! Tự chạy cùng Windows bằng Scheduled Task đăng nhập với quyền cao nhất (port từ System/Autorun.cs).
//!
//! Task chạy thẳng `<exe> --minimized` (không qua rundll32/ShellExecute: đường đó làm UAC hiện một mục
//! tạm chớp trên taskbar mỗi lần đăng nhập). App ở chế độ `--minimized` không tạo cửa sổ nào hiện ra.
//! Dùng tên task riêng (không trùng bản C#) để không ghi đè autorun của bản C# đang cài.
//! Điều khiển bằng `schtasks.exe` với định nghĩa task dạng XML: mỗi lệnh khoảng 30 ms, trong khi PowerShell
//! `ScheduledTasks` mất khoảng 4 giây mỗi lần (làm công tắc Autorun phải đợi rất lâu).

use std::io;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Output};

use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

const TASK_NAME: &str = "LenovoToolkit_Autorun";
/// Tên task trước khi đổi tên ứng dụng: vẫn được nhận ra là đang bật, và bị xóa khi bật/tắt lại.
const LEGACY_TASK_NAME: &str = "LegionToolkitRust_Autorun";
const DELAY: &str = "PT30S";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

fn schtasks(args: &[&str]) -> io::Result<Output> {
    Command::new("schtasks").args(args).creation_flags(CREATE_NO_WINDOW).output()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// Tài khoản Windows đang đăng nhập, dạng `MÁY\tên` như PowerShell `WindowsIdentity.GetCurrent().Name`.
fn current_user() -> String {
    let name = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{name}"),
        _ => name,
    }
}

/// Định nghĩa task: chạy khi `user` đăng nhập, không dừng khi chạy pin, không giới hạn thời gian.
/// `highest` là quyền cao nhất (cần cho app đòi quyền quản trị); tắt khi UAC bị tắt hẳn.
fn task_xml(exe: &str, user: &str, delayed: bool, highest: bool) -> String {
    let dir = Path::new(exe).parent().and_then(Path::to_str).unwrap_or_default();
    let delay = if delayed { format!("\n      <Delay>{DELAY}</Delay>") } else { String::new() };
    let run_level = if highest { "HighestAvailable" } else { "LeastPrivilege" };
    let (exe, dir, user) = (xml_escape(exe), xml_escape(dir), xml_escape(user));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>{delay}
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>{run_level}</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>--minimized</Arguments>
      <WorkingDirectory>{dir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// Trạng thái từ định nghĩa task mà `schtasks /Query /XML` in ra (có `<Delay>` là bật trễ).
fn state_from_xml(xml: &str) -> AutorunState {
    if xml.replace('\0', "").contains("<Delay>") { AutorunState::EnabledDelayed } else { AutorunState::Enabled }
}

/// Đăng ký (ghi đè) task `name` từ XML. schtasks đọc file XML dạng UTF-16 có BOM.
fn create_task(name: &str, xml: &str) -> io::Result<()> {
    let path = std::env::temp_dir().join(format!("lenovo-toolkit-autorun-{}.xml", std::process::id()));
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(&path, bytes)?;
    let out = schtasks(&["/Create", "/TN", name, "/XML", &path.to_string_lossy(), "/F"]);
    let _ = std::fs::remove_file(&path);
    let out = out?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr);
        Err(io::Error::other(format!("schtasks lỗi: {}", msg.trim())))
    }
}

/// Xóa task nếu có; task không tồn tại thì bỏ qua.
fn delete_task(name: &str) {
    let _ = schtasks(&["/Delete", "/TN", name, "/F"]);
}

pub fn state() -> io::Result<AutorunState> {
    for name in [TASK_NAME, LEGACY_TASK_NAME] {
        let out = schtasks(&["/Query", "/TN", name, "/XML"])?;
        if out.status.success() {
            return Ok(state_from_xml(&String::from_utf8_lossy(&out.stdout)));
        }
    }
    Ok(AutorunState::Disabled)
}

/// Cần quyền admin để đăng ký task chạy với quyền cao nhất.
pub fn set(state: AutorunState) -> io::Result<()> {
    if state != AutorunState::Disabled {
        let exe = std::env::current_exe()?;
        let xml = task_xml(&exe.to_string_lossy(), &current_user(), state == AutorunState::EnabledDelayed, !uac_disabled());
        create_task(TASK_NAME, &xml)?;
    } else {
        delete_task(TASK_NAME);
    }
    // Task cũ (trước khi đổi tên) luôn được xóa để app không chạy hai lần.
    delete_task(LEGACY_TASK_NAME);
    Ok(())
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
    fn state_from_task_xml() {
        assert_eq!(state_from_xml("<LogonTrigger><Delay>PT30S</Delay></LogonTrigger>"), AutorunState::EnabledDelayed);
        assert_eq!(state_from_xml("<LogonTrigger><Enabled>true</Enabled></LogonTrigger>"), AutorunState::Enabled);
        // schtasks có thể in UTF-16: các ký tự NUL xen giữa không làm hỏng việc nhận ra <Delay>.
        let utf16ish: String = "<Delay>".chars().flat_map(|c| [c, '\0']).collect();
        assert_eq!(state_from_xml(&utf16ish), AutorunState::EnabledDelayed);
    }

    #[test]
    fn task_xml_has_required_pieces() {
        let x = task_xml(r"C:\Tools\lenovo-toolkit.exe", r"PC\me", true, true);
        // Chạy thẳng exe: đi qua rundll32/ShellExecute sẽ kéo theo cửa sổ UAC tạm hiện chớp trên taskbar.
        assert!(!x.contains("rundll32"));
        assert!(x.contains(r"<Command>C:\Tools\lenovo-toolkit.exe</Command>"));
        assert!(x.contains("<Arguments>--minimized</Arguments>"));
        assert!(x.contains(r"<WorkingDirectory>C:\Tools</WorkingDirectory>"));
        assert!(x.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(x.contains("<Delay>PT30S</Delay>"));
        assert!(x.contains(r"<UserId>PC\me</UserId>"));
        assert!(x.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        let plain = task_xml(r"C:\a.exe", "me", false, false);
        assert!(!plain.contains("<Delay>") && plain.contains("<RunLevel>LeastPrivilege</RunLevel>"));
    }

    #[test]
    fn paths_are_xml_escaped() {
        let x = task_xml(r"C:\R&D <x>\app.exe", "me", false, true);
        assert!(x.contains(r"<Command>C:\R&amp;D &lt;x&gt;\app.exe</Command>"));
    }

    #[test]
    fn reading_state_does_not_require_admin() {
        // Không tạo task trong test; chỉ kiểm tra việc truy vấn chạy được.
        assert!(state().is_ok());
    }

    /// Tạo thật một task thử (quyền thường, không cần admin) rồi xóa ngay: kiểm tra schtasks nhận XML này.
    /// Chạy tay: `cargo test -p lenovo-toolkit-core schtasks_accepts_generated_xml -- --ignored`.
    #[test]
    #[ignore]
    fn schtasks_accepts_generated_xml() {
        let name = "LenovoToolkit_AutorunSelfTest";
        let xml = task_xml(r"C:\Windows\System32\notepad.exe", &current_user(), true, false);
        create_task(name, &xml).expect("schtasks phải nhận XML");
        let out = schtasks(&["/Query", "/TN", name, "/XML"]).unwrap();
        delete_task(name);
        assert!(out.status.success());
        assert_eq!(state_from_xml(&String::from_utf8_lossy(&out.stdout)), AutorunState::EnabledDelayed);
        assert!(!schtasks(&["/Query", "/TN", name]).unwrap().status.success(), "task thử đã được xóa");
    }
}
