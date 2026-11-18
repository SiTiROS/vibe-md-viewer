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
    let name_start = i;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b':') {
        i += 1;
    }
    if i == name_start {
        return None;
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
            | "canvas" | "video" | "audio" | "head" | "title" | "frameset" | "frame" | "applet" | "textarea"
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
}
