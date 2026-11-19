//! A tiny tokenizer for HTML inside markdown. Executes nothing: only the formatting
//! meaning is taken from tags (<b>, <br>, <img src>, <a href>, align=center…).
//! <script>, <style>, <iframe> and the like are skipped together with their content.

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Open { name: String, attrs: Vec<(String, String)>, self_closing: bool },
    Close(String),
    Text(String),
    Comment,
}

/// Linear time: the position of the nearest `>` is remembered and not searched again for every `<`;
/// a tag is parsed only within the segment up to that `>`, and a failed segment is skipped entirely.
pub fn tokenize(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    let mut text_start = 0;
    let mut next_gt: Option<usize> = None;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        // Comment (an unclosed one swallows the rest — that is linear too).
        if s[i..].starts_with("<!--") {
            flush_text(s, text_start, i, &mut out);
            let end = s[i + 4..].find("-->").map(|e| i + 4 + e + 3).unwrap_or(b.len());
            out.push(Tok::Comment);
            i = end;
            text_start = i;
            continue;
        }
        if next_gt.map_or(true, |g| g < i) {
            next_gt = s[i..].find('>').map(|k| k + i);
            if next_gt.is_none() {
                break; // no more '>' ahead — there can be no more tags
            }
        }
        let g = next_gt.unwrap();
        if g - i > MAX_TAG_LEN {
            i += 1;
            continue;
        }
        match parse_tag(&s[i..=g]) {
            Some((tok, len)) => {
                flush_text(s, text_start, i, &mut out);
                out.push(tok);
                i += len;
                text_start = i;
            }
            None => {
                // "<" without a tag name is a cheap failure, move on; otherwise skip the segment.
                let name_start = b.get(i + 1).map(|c| c.is_ascii_alphabetic() || *c == b'/' || *c == b'!' || *c == b'?').unwrap_or(false);
                i = if name_start { g + 1 } else { i + 1 };
            }
        }
    }
    flush_text(s, text_start, b.len(), &mut out);
    out
}

fn flush_text(s: &str, a: usize, b: usize, out: &mut Vec<Tok>) {
    if b > a {
        out.push(Tok::Text(decode_entities(&s[a..b])));
    }
}

/// A tag longer than this is not a tag (otherwise `<a <a <a …` gives quadratic time).
const MAX_TAG_LEN: usize = 4096;
const MAX_ATTRS: usize = 64;

fn parse_tag(s: &str) -> Option<(Tok, usize)> {
    let mut lim = s.len().min(MAX_TAG_LEN);
    while !s.is_char_boundary(lim) {
        lim -= 1;
    }
    let s = &s[..lim];
    let b = s.as_bytes();
    let mut i = 1;
    let closing = b.get(i) == Some(&b'/');
    if closing {
        i += 1;
    }
    if b.get(i) == Some(&b'!') || b.get(i) == Some(&b'?') {
        // <!DOCTYPE ...>, <?xml ...?> — skip
        let end = s.find('>')?;
        return Some((Tok::Comment, end + 1));
    }
    // A tag name starts with a letter (as in HTML): `a <3 and b> 2` is text.
    if !b.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let name_start = i;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b':') {
        i += 1;
    }
    let name = s[name_start..i].to_ascii_lowercase();
    if closing {
        let end = s[i..].find('>')? + i;
        return Some((Tok::Close(name), end + 1));
    }
    let mut attrs = Vec::new();
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
        if b[i] == b'>' {
            return Some((Tok::Open { name, attrs, self_closing: false }, i + 1));
        }
        if b[i] == b'/' && b.get(i + 1) == Some(&b'>') {
            return Some((Tok::Open { name, attrs, self_closing: true }, i + 2));
        }
        let an_start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'>' && b[i] != b'/' {
            i += 1;
        }
        let an = s[an_start..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                let vs = i + 1;
                let ve = s[vs..].find(q as char)? + vs;
                val = decode_entities(&s[vs..ve]);
                i = ve + 1;
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
                val = decode_entities(&s[vs..i]);
            }
        } else if an.is_empty() {
            i += 1; // junk character — skip it
            continue;
        }
        if !an.is_empty() && attrs.len() < MAX_ATTRS {
            attrs.push((an, val));
        }
    }
}

pub fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// Tags whose content is not shown at all.
pub fn is_skipped_container(name: &str) -> bool {
    matches!(
        name,
        "script" | "style" | "iframe" | "object" | "embed" | "template" | "noscript" | "svg" | "math"
            | "canvas" | "video" | "audio" | "head" | "title" | "frameset" | "frame" | "applet" | "textarea" | "form"
    )
}

/// Hiding that survives a paragraph/HTML block boundary (content is not shown until
/// the closing tag). For the other "hidden" tags an unclosed tag must not hide
/// the whole rest of the document — hiding is reset at the block boundary.
pub fn skip_spans_blocks(name: &str) -> bool {
    matches!(name, "script" | "style" | "textarea")
}

pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        // Look for ';' only in the first 32 bytes: otherwise "&&&&…" gives quadratic time.
        let rb = rest.as_bytes();
        let end = rb[1..rb.len().min(34)].iter().position(|&c| c == b';').map(|e| e + 1);
        let decoded = end.filter(|&e| e <= 32).and_then(|e| {
            let ent = &rest[1..e];
            let c = if let Some(num) = ent.strip_prefix('#') {
                let v = if let Some(h) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                    u32::from_str_radix(h, 16).ok()
                } else {
                    num.parse::<u32>().ok()
                };
                v.and_then(char::from_u32).filter(|c| *c != '\0')
            } else {
                named_entity(ent)
            };
            c.map(|c| (c, e + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn named_entity(n: &str) -> Option<char> {
    Some(match n {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{00A0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "times" => '×',
        "divide" => '÷',
        "deg" => '°',
        "plusmn" => '±',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "harr" => '↔',
        "check" => '✓',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "sect" => '§',
        "para" => '¶',
        "shy" => '\u{00AD}',
        "zwj" => '\u{200D}',
        "zwnj" => '\u{200C}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{2003}',
        "thinsp" => '\u{2009}',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "bdquo" => '„',
        "hearts" => '♥',
        "star" => '☆',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_and_entities() {
        let t = tokenize(r#"<p align="center"><img src="a.png" width=200 /></p>&amp;x<!-- c --><br>"#);
        assert!(matches!(&t[0], Tok::Open { name, attrs, .. } if name == "p" && attr(attrs, "align") == Some("center")));
        assert!(matches!(&t[1], Tok::Open { name, attrs, self_closing: true } if name == "img" && attr(attrs, "width") == Some("200")));
        assert_eq!(t[2], Tok::Close("p".into()));
        assert_eq!(t[3], Tok::Text("&x".into()));
        assert_eq!(t[4], Tok::Comment);
        assert!(matches!(&t[5], Tok::Open { name, .. } if name == "br"));
    }

    #[test]
    fn broken_html_is_text() {
        let t = tokenize("a < b and <unclosed");
        assert_eq!(t, vec![Tok::Text("a < b and <unclosed".into())]);
    }

    fn open(name: &str, attrs: &[(&str, &str)], self_closing: bool) -> Tok {
        Tok::Open { name: name.into(), attrs: attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), self_closing }
    }

    fn text(s: &str) -> Tok {
        Tok::Text(s.into())
    }

    #[test]
    fn attribute_syntax_variants() {
        // Quotes of both kinds, unquoted values, bare attributes, spaces around '='.
        assert_eq!(
            tokenize("<a href = \"x y\" title='q \"z\"' data-x=z checked>"),
            vec![open("a", &[("href", "x y"), ("title", "q \"z\""), ("data-x", "z"), ("checked", "")], false)]
        );
        // Names are lower-cased, values are not; entities in values are decoded once.
        assert_eq!(
            tokenize("<IMG SRC=\"A.png\" Alt=\"&lt;b&gt; &amp;amp;\"/>"),
            vec![open("img", &[("src", "A.png"), ("alt", "<b> &amp;")], true)]
        );
        // Junk characters and a value without a name are skipped.
        assert_eq!(tokenize("<img / src=\"a.png\">"), vec![open("img", &[("src", "a.png")], false)]);
        assert_eq!(tokenize("<a =\"x\" href=\"h\">"), vec![open("a", &[("href", "h")], false)]);
        // Namespaced and hyphenated names.
        assert_eq!(tokenize("<my-el x:y=1>"), vec![open("my-el", &[("x:y", "1")], false)]);
        // Unicode in values and text.
        assert_eq!(tokenize("<p title=\"привет\">т</p>"), vec![open("p", &[("title", "привет")], false), text("т"), Tok::Close("p".into())]);
        // A closing tag ignores anything after its name.
        assert_eq!(tokenize("</B junk>"), vec![Tok::Close("b".into())]);
    }

    #[test]
    fn attribute_count_is_capped_and_first_wins() {
        let many: String = (0..100).map(|i| format!(" a{i}=\"{i}\"")).collect();
        match &tokenize(&format!("<x{many}>"))[0] {
            Tok::Open { attrs, .. } => {
                assert_eq!(attrs.len(), MAX_ATTRS);
                assert_eq!(attrs[MAX_ATTRS - 1], (format!("a{}", MAX_ATTRS - 1), format!("{}", MAX_ATTRS - 1)));
            }
            t => panic!("{t:?}"),
        }
        let t = tokenize("<a href=\"1\" HREF=\"2\">");
        match &t[0] {
            Tok::Open { attrs, .. } => {
                assert_eq!(attr(attrs, "href"), Some("1"));
                assert_eq!(attr(attrs, "missing"), None);
            }
            t => panic!("{t:?}"),
        }
    }

    #[test]
    fn comments_doctype_and_processing_instructions() {
        assert_eq!(tokenize("<!DOCTYPE html><?xml version=\"1.0\"?>x</!x>"), vec![Tok::Comment, Tok::Comment, text("x"), Tok::Comment]);
        assert_eq!(tokenize("a<!--x-->b"), vec![text("a"), Tok::Comment, text("b")]);
        // A comment may contain '>' and tags.
        assert_eq!(tokenize("<!-- <b>x</b> > -->y"), vec![Tok::Comment, text("y")]);
        // An unclosed comment swallows the rest.
        assert_eq!(tokenize("a<!-- <b>secret"), vec![text("a"), Tok::Comment]);
    }

    #[test]
    fn malformed_tags_stay_text() {
        for s in [
            "< b>",
            "</ >",
            "<>",
            "a > b",
            "x<",
            "<a href=\"x>t",
            "<1",
            "a <= b",
        ] {
            assert_eq!(tokenize(s), vec![text(s)], "{s:?}");
        }
        // After a broken tag, later tags still parse.
        assert_eq!(tokenize("<a title=\"x>y <b>z"), vec![text("<a title=\"x>y "), open("b", &[], false), text("z")]);
        assert_eq!(tokenize("<b>x"), vec![open("b", &[], false), text("x")]);
        // A tag ends at the first '>' outside quotes only within its own segment.
        assert_eq!(tokenize("<a title='a>b'>"), vec![text("<a title='a>b'>")]);
    }

    #[test]
    fn tag_length_limit() {
        // A segment up to MAX_TAG_LEN bytes is a tag.
        let ok = format!("<a{}>", " x".repeat((MAX_TAG_LEN - 3) / 2));
        assert!(ok.len() <= MAX_TAG_LEN);
        assert!(matches!(&tokenize(&ok)[..], [Tok::Open { name, .. }] if name == "a"));
        // MAX_TAG_LEN + 1 bytes ('>' cut off by the limit) and anything longer is text.
        let edge = format!("<a{}>", " x".repeat((MAX_TAG_LEN - 2) / 2));
        assert_eq!(edge.len(), MAX_TAG_LEN + 1);
        assert_eq!(tokenize(&edge), vec![text(&edge)]);
        let long = format!("<a title=\"{}\">t", "x".repeat(5000));
        assert_eq!(tokenize(&long), vec![text(&long)]);
        // Many unterminated tags stay linear and become text.
        let bomb = "<a ".repeat(50_000);
        let t = std::time::Instant::now();
        assert_eq!(tokenize(&bomb), vec![text(&bomb)]);
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn hidden_containers_and_block_spanning() {
        for n in [
            "script", "style", "iframe", "object", "embed", "template", "noscript", "svg", "math", "canvas", "video", "audio", "head", "title",
            "frameset", "frame", "applet", "textarea", "form",
        ] {
            assert!(is_skipped_container(n), "{n}");
        }
        for n in ["b", "a", "img", "div", "p", "span", "details", "summary", ""] {
            assert!(!is_skipped_container(n), "{n}");
        }
        for n in ["script", "style", "textarea"] {
            assert!(skip_spans_blocks(n), "{n}");
        }
        for n in ["iframe", "template", "svg", "object", "form", "b"] {
            assert!(!skip_spans_blocks(n), "{n}");
        }
    }

    #[test]
    fn tag_names_start_with_a_letter() {
        for s in ["a <3 and b> 2", "x </3> y", "<1x>", "<-a>", "<:a>"] {
            assert_eq!(tokenize(s), vec![text(s)], "{s:?}");
        }
        // Digits, '-' and ':' are fine after the first letter.
        assert_eq!(tokenize("<h1>"), vec![open("h1", &[], false)]);
        assert_eq!(tokenize("</x-1>"), vec![Tok::Close("x-1".into())]);
    }

    /// The tokenizer works on a whole HTML block: comments and tags may span lines.
    #[test]
    fn multi_line_comments_and_tags() {
        assert_eq!(tokenize("<!--\nsecret\n-->\nx"), vec![Tok::Comment, text("\nx")]);
        assert_eq!(tokenize("<img\n  src=\"a.png\"\n  alt=\"b\">"), vec![open("img", &[("src", "a.png"), ("alt", "b")], false)]);
    }

    #[test]
    fn numeric_entities() {
        for (s, want) in [
            ("&#65;", "A"),
            ("&#x41;", "A"),
            ("&#X41;", "A"),
            ("&#x1F600;", "😀"),
            ("&#1078;", "ж"),
            ("&#xE000;", "\u{E000}"),
            // Invalid numbers stay as written: NUL, surrogates, out of range, garbage, empty.
            ("&#0;", "&#0;"),
            ("&#x0;", "&#x0;"),
            ("&#xD800;", "&#xD800;"),
            ("&#1114112;", "&#1114112;"),
            ("&#x110000;", "&#x110000;"),
            ("&#xZZ;", "&#xZZ;"),
            ("&#;", "&#;"),
            ("&#x;", "&#x;"),
            ("&#-1;", "&#-1;"),
            ("&#99999999999999999999;", "&#99999999999999999999;"),
        ] {
            assert_eq!(decode_entities(s), want, "{s}");
        }
    }

    #[test]
    fn named_entities() {
        let all = [
            ("amp", '&'), ("lt", '<'), ("gt", '>'), ("quot", '"'), ("apos", '\''), ("nbsp", '\u{A0}'), ("copy", '©'), ("reg", '®'),
            ("trade", '™'), ("mdash", '—'), ("ndash", '–'), ("hellip", '…'), ("laquo", '«'), ("raquo", '»'), ("bull", '•'),
            ("middot", '·'), ("times", '×'), ("divide", '÷'), ("deg", '°'), ("plusmn", '±'), ("larr", '←'), ("rarr", '→'),
            ("uarr", '↑'), ("darr", '↓'), ("harr", '↔'), ("check", '✓'), ("euro", '€'), ("pound", '£'), ("yen", '¥'), ("sect", '§'),
            ("para", '¶'), ("shy", '\u{AD}'), ("zwj", '\u{200D}'), ("zwnj", '\u{200C}'), ("ensp", '\u{2002}'), ("emsp", '\u{2003}'),
            ("thinsp", '\u{2009}'), ("lsquo", '‘'), ("rsquo", '’'), ("ldquo", '“'), ("rdquo", '”'), ("bdquo", '„'), ("hearts", '♥'),
            ("star", '☆'),
        ];
        for (name, c) in all {
            assert_eq!(decode_entities(&format!("x&{name};y")), format!("x{c}y"), "&{name};");
            // Names are case-sensitive.
            if name.to_uppercase() != name {
                let up = format!("&{};", name.to_uppercase());
                assert_eq!(decode_entities(&up), up);
            }
        }
    }

    #[test]
    fn entity_edge_cases() {
        // Without '&' the input is returned as is.
        assert_eq!(decode_entities("plain <text>"), "plain <text>");
        // No ';', unknown names, stray ampersands.
        assert_eq!(decode_entities("&amp"), "&amp");
        assert_eq!(decode_entities("&unknown; & &;"), "&unknown; & &;");
        assert_eq!(decode_entities("&&amp;&"), "&&&");
        assert_eq!(decode_entities("a&lt;&gt;&quot;&apos;b"), "a<>\"'b");
        // Decoded once: no double decoding into markup.
        assert_eq!(decode_entities("&amp;lt;script&amp;gt;"), "&lt;script&gt;");
        // ';' is looked for only in the first 32 bytes.
        let far = format!("&{};", "a".repeat(40));
        assert_eq!(decode_entities(&far), far);
        let near = format!("&#{}65;", "0".repeat(28));
        assert_eq!(near.find(';'), Some(32));
        assert_eq!(decode_entities(&near), "A");
        let too_far = format!("&#{}65;", "0".repeat(29));
        assert_eq!(decode_entities(&too_far), too_far);
        // Multibyte text around entities.
        assert_eq!(decode_entities("ж&amp;ж&#x416;"), "ж&жЖ");
        // Text tokens are decoded too.
        assert_eq!(tokenize("a &lt; b"), vec![text("a < b")]);
        // A million ampersands stay linear.
        let amps = "&".repeat(1_000_000);
        let t = std::time::Instant::now();
        assert_eq!(decode_entities(&amps), amps);
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
    }
}
