//! Biểu tượng System Tray nhẹ bằng Win32 thuần (`Shell_NotifyIconW`) và giải phóng working set RAM
//! (`EmptyWorkingSet`) khi cửa sổ ẩn xuống khay hệ thống.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;

use lenovo_toolkit_core::power::{self, PowerMode};
use lenovo_toolkit_core::settings::{self, Lang};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen, CreateRoundRectRgn, CreateSolidBrush,
    SetWindowRgn, ExtCreatePen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, Ellipse, Arc, MoveToEx, LineTo,
    GetStockObject, HALFTONE, HDC, LOGBRUSH, BS_SOLID, NULL_BRUSH, PS_ENDCAP_ROUND, PS_GEOMETRIC, PS_JOIN_ROUND,
    PS_SOLID, SRCCOPY, SetBrushOrgEx, SetStretchBltMode, StretchBlt,
    DEFAULT_CHARSET, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint,
    FillRect, FrameRect, GetDC, GetDeviceCaps, GetTextExtentPoint32W, HGDIOBJ, LOGPIXELSY,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_NULL, ReleaseDC, RoundRect, SelectObject, SetBkMode, SetTextColor,
    TRANSPARENT, TextOutW,
};
use crate::icons::{self, IconKind};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_SELECTED};
use windows::Win32::UI::Shell::{
    QUERY_USER_NOTIFICATION_STATE, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
    SHQueryUserNotificationState,
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NIN_POPUPCLOSE, NIN_POPUPOPEN,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetClientRect, GetCursorPos, GetMessageW, GetSystemMetrics, HICON,
    IDI_APPLICATION, IMAGE_ICON, KillTimer, LR_SHARED, LoadIconW, LoadImageW,
    MF_DISABLED, MF_OWNERDRAW, MSG, PostMessageW, RegisterClassW, RegisterWindowMessageW, SM_CXSCREEN,
    SM_CXSMICON, SM_CYSCREEN, SM_CYSMICON, SetForegroundWindow, SetTimer, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
    TPM_NONOTIFY, TPM_RETURNCMD, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_DESTROY, WM_DRAWITEM, WM_ERASEBKGND, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_MEASUREITEM, WM_NULL,
    WM_PAINT, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP, WS_VISIBLE,
};
use windows::core::{PCSTR, PCWSTR, w};

const WM_TRAYICON: u32 = WM_APP + 1;
/// Dữ liệu tooltip đã sẵn sàng (worker gửi về luồng khay).
const WM_TIP_READY: u32 = WM_APP + 2;
/// Thông báo phím đã sẵn sàng (hiện giữa đáy màn hình, tự tắt sau `TOAST_MS`).
const WM_TOAST_READY: u32 = WM_APP + 3;
const TRAY_UID: u32 = 1;
/// Tự đóng tooltip nếu sự kiện rời chuột bị lỡ.
const TIP_AUTOCLOSE_MS: u32 = 8000;
const TOAST_MS: u32 = 1800;

const ID_DASHBOARD: usize = 1001;
const ID_QUIET: usize = 1002;
const ID_BALANCE: usize = 1003;
const ID_PERF: usize = 1004;
const ID_BATTERY: usize = 1005;
const ID_SETTINGS: usize = 1006;
const ID_QUIT: usize = 1009;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    Open,
    /// Mở cửa sổ ở trang chỉ định (0 Dashboard, 1 Battery, 2 Settings) — các mục điều hướng của menu.
    OpenPage(i32),
    SetPower(PowerMode),
    /// Rê chuột vào biểu tượng: cần số liệu mới cho tooltip (trả lời bằng `show_tooltip`).
    Hover,
    Quit,
}

static CALLBACK: OnceLock<Box<dyn Fn(TrayEvent) + Send + Sync>> = OnceLock::new();
static TASKBAR_CREATED_MSG: OnceLock<u32> = OnceLock::new();
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
static TIP_HWND: AtomicIsize = AtomicIsize::new(0);
/// Chuột đang đặt trên biểu tượng (để bỏ qua dữ liệu tooltip về trễ sau khi đã rời chuột).
static HOVERING: AtomicBool = AtomicBool::new(false);
static TIP_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Icon (loại, đang bật?) của thông báo phím; `None` khi cửa sổ đang là tooltip.
static TOAST_ICON: Mutex<Option<(IconKind, bool)>> = Mutex::new(None);
const TOAST_BG: u32 = 0x0020_2020;
const ICON_ON: u32 = 0x00FF_B04A;
const ICON_OFF: u32 = 0x0090_9090;

/// Một thông báo phím: icon, nội dung và trạng thái (tắt thì icon mờ).
pub struct Toast {
    /// Tên cờ cài đặt quyết định loại thông báo này có được hiện không.
    pub flag: &'static str,
    pub icon: IconKind,
    pub text: String,
    pub on: bool,
}

const TIP_CLASS: PCWSTR = w!("LenovoToolkitTrayTip");

/// Gửi nội dung tooltip (dòng đầu là tiêu đề) cho luồng khay để hiện cạnh con trỏ.
pub fn show_tooltip(lines: Vec<String>) {
    if let Ok(mut g) = TOAST_ICON.lock() {
        *g = None;
    }
    if let Ok(mut g) = TIP_LINES.lock() {
        *g = lines;
    }
    let h = TRAY_HWND.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut c_void)), WM_TIP_READY, WPARAM(0), LPARAM(0));
        }
    }
}

/// Game toàn màn hình, trình chiếu hoặc "không làm phiền" thì không chen thông báo (như bản gốc).
fn suppress_toast(state: QUERY_USER_NOTIFICATION_STATE) -> bool {
    matches!(state, QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)
}

/// Hiện một dòng thông báo giữa đáy màn hình rồi tự tắt.
pub fn show_toast(toast: Toast) {
    if unsafe { SHQueryUserNotificationState() }.is_ok_and(suppress_toast) {
        return;
    }
    if let Ok(mut g) = TOAST_ICON.lock() {
        *g = Some((toast.icon, toast.on));
    }
    if let Ok(mut g) = TIP_LINES.lock() {
        *g = vec![toast.text];
    }
    let h = TRAY_HWND.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut c_void)), WM_TOAST_READY, WPARAM(0), LPARAM(0));
        }
    }
}

fn tip_lines() -> Vec<String> {
    TIP_LINES.lock().map(|l| l.clone()).unwrap_or_default()
}

struct TipLayout {
    width: i32,
    height: i32,
    line_h: i32,
    pad: i32,
    font_h: i32,
    /// Chỗ dành cho icon bên trái (kể cả khoảng cách tới chữ); 0 với tooltip.
    icon_w: i32,
    /// Cạnh ô vuông chứa icon.
    icon_size: i32,
    /// Độ đậm của chữ: thông báo phím in đậm, tooltip chữ thường.
    weight: i32,
}

fn toast_icon() -> Option<(IconKind, bool)> {
    TOAST_ICON.lock().ok().and_then(|g| *g)
}

unsafe fn make_font(font_h: i32, face: PCWSTR) -> windows::Win32::Graphics::Gdi::HFONT {
    unsafe { make_font_w(font_h, face, 400) }
}

unsafe fn make_font_w(font_h: i32, face: PCWSTR, weight: i32) -> windows::Win32::Graphics::Gdi::HFONT {
    unsafe {
        CreateFontW(
            font_h,
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
            0,
            face,
        )
    }
}

/// Đo kích thước cửa sổ tooltip theo DPI và nội dung.
unsafe fn tip_layout(lines: &[String], icon: bool) -> TipLayout {
    unsafe {
        let hdc = GetDC(None);
        let dpi = GetDeviceCaps(Some(hdc), LOGPIXELSY).max(96);
        let font_h = -(15 * dpi / 96);
        let weight = if icon { 700 } else { 400 };
        let font = make_font_w(font_h, w!("Segoe UI"), weight);
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let (mut max_w, mut line_h) = (0, 0);
        for l in lines {
            let text: Vec<u16> = l.encode_utf16().collect();
            let mut size = SIZE::default();
            let _ = GetTextExtentPoint32W(hdc, &text, &mut size);
            max_w = max_w.max(size.cx);
            line_h = line_h.max(size.cy);
        }
        SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
        ReleaseDC(None, hdc);
        let (pad, gap) = (12 * dpi / 96, 4 * dpi / 96);
        let icon_size = if icon { 28 * dpi / 96 } else { 0 };
        let icon_w = if icon { icon_size + 12 * dpi / 96 } else { 0 };
        TipLayout {
            weight,
            icon_w,
            icon_size,
            width: max_w + 2 * pad + icon_w,
            height: (lines.len() as i32 * (line_h + gap) - gap).max(icon_size) + 2 * pad,
            line_h: line_h + gap,
            pad,
            font_h,
        }
    }
}

unsafe fn hide_tip() {
    let h = TIP_HWND.swap(0, Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = DestroyWindow(HWND(h as *mut c_void));
        }
    }
}

/// Hiện tooltip cạnh con trỏ (trên con trỏ nếu thanh tác vụ ở nửa dưới màn hình, ngược lại bên dưới);
/// `toast` thì đặt giữa đáy màn hình và tự tắt nhanh.
unsafe fn show_tip_window(toast: bool) {
    unsafe {
        hide_tip();
        let lines = tip_lines();
        if lines.is_empty() {
            return;
        }
        let l = tip_layout(&lines, toast_icon().is_some());
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let (cx, ty) = if toast {
            (sw / 2, sh * 4 / 5)
        } else {
            (pt.x, if pt.y > sh / 2 { pt.y - l.height - 16 } else { pt.y + 24 })
        };
        let x = (cx - l.width / 2).clamp(4, (sw - l.width - 4).max(4));
        let y = ty.clamp(4, (sh - l.height - 4).max(4));
        let hinstance = HINSTANCE(GetModuleHandleW(PCWSTR::null()).unwrap_or_default().0);
        if let Ok(h) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            TIP_CLASS,
            w!(""),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            l.width,
            l.height,
            None,
            None,
            Some(hinstance),
            None,
        ) {
            if toast {
                // Bo góc nhẹ (bán kính 8 px theo DPI); cửa sổ sở hữu vùng cắt sau khi gọi.
                let r = 2 * 8 * screen_dpi() / 96;
                let _ = SetWindowRgn(h, Some(CreateRoundRectRgn(0, 0, l.width + 1, l.height + 1, r, r)), true);
            }
            TIP_HWND.store(h.0 as isize, Ordering::Relaxed);
            SetTimer(Some(h), 1, if toast { TOAST_MS } else { TIP_AUTOCLOSE_MS }, None);
        }
    }
}

/// Vẽ icon tự thiết kế (xem `icons.rs`): dựng ở độ phân giải gấp 4 trong bitmap bộ nhớ rồi thu nhỏ
/// bằng HALFTONE để cạnh mịn mà không cần thư viện đồ họa nào; nền toast đặc nên trộn màu không cần alpha.
unsafe fn draw_icon(hdc: HDC, kind: IconKind, on: bool, x: i32, y: i32, size: i32) {
    use icons::Shape;
    unsafe {
        const SS: i32 = 4;
        let big = size * SS;
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, big, big);
        let old_bmp = SelectObject(mem, HGDIOBJ(bmp.0));

        let bg = CreateSolidBrush(COLORREF(TOAST_BG));
        FillRect(mem, &RECT { left: 0, top: 0, right: big, bottom: big }, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        let k = big as f32 / icons::GRID;
        let color = COLORREF(if on { ICON_ON } else { ICON_OFF });
        let logbrush = LOGBRUSH { lbStyle: BS_SOLID, lbColor: color, lbHatch: 0 };
        let pen = ExtCreatePen(
            PS_GEOMETRIC | PS_SOLID | PS_ENDCAP_ROUND | PS_JOIN_ROUND,
            (icons::STROKE * k).round() as u32,
            &logbrush,
            None,
        );
        let solid = CreateSolidBrush(color);
        let hollow = GetStockObject(NULL_BRUSH);
        let old_pen = SelectObject(mem, HGDIOBJ(pen.0));
        let p = |v: f32| (v * k).round() as i32;

        for shape in icons::shapes(kind, !on) {
            match shape {
                Shape::Line(a, b, c, d) => {
                    let _ = MoveToEx(mem, p(a), p(b), None);
                    let _ = LineTo(mem, p(c), p(d));
                }
                Shape::RoundRect(l, t, r, b, rad) => {
                    SelectObject(mem, hollow);
                    let _ = RoundRect(mem, p(l), p(t), p(r), p(b), p(rad * 2.0), p(rad * 2.0));
                }
                Shape::Ellipse(l, t, r, b) => {
                    SelectObject(mem, hollow);
                    let _ = Ellipse(mem, p(l), p(t), p(r), p(b));
                }
                Shape::Arc(l, t, r, b, start, end) => {
                    let (cx, cy, rx, ry) = ((l + r) / 2.0, (t + b) / 2.0, (r - l) / 2.0, (b - t) / 2.0);
                    // Góc ngược chiều kim đồng hồ, trục y của màn hình hướng xuống.
                    let at = |deg: f32| {
                        let a = deg.to_radians();
                        (p(cx + rx * a.cos()), p(cy - ry * a.sin()))
                    };
                    let ((sx, sy), (ex, ey)) = (at(start), at(end));
                    let _ = Arc(mem, p(l), p(t), p(r), p(b), sx, sy, ex, ey);
                }
                Shape::Dot(cx, cy, r) => {
                    SelectObject(mem, HGDIOBJ(solid.0));
                    let _ = Ellipse(mem, p(cx - r), p(cy - r), p(cx + r), p(cy + r));
                }
            }
        }

        SetStretchBltMode(hdc, HALFTONE);
        let _ = SetBrushOrgEx(hdc, 0, 0, None);
        let _ = StretchBlt(hdc, x, y, size, size, Some(mem), 0, 0, big, big, SRCCOPY);

        SelectObject(mem, old_pen);
        SelectObject(mem, old_bmp);
        let _ = DeleteObject(HGDIOBJ(pen.0));
        let _ = DeleteObject(HGDIOBJ(solid.0));
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
    }
}

unsafe fn paint_tip(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let lines = tip_lines();
        let icon = toast_icon();
        let l = tip_layout(&lines, icon.is_some());
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);

        let bg = CreateSolidBrush(COLORREF(0x0020_2020));
        FillRect(hdc, &rc, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));
        // Khung viền chỉ cho tooltip vuông; thông báo phím bo góc nên không viền (GDI không khử răng cưa).
        if icon.is_none() {
            let border = CreateSolidBrush(COLORREF(0x0050_5050));
            FrameRect(hdc, &rc, border);
            let _ = DeleteObject(HGDIOBJ(border.0));
        }

        let font = make_font_w(l.font_h, w!("Segoe UI"), l.weight);
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        SetBkMode(hdc, TRANSPARENT);
        // Thông báo phím: chữ căn giữa theo chiều dọc, icon bên trái (xanh khi bật, xám khi tắt).
        let block = lines.len() as i32 * l.line_h;
        let text_y = if icon.is_some() { ((l.height - block) / 2).max(l.pad / 2) } else { l.pad };
        for (i, line) in lines.iter().enumerate() {
            SetTextColor(hdc, COLORREF(if i == 0 { 0x00FF_FFFF } else { 0x00DC_DCDC }));
            let text: Vec<u16> = line.encode_utf16().collect();
            let _ = TextOutW(hdc, l.pad + l.icon_w, text_y + i as i32 * l.line_h, &text);
        }
        if let Some((kind, on)) = icon {
            draw_icon(hdc, kind, on, l.pad, (l.height - l.icon_size) / 2, l.icon_size);
        }
        SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn tip_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                paint_tip(hwnd);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_TIMER => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = KillTimer(Some(hwnd), 1);
                let _ = TIP_HWND.compare_exchange(hwnd.0 as isize, 0, Ordering::Relaxed, Ordering::Relaxed);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Thu hồi bộ nhớ đệm của tiến trình khi ẩn cửa sổ xuống System Tray để chống phình RAM.
pub fn trim_working_set() {
    unsafe {
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}

fn emit(ev: TrayEvent) {
    if let Some(cb) = CALLBACK.get() {
        cb(ev);
    }
}

unsafe fn add_tray_icon(hwnd: HWND) {
    unsafe {
        let hmodule = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let hinstance = HINSTANCE(hmodule.0);
        // Nạp đúng kích thước small-icon theo DPI màn hình (SM_CXSMICON x SM_CYSMICON) từ resource ID 1
        let cx = GetSystemMetrics(SM_CXSMICON);
        let cy = GetSystemMetrics(SM_CYSMICON);
        let icon = LoadImageW(
            Some(hinstance),
            PCWSTR(1usize as *const u16),
            IMAGE_ICON,
            cx,
            cy,
            LR_SHARED,
        )
        .map(|h| HICON(h.0))
        .or_else(|_| LoadIconW(Some(hinstance), PCWSTR(1usize as *const u16)))
        .or_else(|_| LoadIconW(None, IDI_APPLICATION))
        .unwrap_or_default();

        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_UID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAYICON,
            hIcon: icon,
            ..Default::default()
        };
        let tip = crate::app_title();
        for (dst, src) in nid.szTip.iter_mut().take(127).zip(tip.encode_utf16()) {
            *dst = src;
        }
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        // Phiên bản 4: nhận NIN_POPUPOPEN/NIN_POPUPCLOSE khi rê chuột (tooltip tự vẽ). Không đặt NIF_SHOWTIP
        // nên Windows không hiện tooltip chuẩn chồng lên tooltip của app.
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    unsafe {
        let nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_UID,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

// ---- Menu chuột phải tự vẽ (owner-draw): nền tối, icon Fluent, hover bo góc — gần với menu của bản gốc ----

#[derive(Clone)]
struct MenuEntry {
    id: usize,
    /// Glyph của font "Segoe Fluent Icons" ("" = không icon).
    glyph: &'static str,
    label: &'static str,
    separator: bool,
}

static MENU: Mutex<Vec<MenuEntry>> = Mutex::new(Vec::new());

const CHECK_GLYPH: &str = "\u{E73E}";
// COLORREF là 0x00BBGGRR.
const MENU_BG: u32 = 0x002B_2B2B;
const MENU_HOVER: u32 = 0x003D_3D3D;
const MENU_TEXT: u32 = 0x00FF_FFFF;
const MENU_SEP: u32 = 0x004A_4A4A;
const MENU_ACCENT: u32 = 0x00FF_C24C; // #4CC2FF

/// Các mục menu: điều hướng như bản gốc, chế độ nguồn (dấu ✓ ở chế độ hiện tại, bỏ nếu máy không đọc được), thoát.
fn build_menu_entries(vi: bool, power: Option<PowerMode>) -> Vec<MenuEntry> {
    let t = |en: &'static str, vi_text: &'static str| if vi { vi_text } else { en };
    let item = |id, glyph, label| MenuEntry { id, glyph, label, separator: false };
    let sep = || MenuEntry { id: 0, glyph: "", label: "", separator: true };

    let mut v = vec![
        item(ID_DASHBOARD, "\u{E80F}", t("Dashboard", "Trang chủ")),
        item(ID_BATTERY, "\u{E83F}", t("Battery", "Pin")),
        item(ID_SETTINGS, "\u{E713}", t("Settings", "Cài đặt")),
        sep(),
    ];
    if let Some(cur) = power {
        let mode = |id, m: PowerMode, label| item(id, if cur == m { CHECK_GLYPH } else { "" }, label);
        v.push(mode(ID_QUIET, PowerMode::Quiet, t("Power: Quiet", "Nguồn: Yên tĩnh")));
        v.push(mode(ID_BALANCE, PowerMode::Balance, t("Power: Balance", "Nguồn: Cân bằng")));
        v.push(mode(ID_PERF, PowerMode::Performance, t("Power: Performance", "Nguồn: Hiệu năng")));
        v.push(sep());
    }
    v.push(item(ID_QUIT, "\u{E711}", t("Quit", "Thoát")));
    v
}

fn menu_entry(item_data: usize) -> Option<MenuEntry> {
    MENU.lock().ok()?.get(item_data.checked_sub(1)?).cloned()
}

unsafe fn screen_dpi() -> i32 {
    unsafe {
        let hdc = GetDC(None);
        let dpi = GetDeviceCaps(Some(hdc), LOGPIXELSY).max(96);
        ReleaseDC(None, hdc);
        dpi
    }
}

unsafe fn measure_menu_item(mi: &mut MEASUREITEMSTRUCT) {
    unsafe {
        let Some(e) = menu_entry(mi.itemData) else { return };
        let dpi = screen_dpi();
        let s = |v: i32| v * dpi / 96;
        if e.separator {
            mi.itemWidth = s(160) as u32;
            mi.itemHeight = s(9) as u32;
            return;
        }
        let hdc = GetDC(None);
        let font = make_font(-s(15), w!("Segoe UI"));
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let text: Vec<u16> = e.label.encode_utf16().collect();
        let mut size = SIZE::default();
        let _ = GetTextExtentPoint32W(hdc, &text, &mut size);
        SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
        ReleaseDC(None, hdc);
        mi.itemWidth = (s(16) + s(30) + size.cx + s(28)) as u32;
        mi.itemHeight = s(34) as u32;
    }
}

unsafe fn draw_menu_item(d: &DRAWITEMSTRUCT) {
    unsafe {
        let Some(e) = menu_entry(d.itemData) else { return };
        let (hdc, rc) = (d.hDC, d.rcItem);
        let dpi = screen_dpi();
        let s = |v: i32| v * dpi / 96;

        let bg = CreateSolidBrush(COLORREF(MENU_BG));
        FillRect(hdc, &rc, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        if e.separator {
            let y = rc.top + (rc.bottom - rc.top) / 2;
            let line = RECT { left: rc.left + s(12), top: y, right: rc.right - s(12), bottom: y + (dpi / 96).max(1) };
            let b = CreateSolidBrush(COLORREF(MENU_SEP));
            FillRect(hdc, &line, b);
            let _ = DeleteObject(HGDIOBJ(b.0));
            return;
        }

        if d.itemState.0 & ODS_SELECTED.0 != 0 {
            let brush = CreateSolidBrush(COLORREF(MENU_HOVER));
            let pen = CreatePen(PS_NULL, 0, COLORREF(0));
            let (ob, op) = (SelectObject(hdc, HGDIOBJ(brush.0)), SelectObject(hdc, HGDIOBJ(pen.0)));
            let _ = RoundRect(hdc, rc.left + s(4), rc.top + s(2), rc.right - s(4), rc.bottom - s(2), s(8), s(8));
            SelectObject(hdc, ob);
            SelectObject(hdc, op);
            let _ = DeleteObject(HGDIOBJ(brush.0));
            let _ = DeleteObject(HGDIOBJ(pen.0));
        }

        SetBkMode(hdc, TRANSPARENT);
        let flags = DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX;

        if !e.glyph.is_empty() {
            let font = make_font(-s(16), w!("Segoe Fluent Icons"));
            let old = SelectObject(hdc, HGDIOBJ(font.0));
            SetTextColor(hdc, COLORREF(if e.glyph == CHECK_GLYPH { MENU_ACCENT } else { MENU_TEXT }));
            let mut r = RECT { left: rc.left + s(16), top: rc.top, right: rc.left + s(16) + s(24), bottom: rc.bottom };
            let mut glyph: Vec<u16> = e.glyph.encode_utf16().collect();
            DrawTextW(hdc, &mut glyph, &mut r, flags);
            SelectObject(hdc, old);
            let _ = DeleteObject(HGDIOBJ(font.0));
        }

        let font = make_font(-s(15), w!("Segoe UI"));
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        SetTextColor(hdc, COLORREF(MENU_TEXT));
        let mut r = RECT { left: rc.left + s(46), top: rc.top, right: rc.right - s(8), bottom: rc.bottom };
        let mut text: Vec<u16> = e.label.encode_utf16().collect();
        DrawTextW(hdc, &mut text, &mut r, flags);
        SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
    }
}

/// Bật chế độ tối cho khung menu hệ thống (ordinal 135 = SetPreferredAppMode, 136 = FlushMenuThemes của
/// uxtheme.dll; chưa có tài liệu chính thức). Không có thì bỏ qua, menu vẫn tự vẽ nền tối cho từng mục.
unsafe fn enable_dark_menus() {
    unsafe {
        let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(f) = GetProcAddress(ux, PCSTR(135usize as *const u8)) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_mode(2); // ForceDark
        }
        if let Some(f) = GetProcAddress(ux, PCSTR(136usize as *const u8)) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

unsafe fn show_context_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let vi = settings::lang() == Lang::Vi;
        let entries = build_menu_entries(vi, power::get().ok());
        if let Ok(mut g) = MENU.lock() {
            *g = entries.clone();
        }
        for (i, e) in entries.iter().enumerate() {
            // itemData = chỉ số + 1 để WM_MEASUREITEM/WM_DRAWITEM tra lại mục; dòng kẻ bị vô hiệu hóa nên không chọn được.
            let flags = if e.separator { MF_OWNERDRAW | MF_DISABLED } else { MF_OWNERDRAW };
            let _ = AppendMenuW(menu, flags, e.id, PCWSTR((i + 1) as *const u16));
        }

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD | TPM_NONOTIFY,
            pt.x,
            pt.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);

        match cmd.0 as usize {
            ID_DASHBOARD => emit(TrayEvent::OpenPage(0)),
            ID_BATTERY => emit(TrayEvent::OpenPage(1)),
            ID_SETTINGS => emit(TrayEvent::OpenPage(2)),
            ID_QUIET => emit(TrayEvent::SetPower(PowerMode::Quiet)),
            ID_BALANCE => emit(TrayEvent::SetPower(PowerMode::Balance)),
            ID_PERF => emit(TrayEvent::SetPower(PowerMode::Performance)),
            ID_QUIT => {
                remove_tray_icon(hwnd);
                emit(TrayEvent::Quit);
            }
            _ => {}
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_TRAYICON {
            let mouse_msg = (lparam.0 as u32) & 0xFFFF;
            match mouse_msg {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                    HOVERING.store(false, Ordering::Relaxed);
                    hide_tip();
                    emit(TrayEvent::Open);
                }
                WM_RBUTTONUP => {
                    HOVERING.store(false, Ordering::Relaxed);
                    hide_tip();
                    show_context_menu(hwnd);
                }
                NIN_POPUPOPEN => {
                    HOVERING.store(true, Ordering::Relaxed);
                    emit(TrayEvent::Hover);
                }
                NIN_POPUPCLOSE => {
                    HOVERING.store(false, Ordering::Relaxed);
                    hide_tip();
                }
                _ => {}
            }
            return LRESULT(0);
        }
        if msg == WM_MEASUREITEM {
            measure_menu_item(&mut *(lparam.0 as *mut MEASUREITEMSTRUCT));
            return LRESULT(1);
        }
        if msg == WM_DRAWITEM {
            draw_menu_item(&*(lparam.0 as *const DRAWITEMSTRUCT));
            return LRESULT(1);
        }
        if msg == WM_TIP_READY {
            if HOVERING.load(Ordering::Relaxed) {
                show_tip_window(false);
            }
            return LRESULT(0);
        }
        if msg == WM_TOAST_READY {
            show_tip_window(true);
            return LRESULT(0);
        }
        if TASKBAR_CREATED_MSG.get().is_some_and(|&m| m != 0 && msg == m) {
            add_tray_icon(hwnd);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_layout_grows_with_content() {
        let short = vec!["Lenovo Toolkit v1.0.1".to_string(), "CPU: 5%".to_string()];
        let long = vec![
            "Lenovo Toolkit v1.0.1".to_string(),
            "CPU:  100% · 99°C · 5200 RPM".to_string(),
            "GPU:  100% · 87°C · 5200 RPM".to_string(),
            "RAM:  15.7/15.7 GB (100%)".to_string(),
        ];
        let (a, b) = unsafe { (tip_layout(&short, false), tip_layout(&long, false)) };
        let with_icon = unsafe { tip_layout(&short, true) };
        assert!(with_icon.width > a.width && with_icon.icon_w > 0 && a.icon_w == 0, "icon chiếm thêm chỗ bên trái");
        assert!(a.width > 0 && a.height > 0);
        assert!(b.width > a.width, "tooltip dài hơn phải rộng hơn");
        assert!(b.height > a.height, "nhiều dòng hơn phải cao hơn");
        assert!(a.line_h > 0 && a.pad > 0 && a.font_h < 0);
    }

    #[test]
    fn menu_has_navigation_power_checkmark_and_quit() {
        let e = build_menu_entries(false, Some(PowerMode::Balance));
        let ids: Vec<usize> = e.iter().filter(|x| !x.separator).map(|x| x.id).collect();
        assert_eq!(ids, vec![ID_DASHBOARD, ID_BATTERY, ID_SETTINGS, ID_QUIET, ID_BALANCE, ID_PERF, ID_QUIT]);
        let checked: Vec<usize> = e.iter().filter(|x| x.glyph == CHECK_GLYPH).map(|x| x.id).collect();
        assert_eq!(checked, vec![ID_BALANCE]);

        // Máy không đọc được chế độ nguồn (không gaming): bỏ các mục đổi chế độ.
        let none = build_menu_entries(true, None);
        assert!(!none.iter().any(|x| x.id == ID_QUIET || x.id == ID_PERF));
        assert_eq!(none.last().map(|x| x.id), Some(ID_QUIT));

        // Dòng kẻ không đứng đầu, cuối hoặc liền nhau.
        for list in [&e, &none] {
            assert!(!list.first().unwrap().separator && !list.last().unwrap().separator);
            assert!(list.windows(2).all(|w| !(w[0].separator && w[1].separator)));
        }
        assert_eq!(build_menu_entries(true, None)[0].label, "Trang chủ");
    }

    #[test]
    fn toast_is_suppressed_for_fullscreen_games_and_presentations() {
        use windows::Win32::UI::Shell::{QUNS_ACCEPTS_NOTIFICATIONS, QUNS_NOT_PRESENT};
        assert!(suppress_toast(QUNS_RUNNING_D3D_FULL_SCREEN));
        assert!(suppress_toast(QUNS_BUSY));
        assert!(suppress_toast(QUNS_PRESENTATION_MODE));
        assert!(!suppress_toast(QUNS_ACCEPTS_NOTIFICATIONS));
        assert!(!suppress_toast(QUNS_NOT_PRESENT));
    }

    #[test]
    fn tooltip_text_roundtrips_through_shared_state() {
        show_tooltip(vec!["a".into(), "b".into()]); // chưa có cửa sổ khay: chỉ lưu nội dung
        assert_eq!(tip_lines(), vec!["a".to_string(), "b".to_string()]);
    }
}

pub fn spawn_tray(on_event: impl Fn(TrayEvent) + Send + Sync + 'static) {
    let _ = CALLBACK.set(Box::new(on_event));
    thread::spawn(|| unsafe {
        enable_dark_menus();
        let _ = TASKBAR_CREATED_MSG.set(RegisterWindowMessageW(w!("TaskbarCreated")));
        let hmodule = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let hinstance = HINSTANCE(hmodule.0);
        let class_name = w!("LegionRustTrayWnd");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let tip_class = WNDCLASSW {
            lpfnWndProc: Some(tip_wnd_proc),
            hInstance: hinstance,
            lpszClassName: TIP_CLASS,
            ..Default::default()
        };
        RegisterClassW(&tip_class);
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("LegionRustTray"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance),
            None,
        ) else {
            return;
        };
        TRAY_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        add_tray_icon(hwnd);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        remove_tray_icon(hwnd);
    });
}
