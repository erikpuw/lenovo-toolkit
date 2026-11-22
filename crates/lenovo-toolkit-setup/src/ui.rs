use crate::install::{APP_NAME, APP_VERSION, TASK_NAME, launch_installed_app};
use crate::shortcut;
use crate::{ActionOutcome, AppState, Mode, Status};
use std::ffi::c_void;
use std::path::PathBuf;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BitBlt, BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH,
    DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_PATH_ELLIPSIS, DT_SINGLELINE, DT_VCENTER,
    DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD,
    FillRect, HBRUSH, HDC, HFONT, HGDIOBJ, InvalidateRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    PS_SOLID, Polyline, RoundRect, SRCCOPY, SelectObject, SetBkColor, SetBkMode, SetTextColor,
    TRANSPARENT, DT_CENTER,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, EN_CHANGE, ES_AUTOHSCROLL, GWLP_USERDATA, GetClientRect, GetMessageW,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, HMENU, IDC_ARROW, IDC_HAND,
    LoadCursorW, LoadIconW, MSG, PostMessageW, PostQuitMessage, RegisterClassW, SPI_GETWORKAREA,
    SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetCursor, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, SystemParametersInfoW,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_COMMAND, WM_CREATE, WM_CTLCOLOREDIT, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY,
    WM_PAINT, WM_SETCURSOR, WM_SETFONT, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN,
    WS_DISABLED, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP,
};
use windows::core::{HSTRING, PCWSTR, w};

const LOGICAL_W: i32 = 560;
const LOGICAL_H_INSTALL: i32 = 500;
const LOGICAL_H_UNINSTALL: i32 = 360;

const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_APP_AUTO_RUN: u32 = WM_APP + 1;
const ID_EDIT_DIR: usize = 101;
const WINDOW_STYLE_FLAGS: WINDOW_STYLE = WINDOW_STYLE(
    WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_CLIPCHILDREN.0,
);

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

#[derive(Clone, Copy)]
struct Palette {
    bg: COLORREF,
    card: COLORREF,
    input_bg: COLORREF,
    border: COLORREF,
    text_pri: COLORREF,
    text_sec: COLORREF,
    accent: COLORREF,
    accent_hover: COLORREF,
    accent_press: COLORREF,
    accent_text: COLORREF,
    btn_bg: COLORREF,
    btn_hover: COLORREF,
    btn_press: COLORREF,
    warn: COLORREF,
    ok: COLORREF,
    err: COLORREF,
}

impl Palette {
    const fn for_theme(dark: bool) -> Self {
        if dark {
            Self {
                bg: rgb(24, 24, 24),
                card: rgb(35, 35, 35),
                input_bg: rgb(28, 28, 28),
                border: rgb(62, 62, 62),
                text_pri: rgb(243, 243, 243),
                text_sec: rgb(168, 168, 168),
                accent: rgb(76, 194, 255),
                accent_hover: rgb(104, 206, 255),
                accent_press: rgb(56, 172, 232),
                accent_text: rgb(16, 16, 16),
                btn_bg: rgb(46, 46, 46),
                btn_hover: rgb(58, 58, 58),
                btn_press: rgb(38, 38, 38),
                warn: rgb(252, 200, 90),
                ok: rgb(108, 214, 134),
                err: rgb(255, 110, 110),
            }
        } else {
            Self {
                bg: rgb(243, 243, 243),
                card: rgb(255, 255, 255),
                input_bg: rgb(255, 255, 255),
                border: rgb(214, 214, 214),
                text_pri: rgb(24, 24, 24),
                text_sec: rgb(96, 96, 96),
                accent: rgb(0, 103, 192),
                accent_hover: rgb(18, 118, 204),
                accent_press: rgb(0, 86, 164),
                accent_text: rgb(255, 255, 255),
                btn_bg: rgb(250, 250, 250),
                btn_hover: rgb(238, 238, 238),
                btn_press: rgb(226, 226, 226),
                warn: rgb(176, 104, 0),
                ok: rgb(20, 130, 56),
                err: rgb(196, 43, 28),
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HitId {
    LangToggle,
    Browse,
    CheckStartup,
    CheckStartMenu,
    CheckDesktop,
    CheckLaunch,
    Primary,
    Cancel,
    Finish,
}

struct Layout {
    title: RECT,
    subtitle: RECT,
    lang_btn: RECT,
    options_card: RECT,
    folder_label: RECT,
    edit_frame: RECT,
    edit_child: RECT,
    browse_btn: RECT,
    chk_startup: RECT,
    chk_start_menu: RECT,
    chk_desktop: RECT,
    chk_launch: RECT,
    uninstall_title: RECT,
    uninstall_folder: RECT,
    perm_card: RECT,
    perm_title: RECT,
    perm_uac: RECT,
    perm_details: RECT,
    notice: RECT,
    message: RECT,
    primary_btn: RECT,
    cancel_btn: RECT,
    finish_btn: RECT,
}

fn sc(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

fn rect(x: i32, y: i32, w: i32, h: i32, dpi: u32) -> RECT {
    RECT {
        left: sc(x, dpi),
        top: sc(y, dpi),
        right: sc(x + w, dpi),
        bottom: sc(y + h, dpi),
    }
}

fn pt_in_rect(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

impl Layout {
    fn compute(mode: Mode, dpi: u32) -> Self {
        let m = 20;
        let cw = LOGICAL_W - m * 2;
        let ch = match mode {
            Mode::Install => LOGICAL_H_INSTALL,
            Mode::Uninstall => LOGICAL_H_UNINSTALL,
        };

        let lang_w = 44;
        let title = rect(m, 16, cw - lang_w - 10, 26, dpi);
        let subtitle = rect(m, 44, cw - lang_w - 10, 18, dpi);
        let lang_btn = rect(m + cw - lang_w, 18, lang_w, 28, dpi);

        let opt_y = 74;
        let opt_h = match mode {
            Mode::Install => 194,
            Mode::Uninstall => 72,
        };
        let options_card = rect(m, opt_y, cw, opt_h, dpi);
        let inner_x = m + 14;
        let inner_w = cw - 28;

        let browse_w = 112;
        let edit_w = inner_w - browse_w - 8;
        let folder_label = rect(inner_x, opt_y + 12, inner_w, 18, dpi);
        let edit_frame = rect(inner_x, opt_y + 34, edit_w, 30, dpi);
        let edit_child = rect(inner_x + 8, opt_y + 40, edit_w - 16, 18, dpi);
        let browse_btn = rect(inner_x + edit_w + 8, opt_y + 34, browse_w, 30, dpi);

        let chk_startup = rect(inner_x, opt_y + 74, inner_w, 26, dpi);
        let chk_start_menu = rect(inner_x, opt_y + 100, inner_w, 26, dpi);
        let chk_desktop = rect(inner_x, opt_y + 126, inner_w, 26, dpi);
        let chk_launch = rect(inner_x, opt_y + 152, inner_w, 26, dpi);

        let uninstall_title = rect(inner_x, opt_y + 14, inner_w, 20, dpi);
        let uninstall_folder = rect(inner_x, opt_y + 38, inner_w, 20, dpi);

        let perm_y = opt_y + opt_h + 12;
        let perm_h = match mode {
            Mode::Install => 104,
            Mode::Uninstall => 86,
        };
        let perm_card = rect(m, perm_y, cw, perm_h, dpi);
        let perm_title = rect(inner_x, perm_y + 10, inner_w, 18, dpi);
        let perm_uac = rect(inner_x, perm_y + 32, inner_w, 18, dpi);
        let perm_details = rect(inner_x, perm_y + 52, inner_w, perm_h - 60, dpi);

        let notice_y = perm_y + perm_h + 10;
        let notice = rect(m, notice_y, cw, 18, dpi);
        let message = rect(m, notice_y + 22, cw, 30, dpi);

        let btn_h = 34;
        let btn_y = ch - 20 - btn_h;
        let primary_w = 148;
        let cancel_w = 92;
        let finish_w = 116;

        let primary_btn = rect(m + cw - primary_w, btn_y, primary_w, btn_h, dpi);
        let cancel_btn = rect(m + cw - primary_w - 10 - cancel_w, btn_y, cancel_w, btn_h, dpi);
        let finish_btn = rect(m + cw - finish_w, btn_y, finish_w, btn_h, dpi);

        Self {
            title,
            subtitle,
            lang_btn,
            options_card,
            folder_label,
            edit_frame,
            edit_child,
            browse_btn,
            chk_startup,
            chk_start_menu,
            chk_desktop,
            chk_launch,
            uninstall_title,
            uninstall_folder,
            perm_card,
            perm_title,
            perm_uac,
            perm_details,
            notice,
            message,
            primary_btn,
            cancel_btn,
            finish_btn,
        }
    }

    fn hit_test(&self, mode: Mode, status: Status, x: i32, y: i32) -> Option<HitId> {
        if pt_in_rect(self.lang_btn, x, y) {
            return Some(HitId::LangToggle);
        }
        if status == Status::Done {
            if pt_in_rect(self.finish_btn, x, y) {
                return Some(HitId::Finish);
            }
            return None;
        }
        if mode == Mode::Install {
            if pt_in_rect(self.browse_btn, x, y) {
                return Some(HitId::Browse);
            }
            if pt_in_rect(self.chk_startup, x, y) {
                return Some(HitId::CheckStartup);
            }
            if pt_in_rect(self.chk_start_menu, x, y) {
                return Some(HitId::CheckStartMenu);
            }
            if pt_in_rect(self.chk_desktop, x, y) {
                return Some(HitId::CheckDesktop);
            }
            if pt_in_rect(self.chk_launch, x, y) {
                return Some(HitId::CheckLaunch);
            }
        }
        if pt_in_rect(self.primary_btn, x, y) {
            return Some(HitId::Primary);
        }
        if pt_in_rect(self.cancel_btn, x, y) {
            return Some(HitId::Cancel);
        }
        None
    }
}

struct GdiResources {
    font_title: HFONT,
    font_bold: HFONT,
    font_body: HFONT,
    font_small: HFONT,
    edit_brush: HBRUSH,
}

impl GdiResources {
    fn new(dpi: u32, pal: Palette) -> Self {
        unsafe {
            Self {
                font_title: make_font(sc(21, dpi), FW_SEMIBOLD.0 as i32),
                font_bold: make_font(sc(14, dpi), FW_SEMIBOLD.0 as i32),
                font_body: make_font(sc(14, dpi), FW_NORMAL.0 as i32),
                font_small: make_font(sc(13, dpi), FW_NORMAL.0 as i32),
                edit_brush: CreateSolidBrush(pal.input_bg),
            }
        }
    }
}

impl Drop for GdiResources {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.font_title.0));
            let _ = DeleteObject(HGDIOBJ(self.font_bold.0));
            let _ = DeleteObject(HGDIOBJ(self.font_body.0));
            let _ = DeleteObject(HGDIOBJ(self.font_small.0));
            let _ = DeleteObject(HGDIOBJ(self.edit_brush.0));
        }
    }
}

unsafe fn make_font(px: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -px,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            w!("Segoe UI"),
        )
    }
}

struct WindowData {
    state: AppState,
    dpi: u32,
    pal: Palette,
    res: GdiResources,
    edit_hwnd: HWND,
    updating_edit: bool,
    hovered: Option<HitId>,
    pressed: Option<HitId>,
    tracking_mouse: bool,
}

impl WindowData {
    fn new(state: AppState, dpi: u32) -> Self {
        let pal = Palette::for_theme(state.dark);
        let res = GdiResources::new(dpi, pal);
        Self {
            state,
            dpi,
            pal,
            res,
            edit_hwnd: HWND::default(),
            updating_edit: false,
            hovered: None,
            pressed: None,
            tracking_mouse: false,
        }
    }

    fn layout(&self) -> Layout {
        Layout::compute(self.state.mode, self.dpi)
    }

    fn sync_edit_control(&mut self) {
        if self.edit_hwnd.0.is_null() {
            return;
        }
        let lay = self.layout();
        unsafe {
            let show = self.state.mode == Mode::Install;
            let _ = ShowWindow(self.edit_hwnd, if show { SW_SHOW } else { SW_HIDE });
            if show {
                let r = lay.edit_child;
                let _ = SetWindowPos(
                    self.edit_hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let _ = SendMessageW(
                    self.edit_hwnd,
                    WM_SETFONT,
                    Some(WPARAM(self.res.font_body.0 as usize)),
                    Some(LPARAM(1)),
                );
                let disabled = self.state.status == Status::Done;
                let style = GetWindowLongPtrW(self.edit_hwnd, windows::Win32::UI::WindowsAndMessaging::GWL_STYLE) as u32;
                let new_style = if disabled {
                    style | WS_DISABLED.0
                } else {
                    style & !WS_DISABLED.0
                };
                if new_style != style {
                    SetWindowLongPtrW(
                        self.edit_hwnd,
                        windows::Win32::UI::WindowsAndMessaging::GWL_STYLE,
                        new_style as isize,
                    );
                }
            }
        }
    }

    fn set_edit_text(&mut self, text: &str) {
        if self.edit_hwnd.0.is_null() {
            return;
        }
        self.updating_edit = true;
        let wide = HSTRING::from(text);
        unsafe {
            let _ = SetWindowTextW(self.edit_hwnd, &wide);
        }
        self.updating_edit = false;
    }

    fn read_edit_text(&mut self) {
        if self.edit_hwnd.0.is_null() || self.updating_edit {
            return;
        }
        unsafe {
            let len = GetWindowTextLengthW(self.edit_hwnd);
            if len < 0 {
                return;
            }
            let mut buf = vec![0u16; len as usize + 1];
            let copied = GetWindowTextW(self.edit_hwnd, &mut buf) as usize;
            self.state.install_dir_text = String::from_utf16_lossy(&buf[..copied]);
        }
    }

    fn handle_click(&mut self, hwnd: HWND, hit: HitId) {
        match hit {
            HitId::LangToggle => {
                self.state.is_vi = !self.state.is_vi;
            }
            HitId::Browse => {
                let cur = PathBuf::from(self.state.install_dir_text.trim());
                let title = if self.state.is_vi {
                    "Chọn thư mục cài đặt Lenovo Toolkit"
                } else {
                    "Choose Lenovo Toolkit installation folder"
                };
                if let Some(chosen) = shortcut::pick_folder(Some(hwnd), &cur, title) {
                    let final_dir = if chosen
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| {
                            n.eq_ignore_ascii_case(APP_NAME)
                                || n.eq_ignore_ascii_case("LenovoToolkit")
                        }) {
                        chosen
                    } else {
                        chosen.join("LenovoToolkit")
                    };
                    self.state.install_dir_text = final_dir.display().to_string();
                    self.set_edit_text(&self.state.install_dir_text.clone());
                }
            }
            HitId::CheckStartup => {
                self.state.enable_startup_task = !self.state.enable_startup_task;
            }
            HitId::CheckStartMenu => {
                self.state.create_start_menu = !self.state.create_start_menu;
            }
            HitId::CheckDesktop => {
                self.state.create_desktop = !self.state.create_desktop;
            }
            HitId::CheckLaunch => {
                self.state.launch_after = !self.state.launch_after;
            }
            HitId::Primary => {
                if self.state.run_primary_action() == ActionOutcome::CloseWindow {
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                    return;
                }
                self.sync_edit_control();
            }
            HitId::Cancel => {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return;
            }
            HitId::Finish => {
                if self.state.mode == Mode::Install
                    && self.state.launch_after
                    && let Some(exe) = &self.state.installed_exe
                {
                    launch_installed_app(exe);
                }
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return;
            }
        }
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn paint(&self, hwnd: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if hdc.0.is_null() {
                return;
            }
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let w = (client.right - client.left).max(1);
            let h = (client.bottom - client.top).max(1);

            let mem_dc = CreateCompatibleDC(Some(hdc));
            let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
            let old_bmp = SelectObject(mem_dc, HGDIOBJ(mem_bmp.0));

            self.draw_scene(mem_dc, client);

            let _ = BitBlt(hdc, 0, 0, w, h, Some(mem_dc), 0, 0, SRCCOPY);
            SelectObject(mem_dc, old_bmp);
            let _ = DeleteObject(HGDIOBJ(mem_bmp.0));
            let _ = DeleteDC(mem_dc);
            let _ = EndPaint(hwnd, &ps);
        }
    }

    unsafe fn draw_scene(&self, hdc: HDC, client: RECT) {
        let lay = self.layout();
        let pal = self.pal;
        let dpi = self.dpi;
        let s = &self.state;
        let need_uac = s.need_uac();

        unsafe {
            let bg_brush = CreateSolidBrush(pal.bg);
            FillRect(hdc, &client, bg_brush);
            let _ = DeleteObject(HGDIOBJ(bg_brush.0));

            SetBkMode(hdc, TRANSPARENT);

            // Header
            draw_text(
                hdc,
                &format!("{APP_NAME} v{APP_VERSION} Setup"),
                lay.title,
                self.res.font_title,
                pal.text_pri,
                DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0,
            );
            let subtitle = match (s.mode, s.is_vi) {
                (Mode::Install, true) => "Trình cài đặt bộ công cụ nhẹ cho laptop Lenovo",
                (Mode::Install, false) => "Installer for lightweight Lenovo laptop toolkit",
                (Mode::Uninstall, true) => "Gỡ cài đặt Lenovo Toolkit khỏi máy tính",
                (Mode::Uninstall, false) => "Uninstall Lenovo Toolkit from this computer",
            };
            draw_text(
                hdc,
                subtitle,
                lay.subtitle,
                self.res.font_small,
                pal.text_sec,
                DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0,
            );
            let lang_lbl = if s.is_vi { "EN" } else { "VI" };
            self.draw_button(hdc, lay.lang_btn, lang_lbl, HitId::LangToggle, false);

            // Options card
            draw_round_box(hdc, lay.options_card, sc(8, dpi), pal.card, pal.border);
            match s.mode {
                Mode::Install => {
                    let folder_lbl = if s.is_vi {
                        "Thư mục cài đặt"
                    } else {
                        "Installation folder"
                    };
                    draw_text(
                        hdc,
                        folder_lbl,
                        lay.folder_label,
                        self.res.font_bold,
                        pal.text_pri,
                        DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0,
                    );
                    draw_round_box(hdc, lay.edit_frame, sc(6, dpi), pal.input_bg, pal.border);
                    let browse_lbl = if s.is_vi { "Chọn thư mục…" } else { "Browse…" };
                    self.draw_button(hdc, lay.browse_btn, browse_lbl, HitId::Browse, false);

                    self.draw_checkbox(
                        hdc,
                        lay.chk_startup,
                        s.enable_startup_task,
                        if s.is_vi {
                            "Khởi động ẩn cùng Windows (Task Scheduler, không chớp cửa sổ)"
                        } else {
                            "Start minimized with Windows (Task Scheduler, no console flash)"
                        },
                        HitId::CheckStartup,
                    );
                    self.draw_checkbox(
                        hdc,
                        lay.chk_start_menu,
                        s.create_start_menu,
                        if s.is_vi {
                            "Tạo lối tắt trong Start Menu"
                        } else {
                            "Create Start Menu shortcut"
                        },
                        HitId::CheckStartMenu,
                    );
                    self.draw_checkbox(
                        hdc,
                        lay.chk_desktop,
                        s.create_desktop,
                        if s.is_vi {
                            "Tạo lối tắt ngoài Desktop"
                        } else {
                            "Create Desktop shortcut"
                        },
                        HitId::CheckDesktop,
                    );
                    self.draw_checkbox(
                        hdc,
                        lay.chk_launch,
                        s.launch_after,
                        if s.is_vi {
                            "Mở Lenovo Toolkit sau khi cài xong"
                        } else {
                            "Launch Lenovo Toolkit after installation"
                        },
                        HitId::CheckLaunch,
                    );
                }
                Mode::Uninstall => {
                    let title = if s.is_vi {
                        "Gỡ cài đặt Lenovo Toolkit khỏi máy tính này"
                    } else {
                        "Uninstall Lenovo Toolkit from this computer"
                    };
                    draw_text(
                        hdc,
                        title,
                        lay.uninstall_title,
                        self.res.font_bold,
                        pal.text_pri,
                        DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0,
                    );
                    let folder = format!(
                        "{}: {}",
                        if s.is_vi { "Thư mục" } else { "Folder" },
                        s.install_dir_text
                    );
                    draw_text(
                        hdc,
                        &folder,
                        lay.uninstall_folder,
                        self.res.font_small,
                        pal.text_sec,
                        DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_PATH_ELLIPSIS.0 | DT_NOPREFIX.0,
                    );
                }
            }

            // Permissions & system changes card
            draw_round_box(hdc, lay.perm_card, sc(8, dpi), pal.card, pal.border);
            let perm_hdr = if s.is_vi {
                "Quyền & thay đổi trên máy"
            } else {
                "Permissions & system changes"
            };
            draw_text(
                hdc,
                perm_hdr,
                lay.perm_title,
                self.res.font_bold,
                pal.text_pri,
                DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0,
            );

            let (uac_txt, uac_col) = if s.elevated {
                (
                    if s.is_vi {
                        "• Quyền quản trị (UAC): Đã cấp"
                    } else {
                        "• Administrator (UAC): Granted"
                    },
                    pal.ok,
                )
            } else if need_uac {
                (
                    if s.is_vi {
                        "• Quyền quản trị (UAC): Sẽ hỏi khi bấm để ghi thư mục hệ thống / tạo Task"
                    } else {
                        "• Administrator (UAC): Will prompt to write system folder / startup task"
                    },
                    pal.warn,
                )
            } else {
                (
                    if s.is_vi {
                        "• Quyền quản trị (UAC): Không cần (cài trong thư mục người dùng)"
                    } else {
                        "• Administrator (UAC): Not required (user folder install)"
                    },
                    pal.ok,
                )
            };
            draw_text(
                hdc,
                uac_txt,
                lay.perm_uac,
                self.res.font_small,
                uac_col,
                DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0 | DT_NOPREFIX.0,
            );

            let details = match s.mode {
                Mode::Install => {
                    let task = if s.enable_startup_task {
                        if s.is_vi {
                            format!(
                                "• Tạo Scheduled Task \"{TASK_NAME}\" chạy thẳng lenovo-toolkit.exe --minimized (HighestAvailable)"
                            )
                        } else {
                            format!(
                                "• Creates Scheduled Task \"{TASK_NAME}\" launching lenovo-toolkit.exe --minimized (HighestAvailable)"
                            )
                        }
                    } else if s.is_vi {
                        "• Không tạo lịch tự khởi động cùng Windows".into()
                    } else {
                        "• No Windows startup task will be created".into()
                    };
                    let reg = if s.is_vi {
                        "• Đăng ký gỡ cài đặt trong Windows Settings -> Apps"
                    } else {
                        "• Registers uninstall entry in Windows Settings -> Apps"
                    };
                    format!("{task}\n{reg}")
                }
                Mode::Uninstall => {
                    if s.is_vi {
                        "• Tắt Lenovo Toolkit, xoá Scheduled Task, lối tắt và mục Settings -> Apps (giữ cài đặt người dùng)".into()
                    } else {
                        "• Stops Lenovo Toolkit, removes Scheduled Task, shortcuts and Settings -> Apps entry (keeps user settings)".into()
                    }
                }
            };
            draw_text(
                hdc,
                &details,
                lay.perm_details,
                self.res.font_small,
                pal.text_sec,
                DT_LEFT.0 | DT_WORDBREAK.0 | DT_NOPREFIX.0,
            );

            // Unsigned notice
            let notice = if s.is_vi {
                "Bản cài đặt chưa có chữ ký số (Unsigned) · Chỉ tải từ github.com/erikpuw/lenovo-toolkit"
            } else {
                "Unsigned installer build · Download only from github.com/erikpuw/lenovo-toolkit"
            };
            draw_text(
                hdc,
                notice,
                lay.notice,
                self.res.font_small,
                pal.text_sec,
                DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0 | DT_NOPREFIX.0,
            );

            // Status message
            if !s.message.is_empty() {
                let msg_col = match s.status {
                    Status::Done => pal.accent,
                    Status::Failed => pal.err,
                    Status::Ready => pal.text_sec,
                };
                draw_text(
                    hdc,
                    &s.message,
                    lay.message,
                    self.res.font_body,
                    msg_col,
                    DT_LEFT.0 | DT_WORDBREAK.0 | DT_NOPREFIX.0,
                );
            }

            // Bottom action buttons
            if s.status == Status::Done {
                let finish_txt = if s.is_vi { "Hoàn tất" } else { "Finish" };
                self.draw_button(hdc, lay.finish_btn, finish_txt, HitId::Finish, true);
            } else {
                let cancel_txt = if s.is_vi { "Đóng" } else { "Cancel" };
                self.draw_button(hdc, lay.cancel_btn, cancel_txt, HitId::Cancel, false);

                let primary_txt = match (s.mode, s.is_vi, need_uac && !s.elevated) {
                    (Mode::Install, true, true) => "Cài đặt (UAC)",
                    (Mode::Install, true, false) => "Cài đặt",
                    (Mode::Install, false, true) => "Install (UAC)",
                    (Mode::Install, false, false) => "Install",
                    (Mode::Uninstall, true, true) => "Gỡ cài đặt (UAC)",
                    (Mode::Uninstall, true, false) => "Gỡ cài đặt",
                    (Mode::Uninstall, false, true) => "Uninstall (UAC)",
                    (Mode::Uninstall, false, false) => "Uninstall",
                };
                self.draw_button(hdc, lay.primary_btn, primary_txt, HitId::Primary, true);
            }
        }
    }

    unsafe fn draw_button(&self, hdc: HDC, r: RECT, label: &str, id: HitId, primary: bool) {
        let hovered = self.hovered == Some(id);
        let pressed = self.pressed == Some(id) && hovered;
        let pal = self.pal;
        let (fill, border, text_col) = if primary {
            let fill = if pressed {
                pal.accent_press
            } else if hovered {
                pal.accent_hover
            } else {
                pal.accent
            };
            (fill, fill, pal.accent_text)
        } else {
            let fill = if pressed {
                pal.btn_press
            } else if hovered {
                pal.btn_hover
            } else {
                pal.btn_bg
            };
            (fill, pal.border, pal.text_pri)
        };
        unsafe {
            draw_round_box(hdc, r, sc(6, self.dpi), fill, border);
            let font = if primary {
                self.res.font_bold
            } else {
                self.res.font_body
            };
            draw_text(
                hdc,
                label,
                r,
                font,
                text_col,
                DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0,
            );
        }
    }

    unsafe fn draw_checkbox(&self, hdc: HDC, row: RECT, checked: bool, label: &str, id: HitId) {
        let hovered = self.hovered == Some(id);
        let pal = self.pal;
        let dpi = self.dpi;
        let box_sz = sc(16, dpi);
        let box_y = row.top + ((row.bottom - row.top) - box_sz) / 2;
        let box_rect = RECT {
            left: row.left,
            top: box_y,
            right: row.left + box_sz,
            bottom: box_y + box_sz,
        };
        let (fill, border) = if checked {
            let f = if hovered { pal.accent_hover } else { pal.accent };
            (f, f)
        } else {
            let f = if hovered { pal.btn_hover } else { pal.input_bg };
            (f, if hovered { pal.accent } else { pal.border })
        };
        unsafe {
            draw_round_box(hdc, box_rect, sc(4, dpi), fill, border);
            if checked {
                let pen = CreatePen(PS_SOLID, sc(2, dpi).max(2), pal.accent_text);
                let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
                let pts = [
                    POINT {
                        x: box_rect.left + sc(3, dpi),
                        y: box_rect.top + sc(8, dpi),
                    },
                    POINT {
                        x: box_rect.left + sc(6, dpi),
                        y: box_rect.top + sc(11, dpi),
                    },
                    POINT {
                        x: box_rect.left + sc(12, dpi),
                        y: box_rect.top + sc(5, dpi),
                    },
                ];
                let _ = Polyline(hdc, &pts);
                SelectObject(hdc, old_pen);
                let _ = DeleteObject(HGDIOBJ(pen.0));
            }
            let text_rect = RECT {
                left: box_rect.right + sc(9, dpi),
                top: row.top,
                right: row.right,
                bottom: row.bottom,
            };
            draw_text(
                hdc,
                label,
                text_rect,
                self.res.font_body,
                pal.text_pri,
                DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0 | DT_NOPREFIX.0,
            );
        }
    }
}

unsafe fn draw_round_box(hdc: HDC, r: RECT, radius: i32, fill: COLORREF, border: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(fill);
        let pen = CreatePen(PS_SOLID, 1, border);
        let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
        let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
        let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(HGDIOBJ(brush.0));
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

unsafe fn draw_text(
    hdc: HDC,
    text: &str,
    mut r: RECT,
    font: HFONT,
    color: COLORREF,
    flags: u32,
) {
    unsafe {
        let old_font = SelectObject(hdc, HGDIOBJ(font.0));
        SetTextColor(hdc, color);
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let _ = DrawTextW(
            hdc,
            &mut wide,
            &mut r,
            windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(flags),
        );
        SelectObject(hdc, old_font);
    }
}

fn loword(v: usize) -> u16 {
    (v & 0xFFFF) as u16
}

fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xFFFF) as u16
}

fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    let x = (lp.0 & 0xFFFF) as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

unsafe fn get_data<'a>(hwnd: HWND) -> Option<&'a mut WindowData> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowData;
    unsafe { ptr.as_mut() }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => unsafe {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        },
        WM_CREATE => unsafe {
            if let Some(data) = get_data(hwnd) {
                let dpi = GetDpiForWindow(hwnd);
                if dpi > 0 && dpi != data.dpi {
                    data.dpi = dpi;
                    data.res = GdiResources::new(dpi, data.pal);
                }
                if data.state.dark {
                    let dark_flag: i32 = 1;
                    let _ = DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_USE_IMMERSIVE_DARK_MODE,
                        (&dark_flag as *const i32).cast::<c_void>(),
                        size_of::<i32>() as u32,
                    );
                }
                let hinstance = GetModuleHandleW(None).unwrap_or_default();
                let lay = data.layout();
                let r = lay.edit_child;
                let init_text = HSTRING::from(&data.state.install_dir_text);
                if let Ok(edit) = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("EDIT"),
                    &init_text,
                    WINDOW_STYLE(WS_CHILD.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    Some(hwnd),
                    Some(HMENU(ID_EDIT_DIR as *mut c_void)),
                    Some(HINSTANCE(hinstance.0)),
                    None,
                ) {
                    data.edit_hwnd = edit;
                    data.sync_edit_control();
                }
                if data.state.auto_run_pending {
                    data.state.auto_run_pending = false;
                    let _ = PostMessageW(Some(hwnd), WM_APP_AUTO_RUN, WPARAM(0), LPARAM(0));
                }
            }
            LRESULT(0)
        },
        WM_APP_AUTO_RUN => unsafe {
            if let Some(data) = get_data(hwnd) {
                if data.state.run_primary_action() == ActionOutcome::CloseWindow {
                    let _ = DestroyWindow(hwnd);
                } else {
                    data.sync_edit_control();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        },
        WM_DPICHANGED => unsafe {
            if let Some(data) = get_data(hwnd) {
                let new_dpi = hiword(wparam.0) as u32;
                if new_dpi > 0 {
                    data.dpi = new_dpi;
                    data.res = GdiResources::new(new_dpi, data.pal);
                    let suggested = &*(lparam.0 as *const RECT);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        suggested.left,
                        suggested.top,
                        suggested.right - suggested.left,
                        suggested.bottom - suggested.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    data.sync_edit_control();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        },
        WM_CTLCOLOREDIT => unsafe {
            if let Some(data) = get_data(hwnd) {
                let hdc = HDC(wparam.0 as *mut c_void);
                SetTextColor(hdc, data.pal.text_pri);
                SetBkColor(hdc, data.pal.input_bg);
                return LRESULT(data.res.edit_brush.0 as isize);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        },
        WM_COMMAND => unsafe {
            if loword(wparam.0) as usize == ID_EDIT_DIR
                && hiword(wparam.0) as u32 == EN_CHANGE
                && let Some(data) = get_data(hwnd)
            {
                data.read_edit_text();
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        },
        WM_MOUSEMOVE => unsafe {
            if let Some(data) = get_data(hwnd) {
                if !data.tracking_mouse {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                    data.tracking_mouse = true;
                }
                let (x, y) = lparam_xy(lparam);
                let hit = data.layout().hit_test(data.state.mode, data.state.status, x, y);
                if hit != data.hovered {
                    data.hovered = hit;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        },
        WM_MOUSELEAVE => unsafe {
            if let Some(data) = get_data(hwnd) {
                data.tracking_mouse = false;
                if data.hovered.is_some() {
                    data.hovered = None;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        },
        WM_SETCURSOR => unsafe {
            if loword(lparam.0 as usize) == 1
                && let Some(data) = get_data(hwnd)
            {
                let cursor_id = if data.hovered.is_some() {
                    IDC_HAND
                } else {
                    IDC_ARROW
                };
                if let Ok(cur) = LoadCursorW(None, cursor_id) {
                    SetCursor(Some(cur));
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        },
        WM_LBUTTONDOWN => unsafe {
            if let Some(data) = get_data(hwnd) {
                let (x, y) = lparam_xy(lparam);
                let hit = data.layout().hit_test(data.state.mode, data.state.status, x, y);
                data.pressed = hit;
                if hit.is_some() {
                    SetCapture(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        },
        WM_LBUTTONUP => unsafe {
            if let Some(data) = get_data(hwnd) {
                let prev_pressed = data.pressed.take();
                if prev_pressed.is_some() {
                    let _ = ReleaseCapture();
                    let (x, y) = lparam_xy(lparam);
                    let hit = data.layout().hit_test(data.state.mode, data.state.status, x, y);
                    if hit == prev_pressed
                        && let Some(h) = hit
                    {
                        data.handle_click(hwnd, h);
                        return LRESULT(0);
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        },
        WM_KEYDOWN => unsafe {
            let vk = wparam.0 as u16;
            if let Some(data) = get_data(hwnd) {
                if vk == VK_ESCAPE.0 {
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }
                if vk == VK_RETURN.0 {
                    let target = if data.state.status == Status::Done {
                        HitId::Finish
                    } else {
                        HitId::Primary
                    };
                    data.handle_click(hwnd, target);
                    return LRESULT(0);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        },
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => unsafe {
            if let Some(data) = get_data(hwnd) {
                data.paint(hwnd);
            }
            LRESULT(0)
        },
        WM_DESTROY => unsafe {
            PostQuitMessage(0);
            LRESULT(0)
        },
        WM_NCDESTROY => unsafe {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowData;
            if !ptr.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub fn run(state: AppState) -> Result<(), String> {
    unsafe {
        let hmodule = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
        let hinstance = HINSTANCE(hmodule.0);
        let class_name = w!("LenovoToolkitSetupWindow");

        let icon = LoadIconW(Some(hinstance), PCWSTR(std::ptr::without_provenance(1)))
            .unwrap_or_default();
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            hIcon: icon,
            hCursor: cursor,
            lpszClassName: class_name,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err("RegisterClassW failed".into());
        }

        let init_dpi = 96u32;
        let logical_h = match state.mode {
            Mode::Install => LOGICAL_H_INSTALL,
            Mode::Uninstall => LOGICAL_H_UNINSTALL,
        };
        let mut wr = RECT {
            left: 0,
            top: 0,
            right: sc(LOGICAL_W, init_dpi),
            bottom: sc(logical_h, init_dpi),
        };
        let _ = AdjustWindowRectExForDpi(&mut wr, WINDOW_STYLE_FLAGS, false, WINDOW_EX_STYLE(0), init_dpi);
        let win_w = wr.right - wr.left;
        let win_h = wr.bottom - wr.top;

        let mut work = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&mut work as *mut RECT).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let x = work.left + ((work.right - work.left) - win_w) / 2;
        let y = work.top + ((work.bottom - work.top) - win_h) / 2;

        let title = HSTRING::from(format!("{APP_NAME} v{APP_VERSION} Setup"));
        let data = Box::new(WindowData::new(state, init_dpi));
        let data_ptr = Box::into_raw(data);

        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            &title,
            WINDOW_STYLE_FLAGS,
            x,
            y,
            win_w,
            win_h,
            None,
            None,
            Some(hinstance),
            Some(data_ptr.cast()),
        ) {
            Ok(h) => h,
            Err(e) => {
                drop(Box::from_raw(data_ptr));
                return Err(format!("CreateWindowExW failed: {e}"));
            }
        };

        // Re-center with real monitor DPI (in case primary monitor != 96 DPI)
        let real_dpi = GetDpiForWindow(hwnd);
        if real_dpi > 0 && real_dpi != init_dpi {
            let mut wr2 = RECT {
                left: 0,
                top: 0,
                right: sc(LOGICAL_W, real_dpi),
                bottom: sc(logical_h, real_dpi),
            };
            let _ = AdjustWindowRectExForDpi(
                &mut wr2,
                WINDOW_STYLE_FLAGS,
                false,
                WINDOW_EX_STYLE(0),
                real_dpi,
            );
            let rw = wr2.right - wr2.left;
            let rh = wr2.bottom - wr2.top;
            let rx = work.left + ((work.right - work.left) - rw) / 2;
            let ry = work.top + ((work.bottom - work.top) - rh) / 2;
            let _ = SetWindowPos(hwnd, None, rx, ry, rw, rh, SWP_NOZORDER | SWP_NOACTIVATE);
            if let Some(d) = get_data(hwnd) {
                d.dpi = real_dpi;
                d.res = GdiResources::new(real_dpi, d.pal);
                d.sync_edit_control();
            }
        }

        let _ = ShowWindow(hwnd, SW_SHOW);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
