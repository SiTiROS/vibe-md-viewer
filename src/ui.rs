//! Home-made lightweight bars (drawn with GDI): the top bar with modes and
//! buttons, the notification banner and the status bar. No comctl32 — faster startup
//! and a proper dark theme.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::theme::Theme;
use crate::util::{get_x_lparam, get_y_lparam, wide};

pub const WM_BAR_HINT: u32 = WM_APP + 3;
const WM_MOUSELEAVE_: u32 = 0x02A3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Tab,
    Button,
    Sep,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    Normal,
    Bold,
    Italic,
    Strike,
    Mono,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: u16,
    pub label: String,
    pub kind: Kind,
    pub active: bool,
    pub right: bool,
    pub look: Look,
    pub hint: String,
}

impl Item {
    pub fn tab(id: u16, label: &str, active: bool, hint: &str) -> Item {
        Item { id, label: label.into(), kind: Kind::Tab, active, right: false, look: Look::Normal, hint: hint.into() }
    }
    pub fn button(id: u16, label: &str, hint: &str) -> Item {
        Item { id, label: label.into(), kind: Kind::Button, active: false, right: false, look: Look::Normal, hint: hint.into() }
    }
    pub fn sep() -> Item {
        Item { id: 0, label: String::new(), kind: Kind::Sep, active: false, right: false, look: Look::Normal, hint: String::new() }
    }
    pub fn look(mut self, l: Look) -> Item {
        self.look = l;
        self
    }
    pub fn right(mut self) -> Item {
        self.right = true;
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BarKind {
    Top,
    Info,
    Status,
}

struct Bar {
    kind: BarKind,
    items: Vec<Item>,
    rects: Vec<RECT>,
    hover: Option<usize>,
    pressed: Option<usize>,
    tracking: bool,
    left: String,
    right: String,
    theme: Theme,
    dpi: u32,
}

thread_local! {
    static BARS: RefCell<HashMap<isize, Bar>> = RefCell::new(HashMap::new());
    static FONTS: RefCell<HashMap<(u32, u8), HFONT>> = RefCell::new(HashMap::new());
}

const CLASS: &str = "VibeMdBar";

pub fn register_class() {
    let name = wide(CLASS);
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(bar_proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR::from_raw(name.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

pub fn create(parent: HWND, kind: BarKind, theme: Theme, dpi: u32) -> HWND {
    let name = wide(CLASS);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR::from_raw(name.as_ptr()),
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            10,
            10,
            Some(parent),
            None,
            GetModuleHandleW(None).ok().map(|h| h.into()),
            None,
        )
        .unwrap_or_default()
    };
    BARS.with(|b| {
        b.borrow_mut().insert(
            hwnd.0 as isize,
            Bar { kind, items: Vec::new(), rects: Vec::new(), hover: None, pressed: None, tracking: false, left: String::new(), right: String::new(), theme, dpi },
        )
    });
    hwnd
}

pub fn height(kind: BarKind, dpi: u32) -> i32 {
    let base = match kind {
        BarKind::Top => 38,
        BarKind::Info => 34,
        BarKind::Status => 24,
    };
    base * dpi as i32 / 96
}

fn with_bar<R>(hwnd: HWND, f: impl FnOnce(&mut Bar) -> R) -> Option<R> {
    BARS.with(|b| b.borrow_mut().get_mut(&(hwnd.0 as isize)).map(f))
}

pub fn set_items(hwnd: HWND, items: Vec<Item>) {
    with_bar(hwnd, |b| {
        b.items = items;
        b.rects.clear();
        b.hover = None;
        b.pressed = None;
    });
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

pub fn set_text(hwnd: HWND, left: &str, right: &str) {
    let changed = with_bar(hwnd, |b| {
        let c = b.left != left || b.right != right;
        b.left = left.to_string();
        b.right = right.to_string();
        c
    })
    .unwrap_or(false);
    if changed {
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

pub fn set_theme(hwnd: HWND, theme: Theme, dpi: u32) {
    with_bar(hwnd, |b| {
        b.theme = theme;
        b.dpi = dpi;
    });
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

fn font(dpi: u32, look: Look, size_pt: i32) -> HFONT {
    let key = (dpi * 1000 + size_pt as u32, look as u8);
    FONTS.with(|f| {
        *f.borrow_mut().entry(key).or_insert_with(|| {
            let face = wide(if look == Look::Mono { "Consolas" } else { "Segoe UI" });
            let h = -(size_pt * dpi as i32 / 72);
            unsafe {
                CreateFontW(
                    h,
                    0,
                    0,
                    0,
                    if look == Look::Bold { 700 } else { 400 },
                    (look == Look::Italic) as u32,
                    0,
                    (look == Look::Strike) as u32,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY,
                    0,
                    PCWSTR::from_raw(face.as_ptr()),
                )
            }
        })
    })
}

fn text_width(hdc: HDC, s: &str) -> i32 {
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut sz = SIZE::default();
    unsafe {
        let _ = GetTextExtentPoint32W(hdc, &w, &mut sz);
    }
    sz.cx
}

fn layout(b: &mut Bar, hdc: HDC, width: i32, height: i32) {
    let s = |v: i32| v * b.dpi as i32 / 96;
    b.rects = vec![RECT::default(); b.items.len()];
    let pad_x = s(10);
    let gap = s(2);
    let mut x = s(6);
    let mut xr = width - s(6);
    let (top, bottom) = (s(4), height - s(4));
    for (i, it) in b.items.iter().enumerate() {
        let w = match it.kind {
            Kind::Sep => s(9),
            _ => {
                unsafe {
                    SelectObject(hdc, font(b.dpi, it.look, 9).into());
                }
                text_width(hdc, &it.label) + pad_x * 2
            }
        };
        let w = if it.kind == Kind::Button && it.label.chars().count() <= 2 { w.max(s(30)) } else { w };
        if it.right {
            b.rects[i] = RECT { left: xr - w, top, right: xr, bottom };
            xr -= w + gap;
        } else {
            b.rects[i] = RECT { left: x, top, right: x + w, bottom };
            x += w + gap;
        }
    }
    // Narrow window: left items that overlap the right ones are hidden (neither drawn nor clickable).
    let limit = xr;
    for (i, it) in b.items.iter().enumerate() {
        if !it.right && b.rects[i].right > limit {
            b.rects[i] = RECT::default();
        }
    }
}

unsafe fn fill(hdc: HDC, r: &RECT, c: u32) {
    let br = CreateSolidBrush(COLORREF(c));
    FillRect(hdc, r, br);
    let _ = DeleteObject(br.into());
}

unsafe fn round(hdc: HDC, r: &RECT, c: u32, radius: i32) {
    let br = CreateSolidBrush(COLORREF(c));
    let pen = CreatePen(PS_SOLID, 1, COLORREF(c));
    let ob = SelectObject(hdc, br.into());
    let op = SelectObject(hdc, pen.into());
    let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
    SelectObject(hdc, ob);
    SelectObject(hdc, op);
    let _ = DeleteObject(br.into());
    let _ = DeleteObject(pen.into());
}

unsafe fn draw_text(hdc: HDC, s: &str, r: &mut RECT, color: u32, flags: DRAW_TEXT_FORMAT) {
    let mut w: Vec<u16> = s.encode_utf16().collect();
    SetTextColor(hdc, COLORREF(color));
    DrawTextW(hdc, &mut w, r, flags | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, w.max(1), h.max(1));
        let old_bmp = SelectObject(mem, bmp.into());
        SetBkMode(mem, TRANSPARENT);
        with_bar(hwnd, |b| {
            let t = b.theme;
            let dpi = b.dpi;
            let s = |v: i32| v * dpi as i32 / 96;
            let bg = match b.kind {
                BarKind::Info => t.info_bg,
                _ => t.bar_bg,
            };
            fill(mem, &rc, bg);
            // Separator line
            let line = match b.kind {
                BarKind::Status => RECT { left: 0, top: 0, right: w, bottom: 1 },
                _ => RECT { left: 0, top: h - 1, right: w, bottom: h },
            };
            fill(mem, &line, t.border);
            layout(b, mem, w, h);
            let mut used_left = s(8);
            let mut used_right = w - s(8);
            for (i, it) in b.items.iter().enumerate() {
                let r = b.rects[i];
                if r.right <= r.left {
                    continue; // did not fit
                }
                if it.right {
                    used_right = used_right.min(r.left - s(6));
                } else {
                    used_left = used_left.max(r.right + s(6));
                }
                match it.kind {
                    Kind::Sep => {
                        let cx = (r.left + r.right) / 2;
                        fill(mem, &RECT { left: cx, top: r.top + s(5), right: cx + 1, bottom: r.bottom - s(5) }, t.border);
                    }
                    _ => {
                        let hot = b.hover == Some(i);
                        let fg = if it.active {
                            round(mem, &r, t.bar_active, s(8));
                            t.bar_active_fg
                        } else {
                            if hot {
                                round(mem, &r, t.bar_hover, s(8));
                            }
                            if b.kind == BarKind::Info { t.info_fg } else { t.bar_fg }
                        };
                        SelectObject(mem, font(b.dpi, it.look, 9).into());
                        let mut rr = r;
                        draw_text(mem, &it.label, &mut rr, fg, DT_CENTER);
                        if b.kind == BarKind::Info && it.kind == Kind::Button && !hot {
                            // border around banner buttons
                            let br = CreateSolidBrush(COLORREF(t.info_fg));
                            let _ = FrameRect(mem, &r, br);
                            let _ = DeleteObject(br.into());
                        }
                    }
                }
            }
            SelectObject(mem, font(b.dpi, Look::Normal, if b.kind == BarKind::Status { 8 } else { 9 }).into());
            let fg = match b.kind {
                BarKind::Info => t.info_fg,
                BarKind::Status => t.muted,
                BarKind::Top => t.muted,
            };
            if !b.right.is_empty() {
                let mut rr = RECT { left: used_left, top: 0, right: used_right, bottom: h };
                draw_text(mem, &b.right, &mut rr, fg, DT_RIGHT | DT_END_ELLIPSIS);
                used_right -= text_width(mem, &b.right) + s(16);
            }
            if !b.left.is_empty() {
                let mut rr = RECT { left: used_left, top: 0, right: used_right.max(used_left), bottom: h };
                draw_text(mem, &b.left, &mut rr, fg, DT_LEFT | DT_END_ELLIPSIS | DT_PATH_ELLIPSIS);
            }
        });
        let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        let _ = EndPaint(hwnd, &ps);
    }
}

fn hit(hwnd: HWND, x: i32, y: i32) -> Option<usize> {
    // rects are recomputed on paint; after the button set changes there may be more of them
    // than buttons — hence only a pairwise walk (no out-of-bounds indexing).
    with_bar(hwnd, |b| {
        b.rects
            .iter()
            .zip(b.items.iter())
            .enumerate()
            .find(|(_, (r, it))| it.kind != Kind::Sep && r.right > r.left && x >= r.left && x < r.right && y >= r.top && y < r.bottom)
            .map(|(i, _)| i)
    })
    .flatten()
}

/// A panic in the window procedure would terminate the process — fall back to default message handling.
unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| bar_proc_inner(hwnd, msg, wp, lp))).unwrap_or_else(|_| DefWindowProcW(hwnd, msg, wp, lp))
}

unsafe fn bar_proc_inner(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            let h = hit(hwnd, get_x_lparam(lp.0), get_y_lparam(lp.0));
            let (changed, need_track, hint_id) = with_bar(hwnd, |b| {
                let c = b.hover != h;
                b.hover = h;
                let nt = !b.tracking;
                b.tracking = true;
                (c, nt, h.map(|i| b.items[i].id).unwrap_or(0))
            })
            .unwrap_or((false, false, 0));
            if need_track {
                let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
            }
            if changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
                if let Ok(parent) = GetParent(hwnd) {
                    let _ = PostMessageW(Some(parent), WM_BAR_HINT, WPARAM(hint_id as usize), LPARAM(0));
                }
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE_ => {
            with_bar(hwnd, |b| {
                b.hover = None;
                b.tracking = false;
                b.pressed = None;
            });
            let _ = InvalidateRect(Some(hwnd), None, false);
            if let Ok(parent) = GetParent(hwnd) {
                let _ = PostMessageW(Some(parent), WM_BAR_HINT, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let h = hit(hwnd, get_x_lparam(lp.0), get_y_lparam(lp.0));
            with_bar(hwnd, |b| b.pressed = h);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let h = hit(hwnd, get_x_lparam(lp.0), get_y_lparam(lp.0));
            let id = with_bar(hwnd, |b| {
                let p = b.pressed.take();
                if p.is_some() && p == h {
                    h.map(|i| b.items[i].id)
                } else {
                    None
                }
            })
            .flatten();
            if let (Some(id), Ok(parent)) = (id, GetParent(hwnd)) {
                let _ = PostMessageW(Some(parent), WM_COMMAND, WPARAM(id as usize), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            BARS.with(|b| b.borrow_mut().remove(&(hwnd.0 as isize)));
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Hint for an item by id (for the status bar).
/// Where the button `id` is on the bar (as of the last paint) — for the self-test's real clicks.
pub fn item_rect(hwnd: HWND, id: u16) -> Option<RECT> {
    with_bar(hwnd, |b| b.items.iter().zip(b.rects.iter()).find(|(it, r)| it.id == id && r.right > r.left).map(|(_, r)| *r)).flatten()
}

pub fn hint_for(hwnd: HWND, id: u16) -> Option<String> {
    with_bar(hwnd, |b| b.items.iter().find(|i| i.id == id).map(|i| i.hint.clone())).flatten()
}
