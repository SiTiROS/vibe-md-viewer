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
        for r in &a.runs {
            if keep_runs.contains(r) {
                continue;
            }
            if let Some(f) = self.font(visual, r.flags) {
                unsafe {
                    let _ = range.SetRange(r.start as i32, (r.end as i32).min(total + 1));
                    let _ = range.SetFont(&f);
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
