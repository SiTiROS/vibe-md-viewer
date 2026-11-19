//! Installer window — starts when the exe is named `…setup….exe` (that is how it ships
//! in dist: `vibe-md-viewer-<version>-setup.exe`) or with the `--setup` switch.
//!
//! A regular setup wizard: folder (Program Files by default), "open .md" and
//! "launch after installation" checkboxes, "Install" / "Cancel" buttons. If the program is
//! already installed — "Update" / "Reinstall" and "Uninstall". Administrator rights (UAC)
//! are requested only for the duration of the operation itself (see install.rs).

use std::cell::RefCell;
use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::install::{self, Installed, VERSION};
use crate::util::{self, wide};

const ID_INSTALL: usize = 1; // IDOK — Enter
const ID_CANCEL: usize = 2; // IDCANCEL — Esc
const ID_UNINSTALL: usize = 11;
const ID_DEFAULT: usize = 12;
const ID_LAUNCH: usize = 13;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SMode {
    Fresh,
    Update,
    Reinstall,
    Downgrade,
}

struct Setup {
    hwnd: HWND,
    edit: HWND,
    status: HWND,
    chk_default: HWND,
    chk_launch: HWND,
    buttons: Vec<HWND>,
    mode: SMode,
    inst: Option<Installed>,
    bg: HBRUSH,
}

thread_local! {
    static SETUP: RefCell<Option<Setup>> = const { RefCell::new(None) };
}

/// Is this an installer launch? (the exe name contains "setup")
pub fn is_setup_exe() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()))
        .map(|s| s.contains("setup"))
        .unwrap_or(false)
}

fn scale(v: i32, dpi: u32) -> i32 {
    v * dpi as i32 / 96
}

fn make_font(dpi: u32, pt: i32, bold: bool) -> HFONT {
    let face = wide("Segoe UI");
    unsafe {
        CreateFontW(
            -(pt * dpi as i32 / 72),
            0,
            0,
            0,
            if bold { 600 } else { 400 },
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            PCWSTR::from_raw(face.as_ptr()),
        )
    }
}

fn child(parent: HWND, class: &str, text: &str, style: u32, ex: u32, id: usize, rc: (i32, i32, i32, i32), font: HFONT) -> HWND {
    let c = wide(class);
    let t = wide(text);
    let h = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(ex),
            PCWSTR::from_raw(c.as_ptr()),
            PCWSTR::from_raw(t.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
            rc.0,
            rc.1,
            rc.2,
            rc.3,
            Some(parent),
            Some(HMENU(id as *mut core::ffi::c_void)),
            GetModuleHandleW(None).ok().map(|m| m.into()),
            None,
        )
        .unwrap_or_default()
    };
    util::send(h, WM_SETFONT, font.0 as usize, 1);
    h
}

fn set_text(h: HWND, s: &str) {
    let w = wide(s);
    unsafe {
        let _ = SetWindowTextW(h, PCWSTR::from_raw(w.as_ptr()));
    }
}

fn checked(h: HWND) -> bool {
    util::send(h, BM_GETCHECK, 0, 0) == 1
}

/// Runs the installer window. `snapshot` — save a window snapshot and exit (layout check).
pub fn run(snapshot: Option<PathBuf>) -> i32 {
    let class = wide("VibeMdSetup");
    let hinst = unsafe { GetModuleHandleW(None).unwrap_or_default() };
    let bg = unsafe { CreateSolidBrush(COLORREF(0x00FFFFFF)) };
    unsafe {
        let icon = LoadImageW(Some(hinst.into()), PCWSTR(1 as _), IMAGE_ICON, GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON), LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .unwrap_or_default();
        let small = LoadImageW(Some(hinst.into()), PCWSTR(1 as _), IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(setup_proc),
            hInstance: hinst.into(),
            hIcon: icon,
            hIconSm: small,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: bg,
            lpszClassName: PCWSTR::from_raw(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }

    let inst = install::installed();
    let mode = match &inst {
        None => SMode::Fresh,
        Some(i) => match install::compare_versions(VERSION, &i.version) {
            std::cmp::Ordering::Greater => SMode::Update,
            std::cmp::Ordering::Equal => SMode::Reinstall,
            std::cmp::Ordering::Less => SMode::Downgrade,
        },
    };

    // Window size for a 540×340 client area (at 96 DPI).
    let title = wide(&trf!("vibe-md-viewer {VERSION} Setup", "Установка vibe-md-viewer {VERSION}"));
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
    let hwnd = unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR::from_raw(class.as_ptr()), PCWSTR::from_raw(title.as_ptr()), style, CW_USEDEFAULT, CW_USEDEFAULT, 600, 420, None, None, Some(hinst.into()), None)
            .unwrap_or_default()
    };
    if hwnd.0.is_null() {
        return 1;
    }
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96);
    let s = |v: i32| scale(v, dpi);
    unsafe {
        let mut rc = RECT { left: 0, top: 0, right: s(540), bottom: s(340) };
        let _ = AdjustWindowRectEx(&mut rc, style, false, WINDOW_EX_STYLE(0));
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        let _ = SetWindowPos(hwnd, None, (sw - w) / 2, (sh - h) / 3, w, h, SWP_NOZORDER);
    }

    let font = make_font(dpi, 9, false);
    let font_title = make_font(dpi, 16, true);
    let font_sub = make_font(dpi, 10, false);
    let ss = 0u32; // SS_LEFT
    child(hwnd, "STATIC", &format!("vibe-md-viewer {VERSION}"), ss, 0, 0, (s(28), s(22), s(484), s(34)), font_title);
    child(hwnd, "STATIC", tr!("Fast Markdown (.md) viewer and editor for Windows", "Быстрый просмотрщик и редактор Markdown (.md) для Windows"), ss, 0, 0, (s(28), s(58), s(484), s(22)), font_sub);

    let body = match (&mode, &inst) {
        (SMode::Fresh, _) => tr!(
            "The program will be installed for all users of this computer. Windows will ask you to confirm administrator rights.",
            "Программа будет установлена для всех пользователей компьютера. Windows попросит подтвердить права администратора."
        )
        .to_string(),
        (SMode::Update, Some(i)) if i.is_legacy() => trf!(
            "Version {ver} (formerly named vibe-mdViewer) is installed in\n{dir}\nIt will be updated to {VERSION} and moved to\n{target}\nYour settings and the .md file association will be kept; the old folder will be removed once the old version is closed.",
            "Установлена версия {ver} (прежнее имя vibe-mdViewer) в папке\n{dir}\nОна будет обновлена до {VERSION} и переедет в папку\n{target}\nНастройки и привязка файлов .md сохранятся; прежняя папка удалится, когда старая версия будет закрыта.",
            ver = i.version,
            dir = i.dir.display(),
            target = i.target_dir().display()
        ),
        (SMode::Update, Some(i)) => trf!(
            "Version {ver} is installed in\n{dir}\nIt will be updated to {VERSION}. Your settings and the .md file association will be kept.",
            "Установлена версия {ver} в папке\n{dir}\nОна будет обновлена до {VERSION}. Настройки и привязка файлов .md сохранятся.",
            ver = i.version,
            dir = i.dir.display()
        ),
        (SMode::Reinstall, Some(i)) => trf!(
            "Version {VERSION} is already installed in\n{}\nYou can reinstall or uninstall the program.",
            "Версия {VERSION} уже установлена в папке\n{}\nМожно переустановить или удалить программу.",
            i.dir.display()
        ),
        (SMode::Downgrade, Some(i)) => trf!(
            "A newer version {ver} is installed in\n{dir}\nYou can replace it with this version ({VERSION}) or uninstall the program.",
            "Установлена более новая версия {ver} в папке\n{dir}\nМожно заменить её этой версией ({VERSION}) или удалить программу.",
            ver = i.version,
            dir = i.dir.display()
        ),
        _ => String::new(),
    };
    // Without the "Installation folder" field (update, reinstall) the text may take its place.
    let body_h = if mode == SMode::Fresh { s(54) } else { s(118) };
    child(hwnd, "STATIC", &body, ss, 0, 0, (s(28), s(96), s(484), body_h), font);

    let mut edit = HWND::default();
    let mut buttons = Vec::new();
    if mode == SMode::Fresh {
        // For all users — Program Files only (the parent folders belong to administrators,
        // so a regular user cannot tamper with the installation; see install::do_install).
        child(hwnd, "STATIC", tr!("Installation folder:", "Папка установки:"), ss, 0, 0, (s(28), s(158), s(484), s(20)), font);
        let dir = install::Scope::Machine.default_dir();
        edit = child(
            hwnd,
            "EDIT",
            &dir.display().to_string(),
            ES_AUTOHSCROLL as u32 | ES_READONLY as u32,
            WS_EX_CLIENTEDGE.0,
            0,
            (s(28), s(180), s(484), s(26)),
            font,
        );
    }
    let chk_default = child(
        hwnd,
        "BUTTON",
        tr!("Open .md files with vibe-md-viewer (Windows will ask you to choose)", "Открывать файлы .md в vibe-md-viewer (Windows покажет окно выбора)"),
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        0,
        ID_DEFAULT,
        (s(28), s(220), s(484), s(24)),
        font,
    );
    let chk_launch = child(hwnd, "BUTTON", tr!("Launch after installation", "Запустить после установки"), WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32, 0, ID_LAUNCH, (s(28), s(246), s(484), s(24)), font);
    util::send(chk_default, BM_SETCHECK, if mode == SMode::Fresh { 1 } else { 0 }, 0);
    util::send(chk_launch, BM_SETCHECK, 1, 0);
    let status = child(hwnd, "STATIC", "", ss, 0, 0, (s(28), s(276), s(484), s(20)), font);

    let install_label = match mode {
        SMode::Fresh => tr!("Install", "Установить"),
        SMode::Update => tr!("Update", "Обновить"),
        SMode::Reinstall => tr!("Reinstall", "Переустановить"),
        SMode::Downgrade => tr!("Replace", "Заменить"),
    };
    if inst.is_some() {
        buttons.push(child(hwnd, "BUTTON", tr!("Uninstall", "Удалить"), WS_TABSTOP.0 | BS_PUSHBUTTON as u32, 0, ID_UNINSTALL, (s(28), s(300), s(110), s(30)), font));
    }
    let btn_install = child(hwnd, "BUTTON", install_label, WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32, 0, ID_INSTALL, (s(300), s(300), s(124), s(30)), font);
    let btn_cancel = child(hwnd, "BUTTON", tr!("Cancel", "Отмена"), WS_TABSTOP.0 | BS_PUSHBUTTON as u32, 0, ID_CANCEL, (s(432), s(300), s(80), s(30)), font);
    buttons.push(btn_install);
    buttons.push(btn_cancel);

    SETUP.with(|st| {
        *st.borrow_mut() = Some(Setup { hwnd, edit, status, chk_default, chk_launch, buttons, mode, inst, bg });
    });

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = UpdateWindow(hwnd);
        let _ = SetFocus(Some(btn_install));
    }

    if let Some(p) = snapshot {
        unsafe {
            let _ = RedrawWindow(Some(hwnd), None, None, RDW_UPDATENOW | RDW_ALLCHILDREN | RDW_INVALIDATE);
        }
        let r = crate::app::snapshot(hwnd, &p);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return if r.is_ok() { 0 } else { 1 };
    }

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if IsDialogMessageW(hwnd, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    0
}

fn set_busy(busy: bool, text: &str) {
    let (buttons, status, hwnd) = SETUP.with(|st| {
        let st = st.borrow();
        let s = st.as_ref().unwrap();
        (s.buttons.clone(), s.status, s.hwnd)
    });
    for b in buttons {
        unsafe {
            let _ = EnableWindow(b, !busy);
        }
    }
    set_text(status, text);
    unsafe {
        let _ = UpdateWindow(hwnd);
    }
}

fn do_install() {
    let (hwnd, edit, chk_default, chk_launch, mode, inst) = SETUP.with(|st| {
        let st = st.borrow();
        let s = st.as_ref().unwrap();
        (s.hwnd, s.edit, s.chk_default, s.chk_launch, s.mode, s.inst.clone())
    });
    let want_default = checked(chk_default);
    let want_launch = checked(chk_launch);
    let owner = Some(hwnd);
    set_busy(true, tr!("Installing… please confirm the Windows prompt (User Account Control).", "Установка… подтвердите запрос Windows (контроль учётных записей)."));
    let result: Result<Option<PathBuf>, String> = match (mode, &inst) {
        (SMode::Fresh, _) => {
            let _ = edit;
            install::install_fresh(owner, &install::Scope::Machine.default_dir()).map(|r| r.map(|(d, _)| d))
        }
        (_, Some(i)) => install::update_installed(owner, i).map(|ok| ok.then(|| i.target_dir())),
        _ => Ok(None),
    };
    match result {
        Ok(Some(dir)) => {
            set_busy(true, tr!("Done.", "Готово."));
            if want_default {
                install::prompt_default_with_hint(owner);
            }
            let what = if mode == SMode::Fresh { tr!("installed", "установлен") } else { tr!("updated", "обновлён") };
            util::info(owner, &trf!("vibe-md-viewer {VERSION} has been {what}.\n\n{}", "vibe-md-viewer {VERSION} {what}.\n\n{}", dir.display()));
            if want_launch {
                let _ = std::process::Command::new(dir.join(install::EXE_NAME)).current_dir(&dir).spawn();
            }
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        Ok(None) => set_busy(false, tr!("Installation canceled.", "Установка отменена.")),
        Err(e) => {
            set_busy(false, tr!("Installation failed.", "Не удалось выполнить установку."));
            util::error(owner, &e);
        }
    }
}

unsafe extern "system" fn setup_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| setup_proc_inner(hwnd, msg, wp, lp))).unwrap_or_else(|_| DefWindowProcW(hwnd, msg, wp, lp))
}

unsafe fn setup_proc_inner(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            match util::loword(wp.0) as usize {
                ID_INSTALL => do_install(),
                ID_CANCEL => {
                    let _ = DestroyWindow(hwnd);
                }
                ID_UNINSTALL => {
                    set_busy(true, tr!("Uninstalling…", "Удаление…"));
                    match install::uninstall_interactive(Some(hwnd)) {
                        Ok(true) => {
                            let _ = DestroyWindow(hwnd);
                        }
                        Ok(false) => set_busy(false, ""),
                        Err(e) => {
                            set_busy(false, "");
                            util::error(Some(hwnd), &e);
                        }
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        // White background for labels and checkboxes.
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wp.0 as *mut core::ffi::c_void);
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(0x00282320));
            let bg = SETUP.with(|st| st.borrow().as_ref().map(|s| s.bg).unwrap_or_default());
            LRESULT(bg.0 as isize)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
