//! Lightweight syntax highlighting for code blocks: no regular expressions, a single pass,
//! linear time (an unclosed string/comment simply runs to the end of the line/block).
//! Used in the viewer (render.rs) and in the editor (styler.rs).

use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Kind {
    Plain,
    Keyword,
    Type,
    Str,
    Num,
    Comment,
    Func,
    Attr,
}

impl Kind {
    pub fn code(self) -> u32 {
        self as u32
    }
    pub fn from_code(c: u32) -> Kind {
        match c {
            1 => Kind::Keyword,
            2 => Kind::Type,
            3 => Kind::Str,
            4 => Kind::Num,
            5 => Kind::Comment,
            6 => Kind::Func,
            7 => Kind::Attr,
            _ => Kind::Plain,
        }
    }
}

/// Above this, no highlighting (huge blocks are shown as is).
pub const MAX_HIGHLIGHT: usize = 256 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    CLike,
    Python,
    Shell,
    PowerShell,
    Sql,
    Markup,
    Css,
    Config,
    Json,
    Lua,
    Ruby,
    Diff,
    Plain,
}

struct Lang {
    family: Family,
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    /// Treat PascalCase identifiers as types
    pascal_types: bool,
    /// `'…'` is a string (not a char/lifetime)
    single_quote_str: bool,
    backtick_str: bool,
    case_insensitive: bool,
}

const RUST_KW: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in",
    "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true",
    "type", "unsafe", "use", "where", "while", "yield",
];
const RUST_TY: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool", "char", "str", "String",
    "Vec", "Option", "Result", "Box", "Rc", "Arc", "Some", "None", "Ok", "Err",
];
const C_KW: &[&str] = &[
    "auto", "break", "case", "catch", "class", "const", "constexpr", "continue", "default", "delete", "do", "else", "enum", "explicit",
    "extern", "false", "for", "friend", "goto", "if", "inline", "namespace", "new", "noexcept", "nullptr", "operator", "override",
    "private", "protected", "public", "return", "sizeof", "static", "struct", "switch", "template", "this", "throw", "true", "try",
    "typedef", "typename", "union", "using", "virtual", "volatile", "while", "NULL",
];
const C_TY: &[&str] = &[
    "int", "char", "short", "long", "float", "double", "void", "bool", "unsigned", "signed", "size_t", "uint8_t", "uint16_t", "uint32_t",
    "uint64_t", "int8_t", "int16_t", "int32_t", "int64_t", "std", "string", "vector", "map", "auto",
];
const CS_KW: &[&str] = &[
    "abstract", "as", "async", "await", "base", "break", "case", "catch", "class", "const", "continue", "default", "delegate", "do",
    "else", "enum", "event", "explicit", "extern", "false", "finally", "fixed", "for", "foreach", "get", "if", "implicit", "in",
    "interface", "internal", "is", "lock", "namespace", "new", "null", "operator", "out", "override", "params", "private", "protected",
    "public", "readonly", "record", "ref", "return", "sealed", "set", "sizeof", "static", "struct", "switch", "this", "throw", "true",
    "try", "typeof", "using", "var", "virtual", "void", "volatile", "while", "yield", "extends", "implements", "import", "package",
    "super", "throws", "final", "synchronized", "fun", "val", "when", "object", "data", "companion", "init",
];
const CS_TY: &[&str] = &["int", "long", "short", "byte", "bool", "boolean", "char", "float", "double", "decimal", "string", "object", "void"];
const JS_KW: &[&str] = &[
    "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export",
    "extends", "false", "finally", "for", "from", "function", "if", "import", "in", "instanceof", "let", "new", "null", "of", "return",
    "static", "super", "switch", "this", "throw", "true", "try", "typeof", "undefined", "var", "void", "while", "yield", "as",
    "interface", "type", "enum", "implements", "private", "public", "protected", "readonly", "declare", "namespace", "keyof",
];
const JS_TY: &[&str] = &["string", "number", "boolean", "any", "unknown", "never", "object", "void", "Promise", "Array", "Map", "Set"];
const GO_KW: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto", "if", "import",
    "interface", "map", "package", "range", "return", "select", "struct", "switch", "type", "var", "true", "false", "nil",
];
const GO_TY: &[&str] = &["int", "int64", "int32", "uint", "uint64", "byte", "rune", "string", "bool", "float64", "float32", "error", "any"];
const PY_KW: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
    "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
    "return", "try", "while", "with", "yield", "match", "case", "self",
];
const PY_TY: &[&str] = &["int", "str", "float", "bool", "list", "dict", "set", "tuple", "bytes", "object", "type"];
const SH_KW: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function", "return", "exit",
    "export", "local", "readonly", "echo", "cd", "sudo", "set", "unset", "source", "true", "false",
];
const PS_KW: &[&str] = &[
    "if", "else", "elseif", "foreach", "for", "while", "do", "until", "switch", "function", "param", "return", "try", "catch",
    "finally", "throw", "begin", "process", "end", "in", "true", "false", "null",
];
const SQL_KW: &[&str] = &[
    "select", "from", "where", "and", "or", "not", "insert", "into", "values", "update", "set", "delete", "create", "table", "drop",
    "alter", "add", "index", "primary", "key", "foreign", "references", "join", "left", "right", "inner", "outer", "on", "group", "by",
    "order", "having", "limit", "offset", "as", "distinct", "union", "all", "null", "is", "in", "like", "between", "case", "when", "then",
    "else", "end", "exists", "with", "default", "unique", "view", "returning", "asc", "desc", "true", "false",
];
const SQL_TY: &[&str] = &["int", "integer", "bigint", "text", "varchar", "char", "boolean", "date", "timestamp", "numeric", "decimal", "real", "serial", "uuid", "json", "jsonb"];
const LUA_KW: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil", "not", "or", "repeat",
    "return", "then", "true", "until", "while",
];
const RUBY_KW: &[&str] = &[
    "alias", "and", "begin", "break", "case", "class", "def", "do", "else", "elsif", "end", "ensure", "false", "for", "if", "in", "module",
    "next", "nil", "not", "or", "redo", "rescue", "retry", "return", "self", "super", "then", "true", "unless", "until", "when", "while",
    "yield", "require",
];
const PHP_KW: &[&str] = &[
    "abstract", "array", "as", "break", "case", "catch", "class", "const", "continue", "default", "do", "echo", "else", "elseif",
    "extends", "false", "final", "finally", "fn", "for", "foreach", "function", "if", "implements", "interface", "namespace", "new", "null",
    "private", "protected", "public", "require", "return", "static", "switch", "throw", "trait", "true", "try", "use", "while",
];
const JSON_KW: &[&str] = &["true", "false", "null"];
const CONFIG_KW: &[&str] = &["true", "false", "null", "yes", "no", "on", "off"];

fn lang_for(info: &str) -> Lang {
    let l = info.trim().to_ascii_lowercase();
    let l = l.split(|c: char| c.is_whitespace() || c == ',' || c == '{').next().unwrap_or("");
    let mk = |family, keywords, types, pascal_types, single_quote_str, backtick_str| Lang {
        family,
        keywords,
        types,
        pascal_types,
        single_quote_str,
        backtick_str,
        case_insensitive: false,
    };
    match l {
        "rust" | "rs" => mk(Family::CLike, RUST_KW, RUST_TY, true, false, false),
        "c" | "h" | "cpp" | "c++" | "cc" | "cxx" | "hpp" | "hxx" | "objc" | "cuda" | "glsl" | "hlsl" => mk(Family::CLike, C_KW, C_TY, false, false, false),
        "cs" | "csharp" | "c#" | "java" | "kotlin" | "kt" | "scala" | "swift" | "dart" => mk(Family::CLike, CS_KW, CS_TY, true, false, false),
        "js" | "javascript" | "jsx" | "mjs" | "cjs" | "ts" | "typescript" | "tsx" | "vue" | "svelte" => mk(Family::CLike, JS_KW, JS_TY, true, true, true),
        "go" | "golang" => mk(Family::CLike, GO_KW, GO_TY, true, false, true),
        "py" | "python" | "python3" | "py3" | "gdscript" => mk(Family::Python, PY_KW, PY_TY, true, true, false),
        "sh" | "bash" | "zsh" | "shell" | "console" | "shellsession" | "fish" | "dockerfile" | "makefile" | "make" => mk(Family::Shell, SH_KW, &[], false, true, true),
        "ps1" | "powershell" | "pwsh" | "ps" | "bat" | "cmd" | "batch" => mk(Family::PowerShell, PS_KW, &[], false, true, false),
        "sql" | "mysql" | "postgres" | "postgresql" | "sqlite" | "plsql" | "tsql" => Lang { case_insensitive: true, ..mk(Family::Sql, SQL_KW, SQL_TY, false, true, false) },
        "html" | "htm" | "xml" | "svg" | "xaml" | "xhtml" | "plist" | "csproj" => mk(Family::Markup, &[], &[], false, true, false),
        "css" | "scss" | "less" | "sass" => mk(Family::Css, &[], &[], false, true, false),
        "yaml" | "yml" | "toml" | "ini" | "cfg" | "conf" | "properties" | "env" | "editorconfig" | "gitignore" => mk(Family::Config, CONFIG_KW, &[], false, true, false),
        "json" | "jsonc" | "json5" | "geojson" => mk(Family::Json, JSON_KW, &[], false, false, false),
        "lua" | "luau" => mk(Family::Lua, LUA_KW, &[], false, true, false),
        "rb" | "ruby" | "rake" => mk(Family::Ruby, RUBY_KW, &[], true, true, false),
        "php" => mk(Family::CLike, PHP_KW, &[], true, true, false),
        "diff" | "patch" => mk(Family::Diff, &[], &[], false, false, false),
        "" | "text" | "txt" | "plain" | "plaintext" | "md" | "markdown" | "output" | "log" => mk(Family::Plain, &[], &[], false, false, false),
        // Unknown language — generic C-like highlighting (strings, numbers, comments).
        _ => mk(Family::CLike, &[], &[], false, true, false),
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Split code into spans (byte ranges in `code`) with highlight kinds.
/// The spans are contiguous and cover the whole text.
pub fn spans(info: &str, code: &str) -> Vec<(Kind, Range<usize>)> {
    let mut out: Vec<(Kind, Range<usize>)> = Vec::new();
    if code.len() > MAX_HIGHLIGHT {
        return vec![(Kind::Plain, 0..code.len())];
    }
    let lang = lang_for(info);
    let b = code.as_bytes();
    let n = b.len();
    let mut push = |k: Kind, r: Range<usize>| {
        if r.start >= r.end {
            return;
        }
        if let Some(last) = out.last_mut() {
            if last.0 == k && last.1.end == r.start {
                last.1.end = r.end;
                return;
            }
        }
        out.push((k, r));
    };
    let line_end = |i: usize| -> usize { code[i..].find('\n').map(|k| i + k).unwrap_or(n) };
    let starts = |i: usize, p: &str| code.as_bytes()[i..].starts_with(p.as_bytes());

    if lang.family == Family::Plain {
        push(Kind::Plain, 0..n);
        return out;
    }
    if lang.family == Family::Diff {
        let mut i = 0;
        while i < n {
            let e = (line_end(i) + 1).min(n);
            let k = match b[i] {
                b'+' => Kind::Str,
                b'-' => Kind::Keyword,
                b'@' => Kind::Attr,
                _ => Kind::Plain,
            };
            push(k, i..e);
            i = e;
        }
        return out;
    }

    let mut i = 0;
    let mut line_start = true;
    while i < n {
        let c = code[i..].chars().next().unwrap();
        let cl = c.len_utf8();
        // ---- comments ----
        let line_comment = match lang.family {
            Family::CLike | Family::Css | Family::Json => starts(i, "//"),
            Family::Python | Family::Shell | Family::PowerShell | Family::Ruby => c == '#',
            Family::Config => (c == '#' || c == ';') && line_start_or_space(code, i),
            Family::Sql | Family::Lua => starts(i, "--"),
            _ => false,
        };
        if line_comment && !(lang.family == Family::Lua && starts(i, "--[[")) {
            let e = line_end(i);
            push(Kind::Comment, i..e);
            i = e;
            continue;
        }
        let block: Option<(&str, &str)> = match lang.family {
            Family::CLike | Family::Css | Family::Json | Family::Sql => Some(("/*", "*/")),
            Family::Markup => Some(("<!--", "-->")),
            Family::PowerShell => Some(("<#", "#>")),
            Family::Lua => Some(("--[[", "]]")),
            _ => None,
        };
        if let Some((open, close)) = block {
            if starts(i, open) {
                let e = code[i + open.len()..].find(close).map(|k| i + open.len() + k + close.len()).unwrap_or(n);
                push(Kind::Comment, i..e);
                i = e;
                continue;
            }
        }
        // ---- strings ----
        if lang.family == Family::Python && (starts(i, "\"\"\"") || starts(i, "'''")) {
            let q = &code[i..i + 3];
            let e = code[i + 3..].find(q).map(|k| i + 3 + k + 3).unwrap_or(n);
            push(Kind::Str, i..e);
            i = e;
            continue;
        }
        let is_quote = c == '"' || (c == '\'' && lang.single_quote_str) || (c == '`' && lang.backtick_str);
        if is_quote {
            let multiline = c == '`';
            let mut j = i + 1;
            while j < n {
                let ch = b[j];
                if ch == b'\\' && lang.family != Family::PowerShell {
                    j += 2;
                    continue;
                }
                if ch == c as u8 {
                    j += 1;
                    break;
                }
                if ch == b'\n' && !multiline {
                    break;
                }
                j += 1;
            }
            let j = j.min(n);
            // In JSON a string before ":" is a key.
            let k = if lang.family == Family::Json && code[j..].trim_start().starts_with(':') { Kind::Attr } else { Kind::Str };
            push(k, i..j);
            i = j;
            line_start = false;
            continue;
        }
        // ---- Markup: tags and attributes ----
        if lang.family == Family::Markup {
            if c == '<' {
                let mut j = i + 1;
                if j < n && (b[j] == b'/' || b[j] == b'!' || b[j] == b'?') {
                    j += 1;
                }
                let s = j;
                while j < n && (b[j].is_ascii_alphanumeric() || b[j] == b'-' || b[j] == b':' || b[j] == b'_' || b[j] == b'.') {
                    j += 1;
                }
                if j > s {
                    push(Kind::Plain, i..s);
                    push(Kind::Keyword, s..j);
                    i = j;
                    continue;
                }
            }
            if is_ident_start(c) {
                let mut j = i + cl;
                while j < n {
                    let ch = code[j..].chars().next().unwrap();
                    if is_ident(ch) || ch == '-' || ch == ':' {
                        j += ch.len_utf8();
                    } else {
                        break;
                    }
                }
                let k = if code[j..].starts_with('=') { Kind::Attr } else { Kind::Plain };
                push(k, i..j);
                i = j;
                continue;
            }
            push(Kind::Plain, i..i + cl);
            i += cl;
            continue;
        }
        // ---- numbers ----
        if c.is_ascii_digit() && !(i > 0 && is_ident(code[..i].chars().next_back().unwrap_or(' '))) {
            let mut j = i + 1;
            while j < n && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || (b[j] == b'.' && j + 1 < n && b[j + 1].is_ascii_digit())) {
                j += 1;
            }
            push(Kind::Num, i..j);
            i = j;
            line_start = false;
            continue;
        }
        // ---- attributes / directives / variables ----
        if lang.family == Family::CLike && c == '#' {
            // #[derive(...)] in Rust, #include in C. The closing "]" is searched only on this line:
            // otherwise thousands of unclosed "#[" give quadratic time (each searches to the end of the block).
            let le = line_end(i);
            let e = if starts(i, "#[") || starts(i, "#![") { code[i..le].find(']').map(|k| i + k + 1).unwrap_or(le) } else { le };
            push(Kind::Attr, i..e);
            i = e;
            continue;
        }
        if c == '@' && matches!(lang.family, Family::CLike | Family::Python | Family::Css) {
            let mut j = i + 1;
            while j < n && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'.' || b[j] == b'-') {
                j += 1;
            }
            push(Kind::Attr, i..j);
            i = j;
            continue;
        }
        if c == '$' && matches!(lang.family, Family::Shell | Family::PowerShell) || (c == '$' && lang.keywords == PHP_KW) {
            let mut j = i + 1;
            if j < n && b[j] == b'{' {
                let le = line_end(i).max(j);
                j = code[j..le].find('}').map(|k| j + k + 1).unwrap_or(le);
            } else {
                while j < n && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b':') {
                    j += 1;
                }
            }
            push(Kind::Attr, i..j);
            i = j;
            continue;
        }
        // ---- identifiers ----
        if is_ident_start(c) || (c == '-' && lang.family == Family::PowerShell && i + 1 < n && b[i + 1].is_ascii_alphabetic()) {
            let mut j = i + cl;
            while j < n {
                let ch = code[j..].chars().next().unwrap();
                let ok = is_ident(ch) || (ch == '-' && matches!(lang.family, Family::Css | Family::PowerShell));
                if !ok {
                    break;
                }
                j += ch.len_utf8();
            }
            let word = &code[i..j];
            let lower;
            let w = if lang.case_insensitive {
                lower = word.to_ascii_lowercase();
                lower.as_str()
            } else {
                word
            };
            let next = code[j..].trim_start_matches([' ', '\t']).chars().next().unwrap_or(' ');
            let kind = if lang.keywords.contains(&w) {
                Kind::Keyword
            } else if lang.types.contains(&w) {
                Kind::Type
            } else if lang.family == Family::Css && next == ':' {
                Kind::Attr
            } else if matches!(lang.family, Family::Config) && (next == '=' || next == ':') && line_start {
                Kind::Attr
            } else if next == '(' || (next == '!' && lang.keywords == RUST_KW) {
                Kind::Func
            } else if lang.pascal_types && word.chars().next().map(|ch| ch.is_uppercase()).unwrap_or(false) && word.chars().any(|ch| ch.is_lowercase()) {
                Kind::Type
            } else {
                Kind::Plain
            };
            // Rust macro: highlight the "!" too
            let j2 = if kind == Kind::Func && next == '!' && code[j..].starts_with('!') { j + 1 } else { j };
            push(kind, i..j2);
            i = j2;
            line_start = false;
            continue;
        }
        if c == '\n' {
            line_start = true;
        } else if !c.is_whitespace() {
            line_start = false;
        }
        push(Kind::Plain, i..i + cl);
        i += cl;
    }
    out
}

fn line_start_or_space(code: &str, i: usize) -> bool {
    i == 0 || matches!(code[..i].chars().next_back(), Some('\n') | Some(' ') | Some('\t'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds<'a>(lang: &str, code: &'a str) -> Vec<(Kind, &'a str)> {
        spans(lang, code).into_iter().filter(|(k, _)| *k != Kind::Plain).map(|(k, r)| (k, &code[r])).collect()
    }

    #[test]
    fn rust() {
        let k = kinds("rust", "fn main() { let x = 42; println!(\"hi\"); } // c");
        assert!(k.contains(&(Kind::Keyword, "fn")));
        assert!(k.contains(&(Kind::Func, "main")));
        assert!(k.contains(&(Kind::Num, "42")));
        assert!(k.contains(&(Kind::Func, "println!")));
        assert!(k.contains(&(Kind::Str, "\"hi\"")));
        assert!(k.contains(&(Kind::Comment, "// c")));
    }

    #[test]
    fn python_and_json() {
        let k = kinds("python", "def f(x):\n    # комментарий\n    return 'да'");
        assert!(k.contains(&(Kind::Keyword, "def")));
        assert!(k.contains(&(Kind::Comment, "# комментарий")));
        assert!(k.contains(&(Kind::Str, "'да'")));
        let j = kinds("json", r#"{"ключ": [1, true, "s"]}"#);
        assert!(j.contains(&(Kind::Attr, "\"ключ\"")));
        assert!(j.contains(&(Kind::Keyword, "true")));
        assert!(j.contains(&(Kind::Str, "\"s\"")));
    }

    #[test]
    fn covers_everything_and_is_linear() {
        for (lang, code) in [
            ("rust", "\"незакрытая строка\n".repeat(10_000)),
            ("c", "/*".repeat(50_000)),
            ("python", "'''".repeat(30_000)),
            ("html", "<a ".repeat(50_000)),
            ("sql", "SELECT ".repeat(30_000)),
            ("diff", "+a\n-b\n".repeat(20_000)),
            // unclosed #[ and ${ — the closing bracket is searched only on its own line
            ("rust", "#[\n".repeat(85_000)),
            ("bash", "${\n".repeat(85_000)),
        ] {
            let t = std::time::Instant::now();
            let s = spans(lang, &code);
            assert!(t.elapsed().as_secs() < 2, "{lang}");
            let mut pos = 0;
            for (_, r) in &s {
                assert_eq!(r.start, pos, "{lang}: gap");
                pos = r.end;
            }
            assert_eq!(pos, code.len(), "{lang}: not all text is covered");
        }
    }
}
