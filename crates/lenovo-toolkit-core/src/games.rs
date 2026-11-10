//! Phát hiện game đang chạy (rút gọn từ GameAutoListener.cs + GameConfigStoreDetector.cs).
//!
//! Chỉ dùng nguồn "danh sách game đã biết" của Windows (GameConfigStore) và thăm dò theo chu kỳ.
//! Chưa có: nguồn dGPU (GPUController), Windows Game Mode + ghim cửa sổ foreground, danh sách
//! Included/ExcludedProcesses của người dùng, sự kiện bắt đầu/kết thúc tiến trình.

use std::io;
use std::path::Path;

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

use crate::process::{self, Proc};

const GAME_CONFIG_STORE: &str = r"System\GameConfigStore\Children";
const MATCHED_EXE: &str = "MatchedExeFullPath";

/// Tiến trình hệ thống/launcher/anti-cheat không bao giờ coi là game (bản C# `IsBlacklisted`).
const BLACKLIST: &[&str] = &[
    "explorer", "Lenovo Legion Toolkit", "lenovo-toolkit", "lenovo-toolkit-cli", "SearchUI", "LockApp", "TextInputHost",
    "ShellExperienceHost", "StartMenuExperienceHost", "SearchHost", "dwm", "csrss", "WmiApSrv", "HWiNFO64",
    "HWiNFO32", "nvidia-smi", "steamwebhelper", "EpicWebHelper", "GalaxyCommunicationService",
    "GalaxyClientHelper", "EABackgroundService", "Link2EA", "RiotClientCrashHandler", "CrashReportClient",
    "UnityCrashHandler64", "UnityCrashHandler32", "UE4-CrashTracker", "crs-handler",
    "cefsharp.browsersubprocess", "conhost", "nvngx_update", "GameGuard", "GameGuard.des", "GameMon",
    "GameMon64", "EasyAntiCheat", "EasyAntiCheat_EOS", "BEService", "BEService_x64", "vgc", "vgtray",
    "EAAntiCheat.GameService", "EAAntiCheat.GameServiceLauncher", "ACE-Base", "UbisoftConnect",
    "UbisoftGameLauncher", "Battle.net Helper", "Agent",
];

pub fn is_blacklisted(name: &str) -> bool {
    BLACKLIST.iter().any(|b| b.eq_ignore_ascii_case(name))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownGame {
    /// Tên tiến trình (không đuôi), chữ thường.
    pub name: String,
    /// Đường dẫn chuẩn hóa, chữ thường.
    pub path: String,
}

fn normalize_path(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

/// Mở rộng `%BIẾN%` như `Environment.ExpandEnvironmentVariables`; biến lạ giữ nguyên.
pub(crate) fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => match std::env::var(&after[..end]) {
                Ok(v) if !after[..end].is_empty() => {
                    out.push_str(&v);
                    rest = &after[end + 1..];
                }
                _ => {
                    out.push('%');
                    rest = after;
                }
            },
            None => {
                out.push('%');
                rest = after;
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn known_games() -> Vec<KnownGame> {
    let Ok(children) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(GAME_CONFIG_STORE, KEY_READ) else {
        return Vec::new();
    };
    let mut games: Vec<KnownGame> = Vec::new();
    for sub in children.enum_keys().flatten() {
        let Ok(key) = children.open_subkey_with_flags(&sub, KEY_READ) else { continue };
        let Ok(raw) = key.get_value::<String, _>(MATCHED_EXE) else { continue };
        let exe = expand_env(raw.trim().trim_matches('"').trim());
        if exe.is_empty() {
            continue;
        }
        let Some(stem) = Path::new(&exe).file_stem().and_then(|s| s.to_str()) else { continue };
        let game = KnownGame { name: stem.to_lowercase(), path: normalize_path(&exe) };
        if !games.contains(&game) {
            games.push(game);
        }
    }
    games
}

/// Các tiến trình đang chạy khớp một game đã biết (cùng tên, và cùng đường dẫn nếu biết cả hai).
pub fn find_running(procs: &[Proc], known: &[KnownGame], path_of: impl Fn(u32) -> Option<String>) -> Vec<Proc> {
    procs
        .iter()
        .filter(|p| !is_blacklisted(&p.name))
        .filter(|p| {
            let name = p.name.to_lowercase();
            known.iter().any(|g| {
                g.name == name
                    && path_of(p.pid).is_none_or(|actual| normalize_path(&actual) == g.path)
            })
        })
        .cloned()
        .collect()
}

pub fn running() -> io::Result<Vec<Proc>> {
    let procs = process::list()?;
    Ok(find_running(&procs, &known_games(), process::image_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, name: &str) -> Proc {
        Proc { pid, parent: 0, name: name.into() }
    }

    fn game(path: &str) -> KnownGame {
        KnownGame {
            name: Path::new(path).file_stem().unwrap().to_str().unwrap().to_lowercase(),
            path: normalize_path(path),
        }
    }

    #[test]
    fn blacklist_is_case_insensitive() {
        assert!(is_blacklisted("Explorer"));
        assert!(is_blacklisted("steamwebhelper"));
        assert!(is_blacklisted("battle.net helper"));
        assert!(!is_blacklisted("eldenring"));
    }

    #[test]
    fn matches_by_name_and_path() {
        let procs = [proc(1, "EldenRing"), proc(2, "other"), proc(3, "EldenRing")];
        let known = [game(r"C:\Games\EldenRing.exe")];
        let path_of = |pid| match pid {
            1 => Some(r"c:\games\eldenring.exe".to_string()),
            3 => Some(r"D:\copy\EldenRing.exe".to_string()), // cùng tên, khác đường dẫn
            _ => None,
        };
        let found = find_running(&procs, &known, path_of);
        assert_eq!(found.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn unknown_path_still_matches_by_name() {
        let procs = [proc(7, "game")];
        let found = find_running(&procs, &[game(r"C:\x\game.exe")], |_| None);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn blacklisted_process_never_counts() {
        let procs = [proc(1, "explorer")];
        assert!(find_running(&procs, &[game(r"C:\Windows\explorer.exe")], |_| None).is_empty());
    }

    #[test]
    fn expands_environment_variables() {
        // SAFETY: test đơn luồng đối với biến này; tên biến duy nhất cho test.
        unsafe { std::env::set_var("LEGION_RS_TEST_DIR", r"C:\Games") };
        assert_eq!(expand_env(r"%LEGION_RS_TEST_DIR%\a.exe"), r"C:\Games\a.exe");
        assert_eq!(expand_env(r"%NOPE_NOT_SET%\a.exe"), r"%NOPE_NOT_SET%\a.exe");
        assert_eq!(expand_env("100%"), "100%");
    }
}
