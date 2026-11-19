//! Small Win32 helpers: wide strings, message sending, dialogs.

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, SendMessageW, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_ICONWARNING,
    MB_OK, MB_YESNO, MB_YESNOCANCEL, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE,
};

pub const APP_NAME: &str = "vibe-md-viewer";

/// String → UTF-16 with a terminating NUL.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn wide_path(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

pub fn pcwstr(v: &[u16]) -> PCWSTR {
    PCWSTR::from_raw(v.as_ptr())
}

/// SendMessageW with "raw" arguments.
pub fn send(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wp)), Some(LPARAM(lp))).0 }
}

fn msgbox(owner: Option<HWND>, text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    let t = wide(text);
    let c = wide(APP_NAME);
    unsafe { MessageBoxW(owner, pcwstr(&t), pcwstr(&c), style) }
}

pub fn info(owner: Option<HWND>, text: &str) {
    msgbox(owner, text, MB_OK | MB_ICONINFORMATION);
}

pub fn error(owner: Option<HWND>, text: &str) {
    msgbox(owner, text, MB_OK | MB_ICONERROR);
}

pub fn warn(owner: Option<HWND>, text: &str) {
    msgbox(owner, text, MB_OK | MB_ICONWARNING);
}

/// Yes/No. true = "Yes".
pub fn ask_yes_no(owner: Option<HWND>, text: &str, warning: bool) -> bool {
    let icon = if warning { MB_ICONWARNING } else { MB_ICONQUESTION };
    msgbox(owner, text, MB_YESNO | icon).0 == 6 // IDYES
}

/// Yes/No/Cancel: Some(true)=Yes, Some(false)=No, None=Cancel.
pub fn ask_yes_no_cancel(owner: Option<HWND>, text: &str) -> Option<bool> {
    match msgbox(owner, text, MB_YESNOCANCEL | MB_ICONQUESTION).0 {
        6 => Some(true),
        7 => Some(false),
        _ => None,
    }
}

/// Current time in 100-ns FILETIME units.
pub fn now_filetime() -> u64 {
    use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    let ft = unsafe { GetSystemTimePreciseAsFileTime() };
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// Milliseconds since the process was created.
pub fn ms_since_process_start() -> f64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let mut c = FILETIME::default();
    let mut e = FILETIME::default();
    let mut k = FILETIME::default();
    let mut u = FILETIME::default();
    unsafe {
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).is_err() {
            return -1.0;
        }
    }
    let created = ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64;
    (now_filetime().saturating_sub(created)) as f64 / 10_000.0
}

pub fn loword(v: usize) -> u16 {
    (v & 0xFFFF) as u16
}

pub fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xFFFF) as u16
}

pub fn get_x_lparam(lp: isize) -> i32 {
    (lp & 0xFFFF) as i16 as i32
}

pub fn get_y_lparam(lp: isize) -> i32 {
    ((lp >> 16) & 0xFFFF) as i16 as i32
}

/// Directory containing the exe.
pub fn exe_dir() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings() {
        assert_eq!(wide(""), [0]);
        assert_eq!(wide("ab"), [0x61, 0x62, 0]);
        assert_eq!(wide("я😀"), [0x44F, 0xD83D, 0xDE00, 0], "surrogate pair + NUL");
        assert_eq!(wide_path(Path::new("C:\\Папка\\a.md")), wide("C:\\Папка\\a.md"));
        // A path that is not valid Unicode (lone surrogate) is passed through unchanged.
        use std::os::windows::ffi::OsStringExt;
        let odd = std::ffi::OsString::from_wide(&[0xD800, 0x61]);
        assert_eq!(wide_path(Path::new(&odd)), [0xD800, 0x61, 0]);
        let w = wide("text");
        let p = pcwstr(&w);
        assert_eq!(p.as_ptr(), w.as_ptr());
        assert_eq!(unsafe { p.to_string() }.unwrap(), "text");
    }

    #[test]
    fn words_and_lparam_coordinates() {
        assert_eq!(loword(0x1234_5678), 0x5678);
        assert_eq!(hiword(0x1234_5678), 0x1234);
        assert_eq!(loword(0xFFFF_FFFF_0001_0002), 0x0002, "high 32 bits are ignored");
        assert_eq!(hiword(0xFFFF_FFFF_0001_0002), 0x0001);
        // Coordinates are signed 16-bit (multi-monitor setups have negative ones).
        let lp = |x: i16, y: i16| ((y as u16 as isize) << 16) | x as u16 as isize;
        assert_eq!((get_x_lparam(lp(120, 45)), get_y_lparam(lp(120, 45))), (120, 45));
        assert_eq!((get_x_lparam(lp(-10, -5)), get_y_lparam(lp(-10, -5))), (-10, -5));
        assert_eq!((get_x_lparam(lp(i16::MIN, i16::MAX)), get_y_lparam(lp(i16::MIN, i16::MAX))), (-32768, 32767));
    }

    #[test]
    fn clocks() {
        // 2020-01-01 in FILETIME units.
        const Y2020: u64 = 132_223_104_000_000_000;
        let a = now_filetime();
        let b = now_filetime();
        assert!(a > Y2020 && b >= a);
        let ms = ms_since_process_start();
        assert!(ms >= 0.0 && ms < 24.0 * 3600.0 * 1000.0, "{ms}");
        assert!(ms_since_process_start() >= ms);
    }

    #[test]
    fn exe_dir_is_the_parent_of_the_exe() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(exe_dir().as_deref(), exe.parent());
        assert_eq!(APP_NAME, "vibe-md-viewer");
    }
}
