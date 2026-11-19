//! Applying formatting to the source editor via TOM (RichEdit's Text Object Model).
//!
//! After each edit the text is re-parsed (styler.rs), and only the changed runs are
//! applied to the control: the prefix/suffix shared with the old text is skipped.
//! Formatting changes do not go into the undo history (tomSuspend).

use std::collections::{HashMap, HashSet};

use windows::core::BSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Controls::RichEdit::*;

use crate::richedit;
use crate::styler::{self, fl, ParaRun, ParaStyle, Run};
use crate::theme::Theme;

const TOM_TRUE: i32 = -1;
const TOM_FALSE: i32 = 0;
const TOM_AUTOCOLOR: i32 = -9999997;

pub struct Styler {
    doc: Option<ITextDocument>,
    range: Option<ITextRange>,
    fonts: HashMap<(bool, u32), ITextFont>,
    paras: HashMap<(bool, ParaStyle), ITextPara>,
    prev_text: Vec<u16>,
    prev_runs: Vec<Run>,
    prev_paras: Vec<ParaRun>,
    prev_visual: Option<bool>,
    prev_caret: Option<usize>,
    theme: Theme,
}

impl Styler {
    pub fn new(hwnd: HWND, theme: Theme) -> Styler {
        let doc = richedit::tom(hwnd);
        let range = doc.as_ref().and_then(|d| unsafe { d.Range(0, 0).ok() });
        Styler {
            doc,
            range,
            fonts: HashMap::new(),
            paras: HashMap::new(),
            prev_text: Vec::new(),
            prev_runs: Vec::new(),
            prev_paras: Vec::new(),
            prev_visual: None,
            prev_caret: None,
            theme,
        }
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.fonts.clear();
        self.invalidate();
    }

    /// Editor text as of the last restyle.
    pub fn text(&self) -> &[u16] {
        &self.prev_text
    }

    /// The next restyle will restyle everything.
    pub fn invalidate(&mut self) {
        self.prev_text.clear();
        self.prev_runs.clear();
        self.prev_paras.clear();
        self.prev_visual = None;
    }

    fn font(&mut self, visual: bool, flags: u32) -> Option<ITextFont> {
        if let Some(f) = self.fonts.get(&(visual, flags)) {
            return Some(f.clone());
        }
        let range = self.range.as_ref()?;
        let t = self.theme;
        let f = unsafe { range.GetFont().ok()?.GetDuplicate().ok()? };
        let h = (flags & fl::H_MASK) >> fl::H_SHIFT;
        let marker = flags & (fl::MARKER | fl::URL) != 0;
        let reveal = flags & fl::REVEAL != 0;
        let mono_bits = fl::CODE | fl::CODEBLOCK | fl::FENCE | fl::TABLE | fl::HTML | fl::MATH | fl::META;
        let (name, size, bold, italic, strike, under, fore, back, hidden);
        if visual {
            let mono = flags & mono_bits != 0;
            name = if mono { "Consolas" } else { "Segoe UI" };
            size = if h > 0 {
                [24.0, 19.0, 16.0, 14.0, 13.0, 12.0][(h as usize - 1).min(5)]
            } else if flags & fl::FENCE != 0 {
                9.0
            } else if mono {
                10.5
            } else {
                12.0
            };
            bold = h > 0 || flags & (fl::STRONG | fl::THEAD) != 0;
            italic = flags & (fl::EMPH | fl::IMAGE) != 0 || is_syn_comment(flags);
            strike = flags & fl::STRIKE != 0;
            under = flags & fl::LINK != 0 && !marker;
            hidden = marker
                && !reveal
                && flags & (fl::FENCE | fl::ORDERED | fl::TASK) == 0
                && !(flags & fl::LISTMARK != 0 && flags & fl::QUOTE != 0 && false);
            fore = if marker {
                t.muted
            } else if h == 6 {
                t.muted
            } else if h > 0 {
                t.heading
            } else if let Some(c) = syn_color(&t, flags) {
                c
            } else if flags & fl::LINK != 0 || flags & fl::FOOTNOTE != 0 {
                t.link
            } else if flags & (fl::IMAGE | fl::HTML | fl::FENCE | fl::META | fl::HR | fl::PIPE) != 0 {
                t.muted
            } else if flags & (fl::ORDERED | fl::TASK | fl::MATH) != 0 {
                t.accent
            } else if flags & (fl::CODE | fl::CODEBLOCK) != 0 {
                t.code_fg
            } else if flags & fl::QUOTE != 0 {
                t.quote
            } else {
                t.fg
            };
            back = if flags & (fl::CODEBLOCK | fl::FENCE | fl::IMAGE) != 0 {
                Some(t.code_bg)
            } else if flags & fl::CODE != 0 {
                Some(t.code_inline_bg)
            } else {
                None
            };
        } else {
            name = "Consolas";
            size = 11.0;
            bold = h > 0 || flags & (fl::STRONG | fl::THEAD) != 0;
            italic = flags & fl::EMPH != 0 || is_syn_comment(flags);
            strike = flags & fl::STRIKE != 0;
            under = false;
            hidden = false;
            fore = if marker || flags & (fl::HTML | fl::FENCE | fl::PIPE | fl::HR | fl::META) != 0 {
                t.muted
            } else if h > 0 {
                t.accent
            } else if let Some(c) = syn_color(&t, flags) {
                c
            } else if flags & (fl::LINK | fl::FOOTNOTE) != 0 {
                t.link
            } else if flags & (fl::LISTMARK | fl::TASK | fl::MATH) != 0 {
                t.accent
            } else if flags & fl::IMAGE != 0 {
                t.warn
            } else if flags & fl::QUOTE != 0 {
                t.quote
            } else if flags & (fl::CODE | fl::CODEBLOCK) != 0 {
                t.code_fg
            } else {
                t.fg
            };
            back = if flags & (fl::CODEBLOCK | fl::FENCE) != 0 {
                Some(t.code_bg)
            } else if flags & fl::CODE != 0 {
                Some(t.code_inline_bg)
            } else {
                None
            };
        }
        unsafe {
            let _ = f.SetName(&BSTR::from(name));
            let _ = f.SetSize(size);
            let _ = f.SetBold(if bold { TOM_TRUE } else { TOM_FALSE });
            let _ = f.SetItalic(if italic { TOM_TRUE } else { TOM_FALSE });
            let _ = f.SetStrikeThrough(if strike { TOM_TRUE } else { TOM_FALSE });
            let _ = f.SetUnderline(if under { 1 } else { 0 });
            let _ = f.SetForeColor(fore as i32);
            let _ = f.SetBackColor(back.map(|c| c as i32).unwrap_or(TOM_AUTOCOLOR));
            let _ = f.SetHidden(if hidden { TOM_TRUE } else { TOM_FALSE });
        }
        self.fonts.insert((visual, flags), f.clone());
        Some(f)
    }

    fn para(&mut self, visual: bool, st: ParaStyle) -> Option<ITextPara> {
        if let Some(p) = self.paras.get(&(visual, st)) {
            return Some(p.clone());
        }
        let range = self.range.as_ref()?;
        let p = unsafe { range.GetPara().ok()?.GetDuplicate().ok()? };
        let mut left = 0.0f32;
        let mut first = 0.0f32;
        let mut before = 0.0f32;
        let mut after = 0.0f32;
        let mut list = 0i32; // tomListNone
        if visual {
            left += st.quote.min(8) as f32 * 14.0;
            if st.list_depth > 0 {
                left += st.list_depth.min(10) as f32 * 18.0;
                if st.bullet {
                    list = 1; // tomListBullet
                    first = -12.0;
                }
            }
            if st.heading > 0 {
                before = [0.0, 12.0, 10.0, 8.0, 6.0, 6.0, 6.0][st.heading as usize];
                after = 3.0;
            }
        }
        unsafe {
            let _ = p.SetListType(list);
            let _ = p.SetIndents(first, left, 0.0);
            let _ = p.SetSpaceBefore(before);
            let _ = p.SetSpaceAfter(after);
            if visual {
                let _ = p.SetLineSpacing(5, 1.15); // tomLineSpaceMultiple
            } else {
                let _ = p.SetLineSpacing(0, 1.0); // tomLineSpaceSingle
            }
            if list != 0 {
                let _ = p.SetListTab(12.0);
            }
        }
        self.paras.insert((visual, st), p.clone());
        Some(p)
    }

    /// Restyle the editor. `caret_line` — the caret line (for visual mode).
    pub fn restyle(&mut self, hwnd: HWND, visual: bool, caret_line: Option<usize>) {
        let Some(doc) = self.doc.clone() else { return };
        let Some(range) = self.range.clone() else { return };
        let text16 = richedit::get_text(hwnd);
        // Same text, same mode and caret line — the formatting is already correct
        // (returning to edit mode without changes).
        if self.prev_visual == Some(visual) && self.prev_caret == caret_line && !self.prev_text.is_empty() && self.prev_text == text16 {
            return;
        }
        let text: String = String::from_utf16_lossy(&text16)
            .chars()
            .map(|c| if c == '\r' || c == '\u{0B}' { '\n' } else { c })
            .collect();
        // A parser panic must not crash the program: then simply no highlighting.
        let Ok(a) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| styler::analyze(&text, visual, caret_line))) else {
            return;
        };

        let full = self.prev_visual != Some(visual) || self.prev_text.is_empty();
        let (keep_runs, keep_paras): (HashSet<Run>, HashSet<ParaRun>) = if full {
            (HashSet::new(), HashSet::new())
        } else {
            let old = &self.prev_text;
            let new = &text16;
            let max_common = old.len().min(new.len());
            let mut p = 0;
            while p < max_common && old[p] == new[p] {
                p += 1;
            }
            let mut s = 0;
            while s < max_common - p && old[old.len() - 1 - s] == new[new.len() - 1 - s] {
                s += 1;
            }
            let old_len = old.len() as i64;
            let delta = new.len() as i64 - old_len;
            let p = p as i64;
            let tail = old_len - s as i64;
            let shift_run = |st: i64, en: i64| -> Option<(u32, u32)> {
                // run entirely within the common prefix (before the edit point) — unchanged
                if en < p {
                    Some((st as u32, en as u32))
                } else if st > tail {
                    Some(((st + delta) as u32, (en + delta) as u32))
                } else {
                    None
                }
            };
            let runs = self
                .prev_runs
                .iter()
                .filter_map(|r| shift_run(r.start as i64, r.end as i64).map(|(a, b)| Run { start: a, end: b, flags: r.flags }))
                .collect();
            let paras = self
                .prev_paras
                .iter()
                .filter_map(|r| shift_run(r.start as i64, r.end as i64).map(|(a, b)| ParaRun { start: a, end: b, style: r.style }))
                .collect();
            (runs, paras)
        };

        let total = text16.len() as i32;
        unsafe {
            let _ = doc.Undo(tomSuspend.0);
            let _ = doc.Freeze();
        }
        let mut applied: Vec<(i32, i32)> = Vec::new();
        for r in &a.runs {
            if keep_runs.contains(r) {
                continue;
            }
            applied.push((r.start as i32, r.end as i32));
            if let Some(f) = self.font(visual, r.flags) {
                unsafe {
                    let _ = range.SetRange(r.start as i32, (r.end as i32).min(total + 1));
                    let _ = range.SetFont(&f);
                }
            }
        }
        // Emoji and pictographic symbols get their own fonts on top of the run fonts: RichEdit's
        // fallback finds no glyph for many of them (□). Only inside re-applied runs: elsewhere
        // the characters keep the font they got before. (Emoji stay monochrome here — the color
        // overlay is the view's, emoji.rs.)
        applied.sort_unstable();
        let mut k = 0;
        for (s, e, emoji) in crate::render::symbol_clusters(&text16) {
            while k < applied.len() && applied[k].1 <= s {
                k += 1;
            }
            if k < applied.len() && applied[k].0 < e {
                unsafe {
                    let _ = range.SetRange(s, e);
                    if let Ok(f) = range.GetFont() {
                        let _ = f.SetName(&BSTR::from(if emoji { "Segoe UI Emoji" } else { "Segoe UI Symbol" }));
                    }
                }
            }
        }
        for p in &a.paras {
            if keep_paras.contains(p) {
                continue;
            }
            if let Some(pp) = self.para(visual, p.style) {
                unsafe {
                    let _ = range.SetRange(p.start as i32, (p.start as i32 + 1).min(total + 1));
                    let _ = range.SetPara(&pp);
                }
            }
        }
        unsafe {
            let _ = doc.Unfreeze();
            let _ = doc.Undo(tomResume.0);
        }
        self.prev_text = text16;
        self.prev_runs = a.runs;
        self.prev_paras = a.paras;
        self.prev_visual = Some(visual);
        self.prev_caret = caret_line;
    }
}

/// Syntax highlight color for a character inside a code block (None — the regular code color).
fn syn_color(t: &Theme, flags: u32) -> Option<u32> {
    use crate::highlight::Kind;
    if flags & fl::CODEBLOCK == 0 {
        return None;
    }
    match Kind::from_code((flags & fl::SYN_MASK) >> fl::SYN_SHIFT) {
        Kind::Plain => None,
        Kind::Keyword => Some(t.syn_keyword),
        Kind::Type => Some(t.syn_type),
        Kind::Str => Some(t.syn_string),
        Kind::Num => Some(t.syn_number),
        Kind::Comment => Some(t.syn_comment),
        Kind::Func => Some(t.syn_func),
        Kind::Attr => Some(t.syn_attr),
    }
}

fn is_syn_comment(flags: u32) -> bool {
    flags & fl::CODEBLOCK != 0
        && crate::highlight::Kind::from_code((flags & fl::SYN_MASK) >> fl::SYN_SHIFT) == crate::highlight::Kind::Comment
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::richedit::tests::Host;
    use crate::testutil::richedit_lock;

    /// Character formatting at one position, read back through TOM.
    #[derive(Debug, PartialEq, Clone)]
    struct F {
        hidden: bool,
        bold: bool,
        italic: bool,
        strike: bool,
        under: bool,
        size10: i32,
        name: String,
        fore: u32,
        back: Option<u32>,
    }

    fn font(doc: &ITextDocument, cp: i32) -> F {
        unsafe {
            let f = doc.Range(cp, cp + 1).unwrap().GetFont().unwrap();
            let back = f.GetBackColor().unwrap();
            F {
                hidden: f.GetHidden().unwrap() == TOM_TRUE,
                bold: f.GetBold().unwrap() == TOM_TRUE,
                italic: f.GetItalic().unwrap() == TOM_TRUE,
                strike: f.GetStrikeThrough().unwrap() == TOM_TRUE,
                under: f.GetUnderline().unwrap() != 0,
                size10: (f.GetSize().unwrap() * 10.0).round() as i32,
                name: f.GetName().unwrap().to_string(),
                fore: f.GetForeColor().unwrap() as u32,
                back: if back == TOM_AUTOCOLOR { None } else { Some(back as u32) },
            }
        }
    }

    /// Paragraph formatting of the paragraph containing `cp`.
    #[derive(Debug, PartialEq, Clone)]
    struct P {
        list: i32,
        left10: i32,
        first10: i32,
        before10: i32,
        rule: i32,
    }

    fn para(doc: &ITextDocument, cp: i32) -> P {
        unsafe {
            let p = doc.Range(cp, cp).unwrap().GetPara().unwrap();
            P {
                list: p.GetListType().unwrap(),
                left10: (p.GetLeftIndent().unwrap() * 10.0).round() as i32,
                first10: (p.GetFirstLineIndent().unwrap() * 10.0).round() as i32,
                before10: (p.GetSpaceBefore().unwrap() * 10.0).round() as i32,
                rule: p.GetLineSpacingRule().unwrap(),
            }
        }
    }

    /// UTF-16 position of the first `needle` in `t` (line breaks are one position, like '\r').
    fn pos(t: &str, needle: &str) -> i32 {
        let i = t.find(needle).unwrap_or_else(|| panic!("{needle:?}"));
        t[..i].encode_utf16().count() as i32
    }

    const DOC: &str = "# Title\n\n**bold** *it* ~~s~~ `code` [link](http://x) ![img](a.png)\n\n```rust\nfn main() { let s: String = \"x\"; } // c\n```\n\n###### six\n> quote\n- item\n1. one\n$m$ <b>h</b> x[^1]\n\n[^1]: n\n\n---\n";

    fn styled(t: &str, theme: Theme, visual: bool) -> (Host, Styler, ITextDocument) {
        let h = Host::new(false, 700, 500);
        richedit::set_plain_text(h.edit, t);
        let mut st = Styler::new(h.edit, theme);
        st.restyle(h.edit, visual, None);
        let doc = richedit::tom(h.edit).unwrap();
        (h, st, doc)
    }

    /// Emoji and symbols get their fonts in both modes, also in a heading and after an edit.
    #[test]
    fn emoji_and_symbol_fonts() {
        let _g = richedit_lock();
        let t = "# Plan 🟡\n\nok ✅ and ✔ x\n";
        for visual in [true, false] {
            let (h, mut st, doc) = styled(t, Theme::dark(), visual);
            assert_eq!(font(&doc, pos(t, "🟡")).name, "Segoe UI Emoji", "visual={visual}");
            assert_eq!(font(&doc, pos(t, "✅")).name, "Segoe UI Emoji", "visual={visual}");
            assert_eq!(font(&doc, pos(t, "✔")).name, "Segoe UI Symbol", "visual={visual}");
            assert_ne!(font(&doc, pos(t, "ok")).name, "Segoe UI Emoji");
            // An edit in front: the incremental restyle keeps / re-applies the fonts.
            let t2 = format!("x{t}");
            richedit::set_plain_text(h.edit, &t2);
            st.restyle(h.edit, visual, None);
            assert_eq!(font(&doc, pos(&t2, "✅")).name, "Segoe UI Emoji", "visual={visual} after edit");
        }
    }

    #[test]
    fn visual_mode_fonts() {
        let _g = richedit_lock();
        let th = Theme::light();
        let (h, st, doc) = styled(DOC, th, true);
        assert_eq!(st.text(), richedit::get_text(h.edit).as_slice());
        let f = |needle: &str| font(&doc, pos(DOC, needle));
        // heading: the "# " marker is hidden, the text is large, bold, heading-colored
        assert!(font(&doc, 0).hidden && font(&doc, 1).hidden);
        let t = f("Title");
        assert!(t.bold && !t.hidden && t.size10 == 240 && t.name == "Segoe UI" && t.fore == th.heading, "{t:?}");
        // emphasis: markers hidden, text styled
        assert!(f("**bold").hidden);
        let b = f("bold**");
        assert!(b.bold && !b.italic && b.fore == th.fg && b.size10 == 120 && b.back.is_none(), "{b:?}");
        assert!(f("it*").italic && f("s~~").strike);
        // inline code
        assert!(f("`code").hidden);
        let c = f("code`");
        assert!(c.name == "Consolas" && c.size10 == 105 && c.fore == th.code_fg && c.back == Some(th.code_inline_bg), "{c:?}");
        // link: underlined text, hidden address
        let l = f("link]");
        assert!(l.under && l.fore == th.link && !l.hidden, "{l:?}");
        assert!(f("(http://x)").hidden && f("http://x").hidden);
        // image alt text
        let i = f("img]");
        assert!(i.italic && i.fore == th.muted && i.back == Some(th.code_bg), "{i:?}");
        // code block: the fence stays visible and small, the body is highlighted
        let fence = f("```rust");
        assert!(!fence.hidden && fence.size10 == 90 && fence.name == "Consolas" && fence.fore == th.muted && fence.back == Some(th.code_bg), "{fence:?}");
        let kw = f("fn main");
        assert!(kw.fore == th.syn_keyword && kw.back == Some(th.code_bg) && kw.size10 == 105 && !kw.italic, "{kw:?}");
        let cm = f("// c");
        assert!(cm.italic && cm.fore == th.syn_comment, "{cm:?}");
        assert_eq!(f("String").fore, th.syn_type);
        assert_eq!(f("\"x\"").fore, th.syn_string);
        assert_eq!(f("main").fore, th.syn_func);
        assert_eq!(f("s: String").fore, th.code_fg, "plain code keeps the code color");
        // a level-6 heading is muted
        let six = f("six");
        assert!(six.bold && six.size10 == 120 && six.fore == th.muted, "{six:?}");
        // quote, bullet marker hidden, ordered marker visible
        assert!(f("> quote").hidden && f("quote").fore == th.quote);
        assert!(f("- item").hidden && !f("item").hidden);
        let one = f("1. one");
        assert!(!one.hidden && one.fore == th.muted, "{one:?}");
        // math, inline HTML, footnote, rule
        let m = f("m$");
        assert!(m.name == "Consolas" && m.fore == th.accent, "{m:?}");
        let html = f("<b>");
        assert!(!html.hidden && html.name == "Consolas" && html.fore == th.muted, "{html:?}");
        assert_eq!(f("[^1]").fore, th.link);
        assert_eq!(f("---").fore, th.muted);
        // formatting is not recorded in the undo history
        assert_eq!(crate::util::send(h.edit, windows::Win32::UI::Controls::EM_CANUNDO, 0, 0), 0);
    }

    #[test]
    fn visual_mode_paragraphs() {
        let _g = richedit_lock();
        let (_h, _st, doc) = styled(DOC, Theme::light(), true);
        let p = |needle: &str| para(&doc, pos(DOC, needle));
        let h1 = p("# Title");
        assert_eq!((h1.before10, h1.list, h1.rule), (120, 0, 5), "{h1:?}");
        assert_eq!(p("###### six").before10, 60);
        assert_eq!(p("> quote").left10, 140);
        let item = p("- item");
        assert_eq!((item.list, item.left10, item.first10), (1, 180, -120), "bullet list: {item:?}");
        let one = p("1. one");
        assert_eq!((one.list, one.left10), (0, 180), "ordered items keep their visible number: {one:?}");
        let plain = p("$m$");
        assert_eq!((plain.list, plain.left10, plain.before10, plain.rule), (0, 0, 0, 5));
    }

    #[test]
    fn code_mode_and_caret_line() {
        let _g = richedit_lock();
        let th = Theme::light();
        let (h, mut st, doc) = styled(DOC, th, false);
        let f = |needle: &str| font(&doc, pos(DOC, needle));
        // nothing is hidden, one monospace font
        let hash = font(&doc, 0);
        assert!(!hash.hidden && hash.bold && hash.name == "Consolas" && hash.size10 == 110 && hash.fore == th.muted, "{hash:?}");
        let t = f("Title");
        assert!(t.bold && t.fore == th.accent && t.size10 == 110, "{t:?}");
        assert!(!f("**bold").hidden && f("bold**").bold);
        let l = f("link]");
        assert!(!l.under && l.fore == th.link, "{l:?}");
        assert_eq!(f("img]").fore, th.warn);
        assert_eq!(f("quote").fore, th.quote);
        assert_eq!(f("code`").back, Some(th.code_inline_bg));
        assert_eq!(f("fn main").fore, th.syn_keyword);
        assert!(f("// c").italic);
        assert_eq!(f("<b>").fore, th.muted);
        assert_eq!(f("[^1]").fore, th.link);
        assert_eq!(f("m$").fore, th.accent);
        let item = para(&doc, pos(DOC, "- item"));
        assert_eq!((item.list, item.left10, item.rule), (0, 0, 0), "{item:?}");
        // visual mode with the caret on the heading line: its markers are revealed
        st.restyle(h.edit, true, Some(0));
        assert!(!font(&doc, 0).hidden && font(&doc, 0).name == "Segoe UI");
        assert!(f("**bold").hidden, "other lines keep hidden markers");
        // the caret moves away: hidden again
        st.restyle(h.edit, true, Some(2));
        assert!(font(&doc, 0).hidden);
        // the same text, mode and caret: nothing to do
        st.restyle(h.edit, true, Some(2));
        assert!(font(&doc, 0).hidden);
    }

    #[test]
    fn theme_switch_restyles_everything() {
        let _g = richedit_lock();
        let (h, mut st, doc) = styled(DOC, Theme::light(), true);
        let dark = Theme::dark();
        st.set_theme(dark);
        assert!(st.text().is_empty(), "invalidated");
        st.restyle(h.edit, true, None);
        let b = font(&doc, pos(DOC, "bold**"));
        assert_eq!(b.fore, dark.fg);
        assert_eq!(font(&doc, pos(DOC, "Title")).fore, dark.heading);
        assert_eq!(font(&doc, pos(DOC, "fn main")).back, Some(dark.code_bg));
    }

    /// After an edit only the changed runs are reapplied; the result must equal a full restyle.
    #[test]
    fn incremental_restyle_equals_full_restyle() {
        let _g = richedit_lock();
        let th = Theme::light();
        let (a, mut st, doc_a) = styled(DOC, th, true);
        // (selection to replace, new text) — in the middle, across constructs, at both ends,
        // turning a paragraph into a heading, deleting a code fence
        let edits: Vec<(&str, &str)> = vec![
            ("bold", "BOLDER **x**"),
            ("it* ~~s~~ `co", ""),
            ("# Title", "Title"),
            ("---\n", "---\nend `z`"),
            ("$m$", "# $m$"),
            ("```\n", ""),
            ("Title", "Ünïcödé 😀 **b**"),
        ];
        for (old, new) in edits {
            let cur = crate::richedit::tests::text(a.edit).replace('\r', "\n");
            let s = pos(&cur, old);
            richedit::set_sel(a.edit, s, s + old.encode_utf16().count() as i32);
            richedit::replace_sel(a.edit, new, true);
            st.restyle(a.edit, true, None);
            let now = crate::richedit::tests::text(a.edit).replace('\r', "\n");
            assert_eq!(now, cur.replacen(old, new, 1));
            // reference: the same text styled from scratch in another control
            let (b, _st_b, doc_b) = styled(&now, th, true);
            let n = richedit::text_len(b.edit);
            for cp in 0..n {
                assert_eq!(font(&doc_a, cp), font(&doc_b, cp), "after {old:?} → {new:?}: char {cp} in {now:?}");
            }
            let mut cp = 0;
            for line in now.split('\n') {
                assert_eq!(para(&doc_a, cp), para(&doc_b, cp), "after {old:?} → {new:?}: line {line:?}");
                cp += line.encode_utf16().count() as i32 + 1;
            }
        }
    }

    /// Without TOM (not a RichEdit) the styler does nothing and does not panic.
    #[test]
    fn no_tom_no_styling() {
        let _g = richedit_lock();
        let h = Host::new(false, 200, 100);
        let edit_class = crate::util::wide("EDIT");
        let edit = unsafe {
            windows::Win32::UI::WindowsAndMessaging::CreateWindowExW(
                Default::default(),
                windows::core::PCWSTR::from_raw(edit_class.as_ptr()),
                windows::core::PCWSTR::null(),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD,
                0,
                0,
                10,
                10,
                Some(h.parent),
                None,
                None,
                None,
            )
        }
        .unwrap();
        let mut st = Styler::new(edit, Theme::light());
        st.restyle(edit, true, Some(0));
        assert!(st.text().is_empty());
    }
}
