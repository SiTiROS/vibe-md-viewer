//! Parsing markdown source into formatting "runs" for the editing modes.
//!
//! The text in the editor is always the markdown itself, so saving is lossless.
//! * "Code" mode: monospace font, markup highlighted, nothing hidden.
//! * "Visual" mode: proportional font, large headings, syntax characters
//!   (`**`, `#`, `](url)` …) hidden everywhere except the caret line.

use pulldown_cmark::{CodeBlockKind, Event, LinkType, Parser, Tag, TagEnd};

pub mod fl {
    pub const STRONG: u32 = 1 << 0;
    pub const EMPH: u32 = 1 << 1;
    pub const STRIKE: u32 = 1 << 2;
    pub const CODE: u32 = 1 << 3;
    pub const CODEBLOCK: u32 = 1 << 4;
    pub const LINK: u32 = 1 << 5;
    pub const URL: u32 = 1 << 6;
    pub const MARKER: u32 = 1 << 7;
    pub const H_SHIFT: u32 = 8;
    pub const H_MASK: u32 = 7 << 8;
    pub const QUOTE: u32 = 1 << 11;
    pub const HTML: u32 = 1 << 12;
    pub const TABLE: u32 = 1 << 13;
    pub const THEAD: u32 = 1 << 14;
    pub const HR: u32 = 1 << 15;
    pub const IMAGE: u32 = 1 << 16;
    pub const LISTMARK: u32 = 1 << 17;
    pub const TASK: u32 = 1 << 18;
    pub const FOOTNOTE: u32 = 1 << 19;
    pub const FENCE: u32 = 1 << 20;
    pub const REVEAL: u32 = 1 << 21;
    pub const PIPE: u32 = 1 << 22;
    pub const ORDERED: u32 = 1 << 23;
    pub const MATH: u32 = 1 << 24;
    pub const META: u32 = 1 << 25;
    /// Syntax highlighting inside a code block: highlight::Kind (0..7) in bits 26..28.
    pub const SYN_SHIFT: u32 = 26;
    pub const SYN_MASK: u32 = 7 << 26;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ParaStyle {
    pub heading: u8,
    pub list_depth: u8,
    pub bullet: bool,
    pub quote: u8,
    pub code: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Run {
    pub start: u32,
    pub end: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParaRun {
    pub start: u32,
    pub end: u32,
    pub style: ParaStyle,
}

#[derive(Default, Debug)]
pub struct Analysis {
    pub runs: Vec<Run>,
    pub paras: Vec<ParaRun>,
}

struct LinkCtx {
    range: std::ops::Range<usize>,
    min: usize,
    max: usize,
    image: bool,
    auto: bool,
}

fn or_range(f: &mut [u32], a: usize, b: usize, bits: u32) {
    let b = b.min(f.len());
    if a < b {
        for x in &mut f[a..b] {
            *x |= bits;
        }
    }
}

/// `text` — source with '\n' line breaks. `caret_line` — the line with the caret
/// (markup on it is not hidden in visual mode).
pub fn analyze(text: &str, visual: bool, caret_line: Option<usize>) -> Analysis {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut f = vec![0u32; n];

    let mut line_starts = vec![0usize];
    for (i, &c) in bytes.iter().enumerate() {
        if c == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let nlines = line_starts.len();
    let line_of = |pos: usize| -> usize {
        match line_starts.binary_search(&pos) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    };
    let line_end = |l: usize| -> usize { if l + 1 < nlines { line_starts[l + 1] - 1 } else { n } };
    let mut lp = vec![ParaStyle::default(); nlines];

    let mut links: Vec<LinkCtx> = Vec::new();
    let mut list_stack: Vec<bool> = Vec::new(); // ordered?

    let parser = Parser::new_ext(text, crate::render::md_options()).into_offset_iter();
    for (ev, r) in parser {
        // Track child ranges for links/images.
        if !matches!(ev, Event::Start(Tag::Link { .. }) | Event::Start(Tag::Image { .. })) {
            if let Some(ctx) = links.last_mut() {
                if !matches!(ev, Event::End(TagEnd::Link) | Event::End(TagEnd::Image)) {
                    ctx.min = ctx.min.min(r.start);
                    ctx.max = ctx.max.max(r.end);
                }
            }
        }
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                let lv = level as u32;
                or_range(&mut f, r.start, r.end, lv << fl::H_SHIFT);
                let l0 = line_of(r.start);
                let l1 = line_of(r.end.saturating_sub(1).max(r.start));
                if bytes.get(r.start) == Some(&b'#') {
                    let mut e = r.start;
                    while e < r.end && bytes[e] == b'#' {
                        e += 1;
                    }
                    while e < r.end && (bytes[e] == b' ' || bytes[e] == b'\t') {
                        e += 1;
                    }
                    or_range(&mut f, r.start, e, fl::MARKER);
                    // Closing sequence "## Heading ##"
                    let le = line_end(l0).min(r.end);
                    let mut t = le;
                    while t > e && (bytes[t - 1] == b' ' || bytes[t - 1] == b'\t') {
                        t -= 1;
                    }
                    let mut h = t;
                    while h > e && bytes[h - 1] == b'#' {
                        h -= 1;
                    }
                    if h < t && h > e && (bytes[h - 1] == b' ' || bytes[h - 1] == b'\t') {
                        or_range(&mut f, h - 1, le, fl::MARKER);
                    }
                    lp[l0].heading = lv as u8;
                } else {
                    // Setext: the last line is the === / --- underline
                    for l in l0..l1 {
                        lp[l].heading = lv as u8;
                    }
                    if l1 > l0 {
                        or_range(&mut f, line_starts[l1], line_end(l1), fl::MARKER);
                    }
                }
            }
            Event::Start(Tag::Emphasis) => {
                or_range(&mut f, r.start, r.end, fl::EMPH);
                or_range(&mut f, r.start, r.start + 1, fl::MARKER);
                or_range(&mut f, r.end.saturating_sub(1), r.end, fl::MARKER);
            }
            Event::Start(Tag::Strong) => {
                or_range(&mut f, r.start, r.end, fl::STRONG);
                or_range(&mut f, r.start, r.start + 2, fl::MARKER);
                or_range(&mut f, r.end.saturating_sub(2), r.end, fl::MARKER);
            }
            Event::Start(Tag::Strikethrough) => {
                let mut k = 0;
                while r.start + k < r.end && bytes[r.start + k] == b'~' {
                    k += 1;
                }
                or_range(&mut f, r.start, r.end, fl::STRIKE);
                or_range(&mut f, r.start, r.start + k, fl::MARKER);
                or_range(&mut f, r.end.saturating_sub(k), r.end, fl::MARKER);
            }
            Event::Code(_) => {
                let mut k = 0;
                while r.start + k < r.end && bytes[r.start + k] == b'`' {
                    k += 1;
                }
                or_range(&mut f, r.start, r.end, fl::CODE);
                or_range(&mut f, r.start, r.start + k, fl::MARKER);
                or_range(&mut f, r.end.saturating_sub(k), r.end, fl::MARKER);
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => or_range(&mut f, r.start, r.end, fl::MATH),
            Event::Start(Tag::Link { link_type, .. }) => {
                let auto = matches!(link_type, LinkType::Autolink | LinkType::Email);
                links.push(LinkCtx { range: r.clone(), min: usize::MAX, max: 0, image: false, auto });
            }
            Event::Start(Tag::Image { .. }) => {
                links.push(LinkCtx { range: r.clone(), min: usize::MAX, max: 0, image: true, auto: false });
            }
            Event::End(TagEnd::Link) | Event::End(TagEnd::Image) => {
                if let Some(ctx) = links.pop() {
                    let rr = ctx.range.clone();
                    let (ta, tb) = if ctx.auto {
                        (rr.start + 1, rr.end.saturating_sub(1))
                    } else if ctx.min == usize::MAX {
                        let s = rr.start + if ctx.image { 2 } else { 1 };
                        (s, s)
                    } else {
                        (ctx.min, ctx.max)
                    };
                    let bit = if ctx.image { fl::IMAGE } else { fl::LINK };
                    or_range(&mut f, ta, tb, bit);
                    or_range(&mut f, rr.start, ta, fl::MARKER | fl::URL);
                    or_range(&mut f, tb, rr.end, fl::MARKER | fl::URL);
                    // Parent link (an image inside a link) — account for the whole range.
                    if let Some(p) = links.last_mut() {
                        p.min = p.min.min(rr.start);
                        p.max = p.max.max(rr.end);
                    }
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let l0 = line_of(r.start);
                let l1 = line_of(r.end.saturating_sub(1).max(r.start));
                let fenced = matches!(kind, CodeBlockKind::Fenced(_));
                for l in l0..=l1.min(nlines - 1) {
                    lp[l].code = true;
                    let (a, b) = (line_starts[l], line_end(l));
                    let line = &text[a..b];
                    let t = line.trim_start();
                    let is_fence = fenced && (l == l0 || (l == l1 && (t.starts_with("```") || t.starts_with("~~~"))));
                    if is_fence {
                        or_range(&mut f, a, b, fl::FENCE | fl::MARKER);
                    } else {
                        or_range(&mut f, a, b, fl::CODEBLOCK);
                    }
                }
                // Syntax highlighting of the block body (between the fence lines).
                let last = l1.min(nlines - 1);
                let body_a = if fenced { line_starts.get(l0 + 1).copied().unwrap_or(r.end) } else { r.start };
                let closing = fenced && last > l0 && {
                    let t = text[line_starts[last]..line_end(last)].trim_start();
                    t.starts_with("```") || t.starts_with("~~~")
                };
                let body_b = if closing { line_starts[last] } else { r.end }.min(text.len());
                if body_a < body_b && text.is_char_boundary(body_a) && text.is_char_boundary(body_b) {
                    let lang = match &kind {
                        CodeBlockKind::Fenced(info) => info.as_ref(),
                        CodeBlockKind::Indented => "",
                    };
                    for (k, sr) in crate::highlight::spans(lang, &text[body_a..body_b]) {
                        if k != crate::highlight::Kind::Plain {
                            or_range(&mut f, body_a + sr.start, body_a + sr.end, k.code() << fl::SYN_SHIFT);
                        }
                    }
                }
            }
            Event::Start(Tag::MetadataBlock(_)) => or_range(&mut f, r.start, r.end, fl::META),
            Event::Start(Tag::BlockQuote(_)) => {
                let l0 = line_of(r.start);
                let l1 = line_of(r.end.saturating_sub(1).max(r.start));
                for l in l0..=l1.min(nlines - 1) {
                    lp[l].quote = lp[l].quote.saturating_add(1);
                }
                or_range(&mut f, r.start, r.end, fl::QUOTE);
            }
            Event::Start(Tag::List(start)) => list_stack.push(start.is_some()),
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                let ordered = *list_stack.last().unwrap_or(&false);
                let mut e = r.start;
                if ordered {
                    while e < r.end && bytes[e].is_ascii_digit() {
                        e += 1;
                    }
                    if e < r.end && (bytes[e] == b'.' || bytes[e] == b')') {
                        e += 1;
                    }
                } else if e < r.end && matches!(bytes[e], b'-' | b'*' | b'+') {
                    e += 1;
                }
                let mut sp = 0;
                while e < r.end && bytes[e] == b' ' && sp < 4 {
                    e += 1;
                    sp += 1;
                }
                let bits = fl::MARKER | fl::LISTMARK | if ordered { fl::ORDERED } else { 0 };
                or_range(&mut f, r.start, e, bits);
                let l = line_of(r.start);
                lp[l].list_depth = list_stack.len().min(9) as u8;
                lp[l].bullet = !ordered;
            }
            Event::TaskListMarker(_) => or_range(&mut f, r.start, r.end, fl::TASK),
            Event::Start(Tag::Table(_)) => {
                or_range(&mut f, r.start, r.end, fl::TABLE);
                let mut esc = false;
                for i in r.start..r.end.min(n) {
                    if bytes[i] == b'|' && !esc {
                        f[i] |= fl::PIPE;
                    }
                    esc = bytes[i] == b'\\' && !esc;
                }
            }
            Event::Start(Tag::TableHead) => {
                or_range(&mut f, r.start, r.end, fl::THEAD);
                // The |---|:--:| delimiter row comes right after the header.
                let l = line_of(r.end.saturating_sub(1).max(r.start)) + 1;
                if l < nlines {
                    let (a, b) = (line_starts[l], line_end(l));
                    if text[a..b].trim().chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t')) {
                        or_range(&mut f, a, b, fl::PIPE);
                    }
                }
            }
            Event::Rule => or_range(&mut f, r.start, r.end, fl::HR),
            Event::Html(_) | Event::InlineHtml(_) => or_range(&mut f, r.start, r.end, fl::HTML),
            Event::FootnoteReference(_) => or_range(&mut f, r.start, r.end, fl::FOOTNOTE),
            Event::Start(Tag::FootnoteDefinition(_)) => {
                if let Some(k) = text[r.start..r.end].find("]:") {
                    or_range(&mut f, r.start, r.start + k + 2, fl::FOOTNOTE);
                }
            }
            _ => {}
        }
    }

    // Quote markers "> " at line starts.
    for l in 0..nlines {
        let depth = lp[l].quote;
        if depth == 0 {
            continue;
        }
        let (a, b) = (line_starts[l], line_end(l));
        let mut i = a;
        let mut found = 0;
        while i < b && found < depth {
            let mut sp = 0;
            while i < b && bytes[i] == b' ' && sp < 3 {
                i += 1;
                sp += 1;
            }
            if i < b && bytes[i] == b'>' {
                let s = i;
                i += 1;
                if i < b && bytes[i] == b' ' {
                    i += 1;
                }
                or_range(&mut f, s, i, fl::MARKER);
                found += 1;
            } else {
                break;
            }
        }
    }

    // In visual mode the markup on the caret line is visible.
    if visual {
        if let Some(cl) = caret_line {
            if cl < nlines {
                let (a, b) = (line_starts[cl], line_end(cl));
                for x in &mut f[a..b] {
                    if *x & fl::MARKER != 0 {
                        *x |= fl::REVEAL;
                    }
                }
                lp[cl].bullet = false;
            }
        }
    } else {
        for p in lp.iter_mut() {
            *p = ParaStyle::default();
        }
    }

    // Line breaks get no formatting (otherwise a hidden "end of paragraph" would merge lines).
    for (i, &c) in bytes.iter().enumerate() {
        if c == b'\n' {
            f[i] = 0;
        }
    }

    // Bytes → UTF-16 runs.
    let mut runs: Vec<Run> = Vec::new();
    let mut u16pos: u32 = 0;
    let mut line_u16 = Vec::with_capacity(nlines + 1);
    line_u16.push(0u32);
    for (i, ch) in text.char_indices() {
        let flags = f[i];
        let w = ch.len_utf16() as u32;
        match runs.last_mut() {
            Some(last) if last.flags == flags && last.end == u16pos => last.end += w,
            _ => runs.push(Run { start: u16pos, end: u16pos + w, flags }),
        }
        u16pos += w;
        if ch == '\n' {
            line_u16.push(u16pos);
        }
    }
    let total = u16pos;
    let mut paras = Vec::with_capacity(nlines);
    for l in 0..nlines {
        let s = line_u16[l];
        let e = if l + 1 < line_u16.len() { line_u16[l + 1] } else { total + 1 };
        paras.push(ParaRun { start: s, end: e.max(s + 1), style: lp[l] });
    }
    Analysis { runs, paras }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags_at(a: &Analysis, pos: u32) -> u32 {
        a.runs.iter().find(|r| r.start <= pos && pos < r.end).map(|r| r.flags).unwrap_or(0)
    }

    #[test]
    fn markers_detected() {
        let t = "# Title\n\n**bold** [l](http://x)\n";
        let a = analyze(t, true, None);
        assert!(flags_at(&a, 0) & fl::MARKER != 0); // '#'
        assert!(flags_at(&a, 2) & fl::MARKER == 0); // 'T'
        assert_eq!((flags_at(&a, 2) & fl::H_MASK) >> fl::H_SHIFT, 1);
        let b = t.find("bold").unwrap() as u32;
        assert!(flags_at(&a, b) & fl::STRONG != 0);
        assert!(flags_at(&a, b - 1) & fl::MARKER != 0);
        let l = t.find("[l]").unwrap() as u32;
        assert!(flags_at(&a, l) & fl::MARKER != 0);
        assert!(flags_at(&a, l + 1) & fl::LINK != 0);
        assert!(flags_at(&a, l + 3) & fl::URL != 0);
        assert_eq!(a.paras[0].style.heading, 1);
    }

    #[test]
    fn reveal_on_caret_line() {
        let a = analyze("**b**\n**c**", true, Some(1));
        assert!(flags_at(&a, 0) & fl::REVEAL == 0);
        assert!(flags_at(&a, 6) & fl::REVEAL != 0);
    }

    #[test]
    fn newlines_never_hidden() {
        let a = analyze("# a\n> b\n", true, None);
        assert_eq!(flags_at(&a, 3), 0);
    }

    #[test]
    fn code_block_syntax_flags() {
        use crate::highlight::Kind;
        let t = "```rust
fn main() { // привет
}
```
";
        let a = analyze(t, true, None);
        let syn = |pos: u32| Kind::from_code((flags_at(&a, pos) & fl::SYN_MASK) >> fl::SYN_SHIFT);
        let body = "```rust
".encode_utf16().count() as u32;
        assert_eq!(syn(body), Kind::Keyword); // fn
        let c = "```rust
fn main() { ".encode_utf16().count() as u32;
        assert_eq!(syn(c), Kind::Comment);
        assert_eq!(syn(0), Kind::Plain); // the fence is not highlighted
    }

    #[test]
    fn utf16_offsets() {
        let a = analyze("Привет **мир**", true, None);
        let pos = "Привет **".encode_utf16().count() as u32;
        assert!(flags_at(&a, pos) & fl::STRONG != 0);
        assert!(flags_at(&a, pos) & fl::MARKER == 0);
    }
}
