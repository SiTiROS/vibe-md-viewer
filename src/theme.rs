//! Light and dark theme colors. A color is stored as COLORREF (0x00BBGGRR).

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub dark: bool,
    pub bg: u32,
    pub fg: u32,
    pub muted: u32,
    pub link: u32,
    pub code_bg: u32,
    pub code_fg: u32,
    /// Background of inline `code` — calmer than the block background so the chips don't "shout".
    pub code_inline_bg: u32,
    pub border: u32,
    pub heading: u32,
    pub quote: u32,
    pub table_head_bg: u32,
    pub accent: u32,
    pub warn: u32,
    pub bar_bg: u32,
    pub bar_fg: u32,
    pub bar_hover: u32,
    pub bar_active: u32,
    pub bar_active_fg: u32,
    pub info_bg: u32,
    pub info_fg: u32,
    /// Syntax highlighting (GitHub-style palette).
    pub syn_keyword: u32,
    pub syn_type: u32,
    pub syn_string: u32,
    pub syn_number: u32,
    pub syn_comment: u32,
    pub syn_func: u32,
    pub syn_attr: u32,
}

pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

pub fn r(c: u32) -> u8 {
    (c & 0xFF) as u8
}
pub fn g(c: u32) -> u8 {
    ((c >> 8) & 0xFF) as u8
}
pub fn b(c: u32) -> u8 {
    ((c >> 16) & 0xFF) as u8
}

impl Theme {
    pub fn light() -> Self {
        Theme {
            dark: false,
            bg: rgb(255, 255, 255),
            fg: rgb(31, 35, 40),
            muted: rgb(101, 109, 118),
            link: rgb(9, 105, 218),
            code_bg: rgb(242, 244, 247),
            code_fg: rgb(31, 35, 40),
            code_inline_bg: rgb(238, 241, 244),
            border: rgb(208, 215, 222),
            heading: rgb(20, 24, 28),
            quote: rgb(89, 99, 110),
            table_head_bg: rgb(246, 248, 250),
            accent: rgb(130, 80, 223),
            warn: rgb(154, 103, 0),
            bar_bg: rgb(246, 248, 250),
            bar_fg: rgb(36, 41, 47),
            bar_hover: rgb(230, 234, 239),
            bar_active: rgb(9, 105, 218),
            bar_active_fg: rgb(255, 255, 255),
            info_bg: rgb(255, 248, 197),
            info_fg: rgb(59, 45, 0),
            syn_keyword: rgb(207, 34, 46),
            syn_type: rgb(149, 56, 0),
            syn_string: rgb(10, 48, 105),
            syn_number: rgb(5, 80, 174),
            syn_comment: rgb(110, 119, 129),
            syn_func: rgb(130, 80, 223),
            syn_attr: rgb(5, 80, 174),
        }
    }

    pub fn dark() -> Self {
        Theme {
            dark: true,
            bg: rgb(13, 17, 23),
            fg: rgb(230, 237, 243),
            muted: rgb(139, 148, 158),
            link: rgb(88, 166, 255),
            // No darker than ~20% brightness: on a darker cell fill RichEdit draws all the
            // text in a single "inverted" color and syntax highlighting disappears
            // (measured: 46,52,60 — colors present, 44,50,58 — gone). Checked by the test
            // render::tests::dark_code_block_keeps_syntax_colors.
            code_bg: rgb(47, 54, 64),
            code_fg: rgb(230, 237, 243),
            code_inline_bg: rgb(33, 39, 48),
            border: rgb(48, 54, 61),
            heading: rgb(240, 246, 252),
            quote: rgb(150, 160, 170),
            table_head_bg: rgb(22, 27, 34),
            accent: rgb(188, 140, 255),
            warn: rgb(210, 153, 34),
            bar_bg: rgb(22, 27, 34),
            bar_fg: rgb(201, 209, 217),
            bar_hover: rgb(39, 46, 56),
            bar_active: rgb(31, 111, 235),
            bar_active_fg: rgb(255, 255, 255),
            info_bg: rgb(56, 45, 10),
            info_fg: rgb(240, 220, 160),
            syn_keyword: rgb(255, 123, 114),
            syn_type: rgb(255, 166, 87),
            syn_string: rgb(165, 214, 255),
            syn_number: rgb(121, 192, 255),
            syn_comment: rgb(139, 148, 158),
            syn_func: rgb(210, 168, 255),
            syn_attr: rgb(121, 192, 255),
        }
    }

    pub fn resolve(pref: ThemePref) -> Self {
        match pref {
            ThemePref::Light => Self::light(),
            ThemePref::Dark => Self::dark(),
            ThemePref::System => {
                if system_prefers_dark() {
                    Self::dark()
                } else {
                    Self::light()
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePref {
    System,
    Light,
    Dark,
}

/// HKCU\...\Themes\Personalize\AppsUseLightTheme == 0 → dark theme.
pub fn system_prefers_dark() -> bool {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let key = crate::util::wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let val = crate::util::wide("AppsUseLightTheme");
    let mut data: u32 = 1;
    let mut size = 4u32;
    unsafe {
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            crate::util::pcwstr(&key),
            crate::util::pcwstr(&val),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        );
        r.is_ok() && data == 0
    }
}
