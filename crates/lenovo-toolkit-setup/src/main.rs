#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod install;
mod shortcut;
mod ui;

use install::{
    APP_NAME, APP_VERSION, InstallOptions, default_install_dir, is_elevated, needs_elevation,
    perform_install, perform_uninstall, relaunch_elevated,
};
use std::path::PathBuf;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::w;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Install,
    Uninstall,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Ready,
    Done,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActionOutcome {
    StayOpen,
    CloseWindow,
}

pub struct CliArgs {
    pub mode: Mode,
    pub auto_run: bool,
    pub is_vi: bool,
    pub dir: Option<PathBuf>,
    pub startup: bool,
    pub start_menu: bool,
    pub desktop: bool,
    pub launch: bool,
}

impl CliArgs {
    fn parse() -> Self {
        let self_is_uninstaller = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()))
            .is_some_and(|n| n.contains("uninstall"));
        let args: Vec<String> = std::env::args().skip(1).collect();
        Self::parse_from(&args, self_is_uninstaller)
    }

    fn parse_from(args: &[String], self_is_uninstaller: bool) -> Self {
        let mut out = CliArgs {
            mode: if self_is_uninstaller {
                Mode::Uninstall
            } else {
                Mode::Install
            },
            auto_run: false,
            is_vi: true,
            dir: None,
            startup: true,
            start_menu: true,
            desktop: true,
            launch: true,
        };
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--uninstall" => out.mode = Mode::Uninstall,
                "--auto-install" => {
                    out.mode = Mode::Install;
                    out.auto_run = true;
                }
                "--auto-uninstall" => {
                    out.mode = Mode::Uninstall;
                    out.auto_run = true;
                }
                "--en" => out.is_vi = false,
                "--vi" => out.is_vi = true,
                "--dir" if i + 1 < args.len() => {
                    i += 1;
                    out.dir = Some(PathBuf::from(&args[i]));
                }
                "--startup=0" => out.startup = false,
                "--startup=1" => out.startup = true,
                "--startmenu=0" => out.start_menu = false,
                "--startmenu=1" => out.start_menu = true,
                "--desktop=0" => out.desktop = false,
                "--desktop=1" => out.desktop = true,
                "--launch=0" => out.launch = false,
                "--launch=1" => out.launch = true,
                _ => {}
            }
            i += 1;
        }
        out
    }
}

fn is_windows_dark_mode() -> bool {
    let mut val = 1u32;
    let mut size = size_of::<u32>() as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut val as *mut u32).cast()),
            Some(&mut size),
        )
    };
    ok.is_ok() && val == 0
}

pub struct AppState {
    pub mode: Mode,
    pub is_vi: bool,
    pub dark: bool,
    pub elevated: bool,
    pub auto_run_pending: bool,
    pub install_dir_text: String,
    pub create_start_menu: bool,
    pub create_desktop: bool,
    pub enable_startup_task: bool,
    pub launch_after: bool,
    pub status: Status,
    pub message: String,
    pub installed_exe: Option<PathBuf>,
}

impl AppState {
    fn new(cli: CliArgs) -> Self {
        let dir = cli.dir.unwrap_or_else(|| {
            if cli.mode == Mode::Uninstall {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(PathBuf::from))
                    .unwrap_or_else(default_install_dir)
            } else {
                default_install_dir()
            }
        });
        Self {
            mode: cli.mode,
            is_vi: cli.is_vi,
            dark: is_windows_dark_mode(),
            elevated: is_elevated(),
            auto_run_pending: cli.auto_run,
            install_dir_text: dir.display().to_string(),
            create_start_menu: cli.start_menu,
            create_desktop: cli.desktop,
            enable_startup_task: cli.startup,
            launch_after: cli.launch,
            status: Status::Ready,
            message: String::new(),
            installed_exe: None,
        }
    }

    pub fn current_options(&self) -> InstallOptions {
        let raw = self.install_dir_text.trim();
        let install_dir = if raw.is_empty() {
            default_install_dir()
        } else {
            PathBuf::from(raw)
        };
        InstallOptions {
            install_dir,
            create_start_menu: self.create_start_menu,
            create_desktop: self.create_desktop,
            enable_startup_task: self.enable_startup_task,
            launch_after: self.launch_after,
        }
    }

    pub fn need_uac(&self) -> bool {
        match self.mode {
            Mode::Install => needs_elevation(&self.current_options()),
            Mode::Uninstall => true,
        }
    }

    pub fn run_primary_action(&mut self) -> ActionOutcome {
        let opts = self.current_options();
        let need_uac = match self.mode {
            Mode::Install => needs_elevation(&opts),
            Mode::Uninstall => true,
        };
        if need_uac && !self.elevated {
            let lang = if self.is_vi { "--vi" } else { "--en" };
            let dir_escaped = opts.install_dir.display().to_string().replace('"', "");
            let args = match self.mode {
                Mode::Install => format!(
                    "--auto-install {lang} --dir \"{dir_escaped}\" --startup={} --startmenu={} --desktop={} --launch={}",
                    u8::from(opts.enable_startup_task),
                    u8::from(opts.create_start_menu),
                    u8::from(opts.create_desktop),
                    u8::from(opts.launch_after)
                ),
                Mode::Uninstall => format!("--auto-uninstall {lang} --dir \"{dir_escaped}\""),
            };
            match relaunch_elevated(&args) {
                Ok(()) => return ActionOutcome::CloseWindow,
                Err(err) => {
                    self.status = Status::Failed;
                    self.message = if self.is_vi {
                        format!("Đã huỷ xin quyền quản trị (UAC): {err}")
                    } else {
                        format!("UAC elevation cancelled: {err}")
                    };
                }
            }
            return ActionOutcome::StayOpen;
        }

        match self.mode {
            Mode::Install => match perform_install(&opts) {
                Ok(exe) => {
                    self.installed_exe = Some(exe);
                    self.status = Status::Done;
                    self.message = if self.is_vi {
                        format!("Đã cài đặt thành công {APP_NAME} v{APP_VERSION}!")
                    } else {
                        format!("{APP_NAME} v{APP_VERSION} installed successfully!")
                    };
                }
                Err(err) => {
                    self.status = Status::Failed;
                    self.message = err;
                }
            },
            Mode::Uninstall => match perform_uninstall(&opts.install_dir) {
                Ok(()) => {
                    self.status = Status::Done;
                    self.message = if self.is_vi {
                        format!("Đã gỡ cài đặt {APP_NAME} khỏi máy.")
                    } else {
                        format!("{APP_NAME} has been uninstalled.")
                    };
                }
                Err(err) => {
                    self.status = Status::Failed;
                    self.message = err;
                }
            },
        }
        ActionOutcome::StayOpen
    }
}

fn main() {
    let state = AppState::new(CliArgs::parse());
    if let Err(err) = ui::run(state) {
        eprintln!("Lenovo Toolkit Setup error: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_args_parses_auto_install_flags() {
        let raw: Vec<String> = [
            "--auto-install",
            "--en",
            "--dir",
            r"D:\Apps\LenovoToolkit",
            "--startup=0",
            "--startmenu=1",
            "--desktop=0",
            "--launch=0",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let cli = CliArgs::parse_from(&raw, false);
        assert_eq!(cli.mode, Mode::Install);
        assert!(cli.auto_run);
        assert!(!cli.is_vi);
        assert_eq!(cli.dir, Some(PathBuf::from(r"D:\Apps\LenovoToolkit")));
        assert!(!cli.startup);
        assert!(cli.start_menu);
        assert!(!cli.desktop);
        assert!(!cli.launch);
    }

    #[test]
    fn cli_args_detects_uninstaller_by_exe_name() {
        let cli = CliArgs::parse_from(&[], true);
        assert_eq!(cli.mode, Mode::Uninstall);
        assert!(!cli.auto_run);
        assert!(cli.is_vi);
    }
}
