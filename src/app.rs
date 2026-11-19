//! Main window: modes (view / visual / code), links, images, files.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmFlush, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::Controls::Dialogs::*;
use windows::Win32::UI::Controls::RichEdit::*;
use windows::Win32::UI::Controls::{SetWindowTheme, NMHDR};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::doc::{self, Encoding, Eol};
use crate::i18n;
use crate::images::{self, Job, Source};
use crate::links::{self, LinkAction};
use crate::render::{self, ImageReq, LinkInfo, RenderOpts};
use crate::settings::{RemoteImages, Settings};
use crate::theme::{Theme, ThemePref};
use crate::ui::{self, BarKind, Item, Look};
use crate::util::{self, send, wide};
use crate::{editor, install, richedit};

/// Start page and help in the current language (`{{STATUS}}` is the installation status line).
pub fn welcome_md() -> &'static str {
    tr!(include_str!("../assets/welcome.en.md"), include_str!("../assets/welcome.ru.md"))
}

const WM_APP_IMG: u32 = WM_APP + 1;
const WM_APP_RESTYLE: u32 = WM_APP + 2;
/// Create the color emoji renderer after the first paint and repaint the view (see paint_emoji).
/// (WM_APP + 3 is ui::WM_BAR_HINT.)
const WM_APP_EMOJI: u32 = WM_APP + 4;

const T_PREVIEW: usize = 1;
const T_RESIZE: usize = 2;
const T_SNAPSHOT: usize = 4;
const T_STATUS: usize = 5;
/// Background loading of document chunks (low priority: WM_TIMER arrives when there is no input or painting).
const T_LOAD: usize = 6;
/// Smooth scrolling frames.
const T_SMOOTH: usize = 7;
/// Autosave after a pause in typing.
const T_AUTOSAVE: usize = 8;

/// The widest table in RichEdit is 22 inches (31680 twips): a \cellx further right is silently
/// clipped (measured). The text column is no wider than that, with a margin for cell padding.
const MAX_COL_TWIPS: i32 = 31_500;

/// A place in the document that must stay on screen after re-rendering
/// (theme, width or zoom change, an edit in "Code" mode).
#[derive(Clone, Copy, Debug)]
struct Anchor {
    /// Source offset of the start of the chunk containing the topmost visible character.
    src_off: usize,
    /// Character index from the start of that chunk.
    delta: i32,
    /// Where that character was vertically (px from the top of the window).
    y: i32,
    /// Instead of an exact place: a scroll fraction (switching from the editor).
    ratio: Option<f64>,
}

/// Incremental loading: RichEdit parses RTF slowly (≈0.5 µs per byte), so the chunk
/// with the target place loads first, the rest in the background one at a time (see render::Rendered::splits).
struct Loader {
    r: render::Rendered,
    /// Chunks [lo, hi) are loaded.
    lo: usize,
    hi: usize,
    start: usize,
    /// Position (cp) of the start of each loaded chunk, in order from lo.
    cp_start: Vec<i32>,
}

impl Loader {
    fn done(&self) -> bool {
        self.lo == 0 && self.hi >= self.r.chunk_count()
    }
}

/// Smooth scrolling by wheel/keys: target and current position in px.
#[derive(Clone, Copy, Default)]
struct Smooth {
    ctl: HWND,
    pos: f64,
    target: f64,
    active: bool,
    last: Option<std::time::Instant>,
}

const WM_MOUSELEAVE_: u32 = 0x02A3;
const ID_VIEW: usize = 100;
const ID_SRC: usize = 101;

// Toolbar / menu commands
const C_VIEW: u16 = 1;
const C_VISUAL: u16 = 2;
const C_CODE: u16 = 3;
const C_OPEN: u16 = 10;
const C_SAVE: u16 = 11;
const C_SAVEAS: u16 = 12;
const C_RELOAD: u16 = 13;
const C_FIND: u16 = 14;
const C_BACK: u16 = 15;
const C_THEME: u16 = 16;
const C_LANG: u16 = 17;
const C_HELP: u16 = 18;
const C_WIDE: u16 = 19;
const C_ZOOM_IN: u16 = 20;
const C_ZOOM_OUT: u16 = 21;
const C_ZOOM_RESET: u16 = 22;
const C_BOLD: u16 = 30;
const C_ITALIC: u16 = 31;
const C_STRIKE: u16 = 32;
const C_H1: u16 = 33;
const C_H2: u16 = 34;
const C_H3: u16 = 35;
const C_UL: u16 = 36;
const C_OL: u16 = 37;
const C_QUOTE: u16 = 38;
const C_CODEFMT: u16 = 39;
const C_LINK: u16 = 40;
const C_IMAGE: u16 = 41;
const C_TABLE: u16 = 42;
const C_HR: u16 = 43;
const C_TASK: u16 = 44;
const C_REMOTE_ONCE: u16 = 60;
const C_REMOTE_ALWAYS: u16 = 61;
const C_INFO_CLOSE: u16 = 62;
const C_SETUP: u16 = 63;
const C_CUT: u16 = 70;
const C_COPY: u16 = 71;
const C_PASTE: u16 = 72;
const C_SELALL: u16 = 73;
const C_COPYLINK: u16 = 74;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    View,
    Visual,
    Code,
}

pub struct Options {
    pub file: Option<PathBuf>,
    pub mode: Mode,
    pub snapshot: Option<PathBuf>,
    pub timing: Option<PathBuf>,
    pub exit_after_paint: bool,
    pub size: Option<(i32, i32)>,
    pub scroll: Option<i32>,
    pub selftest: bool,
    /// `--profile <log>`: measure scrolling and theme/mode/width/zoom changes (scripts\profile.ps1).
    pub profile: Option<PathBuf>,
}

struct Doc {
    path: Option<PathBuf>,
    text: String,
    encoding: Encoding,
    eol: Eol,
    modified: bool,
    welcome: bool,
    mtime: Option<std::time::SystemTime>,
    src_loaded: bool,
    view_stale: bool,
    /// Read with damaged characters replaced; not saved automatically (doc::LoadedFile::lossy).
    lossy: bool,
    /// The file was changed on disk by another program while there were unsaved edits:
    /// autosave is stopped, Ctrl+S asks whether to overwrite.
    conflict: bool,
}

/// How to save: autosave (silently, without overwriting others' changes), on command
/// (ask on conflict), after the user has agreed (no questions).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveHow {
    Auto,
    Ask,
    Force,
}

#[derive(Default)]
struct ViewState {
    links: Vec<LinkInfo>,
    anchors: Vec<String>,
    images: Vec<ImageReq>,
    link_pos: Vec<(i32, i32, usize)>,
    anchor_pos: HashMap<usize, i32>,
    image_pos: HashMap<usize, (i32, i32)>,
    gen: u64,
    pending: usize,
    remote_waiting: Vec<usize>,
    failed: Vec<(String, String)>,
    width_px: i32,
    hover: Option<usize>,
    down: Option<(i32, i32)>,
    /// Image jobs: key (hash of address and size) → all places with that image.
    jobs_by_key: HashMap<u64, Vec<usize>>,
    /// id of the first image the job runs for → key.
    job_key: HashMap<usize, u64>,
    /// Inline `code` chips in the text: positions of the marker space pairs (render::CODE_PAD).
    chips: Vec<(i32, i32)>,
    /// Chip geometry (index in chips → rectangles in document coordinates) and
    /// the layout it was computed for (see paint_chips).
    chip_geo: HashMap<usize, Vec<[i32; 4]>>,
    chip_key: [i32; 7],
    /// Emoji clusters in the text (positions and the cluster's UTF-16 text) for the color overlay,
    /// their geometry (index → x0, x1, baseline in document coordinates, size in px; None —
    /// not on screen, e.g. hidden text) and the layout it was computed for (see paint_emoji).
    emoji: Vec<(i32, i32, Box<[u16]>)>,
    emoji_geo: HashMap<usize, Option<(i32, i32, i32, f32)>>,
    emoji_key: [i32; 7],
    /// Ready images and captions whose document chunks are not loaded yet.
    pending_picts: Vec<(usize, Rc<String>)>,
    pending_unhide: Vec<usize>,
}

struct FindState {
    dlg: HWND,
    buf: Box<[u16; 256]>,
    fr: Box<FINDREPLACEW>,
    msg: u32,
    last: String,
}

pub struct App {
    hwnd: HWND,
    bar: HWND,
    info: HWND,
    status: HWND,
    view: HWND,
    src: Cell<HWND>,
    mode: Cell<Mode>,
    doc: RefCell<Doc>,
    vs: RefCell<ViewState>,
    styler: RefCell<Option<editor::Styler>>,
    theme: Cell<Theme>,
    settings: RefCell<Settings>,
    dpi: Cell<u32>,
    pool: RefCell<images::Pool>,
    pict_cache: RefCell<HashMap<u64, Rc<String>>>,
    history: RefCell<Vec<(PathBuf, Option<Anchor>)>>,
    info_visible: Cell<bool>,
    status_msg: RefCell<String>,
    hint: RefCell<String>,
    caret_line: Cell<i64>,
    src_hash: Cell<u64>,
    restyle_pending: Cell<bool>,
    suppress: Cell<u32>,
    find: RefCell<FindState>,
    opts: Options,
    snapshot_deadline: Cell<u64>,
    loader: RefCell<Option<Loader>>,
    /// Place for the next re-render (reload the file, "Back").
    next_anchor: Cell<Option<Anchor>>,
    /// In "Code" mode, sync the preview scroll once it has finished loading.
    sync_after_load: Cell<bool>,
    /// Scroll fraction to apply once loading finishes (switching from the editor).
    next_ratio: Cell<Option<f64>>,
    smooth: Cell<Smooth>,
    /// Start of a drag selection in the view.
    drag_anchor: Cell<Option<i32>>,
    /// Accumulated Ctrl+wheel delta (zoom from a touchpad).
    zoom_acc: Cell<i32>,
    /// Color/opacity of the inline `code` background (drawn over the view text).
    chip_brush: Cell<(u32, u8)>,
    /// Color emoji renderer: created on the first paint of a document with emoji;
    /// Some(None) — Direct2D/DirectWrite unavailable, the view keeps RichEdit's outlines.
    emoji_painter: RefCell<Option<Option<crate::emoji::Painter>>>,
    /// WM_APP_EMOJI has been posted (the renderer is being created after the first paint).
    emoji_posted: Cell<bool>,
    /// Smooth scrolling frames drawn so far (by the message loop and the fallback timer; for the self-test).
    smooth_frames: Cell<u32>,
    /// The user's answer about images from the internet, per document, for this run:
    /// true — "Load" (load them again on every re-render), false — dismissed with ✕
    /// (don't ask again for this document). Without it the bar came back on every
    /// re-render (theme, width, zoom, back from an editing mode).
    remote_choice: RefCell<HashMap<PathBuf, bool>>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
    static VIEW_PROC: Cell<isize> = const { Cell::new(0) };
    static SRC_PROC: Cell<isize> = const { Cell::new(0) };
}

fn app() -> Option<Rc<App>> {
    APP.with(|a| a.try_borrow().ok().and_then(|a| a.clone()))
}

fn s16(s: &[u16]) -> String {
    String::from_utf16_lossy(s)
}

fn u16len(s: &str) -> i32 {
    s.encode_utf16().count() as i32
}

// ======================================================================
// Startup
// ======================================================================

pub fn run(opts: Options) -> i32 {
    let t_main = util::ms_since_process_start();
    richedit::load_library();
    let t_lib = util::ms_since_process_start();
    ui::register_class();
    let settings = Settings::load();
    let theme = Theme::resolve(settings.theme);
    let class = wide("VibeMdMain");
    let hinst = unsafe { GetModuleHandleW(None).unwrap_or_default() };
    unsafe {
        let big = LoadImageW(Some(hinst.into()), PCWSTR(1 as _), IMAGE_ICON, GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON), LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .unwrap_or_default();
        let small = LoadImageW(Some(hinst.into()), PCWSTR(1 as _), IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(main_proc),
            hInstance: hinst.into(),
            hIcon: big,
            hIconSm: small,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH::default(),
            lpszClassName: PCWSTR::from_raw(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
    let (x, y, w, h, maxed) = match opts.size {
        Some((w, h)) => (0, 0, w, h, false),
        None => settings.window.unwrap_or_else(default_window_rect),
    };
    let title = wide(util::APP_NAME);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_ACCEPTFILES,
            PCWSTR::from_raw(class.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap_or_default()
    };
    if hwnd.0.is_null() {
        util::error(None, tr!("Could not create the window", "Не удалось создать окно"));
        return 1;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    if maxed {
        // While the window is hidden, give it the maximized size right away: the document renders
        // once and at the right width (otherwise at the old size, then again after showing).
        presize_maximized(hwnd, dpi);
    }
    set_dark_titlebar(hwnd, theme.dark);
    let bar = ui::create(hwnd, BarKind::Top, theme, dpi);
    let info = ui::create(hwnd, BarKind::Info, theme, dpi);
    let status = ui::create(hwnd, BarKind::Status, theme, dpi);
    unsafe {
        let _ = ShowWindow(info, SW_HIDE);
    }
    let t_win = util::ms_since_process_start();
    let view = richedit::create(hwnd, ID_VIEW, true);
    if view.0.is_null() {
        util::error(None, tr!("Could not load the RichEdit component (msftedit.dll)", "Не удалось загрузить компонент RichEdit (msftedit.dll)"));
        return 1;
    }
    richedit::set_event_mask(view, 0);
    send(view, EM_SETUNDOLIMIT, 0, 0);
    // Background tables (quotes, rules, code) without the gray cell grid.
    send(view, EM_SETEDITSTYLE, SES_HIDEGRIDLINES as usize, SES_HIDEGRIDLINES as isize);
    unsafe {
        VIEW_PROC.with(|p| p.set(SetWindowLongPtrW(view, GWLP_WNDPROC, view_proc as *const () as isize)));
        let _ = ShowWindow(view, SW_SHOW);
    }
    // The source editor is created lazily, on the first switch to an editing mode.
    let src = HWND::default();
    let zoom = settings.zoom;
    let pool = images::Pool::new(hwnd.0 as isize, WM_APP_IMG);
    let find_msg = unsafe { RegisterWindowMessageW(FINDMSGSTRINGW) };
    let app = Rc::new(App {
        hwnd,
        bar,
        info,
        status,
        view,
        src: Cell::new(src),
        mode: Cell::new(Mode::View),
        doc: RefCell::new(Doc {
            path: None,
            text: String::new(),
            encoding: Encoding::Utf8,
            eol: Eol::Crlf,
            modified: false,
            welcome: false,
            mtime: None,
            src_loaded: false,
            view_stale: false,
            lossy: false,
            conflict: false,
        }),
        vs: RefCell::new(ViewState::default()),
        styler: RefCell::new(None),
        theme: Cell::new(theme),
        settings: RefCell::new(settings),
        dpi: Cell::new(dpi),
        pool: RefCell::new(pool),
        pict_cache: RefCell::new(HashMap::new()),
        history: RefCell::new(Vec::new()),
        info_visible: Cell::new(false),
        status_msg: RefCell::new(String::new()),
        hint: RefCell::new(String::new()),
        caret_line: Cell::new(-1),
        src_hash: Cell::new(0),
        restyle_pending: Cell::new(false),
        suppress: Cell::new(0),
        find: RefCell::new(FindState {
            dlg: HWND::default(),
            buf: Box::new([0u16; 256]),
            fr: Box::new(FINDREPLACEW::default()),
            msg: find_msg,
            last: String::new(),
        }),
        opts,
        snapshot_deadline: Cell::new(0),
        loader: RefCell::new(None),
        next_anchor: Cell::new(None),
        sync_after_load: Cell::new(false),
        next_ratio: Cell::new(None),
        smooth: Cell::new(Smooth::default()),
        drag_anchor: Cell::new(None),
        zoom_acc: Cell::new(0),
        chip_brush: Cell::new((0, 0)),
        emoji_painter: RefCell::new(None),
        emoji_posted: Cell::new(false),
        smooth_frames: Cell::new(0),
        remote_choice: RefCell::new(HashMap::new()),
    });
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));

    app.apply_theme_to_controls();
    richedit::set_zoom(view, zoom);
    app.layout();

    // Document
    let file = app.opts.file.clone();
    let t_ctl = util::ms_since_process_start();
    let loaded = match file {
        Some(p) => app.open_path(&p, false, true),
        None => false,
    };
    if !loaded {
        app.show_welcome(false);
    }
    let t_doc = util::ms_since_process_start();
    let _ = t_doc;
    let start_mode = app.opts.mode;
    if start_mode != Mode::View && !app.doc.borrow().welcome {
        app.set_mode(start_mode);
    }
    app.update_bar();
    let t_shown = util::ms_since_process_start();
    unsafe {
        if maxed {
            // Maximize, keeping the normal window size (it is in the settings).
            let wp = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                flags: WINDOWPLACEMENT_FLAGS(0),
                showCmd: SW_SHOWMAXIMIZED.0 as u32,
                ptMinPosition: POINT { x: -1, y: -1 },
                ptMaxPosition: POINT { x: -1, y: -1 },
                rcNormalPosition: RECT { left: x, top: y, right: x + w, bottom: y + h },
            };
            if SetWindowPlacement(hwnd, &wp).is_err() {
                let _ = ShowWindow(hwnd, SW_SHOWMAXIMIZED);
            }
        } else {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        }
        let _ = RedrawWindow(Some(hwnd), None, None, RDW_UPDATENOW | RDW_ALLCHILDREN | RDW_INVALIDATE);
    }
    let startup = util::ms_since_process_start();
    if let Some(t) = &app.opts.timing {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(t) {
            let _ = writeln!(f, "{startup:.1} main={t_main:.1} msftedit={t_lib:.1} window={t_win:.1} controls={t_ctl:.1} shown={t_shown:.1}");
        }
    }
    app.set_status(&trf!("Opened in {startup:.0} ms", "Открыто за {startup:.0} мс"));
    if app.opts.exit_after_paint {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return 0;
    }
    if let Some(log) = app.opts.profile.clone() {
        app.profile(&log);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return 0;
    }
    if app.opts.selftest {
        let ok = app.selftest();
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return if ok { 0 } else { 3 };
    }
    if app.opts.snapshot.is_some() {
        app.snapshot_deadline.set(util::now_filetime() + 6 * 10_000_000);
        unsafe {
            SetTimer(Some(hwnd), T_SNAPSHOT, 150, None);
        }
    }
    install::cleanup_old();
    drop(app);

    // Message loop. During smooth scrolling, frames follow screen refreshes:
    // drain pending messages, move the text, wait for the next composition
    // frame (DwmFlush). That gives exactly as many steps as the monitor has hertz
    // (60/144/…), evenly spaced; the Windows timer (~15.6 ms step) gave ~60 frames
    // with stutter.
    let mut msg = MSG::default();
    unsafe {
        'run: loop {
            let anim = self::app().map(|a| a.smooth.get().active).unwrap_or(false);
            if anim {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        break 'run;
                    }
                    dispatch_msg(&msg);
                }
                let t = std::time::Instant::now();
                if let Some(a) = self::app() {
                    a.smooth_frame();
                }
                // No composition (or DwmFlush returned immediately): don't spin the CPU for nothing.
                if DwmFlush().is_err() || t.elapsed() < std::time::Duration::from_millis(1) {
                    std::thread::sleep(std::time::Duration::from_millis(4));
                }
            } else {
                if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    break;
                }
                dispatch_msg(&msg);
            }
        }
    }
    APP.with(|a| *a.borrow_mut() = None);
    0
}

unsafe fn dispatch_msg(msg: &MSG) {
    let dlg = self::app().and_then(|a| a.find.try_borrow().ok().map(|f| f.dlg)).unwrap_or_default();
    if !dlg.0.is_null() && IsDialogMessageW(dlg, msg).as_bool() {
        return;
    }
    if (msg.message == WM_KEYDOWN || msg.message == WM_SYSKEYDOWN) && handle_key(msg) {
        return;
    }
    let _ = TranslateMessage(msg);
    DispatchMessageW(msg);
}

/// Window on first launch (no saved position): ~85% of the primary monitor's
/// work area, centered. After that the position is remembered in the settings.
fn default_window_rect() -> (i32, i32, i32, i32, bool) {
    let mut wa = RECT::default();
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut wa as *mut RECT as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    let (aw, ah) = (wa.right - wa.left, wa.bottom - wa.top);
    if ok.is_err() || aw < 640 || ah < 480 {
        return (CW_USEDEFAULT, CW_USEDEFAULT, 1100, 860, false);
    }
    let (w, h) = (aw * 85 / 100, ah * 88 / 100);
    (wa.left + (aw - w) / 2, wa.top + (ah - h) / 2, w, h, false)
}

/// Give a hidden window the size it will have when maximized (the monitor's work area;
/// a maximized window's frame extends past the screen edge, the client area is exactly the work area).
fn presize_maximized(hwnd: HWND, dpi: u32) {
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() {
            return;
        }
        let wa = mi.rcWork;
        let mut fr = RECT::default();
        if AdjustWindowRectExForDpi(&mut fr, WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN, false, WS_EX_ACCEPTFILES, dpi).is_err() {
            return;
        }
        let f = -fr.left;
        let _ = MoveWindow(hwnd, wa.left - f, wa.top - f, wa.right - wa.left + 2 * f, wa.bottom - wa.top + 2 * f, false);
    }
}

fn set_dark_titlebar(hwnd: HWND, dark: bool) {
    let v = windows::core::BOOL(dark as i32);
    unsafe {
        if DwmSetWindowAttribute(hwnd, windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(20), &v as *const _ as *const _, 4).is_err() {
            let _ = DwmSetWindowAttribute(hwnd, windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(19), &v as *const _ as *const _, 4);
        }
    }
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

/// Global hotkeys. true means handled.
fn handle_key(msg: &MSG) -> bool {
    handle_key_with(msg, key_down(VK_CONTROL), key_down(VK_SHIFT), key_down(VK_MENU))
}

/// The hotkeys with the modifier state passed in (the self-test presses combinations this way
/// without touching the real keyboard).
fn handle_key_with(msg: &MSG, ctrl: bool, shift: bool, alt: bool) -> bool {
    let Some(app) = app() else { return false };
    let vk = VIRTUAL_KEY(msg.wParam.0 as u16);
    let in_src = !app.src().0.is_null() && msg.hwnd == app.src();
    let in_view = msg.hwnd == app.view;
    let mode = app.mode.get();
    if ctrl && !alt {
        match vk {
            VK_O => {
                app.cmd_open();
                return true;
            }
            VK_S => {
                if shift {
                    app.cmd_save_as();
                } else {
                    app.cmd_save();
                }
                return true;
            }
            VK_1 if !shift => {
                app.set_mode(Mode::View);
                return true;
            }
            VK_2 if !shift => {
                app.set_mode(Mode::Visual);
                return true;
            }
            VK_3 if !shift => {
                app.set_mode(Mode::Code);
                return true;
            }
            VK_F => {
                app.open_find();
                return true;
            }
            VK_OEM_PLUS | VK_ADD => {
                app.zoom_by(10);
                return true;
            }
            VK_OEM_MINUS | VK_SUBTRACT => {
                app.zoom_by(-10);
                return true;
            }
            VK_0 | VK_NUMPAD0 if !shift => {
                app.zoom_set(100);
                return true;
            }
            _ => {}
        }
    }
    // Ctrl+X without Shift: Ctrl+Shift+X is strikethrough (it used to be taken as Cut here).
    if (in_src || in_view) && ((ctrl && (vk == VK_C || vk == VK_INSERT)) || (ctrl && !shift && vk == VK_X) || (shift && vk == VK_DELETE)) {
        let cut = (ctrl && vk == VK_X) || (shift && vk == VK_DELETE);
        app.copy_selection(if in_src { app.src() } else { app.view }, cut && in_src);
        return true;
    }
    if in_src && ((ctrl && vk == VK_V) || (shift && vk == VK_INSERT)) {
        app.paste_plain();
        return true;
    }
    match vk {
        VK_F5 => {
            app.cmd_reload();
            return true;
        }
        VK_F3 => {
            let last = app.find.borrow().last.clone();
            if !last.is_empty() {
                app.find_next(&last, !shift, false);
            }
            return true;
        }
        VK_LEFT if alt => {
            app.go_back();
            return true;
        }
        VK_ESCAPE if mode != Mode::View && !ctrl => {
            app.set_mode(Mode::View);
            return true;
        }
        _ => {}
    }
    if in_src && mode != Mode::View {
        if ctrl && !alt {
            let cmd = match vk {
                VK_B => Some(C_BOLD),
                VK_I => Some(C_ITALIC),
                VK_X if shift => Some(C_STRIKE),
                VK_K => Some(C_LINK),
                VK_E => Some(C_CODEFMT),
                VK_Q => Some(C_QUOTE),
                VK_8 if shift => Some(C_UL),
                VK_7 if shift => Some(C_OL),
                _ => None,
            };
            if let Some(c) = cmd {
                app.command(c);
                return true;
            }
            if shift {
                let n = match vk {
                    VK_0 => Some(0),
                    VK_1 => Some(1),
                    VK_2 => Some(2),
                    VK_3 => Some(3),
                    VK_4 => Some(4),
                    VK_5 => Some(5),
                    VK_6 => Some(6),
                    _ => None,
                };
                if let Some(n) = n {
                    app.set_heading(n);
                    return true;
                }
            }
        }
        if vk == VK_RETURN && !ctrl && !alt {
            if shift {
                richedit::replace_sel(app.src(), "\r", true);
                return true;
            }
            return app.on_enter();
        }
        if vk == VK_TAB && !ctrl && !alt {
            return app.on_tab(shift);
        }
    }
    if in_view && ctrl && !alt && vk == VK_A {
        app.finish_loading();
        richedit::set_sel(app.view, 0, -1);
        return true;
    }
    if in_view && !ctrl && !alt {
        let page = {
            let mut rc = RECT::default();
            unsafe {
                let _ = GetClientRect(app.view, &mut rc);
            }
            (rc.bottom as f64 - app.line_px(app.view) * 2.0).max(40.0)
        };
        let line = app.line_px(app.view);
        let dy = match vk {
            VK_UP => Some(-line * 2.0),
            VK_DOWN => Some(line * 2.0),
            VK_PRIOR => Some(-page),
            VK_NEXT => Some(page),
            VK_SPACE => Some(if shift { -page } else { page }),
            // RichEdit doesn't take SB_TOP/SB_BOTTOM all the way to the edge of a large document, so we do it.
            VK_HOME | VK_END => {
                app.finish_loading();
                Some(if vk == VK_HOME { -1e9 } else { 1e9 })
            }
            _ => None,
        };
        if let Some(dy) = dy {
            app.smooth_scroll_by(app.view, dy);
            return true;
        }
        if vk == VK_BACK {
            app.go_back();
            return true;
        }
    }
    false
}

// ======================================================================
// Window procedures
// ======================================================================

// A panic inside a window procedure (extern "system") would abort the process, and unwinding through
// user32 frames is not allowed. So each procedure is wrapped: on an unexpected error
// the message gets default handling and the program keeps running.
unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| main_proc_inner(hwnd, msg, wp, lp)))
        .unwrap_or_else(|_| DefWindowProcW(hwnd, msg, wp, lp))
}

unsafe extern "system" fn view_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| view_proc_inner(hwnd, msg, wp, lp))).unwrap_or(LRESULT(0))
}

unsafe extern "system" fn src_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| src_proc_inner(hwnd, msg, wp, lp))).unwrap_or(LRESULT(0))
}

unsafe fn main_proc_inner(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(app) = app() else { return DefWindowProcW(hwnd, msg, wp, lp) };
    // try_borrow: the window procedure can be re-entered (nested messages),
    // and a panic inside it would abort the process.
    let find_msg = app.find.try_borrow().map(|f| f.msg).unwrap_or(0);
    if msg == find_msg && msg != 0 {
        app.on_find_msg(lp);
        return LRESULT(0);
    }
    match msg {
        WM_SIZE => {
            app.layout();
            SetTimer(Some(hwnd), T_RESIZE, 160, None);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_COMMAND => {
            let id = util::loword(wp.0);
            let code = util::hiword(wp.0) as u32;
            if lp.0 != 0 && HWND(lp.0 as _) == app.src() {
                if code == EN_CHANGE {
                    app.on_src_change();
                }
                return LRESULT(0);
            }
            app.command(id);
            LRESULT(0)
        }
        WM_NOTIFY => {
            let nm = &*(lp.0 as *const NMHDR);
            if nm.hwndFrom == app.src() && nm.code == EN_SELCHANGE && app.suppress.get() == 0 {
                app.on_src_selchange();
            }
            LRESULT(0)
        }
        WM_APP_IMG => {
            app.on_images_ready();
            LRESULT(0)
        }
        WM_APP_RESTYLE => {
            app.restyle_now();
            LRESULT(0)
        }
        WM_APP_EMOJI => {
            app.ensure_emoji_painter();
            let _ = InvalidateRect(Some(app.view), None, false);
            LRESULT(0)
        }
        ui::WM_BAR_HINT => {
            let id = wp.0 as u16;
            let h = if id == 0 { String::new() } else { ui::hint_for(app.bar, id).or_else(|| ui::hint_for(app.info, id)).unwrap_or_default() };
            *app.hint.borrow_mut() = h;
            app.update_status();
            LRESULT(0)
        }
        WM_TIMER => {
            match wp.0 {
                T_PREVIEW => {
                    let _ = KillTimer(Some(hwnd), T_PREVIEW);
                    if app.mode.get() == Mode::Code {
                        app.sync_from_src();
                        app.render_view(true);
                        app.sync_preview_scroll();
                    }
                }
                T_RESIZE => {
                    let _ = KillTimer(Some(hwnd), T_RESIZE);
                    app.maybe_rerender_for_width();
                }
                T_SNAPSHOT => app.try_snapshot(),
                T_LOAD => app.load_step(),
                T_SMOOTH => app.smooth_tick(),
                T_AUTOSAVE => {
                    let _ = KillTimer(Some(hwnd), T_AUTOSAVE);
                    app.autosave();
                }
                T_STATUS => {
                    let _ = KillTimer(Some(hwnd), T_STATUS);
                    app.status_msg.borrow_mut().clear();
                    app.update_status();
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DROPFILES => {
            let hdrop = HDROP(wp.0 as _);
            let need = DragQueryFileW(hdrop, 0, None) as usize;
            let mut buf = vec![0u16; need + 1];
            let n = DragQueryFileW(hdrop, 0, Some(&mut buf)) as usize;
            DragFinish(hdrop);
            if n > 0 {
                let p = PathBuf::from(s16(&buf[..n.min(need)]));
                app.navigate(&p, None);
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if util::loword(wp.0) == 0 {
                // Switched to another window: save the edits (as in an IDE).
                app.autosave();
            }
            if util::loword(wp.0) != 0 {
                app.check_external_change();
                let target = if app.mode.get() == Mode::View { app.view } else { app.src() };
                let _ = SetFocus(Some(target));
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            if lp.0 != 0 {
                let s = PCWSTR(lp.0 as *const u16).to_string().unwrap_or_default();
                if s == "ImmersiveColorSet" && app.settings.borrow().theme == ThemePref::System {
                    app.apply_theme(ThemePref::System);
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lp.0 as *mut MINMAXINFO);
            mmi.ptMinTrackSize.x = 420;
            mmi.ptMinTrackSize.y = 300;
            if app.opts.size.is_some() {
                mmi.ptMaxTrackSize.x = 8000;
                mmi.ptMaxTrackSize.y = 8000;
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            if app.confirm_discard() {
                app.save_window_settings();
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // Direct2D/DirectWrite objects are released now, while their DLLs are loaded: the
            // service modes (--selftest, --profile) return with the App still in thread-local
            // storage, and releasing them at process exit, after d2d1.dll has been detached,
            // ends in a fail-fast in ntdll (0xC000000D).
            if let Some(a) = self::app() {
                if let Ok(mut p) = a.emoji_painter.try_borrow_mut() {
                    p.take();
                }
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn call_orig(which: &'static std::thread::LocalKey<Cell<isize>>, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let p = which.with(|p| p.get());
    let f: WNDPROC = std::mem::transmute(p);
    CallWindowProcW(f, hwnd, msg, wp, lp)
}

unsafe fn view_proc_inner(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let r = view_proc_msg(hwnd, msg, wp, lp);
    // The view is read-only: RichEdit does get focus (otherwise mouse selection doesn't
    // work), but we hide the blinking caret. HideCaret accumulates, and RichEdit resets the
    // counter only when it recreates the caret, so we hide it after every message.
    if GetFocus() == hwnd && msg != WM_PAINT && msg != WM_NCHITTEST && msg != WM_SETCURSOR {
        let _ = HideCaret(Some(hwnd));
    }
    r
}

unsafe fn view_proc_msg(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            // Inline `code` background: on top of what RichEdit painted, only in the updated region.
            let rgn = CreateRectRgn(0, 0, 0, 0);
            let kind = GetUpdateRgn(hwnd, rgn, false);
            let t0 = std::time::Instant::now();
            let r = call_orig(&VIEW_PROC, hwnd, msg, wp, lp);
            let t1 = std::time::Instant::now();
            if kind != NULLREGION && kind != RGN_ERROR {
                if let Some(app) = app() {
                    app.paint_chips(hwnd, rgn);
                    app.paint_emoji(hwnd, rgn);
                }
            }
            PAINT_T.with(|p| {
                let (a, b, n) = p.get();
                p.set((a + (t1 - t0).as_secs_f64() * 1000.0, b + t1.elapsed().as_secs_f64() * 1000.0, n + 1));
            });
            let _ = DeleteObject(rgn.into());
            r
        }
        WM_MOUSEWHEEL => {
            if let Some(app) = app() {
                app.on_wheel(hwnd, wp);
            }
            LRESULT(0)
        }
        WM_VSCROLL => {
            if let Some(app) = app() {
                app.stop_smooth();
            }
            call_orig(&VIEW_PROC, hwnd, msg, wp, lp)
        }
        WM_MOUSEMOVE if !(wp.0 & 0x0001 != 0 && app().map(|a| a.drag_anchor.get().is_some()).unwrap_or(false)) => {
            let r = call_orig(&VIEW_PROC, hwnd, msg, wp, lp);
            if let Some(app) = app() {
                app.view_hover(util::get_x_lparam(lp.0), util::get_y_lparam(lp.0));
            }
            r
        }
        WM_SETCURSOR => {
            if let Some(app) = app() {
                if util::loword(lp.0 as usize) as u32 == HTCLIENT && app.vs.try_borrow().map(|v| v.hover.is_some()).unwrap_or(false) {
                    SetCursor(LoadCursorW(None, IDC_HAND).ok());
                    return LRESULT(1);
                }
            }
            call_orig(&VIEW_PROC, hwnd, msg, wp, lp)
        }
        // Drag selection is done by us: a read-only RichEdit didn't do it.
        WM_LBUTTONDOWN => {
            let Some(app) = app() else { return call_orig(&VIEW_PROC, hwnd, msg, wp, lp) };
            let (x, y) = (util::get_x_lparam(lp.0), util::get_y_lparam(lp.0));
            if let Ok(mut v) = app.vs.try_borrow_mut() {
                v.down = Some((x, y));
            }
            if GetFocus() != hwnd {
                let _ = SetFocus(Some(hwnd));
            }
            app.stop_smooth();
            let cp = app.view_cp_at(x, y);
            let shift = wp.0 & 0x0004 != 0;
            let anchor = match (shift, app.drag_anchor.get()) {
                (true, _) => {
                    let (a, b) = richedit::get_sel(hwnd);
                    if (cp - a).abs() > (cp - b).abs() { a } else { b }
                }
                _ => cp,
            };
            app.drag_anchor.set(Some(anchor));
            richedit::set_sel(hwnd, anchor, cp);
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE if wp.0 & 0x0001 != 0 && app().map(|a| a.drag_anchor.get().is_some()).unwrap_or(false) => {
            let Some(app) = app() else { return LRESULT(0) };
            let (x, y) = (util::get_x_lparam(lp.0), util::get_y_lparam(lp.0));
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            // Past the window edge: scroll in that direction.
            if y < 0 {
                let y0 = richedit::scroll_pos(hwnd).1;
                richedit::set_scroll_pos(hwnd, 0, (y0 + y).max(0));
            } else if y > rc.bottom {
                let y0 = richedit::scroll_pos(hwnd).1;
                richedit::set_scroll_pos(hwnd, 0, y0 + (y - rc.bottom));
            }
            if let Some(anchor) = app.drag_anchor.get() {
                let cp = app.view_cp_at(x.clamp(0, rc.right - 1), y.clamp(0, rc.bottom - 1));
                richedit::set_sel(hwnd, anchor, cp);
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            if let Some(app) = app() {
                app.drag_anchor.set(None);
            }
            call_orig(&VIEW_PROC, hwnd, msg, wp, lp)
        }
        WM_LBUTTONUP => {
            let dragging = app().map(|a| a.drag_anchor.take().is_some()).unwrap_or(false);
            let r = if dragging {
                let _ = ReleaseCapture();
                LRESULT(0)
            } else {
                call_orig(&VIEW_PROC, hwnd, msg, wp, lp)
            };
            if let Some(app) = app() {
                let (x, y) = (util::get_x_lparam(lp.0), util::get_y_lparam(lp.0));
                let down = app.vs.try_borrow_mut().ok().and_then(|mut v| v.down.take());
                if let Some((dx, dy)) = down {
                    let (a, b) = richedit::get_sel(hwnd);
                    if (dx - x).abs() < 5 && (dy - y).abs() < 5 && b - a <= 1 {
                        if let Some(id) = app.view_hit(x, y) {
                            let _ = PostMessageW(Some(app.hwnd), WM_COMMAND, WPARAM(0xF000 + id), LPARAM(0));
                        }
                    }
                }
            }
            r
        }
        WM_MOUSELEAVE_ => {
            if let Some(app) = app() {
                app.set_hover(None);
            }
            call_orig(&VIEW_PROC, hwnd, msg, wp, lp)
        }
        WM_CONTEXTMENU => {
            if let Some(app) = app() {
                app.context_menu(hwnd, false);
            }
            LRESULT(0)
        }
        WM_COPY => {
            if let Some(app) = app() {
                app.copy_selection(hwnd, false);
            }
            LRESULT(0)
        }
        WM_PASTE | WM_CUT => LRESULT(0),
        _ => call_orig(&VIEW_PROC, hwnd, msg, wp, lp),
    }
}

unsafe fn src_proc_inner(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let r = src_proc_msg(hwnd, msg, wp, lp);
    if msg != WM_PAINT && msg != WM_NCHITTEST && msg != WM_SETCURSOR && msg != WM_ERASEBKGND {
        fix_margin_strips(hwnd);
    }
    r
}

unsafe fn src_proc_msg(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Paste plain text only: no RTF/OLE objects from the clipboard.
        WM_PASTE => {
            if let Some(app) = app() {
                app.paste_plain();
            }
            LRESULT(0)
        }
        WM_COPY | WM_CUT => {
            if let Some(app) = app() {
                app.copy_selection(hwnd, msg == WM_CUT);
            }
            LRESULT(0)
        }
        WM_CONTEXTMENU => {
            if let Some(app) = app() {
                app.context_menu(hwnd, true);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let r = call_orig(&SRC_PROC, hwnd, msg, wp, lp);
            if key_down(VK_CONTROL) {
                if let Some(app) = app() {
                    app.src_ctrl_click();
                }
            }
            r
        }
        WM_MOUSEWHEEL => {
            if let Some(app) = app() {
                app.on_wheel(hwnd, wp);
            }
            LRESULT(0)
        }
        WM_VSCROLL | WM_KEYUP => {
            if msg == WM_VSCROLL {
                if let Some(app) = app() {
                    app.stop_smooth();
                }
            }
            let r = call_orig(&SRC_PROC, hwnd, msg, wp, lp);
            if let Some(app) = app() {
                if app.mode.get() == Mode::Code {
                    app.sync_preview_scroll();
                }
            }
            r
        }
        _ => call_orig(&SRC_PROC, hwnd, msg, wp, lp),
    }
}

/// Editor with top/bottom margins: when scrolling, RichEdit shifts the margins too but doesn't
/// repaint them, so strips of text were left there. If the scroll position changed,
/// we repaint the margin strips.
unsafe fn fix_margin_strips(hwnd: HWND) {
    thread_local! {
        static LAST: Cell<i32> = const { Cell::new(i32::MIN) };
    }
    let pos = GetScrollPos(hwnd, SB_VERT);
    if LAST.with(|l| l.replace(pos)) == pos {
        return;
    }
    let fr = richedit::get_rect(hwnd);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    if fr.top > 0 {
        let _ = InvalidateRect(Some(hwnd), Some(&RECT { left: 0, top: 0, right: rc.right, bottom: fr.top }), true);
    }
    if fr.bottom < rc.bottom {
        let _ = InvalidateRect(Some(hwnd), Some(&RECT { left: 0, top: fr.bottom, right: rc.right, bottom: rc.bottom }), true);
    }
}

// ======================================================================
// Logic
// ======================================================================

impl App {
    fn src(&self) -> HWND {
        self.src.get()
    }

    /// Create the source editor when first needed (saves ~10 ms of startup).
    fn ensure_src(&self) {
        if !self.src.get().0.is_null() {
            return;
        }
        let src = richedit::create(self.hwnd, ID_SRC, false);
        send(src, EM_SETUNDOLIMIT, 500, 0);
        richedit::set_event_mask(src, ENM_CHANGE | ENM_SELCHANGE);
        unsafe {
            SRC_PROC.with(|p| p.set(SetWindowLongPtrW(src, GWLP_WNDPROC, src_proc as *const () as isize)));
        }
        // OLE drag-and-drop into the editor is disabled: it could bring in RTF and OLE objects.
        // The drop target is registered only if OLE is already initialized (e.g. by a system
        // dialog), so we revoke the registration explicitly.
        unsafe {
            let _ = windows::Win32::System::Ole::RevokeDragDrop(src);
        }
        self.src.set(src);
        self.apply_theme_to_controls();
        richedit::set_zoom(src, self.settings.borrow().zoom);
    }

    fn scale(&self, v: i32) -> i32 {
        v * self.dpi.get() as i32 / 96
    }

    fn base_dir(&self) -> Option<PathBuf> {
        self.doc.borrow().path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf()))
    }

    // ---------------- layout ----------------

    fn layout(&self) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let (w, h) = (rc.right, rc.bottom);
        let dpi = self.dpi.get();
        let top_h = ui::height(BarKind::Top, dpi);
        let info_h = if self.info_visible.get() { ui::height(BarKind::Info, dpi) } else { 0 };
        let st_h = ui::height(BarKind::Status, dpi);
        let content_top = top_h + info_h;
        let content_h = (h - content_top - st_h).max(0);
        unsafe {
            let _ = MoveWindow(self.bar, 0, 0, w, top_h, true);
            if info_h > 0 {
                let _ = MoveWindow(self.info, 0, top_h, w, info_h, true);
            }
            let _ = MoveWindow(self.status, 0, h - st_h, w, st_h, true);
            match self.mode.get() {
                Mode::View => {
                    let _ = MoveWindow(self.view, 0, content_top, w, content_h, true);
                }
                Mode::Visual => {
                    let _ = MoveWindow(self.src(), 0, content_top, w, content_h, true);
                }
                Mode::Code => {
                    let half = w / 2;
                    let _ = MoveWindow(self.src(), 0, content_top, half - 1, content_h, true);
                    let _ = MoveWindow(self.view, half + 1, content_top, w - half - 1, content_h, true);
                }
            }
        }
        self.apply_margins();
    }

    /// Text column in `ctl`: (margin for EM_SETRECT, text width in px at 100% zoom).
    ///
    /// * RichEdit multiplies EM_SETRECT margins by the zoom (EM_SETZOOM), so the margins
    ///   are given "before zoom": on screen they are what is computed here.
    /// * A table in RichEdit can't be wider than 22 inches (`MAX_COL_TWIPS`), and code blocks,
    ///   heading rules and quotes are tables. The column is no wider than that limit, otherwise on
    ///   a large monitor the text extends past the blocks. Extra space is split evenly on both sides.
    /// * `preview` is the preview on the right in "Code" mode: narrow margins, no narrow column.
    fn column(&self, ctl: HWND, preview: bool) -> (i32, i32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(ctl, &mut rc);
        }
        let w = rc.right.max(1);
        let z = self.view_zoom();
        let dpi = self.dpi.get() as i64;
        let max_col = (MAX_COL_TWIPS as i64 * dpi * z as i64 / (1440 * 100)) as i32;
        let col = if preview {
            w - 2 * self.scale(20)
        } else if self.settings.borrow().wide {
            // Full width: only small margins (3% of the width, but at least 28 px).
            w - 2 * (w * 3 / 100).max(self.scale(28))
        } else {
            // Narrow reading column (grows with the zoom).
            (w - 2 * self.scale(28)).min(self.scale(900) * z / 100)
        };
        let col = col.min(max_col).max(self.scale(50).min(w));
        let side = ((w - col) / 2).max(0);
        let inset = (side * 100 + z / 2) / z;
        (inset, (w * 100 / z - 2 * inset).max(50))
    }

    fn apply_margins(&self) {
        // On-screen margins → margins for RichEdit (it multiplies them by the zoom).
        let z = self.view_zoom();
        let unzoom = |px: i32| (px * 100 + z / 2) / z;
        match self.mode.get() {
            Mode::View => {
                // No top/bottom margins: spacing comes from paragraphs in the document itself (render.rs),
                // otherwise strips of text were left in the margins when scrolling.
                let (side, _) = self.column(self.view, false);
                richedit::set_margins(self.view, side, 0, side, 0);
            }
            Mode::Visual => {
                let (side, _) = self.column(self.src(), false);
                richedit::set_margins(self.src(), side, unzoom(self.scale(16)), side, unzoom(self.scale(8)));
            }
            Mode::Code => {
                let m = unzoom(self.scale(12));
                richedit::set_margins(self.src(), m, unzoom(self.scale(8)), m, unzoom(self.scale(8)));
                let (side, _) = self.column(self.view, true);
                richedit::set_margins(self.view, side, 0, side, 0);
            }
        }
    }

    /// Switch the interface language (EN/RU button): toolbars, banners, status bar and
    /// start page update at once; remembered in the settings.
    fn toggle_lang(&self) {
        let next = if i18n::is_ru() { i18n::Lang::En } else { i18n::Lang::Ru };
        i18n::set(next);
        {
            let mut s = self.settings.borrow_mut();
            s.lang = next;
            s.dirty = true;
        }
        self.update_bar();
        if self.doc.borrow().welcome {
            let text = self.welcome_text();
            self.doc.borrow_mut().text = text;
            self.render_view(true);
            self.show_setup_bar();
        } else if self.info_visible.get() {
            let n = self.vs.borrow().remote_waiting.len();
            self.show_info_bar(n);
        }
        self.status_msg.borrow_mut().clear();
        self.update_title();
        self.update_status();
    }

    /// Toggle "full width" ↔ "narrow column" (toolbar button).
    fn toggle_wide(&self) {
        {
            let mut s = self.settings.borrow_mut();
            s.wide = !s.wide;
            s.dirty = true;
        }
        self.apply_margins();
        // Images and tables are sized to the text width, so re-render the view.
        self.maybe_rerender_for_width();
        self.update_bar();
    }

    /// View text width in px at 100% zoom (tables and images are sized to it).
    fn view_text_width(&self) -> i32 {
        self.column(self.view, self.mode.get() == Mode::Code).1
    }

    /// The column width changed (window, "full width"): re-render so that code blocks
    /// and rules again reach exactly to the text edge. Any difference, even 1 px.
    fn maybe_rerender_for_width(&self) {
        let w = self.view_text_width();
        let old = self.vs.borrow().width_px;
        if w != old && self.mode.get() != Mode::Visual {
            self.render_view(true);
        }
    }

    // ---------------- theme ----------------

    fn apply_theme_to_controls(&self) {
        let t = self.theme.get();
        // Inline `code` background: lighter than the background in the dark theme, darker in the light one (as in VS Code).
        self.chip_brush.set(if t.dark { (0x00FF_FFFF, 30) } else { (0x0000_0000, 15) });
        richedit::set_bg(self.view, t.bg);
        richedit::set_bg(self.src(), t.bg);
        let theme_name = wide(if t.dark { "DarkMode_Explorer" } else { "Explorer" });
        unsafe {
            let _ = SetWindowTheme(self.view, PCWSTR::from_raw(theme_name.as_ptr()), PCWSTR::null());
            let _ = SetWindowTheme(self.src(), PCWSTR::from_raw(theme_name.as_ptr()), PCWSTR::null());
        }
        // Default text color for newly typed text.
        let mut cf = CHARFORMAT2W::default();
        cf.Base.cbSize = std::mem::size_of::<CHARFORMAT2W>() as u32;
        cf.Base.dwMask = CFM_COLOR;
        cf.Base.crTextColor = COLORREF(t.fg);
        send(self.src(), EM_SETCHARFORMAT, SCF_ALL as usize, &cf as *const _ as isize);
        let dpi = self.dpi.get();
        ui::set_theme(self.bar, t, dpi);
        ui::set_theme(self.info, t, dpi);
        ui::set_theme(self.status, t, dpi);
        set_dark_titlebar(self.hwnd, t.dark);
    }

    fn apply_theme(&self, pref: ThemePref) {
        {
            let mut s = self.settings.borrow_mut();
            s.theme = pref;
            s.dirty = true;
        }
        let t = Theme::resolve(pref);
        self.theme.set(t);
        self.apply_theme_to_controls();
        if let Some(st) = self.styler.borrow_mut().as_mut() {
            st.set_theme(t);
        }
        // Images don't depend on the theme (the cache is kept), so only the text is re-rendered.
        self.render_view(true);
        if self.mode.get() != Mode::View {
            self.restyle_now();
        }
        unsafe {
            let _ = RedrawWindow(Some(self.hwnd), None, None, RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_ERASE);
        }
    }

    // ---------------- toolbar and status ----------------

    fn update_bar(&self) {
        let m = self.mode.get();
        let mut items = vec![
            Item::tab(C_VIEW, tr!("View", "Просмотр"), m == Mode::View, tr!("View the document (Ctrl+1)", "Просмотр документа (Ctrl+1)")),
            Item::tab(C_VISUAL, tr!("Visual", "Визуальный"), m == Mode::Visual, tr!("Edit as in a text editor (Ctrl+2)", "Редактирование как в текстовом редакторе (Ctrl+2)")),
            Item::tab(C_CODE, tr!("Code", "Код"), m == Mode::Code, tr!("Markdown source + preview (Ctrl+3)", "Исходный markdown + предпросмотр (Ctrl+3)")),
        ];
        // "Open" is Ctrl+O, dragging a file in, or a link on the start page.
        // Saving is automatic (a pause in typing, leaving the window); the button is shown only if
        // autosave is turned off in the settings.
        if m != Mode::View && !self.settings.borrow().autosave {
            items.push(Item::sep());
            items.push(Item::button(C_SAVE, tr!("Save", "Сохранить"), tr!("Save (Ctrl+S), Ctrl+Shift+S — save as", "Сохранить (Ctrl+S), Ctrl+Shift+S — сохранить как")));
        }
        if !self.history.borrow().is_empty() {
            items.push(Item::sep());
            items.push(Item::button(C_BACK, tr!("← Back", "← Назад"), tr!("Go back to the previous document (Alt+←)", "Вернуться к предыдущему документу (Alt+←)")));
        }
        if m != Mode::View {
            items.push(Item::sep());
            items.push(Item::button(C_BOLD, tr!("B", "Ж"), tr!("Bold (Ctrl+B)", "Жирный (Ctrl+B)")).look(Look::Bold));
            items.push(Item::button(C_ITALIC, tr!("I", "К"), tr!("Italic (Ctrl+I)", "Курсив (Ctrl+I)")).look(Look::Italic));
            items.push(Item::button(C_STRIKE, tr!("S", "З"), tr!("Strikethrough (Ctrl+Shift+X)", "Зачёркнутый (Ctrl+Shift+X)")).look(Look::Strike));
            items.push(Item::button(C_H1, "H1", tr!("Heading 1 (Ctrl+Shift+1)", "Заголовок 1 (Ctrl+Shift+1)")));
            items.push(Item::button(C_H2, "H2", tr!("Heading 2 (Ctrl+Shift+2)", "Заголовок 2 (Ctrl+Shift+2)")));
            items.push(Item::button(C_H3, "H3", tr!("Heading 3 (Ctrl+Shift+3)", "Заголовок 3 (Ctrl+Shift+3)")));
            items.push(Item::button(C_UL, "•", tr!("Bulleted list (Ctrl+Shift+8)", "Маркированный список (Ctrl+Shift+8)")));
            items.push(Item::button(C_OL, "1.", tr!("Numbered list (Ctrl+Shift+7)", "Нумерованный список (Ctrl+Shift+7)")));
            items.push(Item::button(C_TASK, "☐", tr!("Task list", "Список задач")));
            items.push(Item::button(C_QUOTE, "❝", tr!("Quote (Ctrl+Q)", "Цитата (Ctrl+Q)")));
            items.push(Item::button(C_CODEFMT, "</>", tr!("Code (Ctrl+E)", "Код (Ctrl+E)")).look(Look::Mono));
            items.push(Item::button(C_LINK, tr!("Link", "Ссылка"), tr!("Insert link (Ctrl+K)", "Вставить ссылку (Ctrl+K)")));
            items.push(Item::button(C_IMAGE, tr!("Image", "Фото"), tr!("Insert an image from a file", "Вставить картинку из файла")));
            items.push(Item::button(C_TABLE, tr!("Table", "Таблица"), tr!("Insert table", "Вставить таблицу")));
            items.push(Item::button(C_HR, "—", tr!("Horizontal rule", "Горизонтальная линия")));
        }
        items.push(Item::button(C_HELP, "?", tr!("Help and installation", "Справка и установка")).right());
        items.push(Item::button(C_LANG, tr!("EN", "RU"), tr!("Interface language: English. Click for Russian", "Язык интерфейса: русский. Нажмите — английский")).right());
        let theme_label = if self.theme.get().dark { "☀" } else { "☾" };
        items.push(Item::button(C_THEME, theme_label, tr!("Light / dark theme", "Светлая / тёмная тема")).right());
        let (wlabel, whint) = if self.settings.borrow().wide {
            ("→←", tr!("Now: full width. Click for a narrow column", "Сейчас: во всю ширину. Нажмите — узкая колонка"))
        } else {
            ("←→", tr!("Now: narrow column. Click for full width", "Сейчас: узкая колонка. Нажмите — во всю ширину"))
        };
        items.push(Item::button(C_WIDE, wlabel, whint).right());
        items.push(Item::button(C_FIND, tr!("Find", "Найти"), tr!("Find (Ctrl+F, F3 — next)", "Поиск (Ctrl+F, F3 — дальше)")).right());
        // Zoom: "−  100%  +" (right-aligned items go from right to left).
        let z = self.settings.borrow().zoom;
        items.push(Item::button(C_ZOOM_IN, "+", tr!("Zoom in (Ctrl+wheel up, Ctrl+plus)", "Крупнее (Ctrl+колесо вверх, Ctrl+плюс)")).right());
        items.push(Item::button(C_ZOOM_RESET, &format!("{z}%"), tr!("Zoom — click to reset to 100% (Ctrl+0)", "Масштаб — нажмите, чтобы вернуть 100% (Ctrl+0)")).right());
        items.push(Item::button(C_ZOOM_OUT, "−", tr!("Zoom out (Ctrl+wheel down, Ctrl+minus)", "Мельче (Ctrl+колесо вниз, Ctrl+минус)")).right());
        ui::set_items(self.bar, items);
    }

    fn set_status(&self, s: &str) {
        *self.status_msg.borrow_mut() = s.to_string();
        self.update_status();
        unsafe {
            SetTimer(Some(self.hwnd), T_STATUS, 6000, None);
        }
    }

    fn update_status(&self) {
        let Ok(d) = self.doc.try_borrow() else { return };
        let hover_url = self.vs.try_borrow().ok().and_then(|v| v.hover.and_then(|i| v.links.get(i).map(|l| l.url.clone())));
        let left = if let Some(u) = hover_url {
            let what = match links::classify(&u, d.path.as_ref().and_then(|p| p.parent())) {
                LinkAction::Blocked(_) => tr!("⛔ blocked: ", "⛔ заблокировано: "),
                LinkAction::Web { .. } => "↗ ",
                LinkAction::OpenMarkdown { .. } => "📄 ",
                LinkAction::Anchor(_) => "# ",
                LinkAction::LocalFile(_) => tr!("📁 (will ask) ", "📁 (спросит) "),
                _ => "",
            };
            format!("{what}{u}")
        } else if !self.hint.borrow().is_empty() {
            self.hint.borrow().clone()
        } else if !self.status_msg.borrow().is_empty() {
            self.status_msg.borrow().clone()
        } else if d.welcome {
            tr!("Start page", "Стартовая страница").to_string()
        } else {
            d.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
        };
        let mode = match self.mode.get() {
            Mode::View => tr!("View", "Просмотр"),
            Mode::Visual => tr!("Visual", "Визуальный"),
            Mode::Code => tr!("Code", "Код"),
        };
        let mut right = String::new();
        if d.modified {
            right.push_str(tr!("● modified   ", "● изменён   "));
        }
        if self.mode.get() != Mode::View && self.caret_line.get() >= 0 {
            right.push_str(&trf!("Ln {}   ", "Стр {}   ", self.caret_line.get() + 1));
        }
        if !d.welcome {
            right.push_str(&format!("{}   {}   ", d.encoding.label(), d.eol.label()));
        }
        right.push_str(mode);
        drop(d);
        ui::set_text(self.status, &left, &right);
    }

    fn update_title(&self) {
        let d = self.doc.borrow();
        let name = if d.welcome {
            tr!("Welcome", "Добро пожаловать").to_string()
        } else {
            d.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| tr!("Untitled", "Без имени").into())
        };
        let t = format!("{}{} — {}", if d.modified { "● " } else { "" }, name, util::APP_NAME);
        let w = wide(&t);
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR::from_raw(w.as_ptr()));
        }
    }

    /// The installer window runs as a separate process (the same exe with --setup).
    fn launch_setup(&self) {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).arg("--setup").spawn();
        }
    }

    /// Banner on the start page: the program is not installed / an older version is installed.
    fn show_setup_bar(&self) {
        if install::running_installed() {
            return;
        }
        let text = match install::installed() {
            None => tr!(
                "vibe-md-viewer is not installed yet: .md files don't open in it on double-click.",
                "vibe-md-viewer ещё не установлен: файлы .md пока не открываются им по двойному щелчку."
            )
            .to_string(),
            Some(i) if install::compare_versions(install::VERSION, &i.version) == std::cmp::Ordering::Greater => {
                trf!("An older version {} is installed — this copy is newer ({}).", "Установлена старая версия {} — эта копия новее ({}).", i.version, install::VERSION)
            }
            Some(_) => return,
        };
        let label = if install::installed().is_some() { tr!("Update…", "Обновить…") } else { tr!("Install…", "Установить…") };
        ui::set_items(
            self.info,
            vec![
                Item::button(C_INFO_CLOSE, "✕", tr!("Hide", "Скрыть")).right(),
                Item::button(C_SETUP, label, tr!("Open the setup wizard", "Открыть мастер установки")).right(),
            ],
        );
        ui::set_text(self.info, &text, "");
        if !self.info_visible.get() {
            self.info_visible.set(true);
            unsafe {
                let _ = ShowWindow(self.info, SW_SHOW);
            }
            self.layout();
        }
    }

    fn show_info_bar(&self, n_remote: usize) {
        if n_remote == 0 {
            if self.info_visible.get() {
                self.info_visible.set(false);
                unsafe {
                    let _ = ShowWindow(self.info, SW_HIDE);
                }
                self.layout();
            }
            return;
        }
        let text = trf!(
            "Images from the internet: {n_remote}. Loading them tells their servers your IP address and when you opened the document.",
            "Картинок из интернета: {n_remote}. Загрузка сообщит их серверам ваш IP-адрес и время открытия."
        );
        ui::set_items(
            self.info,
            vec![
                Item::button(C_INFO_CLOSE, "✕", tr!("Hide", "Скрыть")).right(),
                Item::button(C_REMOTE_ALWAYS, tr!("Always load", "Всегда загружать"), tr!("Don't ask again (setting remote_images=always)", "Больше не спрашивать (настройка remote_images=always)")).right(),
                Item::button(C_REMOTE_ONCE, tr!("Load", "Загрузить"), tr!("Load the images of this document", "Загрузить картинки этого документа")).right(),
            ],
        );
        ui::set_text(self.info, &text, "");
        if !self.info_visible.get() {
            self.info_visible.set(true);
            unsafe {
                let _ = ShowWindow(self.info, SW_SHOW);
            }
            self.layout();
        }
    }

    // ---------------- document ----------------

    fn confirm_discard(&self) -> bool {
        if self.restyle_pending.get() && self.mode.get() != Mode::View {
            self.restyle_now();
        }
        self.sync_from_src();
        let (modified, name, has_path) = {
            let d = self.doc.borrow();
            (d.modified, d.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| tr!("document", "документ").into()), d.path.is_some())
        };
        if !modified {
            return true;
        }
        let (path, risky) = {
            let d = self.doc.borrow();
            let changed = d.path.as_deref().map(|p| d.mtime.is_some() && doc::mtime(p) != d.mtime).unwrap_or(false);
            (d.path.clone(), d.conflict || d.lossy || changed)
        };
        // Autosave: don't ask, just save (on error: an error dialog and the close is refused).
        // Except when writing could overwrite someone else's changes: then ask.
        if self.settings.borrow().autosave && has_path && !risky {
            return self.cmd_save();
        }
        let q = if risky {
            trf!(
                "The file “{name}” was changed on disk by another program or was read with damaged characters replaced.\n\nWrite your version over it?\n\nNo — close without saving your changes.",
                "Файл «{name}» изменён на диске другой программой или был прочитан с заменой повреждённых символов.\n\nЗаписать вашу версию поверх?\n\nНет — закрыть без сохранения ваших правок."
            )
        } else {
            trf!("Save changes to “{name}”?", "Сохранить изменения в «{name}»?")
        };
        match util::ask_yes_no_cancel(Some(self.hwnd), &q) {
            Some(true) => match path {
                Some(p) if risky => self.save_impl(&p, SaveHow::Force),
                _ => self.cmd_save(),
            },
            Some(false) => true,
            None => false,
        }
    }

    /// Open a file. `push`: remember the current one in the history.
    fn open_path(&self, path: &Path, push: bool, initial: bool) -> bool {
        let path = if path.is_relative() { std::env::current_dir().map(|d| d.join(path)).unwrap_or(path.to_path_buf()) } else { path.to_path_buf() };
        let loaded = match doc::load(&path) {
            Ok(f) => f,
            Err(e) => {
                util::error(Some(self.hwnd), &format!("{}\n\n{e}", path.display()));
                return false;
            }
        };
        if !initial && push {
            let cur = self.doc.borrow().path.clone();
            if let Some(cur) = cur {
                let a = self.view_anchor();
                self.history.borrow_mut().push((cur, a));
            }
        }
        {
            let mut d = self.doc.borrow_mut();
            d.path = Some(path.clone());
            d.text = loaded.text;
            d.encoding = loaded.encoding;
            d.eol = loaded.eol;
            d.modified = false;
            d.welcome = false;
            d.mtime = doc::mtime(&path);
            d.src_loaded = false;
            d.view_stale = false;
            d.lossy = loaded.lossy;
            d.conflict = false;
        }
        if loaded.lossy {
            self.set_status(tr!(
                "The file was read with damaged characters replaced (�) — it isn't saved automatically; Ctrl+S saves it explicitly.",
                "Файл прочитан с заменой повреждённых символов (�) — сам он не сохраняется; Ctrl+S — сохранить явно."
            ));
        }
        self.render_view(false);
        if self.mode.get() != Mode::View {
            self.load_src();
        }
        self.update_title();
        self.update_bar();
        self.update_status();
        true
    }

    fn welcome_text(&self) -> String {
        let v = install::VERSION;
        let inst = install::installed();
        let status = if install::running_installed() {
            let d = md_code(&util::exe_dir().map(|d| d.display().to_string()).unwrap_or_default());
            let scope = inst.as_ref().map(|i| i.scope.label()).unwrap_or("");
            trf!(
                "**Version {v} is installed** ({scope}) in {d}.\n\n[Make it the default app for .md](vmd:default) · [Open file…](vmd:open) · [Uninstall](vmd:uninstall)\n\n*Updating:* run a newer `vibe-md-viewer.exe` from anywhere — it updates this installation itself; your settings and the `.md` association are kept.",
                "**Установлена версия {v}** ({scope}) в {d}.\n\n[Сделать программой по умолчанию для .md](vmd:default) · [Открыть файл…](vmd:open) · [Удалить программу](vmd:uninstall)\n\n*Обновление:* запустите новый `vibe-md-viewer.exe` из любого места — он сам обновит эту установку, настройки и привязка `.md` сохранятся.",
            )
        } else if let Some(i) = &inst {
            let old = &i.version;
            let action = match install::compare_versions(v, old) {
                std::cmp::Ordering::Greater => trf!("**[Update the installed version {old} → {v}](vmd:install)**", "**[Обновить установленную версию {old} → {v}](vmd:install)**"),
                std::cmp::Ordering::Equal => tr!("[Reinstall](vmd:install)", "[Переустановить](vmd:install)").to_string(),
                std::cmp::Ordering::Less => trf!("A newer version {old} is installed.", "Установлена более новая версия {old}."),
            };
            trf!(
                "The program is installed in {} (version {old}), but a separate copy of version {v} is running now.\n\n{action} · [Open file…](vmd:open)",
                "Программа установлена в {} (версия {old}), а сейчас запущена отдельная копия версии {v}.\n\n{action} · [Открыть файл…](vmd:open)",
                md_code(&i.dir.display().to_string())
            )
        } else {
            let dir = install::Scope::Machine.default_dir();
            trf!(
                "> [!IMPORTANT]\n> The program is not installed yet. Installing copies a single exe file to {} and registers it for `.md` files, like any regular program (Windows will ask you to confirm administrator rights). Without administrator rights you can install it just for yourself.\n\n**[Install and make it the app for .md](vmd:install)** · [Open file…](vmd:open)\n\nVersion {v}. You can also use it without installing — open files with Ctrl+O or drag them into the window.",
                "> [!IMPORTANT]\n> Программа ещё не установлена. Установка копирует один exe-файл в {} и регистрирует его для файлов `.md` — как обычная программа (Windows попросит подтвердить права администратора). Без прав администратора можно установить только для себя.\n\n**[Установить и сделать программой для .md](vmd:install)** · [Открыть файл…](vmd:open)\n\nВерсия {v}. Можно пользоваться и без установки — открывайте файлы через Ctrl+O или перетаскивайте их в окно.",
                md_code(&dir.display().to_string())
            )
        };
        welcome_md().replace("{{STATUS}}", &status)
    }

    fn show_welcome(&self, push: bool) {
        if push {
            let cur = self.doc.borrow().path.clone();
            if let Some(cur) = cur {
                let a = self.view_anchor();
                self.history.borrow_mut().push((cur, a));
            }
        }
        {
            let mut d = self.doc.borrow_mut();
            d.path = None;
            d.text = self.welcome_text();
            d.encoding = Encoding::Utf8;
            d.eol = Eol::Crlf;
            d.modified = false;
            d.welcome = true;
            d.mtime = None;
            d.src_loaded = false;
            d.view_stale = false;
            d.lossy = false;
            d.conflict = false;
        }
        if self.mode.get() != Mode::View {
            self.mode.set(Mode::View);
            unsafe {
                let _ = ShowWindow(self.src(), SW_HIDE);
                let _ = ShowWindow(self.view, SW_SHOW);
            }
            self.layout();
        }
        self.render_view(false);
        self.show_setup_bar();
        self.update_title();
        self.update_bar();
        self.update_status();
    }

    fn navigate(&self, path: &Path, anchor: Option<String>) {
        if !self.confirm_discard() {
            return;
        }
        let same = self.doc.borrow().path.as_deref() == Some(path);
        if !same && !self.open_path(path, true, false) {
            return;
        }
        if let Some(a) = anchor {
            self.scroll_to_anchor(&a);
        }
    }

    fn go_back(&self) {
        let Some((p, a)) = self.history.borrow_mut().pop() else { return };
        if !self.confirm_discard() {
            self.history.borrow_mut().push((p, a));
            return;
        }
        self.next_anchor.set(a);
        if !self.open_path(&p, false, false) {
            self.next_anchor.set(None);
        }
        self.update_bar();
    }

    fn check_external_change(&self) {
        let (path, mtime, modified) = {
            let d = self.doc.borrow();
            (d.path.clone(), d.mtime, d.modified)
        };
        let Some(path) = path else { return };
        let now = doc::mtime(&path);
        if now.is_some() && now != mtime {
            // Remember the time right away: if the file fails to reload (it became huge, is locked),
            // reactivating the window must not show the error again and again.
            self.doc.borrow_mut().mtime = now;
            if modified {
                self.doc.borrow_mut().conflict = true;
                self.set_status(tr!(
                    "The file was changed on disk by another program — autosave stopped. F5 reloads it (your changes will be lost), Ctrl+S writes your version.",
                    "Файл изменён на диске другой программой — автосохранение остановлено. F5 — перечитать (ваши правки пропадут), Ctrl+S — записать свою версию."
                ));
            } else {
                match doc::load(&path) {
                    Ok(_) => self.reload_keep_scroll(),
                    Err(e) => self.set_status(&trf!("The file changed on disk but was not reloaded: {e}", "Файл изменён на диске, но не перечитан: {e}")),
                }
            }
        }
    }

    fn reload_keep_scroll(&self) {
        let path = self.doc.borrow().path.clone();
        let Some(path) = path else { return };
        let sel = richedit::get_sel(self.src());
        self.next_anchor.set(self.view_anchor());
        if self.open_path(&path, false, false) {
            if self.mode.get() != Mode::View {
                richedit::set_sel(self.src(), sel.0, sel.0);
            }
            self.set_status(tr!("File reloaded from disk", "Файл перечитан с диска"));
        }
    }

    // ---------------- view rendering ----------------

    fn render_view(&self, keep_scroll: bool) {
        let anchor = match self.next_anchor.take() {
            Some(a) => Some(a),
            None if keep_scroll => self.view_anchor(),
            None => None,
        };
        self.render_view_at(anchor);
    }

    /// Text width for the RTF, accounting for zoom: RichEdit scales all of the content,
    /// so tables and code blocks are computed for the "unzoomed" width.
    fn view_zoom(&self) -> i32 {
        (self.settings.borrow().zoom as i32).clamp(10, 500)
    }

    fn render_view_at(&self, anchor: Option<Anchor>) {
        self.stop_smooth();
        // RichEdit margins and table width are computed by one function (column), from the current window.
        self.apply_margins();
        let (text, base) = {
            let d = self.doc.borrow();
            (d.text.clone(), d.path.as_ref().and_then(|p| p.parent().map(|x| x.to_path_buf())))
        };
        let width_px = self.view_text_width();
        let dpi = self.dpi.get() as i32;
        let zoom = self.view_zoom();
        let theme = self.theme.get();
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.view, &mut rc);
        }
        let px_to_twips = |px: i32| px * 1440 * 100 / (dpi * zoom);
        // width_px is already "before zoom"; minus 1 px for margin rounding inside RichEdit.
        let mut opts = RenderOpts::new(&theme, ((width_px - 1) * 1440 / dpi).min(MAX_COL_TWIPS));
        opts.base_dir = base.as_deref();
        opts.top_pad_twips = 16 * 1440 / 96;
        opts.tail_twips = px_to_twips(rc.bottom) * 55 / 100;
        // Safety net: an unexpected panic while parsing must not crash the program;
        // in that case the document is shown as plain text.
        let mut rendered = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render::render(&text, &opts))) {
            Ok(r) => r,
            Err(_) => {
                let fallback = format!("```\n{}\n```\n", text.replace("```", "'''"));
                render::render(&fallback, &opts)
            }
        };
        let n = rendered.chunk_count();
        // Which chunk to start with: the one that contained the target place.
        let start = match anchor {
            Some(Anchor { ratio: Some(r), .. }) => ((r.clamp(0.0, 1.0) * n as f64) as usize).min(n - 1),
            Some(a) => rendered.splits.partition_point(|s| s.1 <= a.src_off).saturating_sub(1),
            None => 0,
        };

        self.suppress.set(self.suppress.get() + 1);
        let frozen = freeze(self.view);
        richedit::set_rtf(self.view, &rendered.chunk_rtf(start, true), false);
        // Replacing all the text resets RichEdit's zoom to 100%; restore it.
        richedit::set_zoom(self.view, zoom as u32);
        let images = std::mem::take(&mut rendered.images);
        let gen = {
            let mut v = self.vs.borrow_mut();
            v.gen += 1;
            v.links = std::mem::take(&mut rendered.links);
            v.anchors = std::mem::take(&mut rendered.anchors);
            v.images = images.clone();
            v.width_px = width_px;
            v.hover = None;
            v.remote_waiting.clear();
            v.failed.clear();
            v.pending = 0;
            v.jobs_by_key.clear();
            v.job_key.clear();
            v.pending_picts.clear();
            v.pending_unhide.clear();
            v.gen
        };
        *self.loader.borrow_mut() = Some(Loader { r: rendered, lo: start, hi: start + 1, start, cp_start: vec![0] });
        // Jobs from previous renders are no longer needed.
        self.pool.borrow().set_gen(gen);
        self.reindex();

        // There must be at least two screens of text below the target place.
        let want_y = match anchor {
            Some(a) if a.ratio.is_none() => {
                let len = richedit::text_len(self.view);
                let cp = a.delta.clamp(0, (len - 2).max(0));
                Some((cp, a.y))
            }
            _ => None,
        };
        loop {
            let more = self.loader.borrow().as_ref().map(|l| l.hi < l.r.chunk_count()).unwrap_or(false);
            if !more {
                break;
            }
            let top = want_y.map(|(cp, y)| richedit::pos_from_char(self.view, cp).1 - y).unwrap_or(0);
            let len = richedit::text_len(self.view);
            let bottom = richedit::pos_from_char(self.view, (len - 1).max(0)).1;
            if bottom - top > rc.bottom * 2 {
                break;
            }
            self.load_chunk(true);
        }

        // Images
        let policy = self.settings.borrow().remote_images;
        let choice = self.doc.borrow().path.as_ref().and_then(|p| self.remote_choice.borrow().get(p).copied());
        let allow_remote = policy == RemoteImages::Always || choice == Some(true);
        let ask_remote = policy == RemoteImages::Ask && choice.is_none();
        let max_w = width_px.max(64) as u32;
        let mut immediate: Vec<(usize, Rc<String>)> = Vec::new();
        let mut unhide: Vec<usize> = Vec::new();
        for img in &images {
            let key = image_key(img, max_w, dpi as u32, base.as_deref());
            if let Some(p) = self.pict_cache.borrow().get(&key) {
                immediate.push((img.id, p.clone()));
                continue;
            }
            match images::resolve(&img.src, base.as_deref()) {
                Ok(src) => {
                    let remote = matches!(src, Source::Remote(_));
                    if remote && !allow_remote {
                        unhide.push(img.id);
                        if ask_remote {
                            self.vs.borrow_mut().remote_waiting.push(img.id);
                        }
                        continue;
                    }
                    self.submit_image(gen, img, src, max_w, key);
                }
                Err(e) => {
                    self.vs.borrow_mut().failed.push((short_src(&img.src), e));
                    unhide.push(img.id);
                }
            }
        }
        self.apply_pictures(immediate);
        for id in unhide {
            self.unhide_image_label(id);
        }
        match want_y {
            Some((cp, y)) => {
                let abs = richedit::pos_from_char(self.view, cp).1;
                richedit::set_scroll_pos(self.view, 0, (abs - y).max(0));
            }
            None => {
                richedit::set_sel(self.view, 0, 0);
                richedit::set_scroll_pos(self.view, 0, 0);
            }
        }
        unfreeze(self.view, frozen);
        self.suppress.set(self.suppress.get() - 1);
        self.doc.borrow_mut().view_stale = false;
        let n_remote = self.vs.borrow().remote_waiting.len();
        self.show_info_bar(n_remote);
        self.report_image_failures();
        if self.loader.borrow().as_ref().map(|l| l.done()).unwrap_or(true) {
            self.on_load_done(anchor);
        } else {
            if let Some(Anchor { ratio: Some(_), .. }) = anchor {
                self.next_ratio.set(anchor.and_then(|a| a.ratio));
            }
            unsafe {
                SetTimer(Some(self.hwnd), T_LOAD, 1, None);
            }
        }
    }

    /// Topmost visible character of the view → place in the document (to re-render "in the same place").
    fn view_anchor(&self) -> Option<Anchor> {
        let l = self.loader.borrow();
        let l = l.as_ref()?;
        let r = richedit::get_rect(self.view);
        let cp = richedit::char_from_pos(self.view, r.left + 2, 2);
        let y = richedit::pos_from_char(self.view, cp).1;
        let k = l.cp_start.partition_point(|&c| c <= cp).saturating_sub(1);
        let chunk = (l.lo + k).min(l.r.chunk_count().saturating_sub(1));
        Some(Anchor { src_off: l.r.splits.get(chunk)?.1, delta: cp - l.cp_start.get(k).copied().unwrap_or(0), y, ratio: None })
    }

    /// Load one more chunk: at the end (`append`) or at the start. false: nothing left to load.
    fn load_chunk(&self, append: bool) -> bool {
        let rtf = {
            let l = self.loader.borrow();
            let Some(l) = l.as_ref() else { return false };
            if append {
                if l.hi >= l.r.chunk_count() {
                    return false;
                }
                l.r.chunk_rtf(l.hi, false)
            } else {
                if l.lo == 0 {
                    return false;
                }
                l.r.chunk_rtf(l.lo - 1, false)
            }
        };
        let view = self.view;
        let old_len = richedit::text_len(view);
        let at = if append { render::chunk_append_pos(old_len) } else { 0 };
        let (sx, sy) = richedit::scroll_pos(view);
        // Inserting above the screen pushes the text down, so we compensate the scroll to keep
        // the place being read from jumping. Redrawing is turned off meanwhile.
        let frozen = if append { false } else { freeze(view) };
        let probe = if append {
            None
        } else {
            let r = richedit::get_rect(view);
            let cp = richedit::char_from_pos(view, r.left + 2, 2);
            Some((cp, richedit::pos_from_char(view, cp).1))
        };
        // TOM: inserting doesn't touch the selection or scroll to the caret.
        let done = richedit::insert_rtf(view, at, &rtf);
        if !done {
            let sel = richedit::get_sel(view);
            richedit::set_readonly(view, false);
            richedit::set_sel(view, at, at);
            richedit::set_rtf(view, &rtf, true);
            richedit::set_readonly(view, true);
            richedit::set_sel(view, sel.0, sel.1);
        }
        let ins = richedit::text_len(view) - old_len;
        match probe {
            Some((cp, y0)) => {
                let y1 = richedit::pos_from_char(view, cp + ins).1;
                let dy = y1 - y0;
                richedit::set_scroll_pos(view, sx, sy + dy);
                let mut sm = self.smooth.get();
                if sm.active && sm.ctl == view {
                    sm.pos += dy as f64;
                    sm.target += dy as f64;
                    self.smooth.set(sm);
                }
                unfreeze(view, frozen);
            }
            None => {
                if !done {
                    richedit::set_scroll_pos(view, sx, sy);
                }
            }
        }
        {
            let mut l = self.loader.borrow_mut();
            if let Some(l) = l.as_mut() {
                if append {
                    l.cp_start.push(at);
                    l.hi += 1;
                } else {
                    for c in l.cp_start.iter_mut() {
                        *c += ins;
                    }
                    l.cp_start.insert(0, 0);
                    l.lo -= 1;
                }
            }
        }
        self.reindex();
        self.apply_pending_pictures();
        true
    }

    /// One background loading step (T_LOAD timer): first a couple of chunks below, then above and below in turn.
    fn load_step(&self) {
        let state = self.loader.borrow().as_ref().map(|l| (l.lo, l.hi, l.start, l.r.chunk_count(), l.done()));
        let Some((lo, hi, start, n, done)) = state else {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), T_LOAD);
            }
            return;
        };
        if done {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), T_LOAD);
            }
            self.on_load_done(None);
            return;
        }
        let append = hi < n && (hi - start < 3 || lo == 0 || (hi - start) <= (start - lo));
        self.suppress.set(self.suppress.get() + 1);
        self.load_chunk(append);
        self.suppress.set(self.suppress.get() - 1);
        if self.loader.borrow().as_ref().map(|l| l.done()).unwrap_or(true) {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), T_LOAD);
            }
            self.on_load_done(None);
        }
    }

    /// Load everything at once (search, jump to a section, "select all", snapshot, self-test).
    fn finish_loading(&self) {
        let mut any = false;
        loop {
            let st = self.loader.borrow().as_ref().map(|l| (l.done(), l.hi < l.r.chunk_count()));
            match st {
                Some((false, more_below)) => {
                    any = true;
                    self.suppress.set(self.suppress.get() + 1);
                    self.load_chunk(more_below);
                    self.suppress.set(self.suppress.get() - 1);
                }
                _ => break,
            }
        }
        if any {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), T_LOAD);
            }
            self.on_load_done(None);
        }
    }

    fn loading(&self) -> bool {
        self.loader.borrow().as_ref().map(|l| !l.done()).unwrap_or(false)
    }

    /// Position between characters under a point: the right half of a character means after it.
    fn view_cp_at(&self, x: i32, y: i32) -> i32 {
        let cp = richedit::char_from_pos(self.view, x, y);
        let (x0, y0) = richedit::pos_from_char(self.view, cp);
        let (x1, y1) = richedit::pos_from_char(self.view, cp + 1);
        if y1 == y0 && x1 > x0 && x > (x0 + x1) / 2 {
            cp + 1
        } else {
            cp
        }
    }

    // ---------------- smooth scrolling ----------------
    fn stop_smooth(&self) {
        let mut sm = self.smooth.get();
        if sm.active {
            sm.active = false;
            self.smooth.set(sm);
            unsafe {
                let _ = KillTimer(Some(self.hwnd), T_SMOOTH);
            }
        }
    }

    /// Wheel step: the line height of normal text, accounting for DPI and zoom.
    fn line_px(&self, ctl: HWND) -> f64 {
        let zoom = (richedit::get_zoom(ctl).max(10)) as f64;
        21.0 * self.dpi.get() as f64 / 96.0 * zoom / 100.0
    }

    /// Smoothly scroll by `dy` px (wheel, keys). Repeated calls accumulate the target.
    fn smooth_scroll_by(&self, ctl: HWND, dy: f64) {
        let mut sm = self.smooth.get();
        if !sm.active || sm.ctl != ctl {
            let y = richedit::scroll_pos(ctl).1 as f64;
            sm = Smooth { ctl, pos: y, target: y, active: false, last: None };
        }
        let max = max_scroll(ctl);
        sm.target = (sm.target + dy).clamp(0.0, max);
        if (sm.target - sm.pos).abs() < 0.5 {
            sm.active = false;
            self.smooth.set(sm);
            return;
        }
        if !sm.active {
            // First frame right away, for an instant response. After that the message loop drives the frames
            // (run: DwmFlush, at the monitor's refresh rate); the timer is a fallback for modal loops
            // (menus, window dragging) where our loop doesn't run.
            sm.last = Some(std::time::Instant::now() - std::time::Duration::from_millis(7));
            sm.active = true;
            unsafe {
                SetTimer(Some(self.hwnd), T_SMOOTH, 30, None);
            }
            self.smooth.set(sm);
            self.smooth_frame();
        } else {
            self.smooth.set(sm);
        }
    }

    /// Fallback timer: step only if the frame loop hasn't done so itself for a while.
    fn smooth_tick(&self) {
        let sm = self.smooth.get();
        if !sm.active {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), T_SMOOTH);
            }
            return;
        }
        if sm.last.map(|t| t.elapsed() > std::time::Duration::from_millis(25)).unwrap_or(true) {
            self.smooth_frame();
        }
    }

    /// One smooth scrolling frame (based on elapsed time, so the same at 60 and 144 Hz).
    fn smooth_frame(&self) {
        let mut sm = self.smooth.get();
        if !sm.active {
            return;
        }
        self.smooth_frames.set(self.smooth_frames.get().wrapping_add(1));
        let now = std::time::Instant::now();
        let dt = sm.last.map(|t| (now - t).as_secs_f64()).unwrap_or(0.016).clamp(0.001, 0.05);
        sm.last = Some(now);
        // Exponential approach to the target: ≈ 60 ms for most of the distance.
        let k = 1.0 - (-dt / 0.06).exp();
        sm.pos += (sm.target - sm.pos) * k;
        if (sm.target - sm.pos).abs() < 0.75 {
            sm.pos = sm.target;
            sm.active = false;
            unsafe {
                let _ = KillTimer(Some(self.hwnd), T_SMOOTH);
            }
        }
        self.smooth.set(sm);
        let x = richedit::scroll_pos(sm.ctl).0;
        richedit::set_scroll_pos(sm.ctl, x, sm.pos.round() as i32);
        unsafe {
            let _ = UpdateWindow(sm.ctl);
        }
        if sm.ctl == self.src() && self.mode.get() == Mode::Code {
            self.sync_preview_scroll();
        }
    }

    /// Mouse wheel: with Ctrl, zoom; otherwise smooth scrolling.
    fn on_wheel(&self, ctl: HWND, wp: WPARAM) {
        let delta = util::hiword(wp.0) as i16 as i32;
        let keys = util::loword(wp.0) as u32;
        if keys & 0x0008 != 0 {
            // Touchpads send small steps; accumulate them up to a full "notch".
            let acc = self.zoom_acc.get() + delta;
            if acc.abs() >= 120 {
                self.zoom_acc.set(0);
                self.zoom_by(if acc > 0 { 10 } else { -10 });
            } else {
                self.zoom_acc.set(acc);
            }
            return;
        }
        let dy = self.wheel_step(ctl, delta as f64 / 120.0);
        self.smooth_scroll_by(ctl, dy);
    }

    /// Scroll distance in pixels for `notches` wheel notches (positive — away from the user, i.e. up):
    /// the Windows "lines per notch" setting, or a page when it is set to "one screen".
    fn wheel_step(&self, ctl: HWND, notches: f64) -> f64 {
        let mut lines: u32 = 3;
        unsafe {
            let _ = SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut lines as *mut u32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        }
        let page = {
            let mut rc = RECT::default();
            unsafe {
                let _ = GetClientRect(ctl, &mut rc);
            }
            (rc.bottom as f64 * 0.9).max(40.0)
        };
        if lines == u32::MAX {
            -notches * page
        } else {
            -notches * lines.clamp(1, 20) as f64 * self.line_px(ctl)
        }
    }

    // ---------------- inline `code` background ----------------

    /// Background of inline `code` chips in the view: a semi-transparent rectangle of text height
    /// on top of what RichEdit has already painted (its \highlight fills the whole line height).
    /// Drawn only in the updated region so it isn't applied twice.
    ///
    /// Chip geometry (position queries to RichEdit take ~50 µs each) is computed once
    /// and stored in document coordinates: on a scroll frame it only needs to be shifted.
    /// The cache is reset on any layout change: new text (reindex), margins, zoom,
    /// text length, document height, an image caption being shown.
    fn paint_chips(&self, hwnd: HWND, rgn: HRGN) {
        let (color, alpha) = self.chip_brush.get();
        if alpha == 0 {
            return;
        }
        let rects = {
            let Ok(mut v) = self.vs.try_borrow_mut() else { return };
            if v.chips.is_empty() {
                return;
            }
            let mut bx = RECT::default();
            unsafe {
                GetRgnBox(rgn, &mut bx);
            }
            let fr = richedit::get_rect(hwnd);
            let zoom = richedit::get_zoom(hwnd) as i32;
            let key = [fr.left, fr.top, fr.right, fr.bottom, zoom, richedit::text_len(hwnd), max_scroll(hwnd) as i32];
            if v.chip_key != key {
                v.chip_geo.clear();
                v.chip_key = key;
            }
            let sy = richedit::scroll_pos(hwnd).1;
            // Visible character range (with a margin: in tables the character order isn't strictly top to bottom).
            let cp_a = richedit::char_from_pos(hwnd, fr.left + 1, (bx.top - 60).max(0)) - 400;
            let cp_b = richedit::char_from_pos(hwnd, fr.right - 1, bx.bottom + 60) + 400;
            let first = v.chips.partition_point(|c| c.1 < cp_a);
            let mut doc: Option<ITextDocument> = None;
            let mut rects: Vec<[i32; 4]> = Vec::new();
            for i in first..v.chips.len().min(first + 4000) {
                let (s, e) = v.chips[i];
                if s > cp_b {
                    break;
                }
                if !v.chip_geo.contains_key(&i) {
                    if doc.is_none() {
                        doc = self.view_doc();
                    }
                    let Some(d) = doc.as_ref() else { break };
                    match self.chip_geometry(hwnd, d, s, e, sy) {
                        Some(g) => {
                            v.chip_geo.insert(i, g);
                        }
                        None => continue,
                    }
                }
                for r in &v.chip_geo[&i] {
                    let (top, bottom) = (r[1] - sy, r[3] - sy);
                    if bottom >= bx.top && top <= bx.bottom {
                        rects.push([r[0], top, r[2], bottom]);
                    }
                }
            }
            rects
        };
        if rects.is_empty() {
            return;
        }
        unsafe {
            let hdc = GetDC(Some(hwnd));
            let _ = SelectClipRgn(hdc, Some(rgn));
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, 1, 1);
            let old = SelectObject(mem, bmp.into());
            SetPixel(mem, 0, 0, COLORREF(color));
            let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: 0 };
            let blend = |x: i32, y: i32, w: i32, h: i32| {
                if w > 0 && h > 0 {
                    let _ = GdiAlphaBlend(hdc, x, y, w, h, mem, 0, 0, 1, 1, bf);
                }
            };
            for [l, top, r, bottom] in rects {
                let h = bottom - top;
                // Corners rounded by 1 px: the middle plus shorter edges.
                blend(l + 1, top, r - l - 2, h);
                blend(l, top + 1, 1, h - 2);
                blend(r - 1, top + 1, 1, h - 2);
            }
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            ReleaseDC(Some(hwnd), hdc);
        }
    }

    /// Rectangles of the inline `code` chip [s, e] (one per wrapped line) in document
    /// coordinates: y = window y + scroll `sy`. None: RichEdit gave no position (don't cache).
    fn chip_geometry(&self, hwnd: HWND, doc: &ITextDocument, s: i32, e: i32, sy: i32) -> Option<Vec<[i32; 4]>> {
        let zoom = self.view_zoom() as f64 / 100.0;
        let dpi = self.dpi.get() as f64;
        let mut out = Vec::new();
        unsafe {
            let range = doc.Range(s, e + 1).ok()?;
            let size = range.GetFont().and_then(|f| f.GetSize()).unwrap_or(10.0) as f64;
            let em = size * dpi / 72.0 * zoom;
            // Line wrap inside the chip: line by line.
            let l0 = richedit::line_from_char(hwnd, s);
            let l1 = richedit::line_from_char(hwnd, e).min(l0 + 30);
            for l in l0..=l1 {
                let a = if l == l0 { s } else { richedit::line_index(hwnd, l) };
                let b = if l == l1 { e + 1 } else { richedit::line_index(hwnd, l + 1).max(a + 1) - 1 };
                let seg = doc.Range(a, b).ok()?;
                let (mut x0, mut y0, mut x1, mut y1) = (0i32, 0i32, 0i32, 0i32);
                // tomStart(32)|TA_BASELINE(24)|TA_LEFT(0); tomEnd(0)|TA_BASELINE|TA_RIGHT(2);
                // tomClientCoord(0x100): window coordinates; tomAllowOffClient(0x200): also beyond its edge.
                const F: i32 = 0x100 | 0x200;
                if seg.GetPoint(tomConstants(F | 32 | 24), &mut x0, &mut y0).is_err() || seg.GetPoint(tomConstants(F | 24 | 2), &mut x1, &mut y1).is_err() {
                    return None;
                }
                let top = y0 - (em * 0.92).round() as i32;
                let bottom = y0 + (em * 0.30).round() as i32;
                let (l, r) = (x0.min(x1), x0.max(x1));
                if r > l {
                    out.push([l, top + sy, r, bottom + sy]);
                }
            }
        }
        Some(out)
    }

    /// Color emoji on top of RichEdit's monochrome ones (emoji.rs), only in the updated region.
    /// Geometry is cached in document coordinates the same way as for the chips.
    fn paint_emoji(&self, hwnd: HWND, rgn: HRGN) {
        let items = {
            let Ok(mut v) = self.vs.try_borrow_mut() else { return };
            if v.emoji.is_empty() {
                return;
            }
            let mut bx = RECT::default();
            unsafe {
                GetRgnBox(rgn, &mut bx);
            }
            let fr = richedit::get_rect(hwnd);
            let zoom = richedit::get_zoom(hwnd) as i32;
            let key = [fr.left, fr.top, fr.right, fr.bottom, zoom, richedit::text_len(hwnd), max_scroll(hwnd) as i32];
            if v.emoji_key != key {
                v.emoji_geo.clear();
                v.emoji_key = key;
            }
            let sy = richedit::scroll_pos(hwnd).1;
            let cp_a = richedit::char_from_pos(hwnd, fr.left + 1, (bx.top - 60).max(0)) - 400;
            let cp_b = richedit::char_from_pos(hwnd, fr.right - 1, bx.bottom + 60) + 400;
            let first = v.emoji.partition_point(|c| c.1 < cp_a);
            let mut doc: Option<ITextDocument> = None;
            let mut items: Vec<(usize, i32, i32, i32, f32)> = Vec::new();
            for i in first..v.emoji.len().min(first + 2000) {
                let (s, e, _) = v.emoji[i];
                if s > cp_b {
                    break;
                }
                if !v.emoji_geo.contains_key(&i) {
                    if doc.is_none() {
                        doc = self.view_doc();
                    }
                    let Some(d) = doc.as_ref() else { break };
                    let g = self.emoji_geometry(d, s, e, sy);
                    v.emoji_geo.insert(i, g);
                }
                if let Some((x0, x1, base, px)) = v.emoji_geo[&i] {
                    let b = base - sy;
                    // The glyph reaches about one em above and half an em below the baseline.
                    if b + px as i32 >= bx.top && b - 2 * px as i32 <= bx.bottom {
                        items.push((i, x0, x1, b, px));
                    }
                }
            }
            items
        };
        if items.is_empty() {
            return;
        }
        // Direct2D/DirectWrite and the emoji font take ~40 ms to load: not on the way to the
        // first picture. This paint keeps RichEdit's outlines; the renderer is created right
        // after it (WM_APP_EMOJI), and the view is repainted in color.
        if self.emoji_painter.try_borrow().map(|p| p.is_none()).unwrap_or(false) {
            if !self.emoji_posted.replace(true) {
                unsafe {
                    let _ = PostMessageW(Some(self.hwnd), WM_APP_EMOJI, WPARAM(0), LPARAM(0));
                }
            }
            return;
        }
        let Ok(mut painter) = self.emoji_painter.try_borrow_mut() else { return };
        let Some(Some(p)) = painter.as_mut() else { return };
        let fg = self.theme.get().fg;
        let Ok(v) = self.vs.try_borrow() else { return };
        unsafe {
            let hdc = GetDC(Some(hwnd));
            let _ = SelectClipRgn(hdc, Some(rgn));
            for (i, x0, x1, base, px) in items {
                p.paint(hdc, &v.emoji[i].2, px, fg, x0, x1, base);
            }
            ReleaseDC(Some(hwnd), hdc);
        }
    }

    /// Creates the color emoji renderer (once; None stays None if Direct2D is unavailable).
    fn ensure_emoji_painter(&self) {
        if let Ok(mut p) = self.emoji_painter.try_borrow_mut() {
            p.get_or_insert_with(crate::emoji::Painter::new);
        }
    }

    /// An emoji cluster [s, e): its advance in window x, the baseline in document coordinates
    /// (y + scroll `sy`) and the font size in pixels. None: not drawn (hidden text, wrapped, no position).
    fn emoji_geometry(&self, doc: &ITextDocument, s: i32, e: i32, sy: i32) -> Option<(i32, i32, i32, f32)> {
        unsafe {
            let range = doc.Range(s, e).ok()?;
            let font = range.GetFont().ok()?;
            // tomTrue = -1.
            if font.GetHidden().map(|h| h == -1).unwrap_or(false) {
                return None;
            }
            let size = font.GetSize().ok()? as f64;
            let (mut x0, mut y0, mut x1, mut y1) = (0i32, 0i32, 0i32, 0i32);
            // Window coordinates (also off the window), at the baseline: the left edge of the
            // first character and the left edge of the position after the cluster (tomEnd with
            // TA_RIGHT would be the right edge of the next character — a comma, a space).
            const F: i32 = 0x100 | 0x200;
            range.GetPoint(tomConstants(F | 32 | 24), &mut x0, &mut y0).ok()?;
            range.GetPoint(tomConstants(F | 24), &mut x1, &mut y1).ok()?;
            if y0 != y1 || x1 - x0 < 2 {
                return None;
            }
            let px = size * self.dpi.get() as f64 / 72.0 * (self.view_zoom() as f64 / 100.0);
            Some((x0, x1, y0 + sy, px as f32))
        }
    }

    /// The document is fully loaded: deferred actions.
    fn on_load_done(&self, _anchor: Option<Anchor>) {
        if let Some(r) = self.next_ratio.take() {
            set_scroll_ratio(self.view, r);
        }
        if self.sync_after_load.replace(false) && self.mode.get() == Mode::Code {
            self.sync_preview_scroll();
        }
    }

    /// Queue an image. Identical images (same address and size) are loaded
    /// by a single job, and the result goes to all places.
    fn submit_image(&self, gen: u64, img: &ImageReq, src: Source, max_w: u32, key: u64) {
        {
            let mut v = self.vs.borrow_mut();
            if let Some(ids) = v.jobs_by_key.get_mut(&key) {
                ids.push(img.id);
                return;
            }
            v.pending += 1;
            v.jobs_by_key.insert(key, vec![img.id]);
            v.job_key.insert(img.id, key);
        }
        self.pool.borrow_mut().submit(Job { gen, id: img.id, source: src, max_w, want_w: img.width, want_h: img.height, dpi: self.dpi.get() });
    }

    /// Rebuild marker positions from the control's text.
    fn reindex(&self) {
        let text = richedit::get_text(self.view);
        let mut v = self.vs.borrow_mut();
        v.link_pos.clear();
        v.anchor_pos.clear();
        v.image_pos.clear();
        v.chips.clear();
        v.chip_geo.clear();
        v.emoji = render::emoji_clusters(&text).into_iter().map(|(s, e)| (s, e, text[s as usize..e as usize].into())).collect();
        v.emoji_geo.clear();
        let mut stack: Vec<(usize, i32)> = Vec::new();
        let mut img_open: Option<(usize, i32)> = None;
        let mut chip_open: Option<i32> = None;
        for (i, &c) in text.iter().enumerate() {
            let u = c as u32;
            if c == render::CODE_PAD {
                match chip_open.take() {
                    Some(s) => v.chips.push((s, i as i32)),
                    None => chip_open = Some(i as i32),
                }
                continue;
            }
            if c == 13 || c == 7 {
                // A marker pair only within a single paragraph/cell.
                chip_open = None;
                continue;
            }
            if !(0xE000..=0xF8FF).contains(&u) {
                continue;
            }
            let i = i as i32;
            if (render::LINK_START..render::LINK_START + render::MAX_LINKS as u32).contains(&u) {
                stack.push(((u - render::LINK_START) as usize, i));
            } else if u == render::LINK_END {
                if let Some((id, s)) = stack.pop() {
                    v.link_pos.push((s, i, id));
                }
            } else if (render::ANCHOR..render::ANCHOR + render::MAX_ANCHORS as u32).contains(&u) {
                v.anchor_pos.insert((u - render::ANCHOR) as usize, i);
            } else if (render::IMAGE..render::IMAGE + render::MAX_IMAGES as u32).contains(&u) {
                img_open = Some(((u - render::IMAGE) as usize, i));
            } else if u == render::IMAGE_END {
                if let Some((id, s)) = img_open.take() {
                    v.image_pos.insert(id, (s, i));
                }
            }
        }
        v.link_pos.sort_by_key(|x| x.0);
    }

    /// Insert ready images (in descending position order so the others don't shift).
    fn apply_pictures(&self, mut items: Vec<(usize, Rc<String>)>) {
        if items.is_empty() {
            return;
        }
        let positions = self.vs.borrow().image_pos.clone();
        // The document chunks with these images aren't loaded yet; insert them later.
        let (later, now): (Vec<_>, Vec<_>) = items.into_iter().partition(|(id, _)| !positions.contains_key(id) && self.loading());
        self.vs.borrow_mut().pending_picts.extend(later);
        items = now;
        items.retain(|(id, _)| positions.contains_key(id));
        if items.is_empty() {
            return;
        }
        items.sort_by_key(|(id, _)| std::cmp::Reverse(positions[id].0));
        let (sx, sy) = richedit::scroll_pos(self.view);
        // Edits to a read-only control: clear the flag temporarily.
        // Insertion goes exactly into the [start, end] range of the hidden group via TOM
        // (a RichEdit selection "spreads" into adjacent hidden text, i.e. link markers).
        richedit::set_readonly(self.view, false);
        let doc = self.view_doc();
        for (id, pict) in &items {
            let (s, e) = positions[id];
            let rtf = format!("{{\\rtf1{{\\v0 {}}}}}", pict);
            let mut done = false;
            if let Some(doc) = &doc {
                unsafe {
                    if let Ok(r2) = doc.Range(s, e + 1).and_then(|r| r.cast::<ITextRange2>()) {
                        done = r2.SetText2(tomConvertRTF.0, &windows::core::BSTR::from(rtf.as_str())).is_ok();
                    }
                }
            }
            if !done {
                richedit::set_sel(self.view, s, e + 1);
                richedit::set_rtf(self.view, &rtf, true);
            }
        }
        richedit::set_readonly(self.view, true);
        richedit::set_scroll_pos(self.view, sx, sy);
        self.reindex();
    }

    fn view_doc(&self) -> Option<ITextDocument> {
        richedit::tom(self.view)
    }

    /// After a chunk loads: insert the images and reveal the captions that were waiting for it.
    fn apply_pending_pictures(&self) {
        let (picts, unhide) = {
            let mut v = self.vs.borrow_mut();
            if v.pending_picts.is_empty() && v.pending_unhide.is_empty() {
                return;
            }
            let pos = v.image_pos.clone();
            let (now_p, later_p): (Vec<_>, Vec<_>) = std::mem::take(&mut v.pending_picts).into_iter().partition(|(id, _)| pos.contains_key(id));
            let (now_u, later_u): (Vec<_>, Vec<_>) = std::mem::take(&mut v.pending_unhide).into_iter().partition(|id| pos.contains_key(id));
            v.pending_picts = later_p;
            v.pending_unhide = later_u;
            (now_p, now_u)
        };
        for id in unhide {
            self.unhide_image_label(id);
        }
        self.apply_pictures(picts);
    }

    fn unhide_image_label(&self, id: usize) {
        let pos = self.vs.borrow().image_pos.get(&id).copied();
        let Some((s, e)) = pos else {
            if self.loading() {
                self.vs.borrow_mut().pending_unhide.push(id);
            }
            return;
        };
        let Some(doc) = self.view_doc() else { return };
        // The caption becomes visible, so the chips on this line shift.
        self.vs.borrow_mut().chip_geo.clear();
        richedit::set_readonly(self.view, false);
        unsafe {
            if let Ok(f) = doc.Range(s + 1, e).and_then(|r| r.GetFont()) {
                let _ = f.SetHidden(0);
            }
        }
        richedit::set_readonly(self.view, true);
    }

    fn on_images_ready(&self) {
        let done = self.pool.borrow().take();
        if done.is_empty() {
            return;
        }
        let gen = self.vs.borrow().gen;
        let mut ok: Vec<(usize, Rc<String>)> = Vec::new();
        let mut bad: Vec<usize> = Vec::new();
        let mut remote_failed: Vec<(String, String)> = Vec::new();
        for d in done {
            if d.gen != gen {
                continue;
            }
            let (key, ids) = {
                let mut v = self.vs.borrow_mut();
                v.pending = v.pending.saturating_sub(1);
                let key = v.job_key.remove(&d.id);
                let ids = key.and_then(|k| v.jobs_by_key.remove(&k)).unwrap_or_else(|| vec![d.id]);
                (key, ids)
            };
            match d.result {
                Ok(p) => {
                    let p = Rc::new(p);
                    if let Some(k) = key {
                        let mut cache = self.pict_cache.borrow_mut();
                        if cache.len() > 300 {
                            cache.clear();
                        }
                        cache.insert(k, p.clone());
                    }
                    for id in ids {
                        ok.push((id, p.clone()));
                    }
                }
                Err(e) => {
                    let (src, remote) = self.vs.borrow().images.get(d.id).map(|i| (short_src(&i.src), is_remote_src(&i.src))).unwrap_or_default();
                    if remote {
                        remote_failed.push((src.clone(), e.clone()));
                    }
                    self.vs.borrow_mut().failed.push((src, e));
                    bad.extend(ids);
                }
            }
        }
        self.suppress.set(self.suppress.get() + 1);
        let frozen = freeze(self.view);
        for id in &bad {
            self.unhide_image_label(*id);
        }
        self.apply_pictures(ok);
        unfreeze(self.view, frozen);
        self.suppress.set(self.suppress.get() - 1);
        self.report_image_failures();
        // Images from the internet that the user asked for and that failed: say why right in
        // the bar (the status bar is overwritten by hover hints, and "Load" looked broken).
        if let Some((src, why)) = remote_failed.last() {
            let n = remote_failed.len();
            self.show_info_message(&trf!(
                "Could not load images from the internet: {n}. Last one: {src} — {why}",
                "Не удалось загрузить картинки из интернета: {n}. Последняя: {src} — {why}"
            ));
        }
    }

    /// Remember the answer about images from the internet for the current document.
    fn remember_remote_choice(&self, load: bool) {
        if let Some(p) = self.doc.borrow().path.clone() {
            self.remote_choice.borrow_mut().insert(p, load);
        }
    }

    /// The yellow bar with a message and only a "✕" button.
    fn show_info_message(&self, text: &str) {
        ui::set_items(self.info, vec![Item::button(C_INFO_CLOSE, "✕", tr!("Hide", "Скрыть")).right()]);
        ui::set_text(self.info, text, "");
        if !self.info_visible.get() {
            self.info_visible.set(true);
            unsafe {
                let _ = ShowWindow(self.info, SW_SHOW);
            }
            self.layout();
        }
    }

    fn report_image_failures(&self) {
        let v = self.vs.borrow();
        if v.failed.is_empty() {
            return;
        }
        let (src, why) = &v.failed[v.failed.len() - 1];
        let msg = if v.failed.len() == 1 {
            trf!("Image not shown: {src} — {why}", "Картинка не показана: {src} — {why}")
        } else {
            trf!("Images not shown: {}. Last one: {src} — {why}", "Не показано картинок: {}. Последняя: {src} — {why}", v.failed.len())
        };
        drop(v);
        self.set_status(&msg);
    }

    fn load_remote_images(&self) {
        let (waiting, gen) = {
            let mut v = self.vs.borrow_mut();
            (std::mem::take(&mut v.remote_waiting), v.gen)
        };
        let base = self.base_dir();
        let max_w = self.vs.borrow().width_px.max(64) as u32;
        let dpi = self.dpi.get();
        for id in waiting {
            let img = self.vs.borrow().images.get(id).cloned();
            let Some(img) = img else { continue };
            if let Ok(src) = images::resolve(&img.src, base.as_deref()) {
                let key = image_key(&img, max_w, dpi, base.as_deref());
                self.submit_image(gen, &img, src, max_w, key);
            }
        }
        self.show_info_bar(0);
    }

    // ---------------- links in the view ----------------

    fn view_hit(&self, x: i32, y: i32) -> Option<usize> {
        let cp = richedit::char_from_pos(self.view, x, y);
        if cp < 0 {
            return None;
        }
        let (x0, y0) = richedit::pos_from_char(self.view, cp);
        let (x1, y1) = richedit::pos_from_char(self.view, cp + 1);
        let tol = self.scale(2);
        let within_x = if y1 == y0 && x1 > x0 { x >= x0 - tol && x <= x1 + tol } else { x >= x0 - tol && x <= x0 + self.scale(14) };
        if !within_x || y < y0 - tol {
            return None;
        }
        let v = self.vs.try_borrow().ok()?;
        v.link_pos.iter().filter(|(s, e, _)| *s <= cp && cp <= *e).max_by_key(|(s, _, _)| *s).map(|(_, _, id)| *id)
    }

    fn view_hover(&self, x: i32, y: i32) {
        let h = self.view_hit(x, y);
        self.set_hover(h);
        if h.is_some() {
            let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.view, dwHoverTime: 0 };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
        }
    }

    fn set_hover(&self, h: Option<usize>) {
        let changed = match self.vs.try_borrow_mut() {
            Ok(mut v) => {
                let c = v.hover != h;
                v.hover = h;
                c
            }
            Err(_) => false,
        };
        if changed {
            self.update_status();
        }
    }

    fn activate_link(&self, id: usize) {
        let info = self.vs.borrow().links.get(id).cloned();
        let Some(info) = info else { return };
        self.open_url(&info.url, &info.text);
    }

    fn open_url(&self, url: &str, text: &str) {
        let base = self.base_dir();
        let welcome = self.doc.borrow().welcome;
        let owner = Some(self.hwnd);
        match links::classify(url, base.as_deref()) {
            LinkAction::Anchor(a) => self.scroll_to_anchor(&a),
            LinkAction::OpenMarkdown { path, anchor } => {
                if links::has_symlink_component(&path) {
                    util::warn(
                        owner,
                        tr!(
                            "Link blocked: the path contains a symbolic link (it could lead to a network resource).",
                            "Ссылка заблокирована: путь содержит символическую ссылку (она могла бы вести на сетевой ресурс)."
                        ),
                    );
                    return;
                }
                if !path.exists() {
                    util::warn(owner, &trf!("File not found:\n{}", "Файл не найден:\n{}", path.display()));
                    return;
                }
                self.navigate(&path, anchor);
            }
            LinkAction::Web { url, host, warn } => {
                let warn = warn.or_else(|| links::spoof_warning(text, &host));
                if let Some(w) = warn {
                    if !util::ask_yes_no(owner, &trf!("{w}\n\nOpen anyway?\n{url}", "{w}\n\nВсё равно открыть?\n{url}"), true) {
                        return;
                    }
                }
                shell_open(self.hwnd, &url);
            }
            LinkAction::Mail(m) => shell_open(self.hwnd, &m),
            LinkAction::LocalFile(p) => {
                // Checked at click time against the real (long) name: 8.3 short names
                // like X~1.LIB won't fool it. Only allowlisted types can be opened;
                // folders and everything else only get "Show in Explorer".
                let real = match links::prepare_local(&p) {
                    Ok(r) => r,
                    Err(e) => {
                        util::warn(owner, &trf!("Link blocked.\n\n{e}", "Ссылка заблокирована.\n\n{e}"));
                        return;
                    }
                };
                if real.is_file() && links::is_openable_ext(&real) {
                    let q = trf!(
                        "Open the file with its default app?\n\n{}\n\nYes — open\nNo — show in Explorer",
                        "Открыть файл программой по умолчанию?\n\n{}\n\nДа — открыть\nНет — показать в проводнике",
                        real.display()
                    );
                    match util::ask_yes_no_cancel(owner, &q) {
                        Some(true) => shell_open(self.hwnd, &real.to_string_lossy()),
                        Some(false) => reveal_in_explorer(&real),
                        None => {}
                    }
                } else {
                    let what = if real.is_dir() { tr!("Folders", "Папки") } else { tr!("Files of this type", "Файлы этого типа") };
                    let q = trf!(
                        "{what} can't be opened from a link in a document.\n\n{}\n\nShow in Explorer?",
                        "{what} не открываются по ссылке из документа.\n\n{}\n\nПоказать в проводнике?",
                        real.display()
                    );
                    if util::ask_yes_no(owner, &q, false) {
                        reveal_in_explorer(&real);
                    }
                }
            }
            LinkAction::Internal(cmd) => {
                if welcome {
                    self.internal(&cmd);
                } else {
                    util::warn(owner, tr!("Internal vmd: links work only on the program's start page.", "Служебные ссылки vmd: работают только на стартовой странице программы."));
                }
            }
            LinkAction::Blocked(reason) => {
                let addr: String = url.chars().take(300).collect();
                util::warn(owner, &trf!("Link blocked.\n\n{reason}\n\nAddress: {addr}", "Ссылка заблокирована.\n\n{reason}\n\nАдрес: {addr}"));
            }
        }
    }

    fn internal(&self, cmd: &str) {
        match cmd {
            "install" => self.launch_setup(),
            "default" => {
                // The installer did the registration; here it's only the user's choice in the system dialog.
                if util::exe_dir().is_some() {
                    install::prompt_default_with_hint(Some(self.hwnd));
                    self.show_welcome(false);
                }
            }
            "uninstall" => match install::uninstall_interactive(Some(self.hwnd)) {
                Ok(true) => unsafe {
                    let _ = DestroyWindow(self.hwnd);
                },
                Ok(false) => {}
                Err(e) => util::error(Some(self.hwnd), &e),
            },
            "open" => self.cmd_open(),
            _ => {}
        }
    }

    fn scroll_to_anchor(&self, a: &str) {
        self.finish_loading();
        let want = a.trim().to_lowercase();
        let want_slug = render::slugify(a);
        let pos = {
            let v = self.vs.borrow();
            v.anchors
                .iter()
                .enumerate()
                .find(|(_, s)| s.to_lowercase() == want || **s == want_slug)
                .and_then(|(id, _)| v.anchor_pos.get(&id).copied())
        };
        match pos {
            Some(p) => {
                if self.mode.get() == Mode::Visual {
                    self.set_mode(Mode::View);
                }
                richedit::scroll_scrollcaret_top(self.view, p);
            }
            None => self.set_status(&trf!("Section “{a}” not found", "Раздел «{a}» не найден")),
        }
    }

    // ---------------- modes ----------------

    fn load_src(&self) {
        self.ensure_src();
        let text = self.doc.borrow().text.replace('\n', "\r");
        self.suppress.set(self.suppress.get() + 1);
        let mask = richedit::set_event_mask(self.src(), 0);
        richedit::set_plain_text(self.src(), &text);
        richedit::set_sel(self.src(), 0, 0);
        richedit::set_event_mask(self.src(), mask);
        self.suppress.set(self.suppress.get() - 1);
        self.src_hash.set(hash16(&richedit::get_text(self.src())));
        self.doc.borrow_mut().src_loaded = true;
        if self.styler.borrow().is_none() {
            *self.styler.borrow_mut() = Some(editor::Styler::new(self.src(), self.theme.get()));
        }
        if let Some(s) = self.styler.borrow_mut().as_mut() {
            s.invalidate();
        }
        self.caret_line.set(0);
    }

    /// Editor text → document (if the editor was ever opened).
    fn sync_from_src(&self) {
        let loaded = self.doc.borrow().src_loaded;
        if !loaded {
            return;
        }
        let t = richedit::get_text(self.src());
        let s: String = s16(&t).chars().map(|c| if c == '\r' || c == '\u{0B}' { '\n' } else { c }).filter(|c| *c != '\u{FFFC}').collect();
        let mut d = self.doc.borrow_mut();
        if d.text != s {
            d.text = s;
            d.view_stale = true;
        }
    }

    pub fn set_mode(&self, m: Mode) {
        let old = self.mode.get();
        if old == m {
            return;
        }
        if self.doc.borrow().welcome && m != Mode::View {
            // The start page is help and isn't edited: offer to open the user's own file.
            let q = tr!(
                "This is the start page (help) — it can't be edited.\n\nOpen a .md file for editing?",
                "Это стартовая страница (справка) — её нельзя редактировать.\n\nОткрыть файл .md для редактирования?"
            );
            if util::ask_yes_no(Some(self.hwnd), q, false) {
                self.cmd_open();
                if !self.doc.borrow().welcome {
                    self.set_mode(m);
                }
            }
            self.update_bar();
            return;
        }
        if old != Mode::View {
            if m == Mode::View {
                self.autosave();
            }
            self.sync_from_src();
        }
        if m != Mode::View {
            self.ensure_src();
        }
        prof("sm_start");
        // Position: try to keep the place in the document.
        let ratio = match old {
            Mode::View => scroll_ratio(self.view),
            _ => scroll_ratio(self.src()),
        };
        self.mode.set(m);
        match m {
            Mode::View => unsafe {
                let _ = ShowWindow(self.src(), SW_HIDE);
                let _ = ShowWindow(self.view, SW_SHOW);
            },
            Mode::Visual => unsafe {
                let _ = ShowWindow(self.view, SW_HIDE);
                let _ = ShowWindow(self.src(), SW_SHOW);
            },
            Mode::Code => unsafe {
                let _ = ShowWindow(self.src(), SW_SHOW);
                let _ = ShowWindow(self.view, SW_SHOW);
            },
        }
        prof("sm_show");
        self.layout();
        prof("sm_layout");
        if m != Mode::View {
            if !self.doc.borrow().src_loaded {
                self.load_src();
            }
            // A full restyle is needed only when switching "Visual" ↔ "Code" (the styler detects
            // that itself) or for new text (load_src); returning to the same mode is almost free.
            self.restyle_now();
            prof("sm_restyle");
            set_scroll_ratio(self.src(), ratio);
            prof("sm_ratio");
            unsafe {
                let _ = SetFocus(Some(self.src()));
            }
            prof("sm_focus");
        }
        if m != Mode::Visual && self.doc.borrow().view_stale {
            if m == Mode::View {
                self.next_anchor.set(Some(Anchor { src_off: 0, delta: 0, y: 0, ratio: Some(ratio) }));
            }
            self.render_view(false);
        }
        if m == Mode::View {
            if self.loading() {
                self.next_ratio.set(Some(ratio));
            } else {
                set_scroll_ratio(self.view, ratio);
            }
            unsafe {
                let _ = SetFocus(Some(self.view));
            }
        } else if m == Mode::Code {
            self.sync_preview_scroll();
        }
        self.update_bar();
        self.update_status();
        prof("sm_bar");
    }

    fn restyle_now(&self) {
        self.restyle_pending.set(false);
        let m = self.mode.get();
        if m == Mode::View {
            return;
        }
        let caret = {
            let (a, _) = richedit::get_sel(self.src());
            self.logical_line(a)
        };
        self.caret_line.set(caret as i64);
        self.suppress.set(self.suppress.get() + 1);
        let mask = richedit::set_event_mask(self.src(), 0);
        let mut text_hash = None;
        if let Ok(mut st) = self.styler.try_borrow_mut() {
            if let Some(s) = st.as_mut() {
                s.restyle(self.src(), m == Mode::Visual, Some(caret));
                text_hash = Some(hash16(s.text()));
            }
        }
        richedit::set_event_mask(self.src(), mask);
        self.suppress.set(self.suppress.get() - 1);
        // "Modified" = the text differs from what was loaded/saved (undoing back to the original clears the mark).
        if let Some(h) = text_hash {
            let modified = h != self.src_hash.get();
            let changed = {
                let mut d = self.doc.borrow_mut();
                let c = d.modified != modified;
                d.modified = modified;
                c
            };
            if changed {
                self.update_title();
                self.update_status();
            }
        }
    }

    /// Logical line (paragraph) number; EM_EXLINEFROMCHAR counts wrapped lines.
    fn logical_line(&self, cp: i32) -> usize {
        if let Some(doc) = richedit::tom(self.src()) {
            unsafe {
                if let Ok(i) = doc.Range(cp, cp).and_then(|r| r.GetIndex(tomParagraph.0)) {
                    return (i.max(1) - 1) as usize;
                }
            }
        }
        richedit::get_range(self.src(), 0, cp).iter().filter(|&&c| c == 13).count()
    }

    fn on_src_change(&self) {
        if self.suppress.get() > 0 {
            return;
        }
        if !self.restyle_pending.get() {
            self.restyle_pending.set(true);
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_APP_RESTYLE, WPARAM(0), LPARAM(0));
            }
        }
        if self.mode.get() == Mode::Code {
            unsafe {
                SetTimer(Some(self.hwnd), T_PREVIEW, 350, None);
            }
        }
        if self.settings.borrow().autosave {
            unsafe {
                SetTimer(Some(self.hwnd), T_AUTOSAVE, 1200, None);
            }
        }
        self.update_status();
    }

    fn on_src_selchange(&self) {
        let (a, _) = richedit::get_sel(self.src());
        let line = self.logical_line(a) as i64;
        if line != self.caret_line.get() {
            self.caret_line.set(line);
            if self.mode.get() == Mode::Visual && !self.restyle_pending.get() {
                self.restyle_pending.set(true);
                unsafe {
                    let _ = PostMessageW(Some(self.hwnd), WM_APP_RESTYLE, WPARAM(0), LPARAM(0));
                }
            }
            self.update_status();
        }
    }

    fn sync_preview_scroll(&self) {
        if self.loading() {
            self.sync_after_load.set(true);
            return;
        }
        let r = scroll_ratio(self.src());
        set_scroll_ratio(self.view, r);
    }

    /// Ctrl+click on a link in the editor.
    fn src_ctrl_click(&self) {
        let (a, b) = richedit::get_sel(self.src());
        if a != b {
            return;
        }
        let line_start = richedit::line_index(self.src(), richedit::line_from_char(self.src(), a));
        let len = richedit::line_length(self.src(), a);
        let line = s16(&richedit::get_range(self.src(), line_start, line_start + len));
        // Caret position in the line: RichEdit counts in UTF-16, Rust strings in UTF-8 bytes.
        let col16 = (a - line_start).max(0) as usize;
        let s = line;
        let mut byte_col = s.len();
        let mut acc = 0usize;
        for (i, ch) in s.char_indices() {
            if acc >= col16 {
                byte_col = i;
                break;
            }
            acc += ch.len_utf16();
        }
        // Look for [text](url), <url> or a bare address around the caret.
        let mut found: Option<(String, String)> = None;
        let mut i = 0;
        while let Some(k) = s[i..].find("](") {
            let open = i + k + 2;
            if let Some(close_rel) = s[open..].find(')') {
                let close = open + close_rel;
                let lb = s[..i + k].rfind('[').unwrap_or(0);
                if byte_col >= lb && byte_col <= close {
                    let text = if s.as_bytes().get(lb) == Some(&b'[') { s[lb + 1..i + k].to_string() } else { String::new() };
                    found = Some((s[open..close].split_whitespace().next().unwrap_or("").to_string(), text));
                    break;
                }
                i = close;
            } else {
                break;
            }
        }
        if found.is_none() {
            let is_sep = |c: char| c.is_whitespace() || c == '<' || c == '(' || c == '>' || c == ')';
            // Boundaries by characters, not bytes: a non-breaking space takes 2 bytes.
            let start = s[..byte_col].char_indices().rev().find(|(_, c)| is_sep(*c)).map(|(i, c)| i + c.len_utf8()).unwrap_or(0);
            let end = s[byte_col..].char_indices().find(|(_, c)| is_sep(*c)).map(|(i, _)| i + byte_col).unwrap_or(s.len());
            let word = &s[start..end.max(start)];
            if word.starts_with("http://") || word.starts_with("https://") {
                found = Some((word.to_string(), word.to_string()));
            }
        }
        if let Some((u, text)) = found {
            // The link text is passed in to check for address spoofing: in visual mode
            // the address itself is hidden, so we compare the visible text with the real site.
            self.open_url(&u, &text);
        }
    }

    // ---------------- clipboard ----------------

    fn copy_selection(&self, ctl: HWND, cut: bool) {
        let t = richedit::get_selected_text(ctl);
        let s: String = s16(&t)
            .chars()
            .filter(|c| !(('\u{E000}'..='\u{F8FF}').contains(c) || *c == '\u{FFFC}'))
            // Padding of code chips in the view (the document has no such spaces; see render::CODE_PAD).
            .filter(|c| !(ctl == self.view && *c as u32 == render::CODE_PAD as u32))
            .map(|c| if c == '\u{0B}' { '\r' } else { c })
            .collect::<String>()
            .replace('\r', "\r\n");
        if s.is_empty() {
            return;
        }
        set_clipboard(self.hwnd, &s);
        if cut && ctl == self.src() {
            richedit::replace_sel(self.src(), "", true);
        }
    }

    fn paste_plain(&self) {
        let Some(t) = get_clipboard(self.hwnd) else { return };
        let t: String = t
            .replace("\r\n", "\r")
            .replace('\n', "\r")
            .chars()
            .filter(|c| *c == '\r' || *c == '\t' || (*c as u32) >= 0x20)
            .filter(|c| !('\u{E000}'..='\u{F8FF}').contains(c) && *c != '\u{FFFC}')
            .collect();
        richedit::replace_sel(self.src(), &t, true);
    }

    fn context_menu(&self, ctl: HWND, editable: bool) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let add = |id: u16, s: &str| {
                let w = wide(s);
                let _ = AppendMenuW(menu, MF_STRING, id as usize, PCWSTR::from_raw(w.as_ptr()));
            };
            if editable {
                add(C_CUT, tr!("Cut\tCtrl+X", "Вырезать\tCtrl+X"));
            }
            add(C_COPY, tr!("Copy\tCtrl+C", "Копировать\tCtrl+C"));
            if editable {
                add(C_PASTE, tr!("Paste\tCtrl+V", "Вставить\tCtrl+V"));
            }
            add(C_SELALL, tr!("Select all\tCtrl+A", "Выделить всё\tCtrl+A"));
            if !editable && self.vs.borrow().hover.is_some() {
                add(C_COPYLINK, tr!("Copy link address", "Копировать адрес ссылки"));
            }
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, self.hwnd, None);
            let _ = DestroyMenu(menu);
            match cmd.0 as u16 {
                C_CUT => self.copy_selection(ctl, true),
                C_COPY => self.copy_selection(ctl, false),
                C_PASTE => self.paste_plain(),
                C_SELALL => richedit::set_sel(ctl, 0, -1),
                C_COPYLINK => {
                    let u = {
                        let v = self.vs.borrow();
                        v.hover.and_then(|i| v.links.get(i).map(|l| l.url.clone()))
                    };
                    if let Some(u) = u {
                        set_clipboard(self.hwnd, &u);
                    }
                }
                _ => {}
            }
        }
    }

    // ---------------- commands ----------------

    fn command(&self, id: u16) {
        if id >= 0xF000 {
            self.activate_link((id - 0xF000) as usize);
            return;
        }
        match id {
            C_VIEW => self.set_mode(Mode::View),
            C_VISUAL => self.set_mode(Mode::Visual),
            C_CODE => self.set_mode(Mode::Code),
            C_OPEN => self.cmd_open(),
            C_SAVE => {
                self.cmd_save();
            }
            C_SAVEAS => {
                self.cmd_save_as();
            }
            C_RELOAD => self.cmd_reload(),
            C_FIND => self.open_find(),
            C_BACK => self.go_back(),
            C_HELP => {
                if !self.doc.borrow().welcome && self.confirm_discard() {
                    self.show_welcome(true);
                    self.update_bar();
                }
            }
            C_THEME => {
                let next = if self.theme.get().dark { ThemePref::Light } else { ThemePref::Dark };
                self.apply_theme(next);
                self.update_bar();
            }
            C_WIDE => self.toggle_wide(),
            C_LANG => self.toggle_lang(),
            C_ZOOM_IN => self.zoom_by(10),
            C_ZOOM_OUT => self.zoom_by(-10),
            C_ZOOM_RESET => self.zoom_set(100),
            C_REMOTE_ONCE => {
                self.remember_remote_choice(true);
                self.load_remote_images();
            }
            C_REMOTE_ALWAYS => {
                {
                    let mut s = self.settings.borrow_mut();
                    s.remote_images = RemoteImages::Always;
                    s.dirty = true;
                    s.save();
                }
                self.load_remote_images();
            }
            C_SETUP => self.launch_setup(),
            C_INFO_CLOSE => {
                let asked = !std::mem::take(&mut self.vs.borrow_mut().remote_waiting).is_empty();
                if asked {
                    self.remember_remote_choice(false);
                }
                self.show_info_bar(0);
            }
            C_BOLD => self.wrap("**", "**"),
            C_ITALIC => self.wrap("*", "*"),
            C_STRIKE => self.wrap("~~", "~~"),
            C_CODEFMT => {
                let sel = s16(&richedit::get_selected_text(self.src()));
                if sel.contains('\r') {
                    self.wrap("```\r", "\r```");
                } else {
                    self.wrap("`", "`");
                }
            }
            C_H1 => self.set_heading(1),
            C_H2 => self.set_heading(2),
            C_H3 => self.set_heading(3),
            C_UL => self.toggle_prefix(Prefix::Bullet),
            C_OL => self.toggle_prefix(Prefix::Ordered),
            C_TASK => self.toggle_prefix(Prefix::Task),
            C_QUOTE => self.toggle_prefix(Prefix::Quote),
            C_LINK => self.insert_link(),
            C_IMAGE => self.insert_image(),
            C_TABLE => self.insert_block(tr!(
                "| Column 1 | Column 2 | Column 3 |\r| --- | --- | --- |\r|  |  |  |\r|  |  |  |\r",
                "| Колонка 1 | Колонка 2 | Колонка 3 |\r| --- | --- | --- |\r|  |  |  |\r|  |  |  |\r"
            )),
            C_HR => self.insert_block("---\r"),
            _ => {}
        }
        if self.mode.get() != Mode::View && (C_BOLD..=C_TASK).contains(&id) {
            unsafe {
                let _ = SetFocus(Some(self.src()));
            }
        }
    }

    fn cmd_open(&self) {
        let dir = self.base_dir();
        if let Some(p) = crate::dialogs::open_markdown(self.hwnd, dir.as_deref()) {
            self.navigate(&p, None);
        }
    }

    fn cmd_save(&self) -> bool {
        self.sync_from_src();
        let (path, welcome) = {
            let d = self.doc.borrow();
            (d.path.clone(), d.welcome)
        };
        if welcome {
            return true;
        }
        let Some(path) = path else { return self.cmd_save_as() };
        self.save_to(&path)
    }

    fn cmd_save_as(&self) -> bool {
        self.sync_from_src();
        if self.doc.borrow().welcome {
            return true;
        }
        let (dir, name) = {
            let d = self.doc.borrow();
            (
                d.path.as_ref().and_then(|p| p.parent().map(|x| x.to_path_buf())),
                d.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| tr!("document.md", "документ.md").into()),
            )
        };
        let Some(p) = crate::dialogs::save_markdown(self.hwnd, dir.as_deref(), &name) else { return false };
        if self.save_to(&p) {
            self.doc.borrow_mut().path = Some(p);
            self.update_title();
            self.update_status();
            return true;
        }
        false
    }

    /// Autosave: silently (an error goes to the status bar, no dialog), only if there are edits.
    fn autosave(&self) {
        if !self.settings.borrow().autosave {
            return;
        }
        let (path, welcome, src_loaded) = {
            let d = self.doc.borrow();
            (d.path.clone(), d.welcome, d.src_loaded)
        };
        let Some(path) = path else { return };
        if welcome || !src_loaded {
            return;
        }
        if self.restyle_pending.get() && self.mode.get() != Mode::View {
            self.restyle_now();
        }
        self.sync_from_src();
        if !self.doc.borrow().modified {
            return;
        }
        self.save_impl(&path, SaveHow::Auto);
    }

    fn save_to(&self, path: &Path) -> bool {
        self.save_impl(path, SaveHow::Ask)
    }

    fn save_impl(&self, path: &Path, how: SaveHow) -> bool {
        let quiet = how == SaveHow::Auto;
        // Writing to the open file must not silently overwrite what was changed in it externally
        // (git pull, another editor), nor a file that was read with damaged characters replaced.
        let (same, lossy, conflict) = {
            let d = self.doc.borrow();
            let same = d.path.as_deref() == Some(path);
            let changed = same && d.mtime.is_some() && doc::mtime(path) != d.mtime;
            (same, d.lossy, d.conflict || changed)
        };
        if same && how != SaveHow::Force {
            if quiet && lossy {
                self.set_status(tr!(
                    "The file was read with damaged characters replaced (�) — it isn't saved automatically; Ctrl+S saves it explicitly.",
                    "Файл прочитан с заменой повреждённых символов (�) — сам он не сохраняется; Ctrl+S — сохранить явно."
                ));
                return false;
            }
            if quiet && conflict {
                self.doc.borrow_mut().conflict = true;
                self.set_status(tr!(
                    "The file was changed on disk by another program — autosave stopped. F5 reloads it (your changes will be lost), Ctrl+S writes your version.",
                    "Файл изменён на диске другой программой — автосохранение остановлено. F5 — перечитать (ваши правки пропадут), Ctrl+S — записать свою версию."
                ));
                return false;
            }
            let q_conflict = tr!(
                "The file was changed on disk by another program after it was opened.\n\nWrite your version over it?",
                "Файл изменён на диске другой программой после открытия.\n\nЗаписать вашу версию поверх?"
            );
            if conflict && !util::ask_yes_no(Some(self.hwnd), q_conflict, true) {
                return false;
            }
            let q_lossy = tr!(
                "The file was read with damaged or invalid characters replaced (�).\n\nSave it with these replacements?",
                "Файл был прочитан с заменой повреждённых или недопустимых символов (�).\n\nСохранить его с этой заменой?"
            );
            if lossy && !util::ask_yes_no(Some(self.hwnd), q_lossy, true) {
                return false;
            }
        }
        let (text, enc, eol) = {
            let d = self.doc.borrow();
            (d.text.clone(), d.encoding, d.eol)
        };
        match doc::save(path, &text, enc, eol) {
            Ok(used) => {
                let src_loaded = {
                    let mut d = self.doc.borrow_mut();
                    d.modified = false;
                    d.encoding = used;
                    d.mtime = doc::mtime(path);
                    d.conflict = false;
                    d.lossy = false;
                    d.src_loaded
                };
                if src_loaded {
                    self.src_hash.set(hash16(&richedit::get_text(self.src())));
                }
                self.update_title();
                self.update_status();
                if used != enc {
                    self.set_status(tr!(
                        "Saved as UTF-8: the original encoding can't represent some characters",
                        "Сохранено в UTF-8: в исходной кодировке нет некоторых символов"
                    ));
                } else if quiet {
                    self.set_status(tr!("Saved automatically", "Сохранено автоматически"));
                } else {
                    self.set_status(tr!("Saved", "Сохранено"));
                }
                if self.doc.borrow().view_stale && self.mode.get() == Mode::View {
                    self.render_view(true);
                }
                true
            }
            Err(e) => {
                if quiet {
                    self.set_status(&trf!("Autosave failed: {e}. Ctrl+S saves manually.", "Автосохранение не удалось: {e}. Ctrl+S — сохранить вручную."));
                } else {
                    util::error(Some(self.hwnd), &e);
                }
                false
            }
        }
    }

    fn cmd_reload(&self) {
        let has_path = self.doc.borrow().path.is_some();
        if !has_path {
            if self.doc.borrow().welcome {
                self.show_welcome(false);
            }
            return;
        }
        let modified = {
            self.sync_from_src();
            self.doc.borrow().modified
        };
        if modified && !util::ask_yes_no(Some(self.hwnd), tr!("Reload the file from disk? Unsaved changes will be lost.", "Перечитать файл с диска? Несохранённые правки будут потеряны."), true) {
            return;
        }
        self.reload_keep_scroll();
    }

    fn zoom_by(&self, d: i32) {
        let z = (self.settings.borrow().zoom as i32 + d).clamp(50, 300) as u32;
        self.zoom_set(z);
    }

    fn zoom_set(&self, z: u32) {
        let z = z.clamp(50, 300);
        if z == self.settings.borrow().zoom && z == richedit::get_zoom(self.view) {
            return;
        }
        let anchor = self.view_anchor();
        self.stop_smooth();
        richedit::set_zoom(self.view, z);
        if !self.src().0.is_null() {
            richedit::set_zoom(self.src(), z);
        }
        {
            let mut s = self.settings.borrow_mut();
            s.zoom = z;
            s.dirty = true;
        }
        self.apply_margins();
        // Table and code block widths account for the zoom, so re-render.
        if self.mode.get() != Mode::Visual {
            self.next_anchor.set(anchor);
            self.render_view(false);
        } else {
            self.doc.borrow_mut().view_stale = true;
        }
        self.update_bar();
        self.set_status(&trf!("Zoom {z}%", "Масштаб {z}%"));
    }

    fn save_window_settings(&self) {
        let mut wp = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        unsafe {
            let _ = GetWindowPlacement(self.hwnd, &mut wp);
        }
        let r = wp.rcNormalPosition;
        let z = self.settings.borrow().zoom;
        let mut s = self.settings.borrow_mut();
        let w = Some((r.left, r.top, r.right - r.left, r.bottom - r.top, wp.showCmd == SW_SHOWMAXIMIZED.0 as u32));
        if s.window != w || s.zoom != z {
            s.window = w;
            s.zoom = z;
            s.dirty = true;
        }
        s.save();
    }

    // ---------------- markdown editing ----------------

    fn wrap(&self, left: &str, right: &str) {
        if self.mode.get() == Mode::View {
            return;
        }
        let (a, b) = richedit::get_sel(self.src());
        let (ll, rl) = (u16len(left), u16len(right));
        let before = s16(&richedit::get_range(self.src(), (a - ll).max(0), a));
        let after = s16(&richedit::get_range(self.src(), b, b + rl));
        if before == left && after == right && a >= ll {
            // remove the formatting
            richedit::set_sel(self.src(), b, b + rl);
            richedit::replace_sel(self.src(), "", true);
            richedit::set_sel(self.src(), a - ll, a);
            richedit::replace_sel(self.src(), "", true);
            richedit::set_sel(self.src(), a - ll, b - ll);
            return;
        }
        let sel = s16(&richedit::get_range(self.src(), a, b));
        if sel.len() >= left.len() + right.len() && sel.starts_with(left) && sel.ends_with(right) && b - a > ll + rl {
            let inner = &sel[left.len()..sel.len() - right.len()];
            richedit::replace_sel(self.src(), inner, true);
            richedit::set_sel(self.src(), a, a + u16len(inner));
            return;
        }
        // Spaces at the edges of the selection stay outside.
        let lead = sel.len() - sel.trim_start().len();
        let trail = sel.len() - sel.trim_end().len();
        let core = sel.trim();
        let text = format!("{}{left}{core}{right}{}", &sel[..lead], &sel[sel.len() - trail..]);
        richedit::replace_sel(self.src(), &text, true);
        let start = a + u16len(&sel[..lead]) + ll;
        richedit::set_sel(self.src(), start, start + u16len(core));
    }

    /// Starts of the logical lines that intersect the selection.
    fn selected_lines(&self) -> (Vec<u16>, Vec<(i32, i32)>) {
        let text = richedit::get_text(self.src());
        let (a, b) = richedit::get_sel(self.src());
        let mut starts = vec![0i32];
        for (i, &c) in text.iter().enumerate() {
            if c == 13 {
                starts.push(i as i32 + 1);
            }
        }
        let mut lines = Vec::new();
        for (k, &s) in starts.iter().enumerate() {
            let e = if k + 1 < starts.len() { starts[k + 1] - 1 } else { text.len() as i32 };
            let b_eff = if b > a && b == s { b - 1 } else { b };
            if e >= a && s <= b_eff {
                lines.push((s, e));
            }
        }
        (text, lines)
    }

    fn set_heading(&self, level: usize) {
        if self.mode.get() == Mode::View {
            return;
        }
        let (text, lines) = self.selected_lines();
        for &(s, e) in lines.iter().rev() {
            let line = s16(&text[s as usize..e as usize]);
            let existing = line.chars().take_while(|c| *c == '#').count();
            let mut strip = existing;
            if existing > 0 {
                strip += line[existing..].chars().take_while(|c| *c == ' ').count();
            }
            let new_prefix = if level == 0 || existing == level { String::new() } else { format!("{} ", "#".repeat(level)) };
            richedit::set_sel(self.src(), s, s + strip as i32);
            richedit::replace_sel(self.src(), &new_prefix, true);
        }
    }

    fn toggle_prefix(&self, kind: Prefix) {
        if self.mode.get() == Mode::View {
            return;
        }
        let (text, lines) = self.selected_lines();
        let all_have = lines.iter().all(|&(s, e)| parse_prefix(&s16(&text[s as usize..e as usize])).kind == Some(kind));
        for (n, &(s, e)) in lines.iter().enumerate().rev() {
            let line = s16(&text[s as usize..e as usize]);
            let p = parse_prefix(&line);
            let indent: String = line.chars().take_while(|c| *c == ' ').collect();
            let remove_len = if p.kind.is_some() { p.len } else { indent.len() };
            let new_prefix = if all_have {
                indent.clone()
            } else {
                match kind {
                    Prefix::Bullet => format!("{indent}- "),
                    Prefix::Ordered => format!("{indent}{}. ", n + 1),
                    Prefix::Task => format!("{indent}- [ ] "),
                    Prefix::Quote => format!("{indent}> "),
                }
            };
            let rl = u16len(&line[..remove_len.min(line.len())]);
            richedit::set_sel(self.src(), s, s + rl);
            richedit::replace_sel(self.src(), &new_prefix, true);
        }
    }

    fn insert_link(&self) {
        let (a, b) = richedit::get_sel(self.src());
        let sel = s16(&richedit::get_range(self.src(), a, b));
        if sel.starts_with("http://") || sel.starts_with("https://") {
            let t = format!("[]({sel})");
            richedit::replace_sel(self.src(), &t, true);
            richedit::set_sel(self.src(), a + 1, a + 1);
        } else {
            let text = if sel.is_empty() { tr!("link text", "текст ссылки").to_string() } else { sel.clone() };
            let t = format!("[{text}](https://)");
            richedit::replace_sel(self.src(), &t, true);
            let url_start = a + 1 + u16len(&text) + 2;
            richedit::set_sel(self.src(), url_start, url_start + 8);
        }
    }

    fn insert_image(&self) {
        let dir = self.base_dir();
        let Some(p) = crate::dialogs::open_image(self.hwnd, dir.as_deref()) else { return };
        let rel = match &dir {
            Some(d) => relative_path(d, &p).unwrap_or_else(|| p.to_string_lossy().replace('\\', "/")),
            None => p.to_string_lossy().replace('\\', "/"),
        };
        let rel = rel.replace(' ', "%20").replace('(', "%28").replace(')', "%29");
        let alt = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        richedit::replace_sel(self.src(), &format!("![{alt}]({rel})"), true);
        unsafe {
            let _ = SetFocus(Some(self.src()));
        }
    }

    fn insert_block(&self, block: &str) {
        let (a, _) = richedit::get_sel(self.src());
        let prev = if a > 0 { richedit::get_range(self.src(), a - 1, a) } else { vec![13] };
        let lead = if prev.first() == Some(&13) { "\r" } else { "\r\r" };
        let t = format!("{lead}{block}\r");
        richedit::replace_sel(self.src(), &t, true);
    }

    /// Enter in a list/quote continues the markup; on an empty item it ends the list.
    fn on_enter(&self) -> bool {
        let (a, b) = richedit::get_sel(self.src());
        if a != b {
            return false;
        }
        let text = richedit::get_text(self.src());
        let a_u = (a.max(0) as usize).min(text.len());
        let ls = text[..a_u].iter().rposition(|&c| c == 13).map(|i| i + 1).unwrap_or(0);
        let le = text[a_u..].iter().position(|&c| c == 13).map(|i| i + a_u).unwrap_or(text.len());
        let line = s16(&text[ls..le]);
        let p = parse_prefix(&line);
        let Some(kind) = p.kind else { return false };
        let prefix_u16 = u16len(&line[..p.len]);
        if (a_u - ls) < prefix_u16 as usize {
            return false;
        }
        if line[p.len..].trim().is_empty() {
            richedit::set_sel(self.src(), ls as i32, le as i32);
            richedit::replace_sel(self.src(), "", true);
            return true;
        }
        let indent: String = line.chars().take_while(|c| *c == ' ').collect();
        let next = match kind {
            Prefix::Bullet => format!("{indent}{} ", p.bullet),
            Prefix::Task => format!("{indent}{} [ ] ", p.bullet),
            Prefix::Ordered => format!("{indent}{}{} ", p.number + 1, p.delim),
            Prefix::Quote => line[..p.len].to_string(),
        };
        richedit::replace_sel(self.src(), &format!("\r{next}"), true);
        true
    }

    fn on_tab(&self, shift: bool) -> bool {
        let (a, b) = richedit::get_sel(self.src());
        let text = richedit::get_text(self.src());
        let a_u = (a.max(0) as usize).min(text.len());
        let ls = text[..a_u].iter().rposition(|&c| c == 13).map(|i| i + 1).unwrap_or(0);
        let le = text[a_u..].iter().position(|&c| c == 13).map(|i| i + a_u).unwrap_or(text.len());
        let line = s16(&text[ls..le]);
        let p = parse_prefix(&line);
        if !matches!(p.kind, Some(Prefix::Bullet | Prefix::Ordered | Prefix::Task)) || b != a {
            return false;
        }
        let width = if p.kind == Some(Prefix::Ordered) { 3 } else { 2 };
        if shift {
            let spaces = line.chars().take_while(|c| *c == ' ').count().min(width);
            if spaces == 0 {
                return true;
            }
            richedit::set_sel(self.src(), ls as i32, ls as i32 + spaces as i32);
            richedit::replace_sel(self.src(), "", true);
            richedit::set_sel(self.src(), a - spaces as i32, a - spaces as i32);
        } else {
            richedit::set_sel(self.src(), ls as i32, ls as i32);
            richedit::replace_sel(self.src(), &" ".repeat(width), true);
            richedit::set_sel(self.src(), a + width as i32, a + width as i32);
        }
        true
    }

    // ---------------- search ----------------

    fn open_find(&self) {
        let existing = self.find.borrow().dlg;
        if !existing.0.is_null() {
            unsafe {
                let _ = SetFocus(Some(existing));
            }
            return;
        }
        // Prefill with the selected text.
        let ctl = if self.mode.get() == Mode::View { self.view } else { self.src() };
        let sel: String = s16(&richedit::get_selected_text(ctl)).chars().filter(|c| !('\u{E000}'..='\u{F8FF}').contains(c)).collect();
        let fr_ptr: *mut FINDREPLACEW = {
            let mut f = self.find.borrow_mut();
            let init = if !sel.is_empty() && !sel.contains('\r') && sel.len() < 200 { sel } else { f.last.clone() };
            let w: Vec<u16> = init.encode_utf16().take(250).collect();
            f.buf.fill(0);
            f.buf[..w.len()].copy_from_slice(&w);
            let buf_ptr = f.buf.as_mut_ptr();
            let fr = &mut *f.fr;
            *fr = FINDREPLACEW::default();
            fr.lStructSize = std::mem::size_of::<FINDREPLACEW>() as u32;
            fr.hwndOwner = self.hwnd;
            fr.Flags = FR_DOWN | FR_HIDEWHOLEWORD;
            fr.lpstrFindWhat = windows::core::PWSTR(buf_ptr);
            fr.wFindWhatLen = 256;
            fr as *mut FINDREPLACEW
        };
        // No active borrow: creating the dialog synchronously sends messages to the main window.
        // The Box keeps FINDREPLACEW at a stable address while the dialog is open.
        let dlg = unsafe { FindTextW(fr_ptr) };
        self.find.borrow_mut().dlg = dlg;
    }

    fn on_find_msg(&self, lp: LPARAM) {
        let fr = unsafe { &*(lp.0 as *const FINDREPLACEW) };
        if fr.Flags.contains(FR_DIALOGTERM) {
            self.find.borrow_mut().dlg = HWND::default();
            return;
        }
        if fr.Flags.contains(FR_FINDNEXT) {
            let needle = {
                let f = self.find.borrow();
                let n = f.buf.iter().position(|&c| c == 0).unwrap_or(f.buf.len());
                s16(&f.buf[..n])
            };
            self.find.borrow_mut().last = needle.clone();
            self.find_next(&needle, fr.Flags.contains(FR_DOWN), fr.Flags.contains(FR_MATCHCASE));
        }
    }

    fn find_next(&self, needle: &str, forward: bool, case: bool) {
        if needle.is_empty() {
            return;
        }
        let ctl = if self.mode.get() == Mode::View { self.view } else { self.src() };
        if ctl == self.view {
            self.finish_loading();
        }
        let (a, b) = richedit::get_sel(ctl);
        let from = if forward { b } else { a };
        let mut r = richedit::find_text(ctl, needle, from, forward, case);
        if r.is_none() {
            let wrap = if forward { 0 } else { richedit::text_len(ctl) };
            r = richedit::find_text(ctl, needle, wrap, forward, case);
        }
        match r {
            Some((s, e)) => {
                richedit::set_sel(ctl, s, e);
                richedit::scroll_caret(ctl);
                self.set_status("");
            }
            None => {
                self.set_status(&trf!("“{needle}” not found", "«{needle}» не найдено"));
            }
        }
    }

    // ---------------- self-test (scripts\selftest.ps1) ----------------

    /// End-to-end check on a real window: modes, lossless saving, editing,
    /// undo, formatting commands, markup hiding. The log goes next to the file.
    fn selftest(&self) -> bool {
        self.finish_loading();
        let mut log = String::new();
        let mut ok = true;
        let mut check = |name: &str, cond: bool| {
            log.push_str(&format!("{} {name}\n", if cond { "OK  " } else { "FAIL" }));
            ok &= cond;
        };
        let Some(path) = self.doc.borrow().path.clone() else { return false };
        let original = std::fs::read(&path).unwrap_or_default();
        let eol = self.doc.borrow().eol;
        let modified = || self.doc.borrow().modified;
        let src_text = || s16(&richedit::get_text(self.src()));

        // View: links and images are found.
        let (n_links, n_link_info, n_img) = {
            let v = self.vs.borrow();
            (v.link_pos.len(), v.links.len(), v.images.len())
        };
        check("view: all links are tagged with markers", n_links > 0 && n_links == n_link_info);
        check("view: images found", n_img > 0);

        // Mouse hit-testing: a point inside link text → that link; a heading → nothing.
        let first = self.vs.borrow().link_pos.first().copied();
        if let Some((s, _e, id)) = first {
            let (x, y) = richedit::pos_from_char(self.view, s + 2);
            check("hovering over a link finds it", self.view_hit(x + 2, y + 6) == Some(id));
            let url = self.vs.borrow().links[id].url.clone();
            check("showcase link is classified as web", matches!(links::classify(&url, None), LinkAction::Web { .. }));
        }
        let h1 = self.vs.borrow().anchor_pos.get(&0).copied();
        if let Some(p) = h1 {
            let (x, y) = richedit::pos_from_char(self.view, p + 2);
            check("hovering over a heading is not a link", self.view_hit(x + 2, y + 6).is_none());
        }

        // Inline `code` chips are found (the view paints their background).
        check("view: inline `code` chips are tagged", !self.vs.borrow().chips.is_empty());

        // Mouse selection in the view: press at the start of a paragraph and drag.
        let raw = s16(&richedit::get_text(self.view));
        if let Some(i) = raw.find("Обычный абзац") {
            let cp = raw[..i].encode_utf16().count() as i32;
            let (x0, y0) = richedit::pos_from_char(self.view, cp + 1);
            let (x1, _) = richedit::pos_from_char(self.view, cp + 12);
            let at = |x: i32, y: i32| LPARAM(((y as u32 as isize) << 16) | (x as u16 as isize));
            let mk = WPARAM(0x0001);
            send(self.view, WM_LBUTTONDOWN, mk.0, at(x0 + 1, y0 + 6).0);
            send(self.view, WM_MOUSEMOVE, mk.0, at((x0 + x1) / 2, y0 + 6).0);
            send(self.view, WM_MOUSEMOVE, mk.0, at(x1, y0 + 6).0);
            send(self.view, WM_LBUTTONUP, 0, at(x1, y0 + 6).0);
            let (a, b) = richedit::get_sel(self.view);
            check(&format!("view: text can be selected with the mouse ({} chars selected)", b - a), b - a >= 8);
            richedit::set_sel(self.view, 0, 0);
        }

        // Smooth wheel scrolling: the view moves by one wheel step (Windows' lines-per-notch
        // setting) over several frames, paced like the message loop (DwmFlush). Without a real
        // display (CI runners) frames are slow and the fallback timer draws some of them, so the
        // check waits for the end of the animation and counts every frame.
        {
            richedit::set_scroll_pos(self.view, 0, 0);
            let y0 = richedit::scroll_pos(self.view).1;
            let want = self.wheel_step(self.view, -1.0).min((max_scroll(self.view) - y0 as f64).max(0.0));
            self.smooth_frames.set(0);
            send(self.view, WM_MOUSEWHEEL, ((-120i32 as u32 as usize) << 16) & 0xFFFF_0000, 0);
            let t = std::time::Instant::now();
            let mut msg = MSG::default();
            while self.smooth.get().active && t.elapsed() < std::time::Duration::from_secs(3) {
                unsafe {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if self.smooth.get().active {
                        self.smooth_frame();
                        if DwmFlush().is_err() {
                            std::thread::sleep(std::time::Duration::from_millis(4));
                        }
                    }
                }
            }
            let (dy, frames, done) = (richedit::scroll_pos(self.view).1 - y0, self.smooth_frames.get(), !self.smooth.get().active);
            let close = (dy as f64 - want).abs() <= (want * 0.1).max(3.0);
            check(&format!("smooth wheel scrolling reaches the target: {dy} of {want:.0} px in {frames} frames, finished {done}"), done && close && frames >= 3);
            richedit::set_scroll_pos(self.view, 0, 0);
        }

        // Inline `code` backgrounds when scrolling frame by frame (cached geometry) are the same as
        // after a full repaint, and they really are visible in the capture.
        {
            let redraw = || unsafe {
                let _ = RedrawWindow(Some(self.view), None, None, RDW_INVALIDATE | RDW_UPDATENOW);
            };
            richedit::set_scroll_pos(self.view, 0, 0);
            redraw();
            for _ in 0..40 {
                let y = richedit::scroll_pos(self.view).1;
                richedit::set_scroll_pos(self.view, 0, y + 7);
                unsafe {
                    let _ = UpdateWindow(self.view);
                }
            }
            let (_, _, inc) = capture_client(self.hwnd);
            redraw();
            let (_, _, full) = capture_client(self.hwnd);
            let brush = self.chip_brush.get();
            self.chip_brush.set((brush.0, 0));
            redraw();
            let (_, _, none) = capture_client(self.hwnd);
            self.chip_brush.set(brush);
            redraw();
            let differ = |a: &[u8], b: &[u8]| a.chunks_exact(4).zip(b.chunks_exact(4)).filter(|(p, q)| p[..3] != q[..3]).count();
            let (d_inc, d_chips) = (differ(&inc, &full), differ(&full, &none));
            check(&format!("inline `code` backgrounds are visible in the capture ({d_chips} px)"), d_chips > 50);
            check(&format!("inline `code` backgrounds after scrolling = full repaint (difference {d_inc} px)"), d_inc <= 20);
            richedit::set_scroll_pos(self.view, 0, 0);
        }

        // Color emoji (emoji.rs over RichEdit's outlines): yellow pixels of 🟡 appear only with
        // the overlay, and scrolling frame by frame gives the same picture as a full repaint.
        {
            let redraw = || unsafe {
                let _ = RedrawWindow(Some(self.view), None, None, RDW_INVALIDATE | RDW_UPDATENOW);
            };
            // A hue family, not an exact color: Segoe UI Emoji differs between Windows versions.
            // Counted only where the overlay changed the pixel: yellowish pixels present in both
            // pictures (ClearType fringes, the theme's amber) don't count.
            let is_yellow = |p: &[u8]| {
                let (r, g, b) = (p[2] as i32, p[1] as i32, p[0] as i32);
                r >= 200 && g >= 165 && b <= 130 && r - b >= 110 && r - g <= 70
            };
            let yellow = |with: &[u8], without: &[u8]| with.chunks_exact(4).zip(without.chunks_exact(4)).filter(|(p, q)| is_yellow(p) && !is_yellow(q)).count();
            let differ = |a: &[u8], b: &[u8]| a.chunks_exact(4).zip(b.chunks_exact(4)).filter(|(p, q)| p[..3] != q[..3]).count();
            let found = self.vs.borrow().emoji.len();
            // The renderer is created after the first paint (WM_APP_EMOJI); here the message
            // loop does not run yet.
            self.ensure_emoji_painter();
            richedit::set_scroll_pos(self.view, 0, 0);
            redraw();
            for _ in 0..8 {
                let y = richedit::scroll_pos(self.view).1;
                richedit::set_scroll_pos(self.view, 0, y + 5);
                unsafe {
                    let _ = UpdateWindow(self.view);
                }
            }
            let (_, _, inc) = capture_client(self.hwnd);
            redraw();
            let (_, _, full) = capture_client(self.hwnd);
            let saved = self.emoji_painter.replace(Some(None));
            redraw();
            let (_, _, plain) = capture_client(self.hwnd);
            *self.emoji_painter.borrow_mut() = saved;
            redraw();
            let (y_full, y_plain, d_inc) = (yellow(&full, &plain), yellow(&plain, &full), differ(&inc, &full));
            check(&format!("color emoji: {found} found, {y_full} px turn yellow with the overlay, {y_plain} the other way"), found >= 6 && y_full > 40 && y_plain < y_full / 4);
            check(&format!("color emoji after scrolling = full repaint (difference {d_inc} px)"), d_inc <= 20);
            richedit::set_scroll_pos(self.view, 0, 0);
        }

        // Right edge of heading rules and code blocks (RTF tables) = text column edge:
        // after resizing the window by a few px, at 150% and 50% zoom, in the narrow column.
        {
            let t = self.theme.get();
            let edge_ok = |what: &str| -> (bool, String) {
                self.finish_loading();
                richedit::set_scroll_pos(self.view, 0, 0);
                unsafe {
                    let _ = RedrawWindow(Some(self.view), None, None, RDW_INVALIDATE | RDW_UPDATENOW);
                }
                let (w, _, buf) = capture_client(self.hwnd);
                let mut o = POINT::default();
                let mut vr = RECT::default();
                unsafe {
                    let _ = MapWindowPoints(Some(self.view), Some(self.hwnd), std::slice::from_mut(&mut o));
                    let _ = GetClientRect(self.view, &mut vr);
                }
                let (inset, _) = self.column(self.view, false);
                let want = o.x + vr.right - (inset * self.view_zoom() + 50) / 100;
                let cols = [t.code_bg, t.border].map(|c| (crate::theme::r(c), crate::theme::g(c), crate::theme::b(c)));
                let mut right = -1;
                for (i, p) in buf.chunks_exact(4).enumerate() {
                    let (x, y) = (i as i32 % w, i as i32 / w);
                    if x < o.x || x >= o.x + vr.right || y < o.y || y >= o.y + vr.bottom {
                        continue;
                    }
                    if cols.iter().any(|&(r, g, b)| p[2] == r && p[1] == g && p[0] == b) {
                        right = right.max(x);
                    }
                }
                ((right - want).abs() <= 3, format!("{what}: block edge {right}, column edge {want}"))
            };
            let mut edge_check = |what: &str| {
                let (good, msg) = edge_ok(what);
                check(&msg, good);
            };
            edge_check("width: original window");
            let mut wr = RECT::default();
            unsafe {
                let _ = GetWindowRect(self.hwnd, &mut wr);
                let _ = MoveWindow(self.hwnd, wr.left, wr.top, wr.right - wr.left + 13, wr.bottom - wr.top, true);
            }
            self.maybe_rerender_for_width();
            edge_check("width: window 13 px wider");
            unsafe {
                let _ = MoveWindow(self.hwnd, wr.left, wr.top, wr.right - wr.left, wr.bottom - wr.top, true);
            }
            self.maybe_rerender_for_width();
            let z0 = self.settings.borrow().zoom;
            for z in [150, 50] {
                self.zoom_set(z);
                edge_check(&format!("width: zoom {z}%"));
            }
            self.zoom_set(z0);
            self.toggle_wide();
            edge_check("width: other column mode (narrow/full width)");
            self.toggle_wide();
            self.settings.borrow_mut().dirty = false;
        }

        let pump = |ms: u64| {
            let t = std::time::Instant::now();
            let mut msg = MSG::default();
            while t.elapsed() < std::time::Duration::from_millis(ms) {
                unsafe {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        let differ = |a: &[u8], b: &[u8]| a.chunks_exact(4).zip(b.chunks_exact(4)).filter(|(p, q)| p[..3] != q[..3]).count();

        // Images from the internet (policy "ask"): the bar asks; after ✕ the question does not
        // come back for this document on a re-render (it used to come back on every one).
        {
            let policy = self.settings.borrow().remote_images;
            self.settings.borrow_mut().remote_images = RemoteImages::Ask;
            self.render_view(true);
            let asked = !self.vs.borrow().remote_waiting.is_empty() && self.info_visible.get();
            self.command(C_INFO_CLOSE);
            self.render_view(true);
            let again = !self.vs.borrow().remote_waiting.is_empty() || self.info_visible.get();
            check(&format!("images from the internet: the bar asks once, ✕ is remembered (asked {asked}, asked again {again})"), asked && !again);
            self.remote_choice.borrow_mut().clear();
            self.settings.borrow_mut().remote_images = policy;
            self.render_view(true);
            self.command(C_INFO_CLOSE);
            self.remote_choice.borrow_mut().clear();
        }

        // The "?" button: a real mouse click on the toolbar opens the help page; Back returns.
        {
            unsafe {
                let _ = RedrawWindow(Some(self.bar), None, None, RDW_INVALIDATE | RDW_UPDATENOW);
            }
            self.set_mode(Mode::Visual);
            self.restyle_now();
            unsafe {
                let _ = RedrawWindow(Some(self.bar), None, None, RDW_INVALIDATE | RDW_UPDATENOW);
            }
            let rect = ui::item_rect(self.bar, C_HELP);
            if let Some(r) = rect {
                let at = (((r.top + r.bottom) / 2) as u32 as isize) << 16 | ((r.left + r.right) / 2) as u16 as isize;
                send(self.bar, WM_LBUTTONDOWN, 1, at);
                send(self.bar, WM_LBUTTONUP, 0, at);
                pump(150);
            }
            let help = self.doc.borrow().welcome;
            check(&format!("the “?” button opens the help page (button found: {})", rect.is_some()), help);
            self.go_back();
            pump(50);
            let back = !self.doc.borrow().welcome && self.doc.borrow().path.as_deref() == Some(path.as_path());
            check("Back returns from the help page to the document", back);
        }

        // Visual mode: changing the column width repaints the whole editor (the old layout used
        // to stay next to the new one — the text looked duplicated side by side).
        {
            self.set_mode(Mode::Visual);
            self.restyle_now();
            unsafe {
                let _ = RedrawWindow(Some(self.src()), None, None, RDW_INVALIDATE | RDW_UPDATENOW);
            }
            self.toggle_wide();
            unsafe {
                let _ = UpdateWindow(self.src());
            }
            let (_, _, after) = capture_client(self.hwnd);
            unsafe {
                let _ = RedrawWindow(Some(self.src()), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW);
            }
            let (_, _, full) = capture_client(self.hwnd);
            let d = differ(&after, &full);
            self.toggle_wide();
            self.settings.borrow_mut().dirty = false;
            self.set_mode(Mode::View);
            check(&format!("visual mode: changing the column width repaints the editor (difference {d} px)"), d <= 50);
        }

        // Switching modes without edits doesn't change the file.
        self.set_mode(Mode::Visual);
        self.restyle_now();
        check("visual mode: document is not marked modified", !modified());
        self.set_mode(Mode::Code);
        self.restyle_now();
        self.set_mode(Mode::View);
        check("save without edits succeeds", self.cmd_save());
        check("save without edits: file matches byte for byte", std::fs::read(&path).unwrap_or_default() == original);

        // An edit in visual mode.
        self.set_mode(Mode::Visual);
        let len = richedit::text_len(self.src());
        richedit::set_sel(self.src(), len, len);
        richedit::replace_sel(self.src(), "\rНовая строка **жирная**", true);
        self.restyle_now();
        check("after an edit the document is marked modified", modified());
        send(self.src(), windows::Win32::UI::Controls::EM_UNDO, 0, 0);
        self.restyle_now();
        check("undoing the edit clears the “modified” mark", !modified());
        let len = richedit::text_len(self.src());
        richedit::set_sel(self.src(), len, len);
        richedit::replace_sel(self.src(), "\rНовая строка **жирная**", true);
        self.restyle_now();

        // Caret at the start of the document → markup on the last line is hidden.
        richedit::set_sel(self.src(), 0, 0);
        self.restyle_now();
        let t = src_text();
        let star = t.encode_utf16().count() - "**жирная**".encode_utf16().count();
        let hidden = richedit::tom(self.src())
            .and_then(|d| unsafe { d.Range(star as i32, star as i32 + 1).and_then(|r| r.GetFont()).and_then(|f| f.GetHidden()).ok() });
        check("visual mode: “**” outside the caret line is hidden", hidden == Some(-1));
        let hidden_word = richedit::tom(self.src())
            .and_then(|d| unsafe { d.Range(star as i32 + 2, star as i32 + 3).and_then(|r| r.GetFont()).and_then(|f| f.GetHidden()).ok() });
        check("visual mode: the text itself is not hidden", hidden_word == Some(0));
        let bold = richedit::tom(self.src())
            .and_then(|d| unsafe { d.Range(star as i32 + 2, star as i32 + 3).and_then(|r| r.GetFont()).and_then(|f| f.GetBold()).ok() });
        check("visual mode: text between ** is bold", bold == Some(-1));

        // The "Italic" command wraps the selected word.
        let t = src_text();
        if let Some(pos) = t.rfind("строка") {
            let a = t[..pos].encode_utf16().count() as i32;
            richedit::set_sel(self.src(), a, a + "строка".encode_utf16().count() as i32);
            self.command(C_ITALIC);
            check("Ctrl+I wraps the selection in *…*", src_text().contains("Новая *строка* **жирная**"));
            self.command(C_ITALIC);
            check("a second Ctrl+I removes the italics", src_text().contains("Новая строка **жирная**"));
        }

        // Enter in a list continues the list.
        let t = src_text();
        if let Some(pos) = t.find("- Третий пункт") {
            let end = t[..pos].encode_utf16().count() as i32 + "- Третий пункт".encode_utf16().count() as i32;
            richedit::set_sel(self.src(), end, end);
            let handled = self.on_enter();
            check("Enter on a list item continues the list", handled && src_text().contains("- Третий пункт\r- "));
            // Enter on an empty item removes the marker
            let handled2 = self.on_enter();
            check("Enter on an empty item ends the list", handled2 && src_text().contains("- Третий пункт\r\r"));
            send(self.src(), windows::Win32::UI::Controls::EM_UNDO, 0, 0);
            send(self.src(), windows::Win32::UI::Controls::EM_UNDO, 0, 0);
            self.restyle_now();
        }

        // Saving the edit: original text + the added line, line endings as in the original.
        // Autosave (as after a pause in typing): no dialogs, the file is written, the mark is cleared.
        self.restyle_now();
        check("edit is marked before autosave", modified());
        self.autosave();
        check("autosave wrote the edit", !modified() && std::fs::read(&path).unwrap_or_default() != original);
        check("saving the edit succeeds", self.cmd_save());
        let saved = std::fs::read(&path).unwrap_or_default();
        let nl: &[u8] = match eol {
            Eol::Crlf => b"\r\n",
            Eol::Lf => b"\n",
            Eol::Cr => b"\r",
        };
        let mut expected = original.clone();
        expected.extend_from_slice(nl);
        expected.extend_from_slice("Новая строка **жирная**".as_bytes());
        check("saved file = original + new line (lossless)", saved == expected);
        check("after saving, the “modified” mark is cleared", !modified());

        // The file is changed externally (git pull, another editor) while the editor has an edit:
        // autosave doesn't overwrite the other changes but stops.
        {
            let len = richedit::text_len(self.src());
            richedit::set_sel(self.src(), len, len);
            richedit::replace_sel(self.src(), "\rправка до внешнего изменения", true);
            self.restyle_now();
            std::thread::sleep(std::time::Duration::from_millis(30));
            let external = b"# changed by another program\n".to_vec();
            let _ = std::fs::write(&path, &external);
            self.autosave();
            check(
                "autosave doesn't overwrite a file changed by another program",
                std::fs::read(&path).unwrap_or_default() == external && modified() && self.doc.borrow().conflict,
            );
            // Restore the previous state (otherwise Ctrl+S would later ask about overwriting).
            let _ = std::fs::write(&path, &saved);
            send(self.src(), windows::Win32::UI::Controls::EM_UNDO, 0, 0);
            self.restyle_now();
            self.sync_from_src();
            let mut d = self.doc.borrow_mut();
            d.conflict = false;
            d.mtime = doc::mtime(&path);
        }

        // The view is re-rendered with the edit.
        self.set_mode(Mode::View);
        let vt = s16(&richedit::get_text(self.view));
        check("view shows the edit", vt.contains("Новая строка"));

        // Interface language: the EN/RU button switches it on the fly and back (the setting is remembered).
        {
            let was = i18n::is_ru();
            self.toggle_lang();
            let flipped = i18n::is_ru() != was && (self.settings.borrow().lang == i18n::Lang::Ru) == !was;
            let label = tr!("View", "Просмотр");
            self.toggle_lang();
            check(
                &format!("interface language switches with the EN/RU button (“{label}”)"),
                flipped && i18n::is_ru() == was && label == if was { "View" } else { "Просмотр" },
            );
            self.settings.borrow_mut().dirty = false;
        }

        // Link policy on a live document: javascript and UNC are blocked.
        check("javascript: is blocked", matches!(links::classify("javascript:alert(1)", None), LinkAction::Blocked(_)));
        check("UNC is blocked", matches!(links::classify("\\\\evil\\share\\a.md", None), LinkAction::Blocked(_)));

        // Search: the find dialog opens and closes without crashing, and search finds text.
        self.open_find();
        let dlg = self.find.borrow().dlg;
        check("Ctrl+F opens the find dialog", !dlg.0.is_null());
        if !dlg.0.is_null() {
            unsafe {
                SendMessageW(dlg, WM_COMMAND, Some(WPARAM(2 /* IDCANCEL */)), Some(LPARAM(0)));
            }
            check("find dialog closes", self.find.borrow().dlg.0.is_null());
        }
        richedit::set_sel(self.view, 0, 0);
        self.find_next("Списки", true, false);
        let (fa, fb) = richedit::get_sel(self.view);
        check("search finds text in the view", fb > fa);

        // Keyboard shortcuts and editor commands. The modifier state is passed to the handler
        // directly, so the self-test presses Ctrl/Shift/Alt combinations without the real keyboard.
        // File dialogs (Ctrl+O, Ctrl+Shift+S) and the clipboard are not exercised here.
        {
            let key = |hwnd: HWND, vk: VIRTUAL_KEY, ctrl: bool, shift: bool, alt: bool| -> bool {
                let msg = MSG { hwnd, message: WM_KEYDOWN, wParam: WPARAM(vk.0 as usize), ..Default::default() };
                handle_key_with(&msg, ctrl, shift, alt)
            };
            self.set_mode(Mode::View);
            key(self.view, VK_2, true, false, false);
            let m2 = self.mode.get() == Mode::Visual;
            key(self.src(), VK_3, true, false, false);
            let m3 = self.mode.get() == Mode::Code;
            key(self.src(), VK_ESCAPE, false, false, false);
            let esc = self.mode.get() == Mode::View;
            key(self.view, VK_3, true, false, false);
            key(self.src(), VK_1, true, false, false);
            check("keys: Ctrl+2 / Ctrl+3 / Esc / Ctrl+1 switch modes", m2 && m3 && esc && self.mode.get() == Mode::View);

            let z0 = self.settings.borrow().zoom;
            key(self.view, VK_OEM_PLUS, true, false, false);
            let zin = self.settings.borrow().zoom == z0 + 10;
            key(self.view, VK_OEM_MINUS, true, false, false);
            key(self.view, VK_OEM_MINUS, true, false, false);
            let zout = self.settings.borrow().zoom + 10 == z0;
            key(self.view, VK_0, true, false, false);
            check("keys: Ctrl+plus / Ctrl+minus / Ctrl+0 change the zoom", zin && zout && self.settings.borrow().zoom == 100);
            self.zoom_set(z0);

            richedit::set_scroll_pos(self.view, 0, 0);
            let mut ys = Vec::new();
            for vk in [VK_DOWN, VK_NEXT, VK_SPACE, VK_END, VK_UP, VK_PRIOR, VK_HOME] {
                key(self.view, vk, false, false, false);
                // Wait for the smooth scroll animation to finish (up to 2 s).
                let t0 = std::time::Instant::now();
                pump(30);
                while self.smooth.get().active && t0.elapsed() < std::time::Duration::from_secs(2) {
                    pump(30);
                }
                ys.push(richedit::scroll_pos(self.view).1);
            }
            let moved = ys[0] > 0 && ys[1] > ys[0] && ys[2] > ys[1] && ys[3] >= ys[2] && ys[4] < ys[3] && ys[5] < ys[4] && ys[6] == 0;
            key(self.view, VK_A, true, false, false);
            let (sa, sb) = richedit::get_sel(self.view);
            check(&format!("keys: Down/PgDn/Space/End/Up/PgUp/Home scroll {ys:?}, Ctrl+A selects all ({sa}..{sb})"), moved && sa == 0 && sb > 100);
            richedit::set_sel(self.view, 0, 0);
            if !self.find.borrow().last.is_empty() {
                key(self.view, VK_F3, false, false, false);
                key(self.view, VK_F3, false, true, false);
            }

            // Formatting in the Visual editor on the word "Обычный" of a plain paragraph;
            // the original text is restored after every step.
            self.set_mode(Mode::Visual);
            self.restyle_now();
            let original = src_text();
            let restore = |orig: &str| {
                richedit::set_sel(self.src(), 0, -1);
                richedit::replace_sel(self.src(), orig, false);
                self.restyle_now();
            };
            let select_word = || {
                let t = src_text();
                t.find("Обычный").map(|i| {
                    let a = t[..i].encode_utf16().count() as i32;
                    richedit::set_sel(self.src(), a, a + "Обычный".encode_utf16().count() as i32);
                })
            };
            let mut results = Vec::new();
            let steps: [(&str, VIRTUAL_KEY, bool, &str); 12] = [
                ("Ctrl+B", VK_B, false, "**Обычный**"),
                ("Ctrl+I", VK_I, false, "*Обычный*"),
                ("Ctrl+Shift+X", VK_X, true, "~~Обычный~~"),
                ("Ctrl+E", VK_E, false, "`Обычный`"),
                ("Ctrl+K", VK_K, false, "[Обычный](https://)"),
                ("Ctrl+Q", VK_Q, false, "> "),
                ("Ctrl+Shift+8", VK_8, true, "- "),
                ("Ctrl+Shift+7", VK_7, true, "1. "),
                ("Ctrl+Shift+1", VK_1, true, "# "),
                ("Ctrl+Shift+2", VK_2, true, "## "),
                ("Ctrl+Shift+6", VK_6, true, "###### "),
                ("Ctrl+Shift+0", VK_0, true, ""),
            ];
            for (name, vk, shift, want) in steps {
                if select_word().is_none() {
                    results.push(format!("{name}: word not found"));
                    continue;
                }
                let before = src_text();
                key(self.src(), vk, true, shift, false);
                self.restyle_now();
                let after = src_text();
                let good = if want.is_empty() { after == before } else { after != before && after.contains(want) };
                if !good {
                    results.push(name.to_string());
                }
                restore(&original);
            }
            // Toolbar commands without shortcuts: headings 2/3, task list, table, rule.
            for (name, cmd, want) in [("H2", C_H2, "## "), ("H3", C_H3, "### "), ("task", C_TASK, "- [ ] "), ("table", C_TABLE, "| --- |"), ("rule", C_HR, "---")] {
                if select_word().is_none() {
                    results.push(format!("{name}: word not found"));
                    continue;
                }
                self.command(cmd);
                self.restyle_now();
                if !src_text().contains(want) {
                    results.push(name.to_string());
                }
                restore(&original);
            }
            // Enter / Shift+Enter / Tab / Shift+Tab in a list item.
            let t = src_text();
            if let Some(i) = t.find("- Третий пункт") {
                let end = t[..i].encode_utf16().count() as i32 + "- Третий пункт".encode_utf16().count() as i32;
                richedit::set_sel(self.src(), end, end);
                key(self.src(), VK_RETURN, false, false, false);
                key(self.src(), VK_TAB, false, false, false);
                let indented = src_text().contains("\r  - ");
                key(self.src(), VK_TAB, false, true, false);
                key(self.src(), VK_RETURN, false, true, false);
                self.restyle_now();
                if !indented {
                    results.push("Enter/Tab in a list".into());
                }
                restore(&original);
            }
            check(&format!("keys and toolbar commands edit the text ({})", if results.is_empty() { "all".to_string() } else { format!("failed: {}", results.join(", ")) }), results.is_empty());
            check("the editor text is restored after the formatting checks", src_text() == original);
            key(self.src(), VK_ESCAPE, false, false, false);

            // Theme toggle there and back.
            let dark = self.theme.get().dark;
            self.command(C_THEME);
            let flipped = self.theme.get().dark != dark;
            self.command(C_THEME);
            check("the theme button switches light/dark and back", flipped && self.theme.get().dark == dark);
            self.settings.borrow_mut().dirty = false;

            // Reload (F5) of an unmodified document keeps it open.
            key(self.view, VK_F5, false, false, false);
            check("F5 reloads the document", self.doc.borrow().path.as_deref() == Some(path.as_path()) && !modified());
        }

        // RTF injection: a document starting with {\rtf is plain text in the editor.
        let inj_path = path.with_file_name("rtf-inject.md");
        let payload = "{\\rtf1\\ansi{\\fonttbl{\\f0 Arial;}}\\f0\\fs60 RTF-INJECTED{\\field{\\*\\fldinst HYPERLINK \"https://evil.example\"}{\\fldrslt click}}}\n";
        let _ = std::fs::write(&inj_path, payload);
        if self.open_path(&inj_path, false, false) {
            let vt = s16(&richedit::get_text(self.view));
            check("RTF from the document is shown as text in the view", vt.contains("\\rtf1") && vt.contains("fldinst"));
            self.set_mode(Mode::Code);
            let st = src_text();
            check("RTF from the document is plain text in the editor", st.starts_with("{\\rtf1") && st.contains("HYPERLINK"));
            let end = richedit::text_len(self.src());
            richedit::set_sel(self.src(), end, end);
            richedit::replace_sel(self.src(), "{\\rtf1{\\b VSTAVKA}}", true);
            check("pasting RTF-like text keeps it as text", src_text().contains("{\\rtf1{\\b VSTAVKA}}"));
            self.set_mode(Mode::View);
        }
        let _ = std::fs::remove_file(&inj_path);

        // For reference: how the old approach (EM_SETTEXTEX with "plain" text) would behave.
        let probe = richedit::create(self.hwnd, 999, false);
        if !probe.0.is_null() {
            let w = wide(payload);
            let st = SETTEXTEX { flags: ST_DEFAULT | ST_UNICODE, codepage: 1200 };
            send(probe, EM_SETTEXTEX, &st as *const _ as usize, w.as_ptr() as isize);
            let t = s16(&richedit::get_text(probe));
            let parsed = !t.contains("\\rtf1") && t.contains("RTF-INJECTED");
            log.push_str(&format!(
                "INFO old approach EM_SETTEXTEX {} RTF from the document\n",
                if parsed { "PARSED (the vulnerability was real, now fixed)" } else { "did not parse" }
            ));
            unsafe {
                let _ = DestroyWindow(probe);
            }
        }

        let report = path.with_extension("selftest.log");
        let _ = std::fs::write(&report, &log);
        ok
    }

    // ---------------- snapshot (for layout checks) ----------------

    /// Timing of "heavy" actions on a real window (scripts\profile.ps1). Writes "name ms".
    /// `*_sync`: how long the window is busy before it shows; `*_rest`: background loading of the rest.
    fn profile(&self, log: &Path) {
        use std::fmt::Write as _;
        let out = RefCell::new(String::new());
        let paint = |h: HWND| unsafe {
            let _ = RedrawWindow(Some(h), None, None, RDW_UPDATENOW | RDW_ALLCHILDREN);
        };
        let time = |name: &str, f: &dyn Fn()| {
            let t = std::time::Instant::now();
            f();
            paint(self.hwnd);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let _ = writeln!(out.borrow_mut(), "{name} {ms:.1}");
        };
        let top_text = || {
            let r = richedit::get_rect(self.view);
            let cp = richedit::char_from_pos(self.view, r.left + 2, 2);
            s16(&richedit::get_range(self.view, cp, cp + 40)).chars().filter(|c| !c.is_control() && !('\u{E000}'..='\u{F8FF}').contains(c)).collect::<String>()
        };
        time("open_sync", &|| self.render_view(false));
        let _ = writeln!(out.borrow_mut(), "  chunks {}", self.loader.borrow().as_ref().map(|l| l.r.chunk_count()).unwrap_or(0));
        time("open_rest", &|| self.finish_loading());
        time("full_repaint", &|| unsafe {
            let _ = InvalidateRect(Some(self.view), None, true);
        });
        // Smooth scrolling frames (like smooth_frame): average, 95th percentile, worst.
        // At 144 Hz there are 6.9 ms per frame.
        for step in [4, 16, 48] {
            PAINT_T.with(|p| p.set((0.0, 0.0, 0)));
            let mut v: Vec<f64> = Vec::new();
            for _ in 0..150 {
                let t = std::time::Instant::now();
                let y = richedit::scroll_pos(self.view).1;
                richedit::set_scroll_pos(self.view, 0, y + step);
                unsafe {
                    let _ = UpdateWindow(self.view);
                }
                v.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let avg = v.iter().sum::<f64>() / v.len() as f64;
            let (pr, pc, pn) = PAINT_T.with(|p| p.replace((0.0, 0.0, 0)));
            let pn = pn.max(1) as f64;
            let _ = writeln!(out.borrow_mut(), "scroll_frame_{step}px avg {avg:.2} p95 {:.2} max {:.2} | richedit_paint {:.2} chips {:.2}", v[v.len() * 95 / 100], v[v.len() - 1], pr / pn, pc / pn);
        }
        set_scroll_ratio(self.view, 0.6);
        paint(self.hwnd);
        let before = top_text();
        time("theme_light_sync", &|| self.apply_theme(ThemePref::Light));
        time("theme_light_rest", &|| self.finish_loading());
        let after = top_text();
        let _ = writeln!(out.borrow_mut(), "  anchor_kept {} | {before} | {after}", before == after);
        time("theme_dark_sync", &|| self.apply_theme(ThemePref::Dark));
        time("theme_dark_rest", &|| self.finish_loading());
        time("mode_visual_1", &|| self.set_mode(Mode::Visual));
        time("mode_code_1", &|| self.set_mode(Mode::Code));
        time("mode_view_1", &|| self.set_mode(Mode::View));
        PROF.with(|p| *p.borrow_mut() = Some((std::time::Instant::now(), String::new())));
        time("mode_visual_2", &|| self.set_mode(Mode::Visual));
        let marks = PROF.with(|p| p.borrow_mut().take().map(|x| x.1).unwrap_or_default());
        let _ = writeln!(out.borrow_mut(), "  {marks}");
        time("mode_view_2", &|| self.set_mode(Mode::View));
        time("view_rest", &|| self.finish_loading());
        // Breakdown of switching to "Visual" by stage.
        self.mode.set(Mode::Visual);
        time("  v_show", &|| unsafe {
            let _ = ShowWindow(self.view, SW_HIDE);
            let _ = ShowWindow(self.src(), SW_SHOW);
        });
        time("  v_layout", &|| self.layout());
        time("  v_restyle", &|| self.restyle_now());
        time("  v_ratio", &|| set_scroll_ratio(self.src(), 0.6));
        time("  v_bar", &|| {
            self.update_bar();
            self.update_status();
        });
        time("  v_paint_again", &|| unsafe {
            let _ = InvalidateRect(Some(self.src()), None, true);
        });
        self.mode.set(Mode::Visual);
        self.set_mode(Mode::View);
        self.finish_loading();
        let before = top_text();
        time("wide_toggle_sync", &|| self.toggle_wide());
        time("wide_toggle_rest", &|| self.finish_loading());
        let _ = writeln!(out.borrow_mut(), "  anchor_kept {} | {before} | {}", before == top_text(), top_text());
        time("wide_back_sync", &|| self.toggle_wide());
        time("wide_back_rest", &|| self.finish_loading());
        time("zoom_120_sync", &|| self.zoom_set(120));
        time("zoom_120_rest", &|| self.finish_loading());
        let _ = writeln!(out.borrow_mut(), "  zoom_now {} settings {}", richedit::get_zoom(self.view), self.settings.borrow().zoom);
        time("zoom_100_sync", &|| self.zoom_set(100));
        time("zoom_100_rest", &|| self.finish_loading());
        let _ = std::fs::write(log, out.into_inner());
    }
    fn try_snapshot(&self) {
        self.finish_loading();
        let pending = self.vs.borrow().pending;
        if pending > 0 && util::now_filetime() < self.snapshot_deadline.get() {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_SNAPSHOT);
        }
        if let Some(y) = self.opts.scroll {
            let ctl = if self.mode.get() == Mode::View { self.view } else { self.src() };
            richedit::set_scroll_pos(ctl, 0, y);
            unsafe {
                let _ = RedrawWindow(Some(self.hwnd), None, None, RDW_UPDATENOW | RDW_ALLCHILDREN | RDW_INVALIDATE);
            }
        }
        if let Some(p) = &self.opts.snapshot {
            if let Err(e) = snapshot(self.hwnd, p) {
                util::error(Some(self.hwnd), &e);
            }
        }
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

// ======================================================================
// Helper functions
// ======================================================================

pub fn install_message(o: &install::Outcome) -> String {
    match o {
        install::Outcome::Installed { dir, scope } => {
            trf!("Done! vibe-md-viewer {} is installed ({}) in\n{}", "Готово! vibe-md-viewer {} установлен ({}) в\n{}", install::VERSION, scope.label(), dir.display())
        }
        install::Outcome::Updated { dir, from } => trf!(
            "Done! vibe-md-viewer has been updated: {from} → {}.\n{}\n\nSettings and the .md association are kept. Windows of the old version that are already open keep working until you close them.",
            "Готово! vibe-md-viewer обновлён: {from} → {}.\n{}\n\nНастройки и привязка .md сохранены. Уже открытые окна старой версии продолжат работать до закрытия.",
            install::VERSION,
            dir.display()
        ),
    }
}

/// A string as a markdown `code` span that can't be "escaped": the wrapper is longer than any run
/// of backticks inside (otherwise a folder path containing ` would inject its own markup into the
/// start page, e.g. vmd: links, which work only there).
fn md_code(s: &str) -> String {
    let s = s.replace(['\r', '\n'], " ");
    let mut run = 0;
    let mut max = 0;
    for c in s.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        max = max.max(run);
    }
    let fence = "`".repeat(max + 1);
    let pad = if s.starts_with('`') || s.ends_with('`') { " " } else { "" };
    format!("{fence}{pad}{s}{pad}{fence}")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Prefix {
    Bullet,
    Ordered,
    Task,
    Quote,
}

struct ParsedPrefix {
    kind: Option<Prefix>,
    len: usize,
    bullet: char,
    number: u64,
    delim: char,
}

fn parse_prefix(line: &str) -> ParsedPrefix {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i] == b' ' {
        i += 1;
    }
    let none = ParsedPrefix { kind: None, len: 0, bullet: '-', number: 0, delim: '.' };
    if i >= b.len() {
        return none;
    }
    if b[i] == b'>' {
        let mut j = i;
        while j < b.len() && (b[j] == b'>' || b[j] == b' ') {
            j += 1;
        }
        return ParsedPrefix { kind: Some(Prefix::Quote), len: j, ..none };
    }
    if matches!(b[i], b'-' | b'*' | b'+') && b.get(i + 1) == Some(&b' ') {
        let bullet = b[i] as char;
        let mut j = i + 2;
        if line[j..].starts_with("[ ] ") || line[j..].starts_with("[x] ") || line[j..].starts_with("[X] ") {
            j += 4;
            return ParsedPrefix { kind: Some(Prefix::Task), len: j, bullet, ..none };
        }
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        return ParsedPrefix { kind: Some(Prefix::Bullet), len: j, bullet, ..none };
    }
    let ds = i;
    while i < b.len() && b[i].is_ascii_digit() && i - ds < 9 {
        i += 1;
    }
    if i > ds && i < b.len() && (b[i] == b'.' || b[i] == b')') && b.get(i + 1) == Some(&b' ') {
        let number = line[ds..i].parse().unwrap_or(1);
        let delim = b[i] as char;
        let mut j = i + 2;
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        return ParsedPrefix { kind: Some(Prefix::Ordered), len: j, number, delim, ..none };
    }
    none
}

fn relative_path(base: &Path, target: &Path) -> Option<String> {
    let b: Vec<_> = base.components().collect();
    let t: Vec<_> = target.components().collect();
    if b.first()? != t.first()? {
        return None;
    }
    let common = b.iter().zip(t.iter()).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..b.len() {
        parts.push("..".into());
    }
    for c in &t[common..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    Some(parts.join("/"))
}

/// Disable redrawing. Important: WM_SETREDRAW(TRUE) makes the window visible,
/// so hidden controls are left untouched.
fn freeze(ctl: HWND) -> bool {
    let vis = unsafe { IsWindowVisible(ctl).as_bool() };
    if vis {
        send(ctl, WM_SETREDRAW, 0, 0);
    }
    vis
}

fn unfreeze(ctl: HWND, was_visible: bool) {
    if was_visible {
        send(ctl, WM_SETREDRAW, 1, 0);
        unsafe {
            let _ = InvalidateRect(Some(ctl), None, true);
        }
    }
}

/// Image cache/deduplication key: hash of the address, size, column width, DPI and folder.
fn image_key(img: &ImageReq, max_w: u32, dpi: u32, base: Option<&Path>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    img.src.hash(&mut h);
    img.width.hash(&mut h);
    img.height.hash(&mut h);
    max_w.hash(&mut h);
    dpi.hash(&mut h);
    base.hash(&mut h);
    h.finish()
}

/// Image address for messages (data: URLs can be megabytes long).
/// An image address from the internet (http/https), as opposed to a local file or data:.
fn is_remote_src(s: &str) -> bool {
    let s = s.trim_start().to_ascii_lowercase();
    s.starts_with("http://") || s.starts_with("https://") || s.starts_with("//")
}

fn short_src(s: &str) -> String {
    if s.chars().count() <= 120 {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(120).collect::<String>())
    }
}

fn hash16(t: &[u16]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

thread_local! {
    static PROF: RefCell<Option<(std::time::Instant, String)>> = const { RefCell::new(None) };
    /// For --profile: view paint time (RichEdit, code backgrounds) in ms, and the frame count.
    static PAINT_T: Cell<(f64, f64, u32)> = const { Cell::new((0.0, 0.0, 0)) };
}

/// Stage mark for --profile (does nothing outside a measurement).
fn prof(name: &str) {
    PROF.with(|p| {
        if let Some((t, s)) = p.borrow_mut().as_mut() {
            use std::fmt::Write as _;
            let _ = write!(s, " {name}={:.1}", t.elapsed().as_secs_f64() * 1000.0);
        }
    });
}
/// Scroll limit (px): content height minus window height.
fn max_scroll(ctl: HWND) -> f64 {
    let mut si = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_RANGE | SIF_PAGE, ..Default::default() };
    unsafe {
        if GetScrollInfo(ctl, SB_VERT, &mut si).is_err() {
            return 0.0;
        }
    }
    (si.nMax - si.nMin + 1 - si.nPage as i32).max(0) as f64
}
fn scroll_ratio(ctl: HWND) -> f64 {
    let mut si = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_ALL, ..Default::default() };
    unsafe {
        if GetScrollInfo(ctl, SB_VERT, &mut si).is_err() {
            return 0.0;
        }
    }
    let range = (si.nMax - si.nMin + 1 - si.nPage as i32).max(1);
    (si.nPos - si.nMin) as f64 / range as f64
}

fn set_scroll_ratio(ctl: HWND, r: f64) {
    let mut si = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_ALL, ..Default::default() };
    unsafe {
        if GetScrollInfo(ctl, SB_VERT, &mut si).is_err() {
            return;
        }
    }
    let range = (si.nMax - si.nMin + 1 - si.nPage as i32).max(0);
    let y = (r.clamp(0.0, 1.0) * range as f64) as i32;
    richedit::set_scroll_pos(ctl, 0, y);
}

fn shell_open(owner: HWND, target: &str) {
    let w = wide(target);
    let verb = wide("open");
    unsafe {
        ShellExecuteW(Some(owner), PCWSTR::from_raw(verb.as_ptr()), PCWSTR::from_raw(w.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

fn reveal_in_explorer(p: &Path) {
    let w = util::wide_path(p);
    unsafe {
        let pidl = ILCreateFromPathW(PCWSTR::from_raw(w.as_ptr()));
        if !pidl.is_null() {
            let _ = SHOpenFolderAndSelectItems(pidl, None, 0);
            ILFree(Some(pidl));
        }
    }
}

fn set_clipboard(owner: HWND, s: &str) {
    let w = wide(s);
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2) {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                let _ = GlobalUnlock(h);
                let _ = SetClipboardData(13, Some(HANDLE(h.0)));
            }
        }
        let _ = CloseClipboard();
    }
}

fn get_clipboard(owner: HWND) -> Option<String> {
    unsafe {
        IsClipboardFormatAvailable(13).ok()?;
        OpenClipboard(Some(owner)).ok()?;
        let res = (|| {
            let h = GetClipboardData(13).ok()?;
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u16;
            if p.is_null() {
                return None;
            }
            let size = GlobalSize(g) / 2;
            let mut n = 0;
            while n < size && *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(g);
            Some(s)
        })();
        let _ = CloseClipboard();
        res
    }
}

pub(crate) fn snapshot(hwnd: HWND, path: &Path) -> Result<(), String> {
    let (w, h, mut buf) = capture_client(hwnd);
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    image::save_buffer(path, &buf, w as u32, h as u32, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())
}

/// Capture of what is currently painted in the window (BGRA, top to bottom).
fn capture_client(hwnd: HWND) -> (i32, i32, Vec<u8>) {
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        let hdc = GetDC(Some(hwnd));
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(mem, bmp.into());
        let _ = PrintWindow(hwnd, mem, PRINT_WINDOW_FLAGS(3));
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -h;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        SelectObject(mem, old);
        GetDIBits(mem, bmp, 0, h as u32, Some(buf.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(Some(hwnd), hdc);
        (w, h, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The program's own window messages never share a number (one used to swallow another).
    #[test]
    fn app_messages_are_distinct() {
        let ids = [WM_APP_IMG, WM_APP_RESTYLE, WM_APP_EMOJI, ui::WM_BAR_HINT];
        for (i, a) in ids.iter().enumerate() {
            assert!(*a >= WM_APP, "{a:#x} is below WM_APP");
            assert!(!ids[i + 1..].contains(a), "message {a:#x} is used twice");
        }
    }

    /// A path with backticks doesn't escape the `code` span on the start page (0.2.0 audit finding).
    #[test]
    fn paths_stay_inside_code_span() {
        assert_eq!(md_code("C:\\Program Files\\x"), "`C:\\Program Files\\x`");
        let evil = "D:\\a` [x](vmd:uninstall) `b";
        let md = format!("в {}.", md_code(evil));
        let out = render::render(&md, &RenderOpts::new(&Theme::dark(), 8000));
        assert!(out.links.is_empty(), "a link from the folder name must not appear: {md}");
        assert!(md_code("`x`").starts_with("`` `"));
    }

    #[test]
    fn prefixes() {
        assert_eq!(parse_prefix("- a").kind, Some(Prefix::Bullet));
        assert_eq!(parse_prefix("  * a").len, 4);
        assert_eq!(parse_prefix("- [ ] task").kind, Some(Prefix::Task));
        let p = parse_prefix("12. x");
        assert_eq!((p.kind, p.number, p.len), (Some(Prefix::Ordered), 12, 4));
        assert_eq!(parse_prefix("> > q").kind, Some(Prefix::Quote));
        assert_eq!(parse_prefix("plain").kind, None);
        assert_eq!(parse_prefix("-not").kind, None);
    }

    #[test]
    fn relpath() {
        assert_eq!(relative_path(Path::new("D:\\a\\b"), Path::new("D:\\a\\b\\img\\x.png")).as_deref(), Some("img/x.png"));
        assert_eq!(relative_path(Path::new("D:\\a\\b"), Path::new("D:\\a\\c\\x.png")).as_deref(), Some("../c/x.png"));
        assert!(relative_path(Path::new("D:\\a"), Path::new("E:\\x.png")).is_none());
    }
}
