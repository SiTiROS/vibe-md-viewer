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

    /// Soft slate gray in the style of Claude's Markdown preview (colors taken from it):
    /// a dark gray page instead of near-black, slightly dimmed body text, bright headings.
    pub fn dark() -> Self {
        Theme {
            dark: true,
            bg: rgb(35, 41, 46),
            fg: rgb(205, 211, 217),
            muted: rgb(150, 157, 164),
            link: rgb(122, 166, 218),
            // No darker than ~20% brightness: on a darker cell fill RichEdit draws all the
            // text in a single "inverted" color and syntax highlighting disappears
            // (measured: 46,52,60 — colors present, 44,50,58 — gone). Checked by the test
            // render::tests::dark_code_block_keeps_syntax_colors.
            code_bg: rgb(49, 55, 62),
            code_fg: rgb(222, 227, 232),
            code_inline_bg: rgb(51, 57, 62),
            border: rgb(62, 68, 74),
            heading: rgb(240, 242, 243),
            quote: rgb(165, 172, 179),
            table_head_bg: rgb(48, 52, 57),
            accent: rgb(188, 140, 255),
            warn: rgb(210, 153, 34),
            bar_bg: rgb(30, 35, 40),
            bar_fg: rgb(205, 211, 217),
            bar_hover: rgb(51, 57, 62),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn colors(t: &Theme) -> Vec<(&'static str, u32)> {
        vec![
            ("bg", t.bg), ("fg", t.fg), ("muted", t.muted), ("link", t.link), ("code_bg", t.code_bg), ("code_fg", t.code_fg),
            ("code_inline_bg", t.code_inline_bg), ("border", t.border), ("heading", t.heading), ("quote", t.quote),
            ("table_head_bg", t.table_head_bg), ("accent", t.accent), ("warn", t.warn), ("bar_bg", t.bar_bg), ("bar_fg", t.bar_fg),
            ("bar_hover", t.bar_hover), ("bar_active", t.bar_active), ("bar_active_fg", t.bar_active_fg), ("info_bg", t.info_bg),
            ("info_fg", t.info_fg), ("syn_keyword", t.syn_keyword), ("syn_type", t.syn_type), ("syn_string", t.syn_string),
            ("syn_number", t.syn_number), ("syn_comment", t.syn_comment), ("syn_func", t.syn_func), ("syn_attr", t.syn_attr),
        ]
    }

    /// WCAG relative luminance.
    fn luminance(c: u32) -> f64 {
        let lin = |v: u8| {
            let s = v as f64 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r(c)) + 0.7152 * lin(g(c)) + 0.0722 * lin(b(c))
    }

    /// WCAG contrast ratio (1..21).
    fn contrast(a: u32, b: u32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    fn same(a: &Theme, b: &Theme) -> bool {
        format!("{a:?}") == format!("{b:?}")
    }

    #[test]
    fn rgb_packs_as_colorref() {
        assert_eq!(rgb(1, 2, 3), 0x0003_0201);
        assert_eq!(rgb(255, 0, 0), 0x0000_00FF);
        assert_eq!(rgb(0, 0, 255), 0x00FF_0000);
        assert_eq!((r(0x00AB_CDEF), g(0x00AB_CDEF), b(0x00AB_CDEF)), (0xEF, 0xCD, 0xAB));
        for c in [rgb(1, 2, 3), rgb(255, 128, 0), rgb(0, 0, 255), rgb(255, 255, 255)] {
            assert_eq!(rgb(r(c), g(c), b(c)), c);
        }
        // The high byte is not a color channel.
        assert_eq!((r(0xFF00_0000), g(0xFF00_0000), b(0xFF00_0000)), (0, 0, 0));
    }

    #[test]
    fn themes_are_distinct_and_valid() {
        let (l, d) = (Theme::light(), Theme::dark());
        assert!(!l.dark && d.dark);
        for t in [l, d] {
            for (name, c) in colors(&t) {
                assert_eq!(c >> 24, 0, "{name} is not a COLORREF");
            }
            // Backgrounds that must be visible on the page.
            assert_ne!(t.code_bg, t.bg);
            assert_ne!(t.code_inline_bg, t.bg);
            assert_ne!(t.bar_hover, t.bar_bg);
        }
        assert!(luminance(l.bg) > 0.9 && luminance(d.bg) < 0.03);
        assert!(luminance(l.fg) < luminance(l.bg) && luminance(d.fg) > luminance(d.bg));
    }

    #[test]
    fn text_contrast_is_readable() {
        for t in [Theme::light(), Theme::dark()] {
            let k = if t.dark { "dark" } else { "light" };
            let need = |name: &str, fg: u32, bg: u32, min: f64| {
                let c = contrast(fg, bg);
                assert!(c >= min, "{k}: {name} contrast {c:.2} < {min}");
            };
            // Body text and headings: WCAG AAA.
            need("fg", t.fg, t.bg, 7.0);
            need("heading", t.heading, t.bg, 7.0);
            need("code_fg on code_bg", t.code_fg, t.code_bg, 7.0);
            need("code_fg on code_inline_bg", t.code_fg, t.code_inline_bg, 7.0);
            need("bar_fg", t.bar_fg, t.bar_bg, 7.0);
            need("info_fg", t.info_fg, t.info_bg, 7.0);
            // Secondary text, links, warnings: WCAG AA.
            for (name, c) in [("muted", t.muted), ("link", t.link), ("warn", t.warn), ("quote", t.quote), ("accent", t.accent)] {
                need(name, c, t.bg, 4.5);
            }
            need("bar_active_fg", t.bar_active_fg, t.bar_active, 4.5);
            // Syntax colors on the code block background.
            for (name, c) in [
                ("syn_keyword", t.syn_keyword), ("syn_type", t.syn_type), ("syn_string", t.syn_string), ("syn_number", t.syn_number),
                ("syn_comment", t.syn_comment), ("syn_func", t.syn_func), ("syn_attr", t.syn_attr),
            ] {
                need(name, c, t.code_bg, 3.0);
            }
        }
    }

    /// RichEdit replaces text colors on a too-dark cell fill (see the comment on `code_bg`).
    #[test]
    fn dark_code_background_is_not_too_dark_for_richedit() {
        let c = Theme::dark().code_bg;
        assert!(r(c) >= 46 && g(c) >= 52 && b(c) >= 60, "code_bg {:?}", (r(c), g(c), b(c)));
    }

    #[test]
    fn resolve_follows_preference() {
        assert!(same(&Theme::resolve(ThemePref::Light), &Theme::light()));
        assert!(same(&Theme::resolve(ThemePref::Dark), &Theme::dark()));
        // System: whatever Windows reports (a read-only registry query).
        let sys = Theme::resolve(ThemePref::System);
        assert_eq!(sys.dark, system_prefers_dark());
        let want = if sys.dark { Theme::dark() } else { Theme::light() };
        assert!(same(&sys, &want));
    }
}
