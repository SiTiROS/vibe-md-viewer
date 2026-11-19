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

    /// UTF-16 position of the `nth` occurrence of `needle` in `t`.
    fn at(t: &str, needle: &str, nth: usize) -> u32 {
        let (i, _) = t.match_indices(needle).nth(nth).unwrap_or_else(|| panic!("{needle:?} #{nth} in {t:?}"));
        t[..i].encode_utf16().count() as u32
    }

    fn has(a: &Analysis, pos: u32, bits: u32) -> bool {
        flags_at(a, pos) & bits == bits
    }

    fn heading_of(a: &Analysis, pos: u32) -> u32 {
        (flags_at(a, pos) & fl::H_MASK) >> fl::H_SHIFT
    }

    /// Runs and paragraph runs are contiguous and cover the whole text (UTF-16).
    fn check_runs(a: &Analysis, t: &str) {
        let total = t.encode_utf16().count() as u32;
        let mut pos = 0;
        for r in &a.runs {
            assert_eq!(r.start, pos);
            assert!(r.end > r.start);
            pos = r.end;
        }
        assert_eq!(pos, total);
        assert_eq!(a.paras.len(), t.split('\n').count());
        let mut pos = 0;
        for p in &a.paras {
            assert_eq!(p.start, pos);
            assert!(p.end > p.start);
            pos = p.end;
        }
        assert_eq!(pos, total + 1, "the last paragraph run ends one past the text");
    }

    #[test]
    fn atx_heading_closing_sequence() {
        let t = "## Title ##  \n### T\t#\t\n# C#\n";
        let a = analyze(t, true, None);
        check_runs(&a, t);
        assert!(has(&a, 0, fl::MARKER) && has(&a, 2, fl::MARKER), "opening \"## \"");
        assert!(!has(&a, at(t, "Title", 0), fl::MARKER));
        assert_eq!(heading_of(&a, at(t, "Title", 0)), 2);
        // " ##  " is a closing sequence, hidden together with the trailing spaces
        for p in at(t, " ##", 0)..at(t, "\n", 0) {
            assert!(has(&a, p, fl::MARKER), "closing sequence at {p}");
        }
        // a tab before the closing sequence counts as a separator
        assert!(has(&a, at(t, "\t#", 0), fl::MARKER) && has(&a, at(t, "#\t", 0), fl::MARKER));
        assert!(!has(&a, at(t, "T\t", 0), fl::MARKER));
        assert_eq!(heading_of(&a, at(t, "T\t", 0)), 3);
        // "C#" is text, not a closing sequence
        assert!(!has(&a, at(t, "C#", 0) + 1, fl::MARKER));
        assert_eq!(a.paras.iter().map(|p| p.style.heading).collect::<Vec<_>>(), vec![2, 3, 1, 0]);
    }

    #[test]
    fn setext_headings() {
        let t = "Title\nmore\n=====\n\nSub\n---\n";
        let a = analyze(t, true, None);
        check_runs(&a, t);
        assert_eq!(heading_of(&a, 0), 1);
        assert!(!has(&a, 0, fl::MARKER), "setext text is visible");
        assert!(has(&a, at(t, "=", 0), fl::MARKER) && has(&a, at(t, "=", 4), fl::MARKER), "the underline is markup");
        assert_eq!(heading_of(&a, at(t, "Sub", 0)), 2);
        assert!(has(&a, at(t, "---", 0), fl::MARKER));
        // every text line of the heading gets the heading paragraph style, the underline does not
        assert_eq!(a.paras.iter().map(|p| p.style.heading).collect::<Vec<_>>(), vec![1, 1, 0, 0, 2, 0, 0]);
    }

    #[test]
    fn inline_constructs() {
        let t = "~~s~~ `c` $m$ ![i](p.png) <https://x.y> [](http://e) [^1] <b>h</b>\n\n[^1]: note\n";
        let a = analyze(t, true, None);
        check_runs(&a, t);
        // strikethrough: the tildes are markers
        assert!(has(&a, 0, fl::STRIKE | fl::MARKER) && has(&a, 4, fl::STRIKE | fl::MARKER));
        assert!(has(&a, 2, fl::STRIKE) && !has(&a, 2, fl::MARKER));
        // inline code
        let c = at(t, "`c`", 0);
        assert!(has(&a, c, fl::CODE | fl::MARKER) && has(&a, c + 2, fl::CODE | fl::MARKER));
        assert!(has(&a, c + 1, fl::CODE) && !has(&a, c + 1, fl::MARKER));
        // math
        assert!(has(&a, at(t, "$m$", 0) + 1, fl::MATH));
        // image: alt text is IMAGE, the syntax around is hidden markup with the URL
        let i = at(t, "![i]", 0);
        assert!(has(&a, i, fl::MARKER | fl::URL) && has(&a, i + 1, fl::MARKER | fl::URL));
        assert!(has(&a, i + 2, fl::IMAGE) && !has(&a, i + 2, fl::MARKER));
        assert!(has(&a, at(t, "p.png", 0), fl::MARKER | fl::URL));
        // autolink: the address itself is the link text, only the angle brackets are markup
        let l = at(t, "<https", 0);
        assert!(has(&a, l, fl::MARKER | fl::URL));
        assert!(has(&a, l + 1, fl::LINK) && !has(&a, l + 1, fl::MARKER));
        assert!(has(&a, at(t, "x.y>", 0) + 3, fl::MARKER | fl::URL));
        // a link without text is all markup
        let e = at(t, "[](", 0);
        for p in e..at(t, "http://e)", 0) + 9 {
            assert!(has(&a, p, fl::MARKER | fl::URL) && !has(&a, p, fl::LINK), "{p}");
        }
        // footnote reference and definition label
        assert!(has(&a, at(t, "[^1]", 0) + 1, fl::FOOTNOTE));
        let d = at(t, "[^1]:", 0);
        assert!(has(&a, d, fl::FOOTNOTE) && has(&a, d + 4, fl::FOOTNOTE));
        assert!(!has(&a, at(t, "note", 0), fl::FOOTNOTE));
        // inline HTML tags
        assert!(has(&a, at(t, "<b>", 0), fl::HTML) && has(&a, at(t, "</b>", 0), fl::HTML));
        assert!(!has(&a, at(t, "h</b>", 0), fl::HTML));
    }

    #[test]
    fn image_inside_link() {
        let t = "[![a](i.png)](http://x)";
        let a = analyze(t, true, None);
        check_runs(&a, t);
        let alt = at(t, "a]", 0);
        assert!(has(&a, alt, fl::IMAGE | fl::LINK), "the image is the link's content");
        assert!(has(&a, 0, fl::MARKER | fl::URL), "outer '['");
        assert!(has(&a, at(t, "](http", 0), fl::MARKER | fl::URL));
        assert!(has(&a, at(t, "i.png", 0), fl::MARKER | fl::URL | fl::LINK));
    }

    #[test]
    fn block_constructs() {
        let t = "---\ntitle: x\n---\n\n***\n\n<div>\nx\n</div>\n\n| a | b |\n|---|:-:|\n| 1 | \\| |\n\n- [x] done\n- b\n  - c\n\n1. one\n2) two\n";
        let a = analyze(t, true, None);
        check_runs(&a, t);
        assert!(has(&a, at(t, "title", 0), fl::META), "front matter");
        assert!(has(&a, at(t, "***", 0), fl::HR));
        assert!(has(&a, at(t, "<div>", 0), fl::HTML) && has(&a, at(t, "</div>", 0), fl::HTML));
        // table: unescaped pipes, the header and the delimiter row
        let row = at(t, "| a", 0);
        assert!(has(&a, row, fl::TABLE | fl::PIPE | fl::THEAD));
        assert!(has(&a, row + 2, fl::TABLE | fl::THEAD) && !has(&a, row + 2, fl::PIPE));
        assert!(has(&a, at(t, "|---", 0) + 1, fl::PIPE), "the delimiter row is markup");
        assert!(has(&a, at(t, ":-:", 0), fl::PIPE));
        let esc = at(t, "\\|", 0);
        assert!(has(&a, esc + 1, fl::TABLE) && !has(&a, esc + 1, fl::PIPE), "an escaped pipe is text");
        assert!(!has(&a, at(t, "| 1", 0) + 2, fl::THEAD));
        // task list
        let task = at(t, "- [x]", 0);
        assert!(has(&a, task, fl::MARKER | fl::LISTMARK) && !has(&a, task, fl::ORDERED));
        assert!(has(&a, task + 2, fl::TASK));
        // ordered lists, both delimiters
        let one = at(t, "1. one", 0);
        assert!(has(&a, one, fl::MARKER | fl::LISTMARK | fl::ORDERED) && has(&a, one + 2, fl::MARKER | fl::ORDERED));
        assert!(!has(&a, one + 3, fl::MARKER));
        assert!(has(&a, at(t, "2)", 0) + 1, fl::MARKER | fl::ORDERED));
        // paragraph styles: list depth and bullets
        let line = |s: &str| t[..t.find(s).unwrap()].matches('\n').count();
        let st = |s: &str| a.paras[line(s)].style;
        assert_eq!((st("- [x]").list_depth, st("- [x]").bullet), (1, true));
        assert_eq!((st("  - c").list_depth, st("  - c").bullet), (2, true));
        assert_eq!((st("1. one").list_depth, st("1. one").bullet), (1, false));
    }

    #[test]
    fn code_blocks() {
        let t = "```py\nx = 1\n```\n\n    indented\n\n~~~\nopen\n~~~\n\n```\nunclosed\n";
        let a = analyze(t, true, None);
        check_runs(&a, t);
        assert!(has(&a, 0, fl::FENCE | fl::MARKER) && has(&a, 4, fl::FENCE | fl::MARKER), "opening fence with the info string");
        let body = at(t, "x = 1", 0);
        assert!(has(&a, body, fl::CODEBLOCK) && !has(&a, body, fl::MARKER));
        assert_eq!(Kind::from_code((flags_at(&a, body + 4) & fl::SYN_MASK) >> fl::SYN_SHIFT), Kind::Num);
        assert!(has(&a, at(t, "```\n", 1), fl::FENCE | fl::MARKER), "closing fence");
        // an indented block: code without fences, no highlighting language
        let ind = at(t, "indented", 0);
        assert!(has(&a, ind, fl::CODEBLOCK) && !has(&a, ind, fl::FENCE));
        assert_eq!(flags_at(&a, ind) & fl::SYN_MASK, 0);
        assert!(has(&a, at(t, "open", 0), fl::CODEBLOCK));
        assert!(has(&a, at(t, "~~~", 1), fl::FENCE | fl::MARKER));
        // an unclosed fence runs to the end; its last line is code, not a fence
        assert!(has(&a, at(t, "unclosed", 0), fl::CODEBLOCK));
        let line = |s: &str| t[..t.find(s).unwrap()].matches('\n').count();
        assert!(a.paras[line("x = 1")].style.code && a.paras[line("    indented")].style.code && a.paras[line("unclosed")].style.code);
        assert!(!a.paras[line("    indented") - 1].style.code, "the blank line between blocks");
    }

    #[test]
    fn quote_markers() {
        let t = "> a\n> > b\n\n  > c\nlazy\n";
        let a = analyze(t, true, None);
        check_runs(&a, t);
        assert!(has(&a, 0, fl::MARKER | fl::QUOTE) && has(&a, 1, fl::MARKER), "\"> \"");
        assert!(!has(&a, 2, fl::MARKER) && has(&a, 2, fl::QUOTE));
        let l1 = at(t, "> > b", 0);
        for p in l1..l1 + 4 {
            assert!(has(&a, p, fl::MARKER), "nested markers at {p}");
        }
        assert!(!has(&a, l1 + 4, fl::MARKER));
        // up to three leading spaces are skipped (and stay visible), "> " is the marker
        let l2 = at(t, "  > c", 0);
        assert!(!has(&a, l2, fl::MARKER) && !has(&a, l2 + 1, fl::MARKER));
        assert!(has(&a, l2 + 2, fl::MARKER) && has(&a, l2 + 3, fl::MARKER) && !has(&a, l2 + 4, fl::MARKER));
        // a lazy continuation line belongs to the quote but has no marker
        let lazy = at(t, "lazy", 0);
        assert!(has(&a, lazy, fl::QUOTE) && !has(&a, lazy, fl::MARKER));
        let q: Vec<u8> = a.paras.iter().map(|p| p.style.quote).collect();
        assert_eq!(q, vec![1, 2, 0, 1, 1, 0]);
    }

    #[test]
    fn caret_line_and_modes() {
        let t = "- **a**\n- b";
        // visual: the caret line reveals its markers and loses the bullet
        let a = analyze(t, true, Some(0));
        assert!(has(&a, 0, fl::MARKER | fl::REVEAL) && has(&a, 2, fl::MARKER | fl::REVEAL));
        assert!(!a.paras[0].style.bullet && a.paras[1].style.bullet);
        assert!(!has(&a, at(t, "- b", 0), fl::REVEAL));
        // a caret line past the end is ignored
        let b = analyze(t, true, Some(100));
        assert!(!b.runs.iter().any(|r| r.flags & fl::REVEAL != 0));
        assert!(b.paras[0].style.bullet);
        // code mode: no paragraph styles, no reveal, but the same character flags
        let c = analyze(t, false, Some(0));
        assert!(c.paras.iter().all(|p| p.style == ParaStyle::default()));
        assert!(!c.runs.iter().any(|r| r.flags & fl::REVEAL != 0));
        assert!(has(&c, 2, fl::MARKER | fl::STRONG));
    }

    #[test]
    fn empty_and_surrogate_text() {
        let a = analyze("", true, Some(0));
        assert!(a.runs.is_empty());
        assert_eq!(a.paras, vec![ParaRun { start: 0, end: 1, style: ParaStyle::default() }]);
        let t = "😀 **b**\n# 日本";
        let a = analyze(t, true, None);
        check_runs(&a, t);
        assert!(has(&a, 5, fl::STRONG) && !has(&a, 5, fl::MARKER), "'b' after a surrogate pair");
        assert_eq!(a.paras[0], ParaRun { start: 0, end: 9, style: ParaStyle::default() });
        assert_eq!(a.paras[1].style.heading, 1);
        assert_eq!(a.paras[1].end, t.encode_utf16().count() as u32 + 1);
    }

    use crate::highlight::Kind;
}
