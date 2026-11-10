//! Liệt kê, truy vấn đường dẫn và kết thúc tiến trình (ToolHelp32 + OpenProcess).
//! Dùng chung cho phát hiện game và SoftwareDisabler.

use std::collections::{HashMap, HashSet};
use std::io;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, TerminateProcess,
};
use windows::core::PWSTR;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    /// Tên file thực thi bỏ đuôi `.exe` (giống `Process.ProcessName` của .NET).
    pub name: String,
}

fn strip_exe(file: &str) -> String {
    match file.len().checked_sub(4) {
        Some(i) if file.is_char_boundary(i) && file[i..].eq_ignore_ascii_case(".exe") => file[..i].to_string(),
        _ => file.to_string(),
    }
}

pub fn list() -> io::Result<Vec<Proc>> {
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|e| io::Error::other(format!("CreateToolhelp32Snapshot lỗi: {e}")))?;
    let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut result = Vec::new();
    unsafe {
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                result.push(Proc {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    name: strip_exe(&String::from_utf16_lossy(&entry.szExeFile[..len])),
                });
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    Ok(result)
}

/// Đường dẫn đầy đủ của file thực thi; `None` nếu không đủ quyền hoặc tiến trình đã thoát.
pub fn image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Các pid con cháu của `root`, con cháu sâu nhất đứng trước (để kết thúc từ lá lên gốc).
pub fn descendants(procs: &[Proc], root: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs {
        if p.pid != p.parent {
            children.entry(p.parent).or_default().push(p.pid);
        }
    }
    let mut order = Vec::new();
    let mut seen: HashSet<u32> = HashSet::from([root]);
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        for &c in children.get(&pid).into_iter().flatten() {
            if seen.insert(c) {
                order.push(c);
                queue.push(c);
            }
        }
    }
    order.reverse();
    order
}

fn terminate(pid: u32) -> io::Result<()> {
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, false, pid)
            .map_err(|e| io::Error::other(format!("không mở được tiến trình {pid}: {e}")))?;
        let r = TerminateProcess(h, 1);
        let _ = CloseHandle(h);
        r.map_err(|e| io::Error::other(format!("không kết thúc được tiến trình {pid}: {e}")))
    }
}

/// Kết thúc tiến trình và toàn bộ con cháu (giống `Process.Kill(true)`).
pub fn kill_tree(pid: u32) -> io::Result<()> {
    let procs = list()?;
    for child in descendants(&procs, pid) {
        let _ = terminate(child); // con có thể đã thoát
    }
    terminate(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_exe_suffix_only() {
        assert_eq!(strip_exe("chrome.exe"), "chrome");
        assert_eq!(strip_exe("Foo.EXE"), "Foo");
        assert_eq!(strip_exe("exe"), "exe");
        assert_eq!(strip_exe("a.exe.txt"), "a.exe.txt");
        assert_eq!(strip_exe(""), "");
    }

    #[test]
    fn descendants_are_leaves_first_and_cycle_safe() {
        let p = |pid, parent| Proc { pid, parent, name: String::new() };
        let procs = [p(1, 0), p(2, 1), p(3, 2), p(4, 1), p(5, 99), p(6, 6)];
        let d = descendants(&procs, 1);
        assert_eq!(d.len(), 3);
        let pos = |x| d.iter().position(|&v| v == x).unwrap();
        assert!(pos(3) < pos(2)); // cháu trước con
        assert!(!d.contains(&5) && !d.contains(&6));
    }

    #[test]
    fn lists_current_process() {
        let me = std::process::id();
        let procs = list().unwrap();
        assert!(procs.iter().any(|p| p.pid == me));
        assert!(image_path(me).is_some());
    }
}
