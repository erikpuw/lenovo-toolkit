//! AI Engine ở chế độ Cân bằng (port từ AIController.cs).
//!
//! Khi bật trong cài đặt: nếu đang cắm sạc đủ công suất + chế độ Cân bằng + có game chạy thì đặt
//! sub-mode thông minh của firmware (theo danh sách LENOVO_INTELLIGENT_OP_LIST, mặc định 1);
//! hết điều kiện thì trả về 0.
//!
//! Khác bản C#: không nghe sự kiện mà được gọi định kỳ (`refresh`), nên quyết định là idempotent
//! (chỉ ghi khi sub-mode khác mục tiêu) để không đảo 0→1 liên tục như chuỗi tắt-rồi-bật của C#.

use std::collections::HashMap;
use std::io;

use wmi::Variant;

use crate::games;
use crate::power::{self, AdapterStatus, PowerMode};
use crate::process::{self, Proc};
use crate::settings;
use crate::wmi_util::{Wmi, variant_to_i32};

const CLASS: &str = "LENOVO_GAMEZONE_DATA";
const DEFAULT_SUB_MODE: i32 = 1;

pub fn sub_mode_get() -> io::Result<i32> {
    Wmi::open()?.call_i32(CLASS, "GetIntelligentSubMode", &[], "Data")
}

pub fn sub_mode_set(value: i32) -> io::Result<()> {
    Wmi::open()?.call(CLASS, "SetIntelligentSubMode", &[("Data", value)])?;
    Ok(())
}

///(tên tiến trình, sub-mode) theo thứ tự WMI, bỏ trùng tên.
pub fn op_list_with(w: &Wmi) -> io::Result<Vec<(String, i32)>> {
    let rows: Vec<HashMap<String, Variant>> =
        w.query("SELECT processname, mode FROM LENOVO_INTELLIGENT_OP_LIST")?;
    let mut result: Vec<(String, i32)> = Vec::new();
    for mut row in rows {
        let name = match row.remove("processname") {
            Some(Variant::String(s)) => s,
            _ => continue,
        };
        let Some(mode) = row.remove("mode").and_then(|v| variant_to_i32(v).ok()) else { continue };
        if !result.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            result.push((name, mode));
        }
    }
    Ok(result)
}

/// (tên tiến trình, sub-mode) theo thứ tự WMI, bỏ trùng tên.
pub fn op_list() -> io::Result<Vec<(String, i32)>> {
    op_list_with(&Wmi::open()?)
}

/// Sub-mode mục tiêu: của tiến trình đầu tiên trong danh sách đang chạy, nếu không có thì 1.
pub fn target_sub_mode(op_list: &[(String, i32)], procs: &[Proc]) -> i32 {
    for (file, mode) in op_list {
        let stem = std::path::Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or(file);
        if procs.iter().any(|p| p.name.eq_ignore_ascii_case(stem)) {
            return *mode;
        }
    }
    DEFAULT_SUB_MODE
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inputs {
    pub adapter: AdapterStatus,
    pub power_mode: Option<PowerMode>,
    pub games_running: bool,
    pub sub_mode: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Enable(i32),
    Disable,
}

pub fn decide(i: &Inputs, target: i32) -> Action {
    let balance = i.power_mode == Some(PowerMode::Balance);
    let want = i.adapter == AdapterStatus::Connected && balance && i.games_running;
    if want {
        if i.sub_mode == target { Action::None } else { Action::Enable(target) }
    } else if balance && i.sub_mode != 0 {
        Action::Disable
    } else {
        Action::None
    }
}

/// Đọc lại điều kiện và áp dụng với kết nối WMI có sẵn + danh sách `op_list` được cache.
pub fn refresh_with(w: &Wmi, cached_op_list: &mut Option<Vec<(String, i32)>>) -> io::Result<Action> {
    let adapter = power::adapter_status_with(w);
    let power_mode = power::get_with(w).ok();
    let sub_mode = w.call_i32(CLASS, "GetIntelligentSubMode", &[], "Data")?;

    // Chỉ quét danh sách tiến trình khi đang cắm sạc và ở chế độ Balance (hoặc khi cần tắt sub_mode != 0).
    let balance = power_mode == Some(PowerMode::Balance);
    if !balance {
        return Ok(Action::None);
    }
    if adapter != AdapterStatus::Connected {
        let action = if sub_mode != 0 { Action::Disable } else { Action::None };
        if action == Action::Disable {
            w.call(CLASS, "SetIntelligentSubMode", &[("Data", 0)])?;
        }
        return Ok(action);
    }

    let running_games = games::running()?;
    let games_running = !running_games.is_empty();
    let inputs = Inputs { adapter, power_mode, games_running, sub_mode };
    let target = if games_running {
        let procs = process::list()?;
        if cached_op_list.is_none() {
            *cached_op_list = op_list_with(w).ok();
        }
        target_sub_mode(cached_op_list.as_deref().unwrap_or(&[]), &procs)
    } else {
        DEFAULT_SUB_MODE
    };
    let action = decide(&inputs, target);
    match action {
        Action::None => {}
        Action::Enable(v) => {
            w.call(CLASS, "SetIntelligentSubMode", &[("Data", v)])?;
        }
        Action::Disable => {
            w.call(CLASS, "SetIntelligentSubMode", &[("Data", 0)])?;
        }
    }
    Ok(action)
}

/// Đọc lại điều kiện và áp dụng. Chỉ gọi khi AI Engine đang bật trong cài đặt.
pub fn refresh() -> io::Result<Action> {
    let mut cache = None;
    refresh_with(&Wmi::open()?, &mut cache)
}

/// Khi người dùng tắt AI Engine: nếu đang ở Cân bằng và sub-mode khác 0 thì trả về 0
/// (giống `StopAsync`). Trả về `true` nếu đã ghi.
pub fn disable_if_needed() -> io::Result<bool> {
    let balance = power::get().ok() == Some(PowerMode::Balance);
    if balance && sub_mode_get()? != 0 {
        sub_mode_set(0)?;
        return Ok(true);
    }
    Ok(false)
}

pub fn is_enabled() -> bool {
    settings::ai_mode_enabled()
}

pub fn set_enabled(value: bool) -> io::Result<()> {
    settings::set_ai_mode_enabled(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(adapter: AdapterStatus, mode: PowerMode, games: bool, sub: i32) -> Inputs {
        Inputs { adapter, power_mode: Some(mode), games_running: games, sub_mode: sub }
    }

    #[test]
    fn enables_only_with_all_conditions() {
        use AdapterStatus::*;
        use PowerMode::*;
        assert_eq!(decide(&inputs(Connected, Balance, true, 0), 1), Action::Enable(1));
        assert_eq!(decide(&inputs(ConnectedLowWattage, Balance, true, 0), 1), Action::None);
        assert_eq!(decide(&inputs(Disconnected, Balance, true, 0), 1), Action::None);
        assert_eq!(decide(&inputs(Connected, Performance, true, 0), 1), Action::None);
        assert_eq!(decide(&inputs(Connected, Balance, false, 0), 1), Action::None);
    }

    #[test]
    fn is_idempotent_while_conditions_hold() {
        use AdapterStatus::*;
        use PowerMode::*;
        assert_eq!(decide(&inputs(Connected, Balance, true, 1), 1), Action::None);
        assert_eq!(decide(&inputs(Connected, Balance, true, 1), 3), Action::Enable(3));
    }

    #[test]
    fn disables_when_conditions_end_but_only_in_balance() {
        use AdapterStatus::*;
        use PowerMode::*;
        assert_eq!(decide(&inputs(Connected, Balance, false, 1), 1), Action::Disable);
        assert_eq!(decide(&inputs(Disconnected, Balance, true, 2), 1), Action::Disable);
        assert_eq!(decide(&inputs(Connected, Quiet, false, 1), 1), Action::None);
        assert_eq!(decide(&inputs(Connected, Balance, false, 0), 1), Action::None);
    }

    #[test]
    fn target_prefers_running_process_from_list() {
        let list = vec![("a.exe".to_string(), 3), ("b.exe".to_string(), 4)];
        let p = |name: &str| Proc { pid: 1, parent: 0, name: name.into() };
        assert_eq!(target_sub_mode(&list, &[p("B"), p("zzz")]), 4);
        assert_eq!(target_sub_mode(&list, &[p("a"), p("b")]), 3); // thứ tự danh sách quyết định
        assert_eq!(target_sub_mode(&list, &[p("other")]), 1);
        assert_eq!(target_sub_mode(&[], &[p("a")]), 1);
    }
}
