//! Link and path policy — the main line of defense against "injections" via .md.
//!
//! The principle is a whitelist. Only the following is allowed:
//!   * anchors within the document (#section);
//!   * http/https (opened in the default browser);
//!   * mailto;
//!   * local .md files (opened in this same window);
//!   * other local files — only after confirmation, and never
//!     executable/script types.
//! Everything else is blocked: javascript:, vbscript:, data:, ms-msdt:, search-ms:,
//! ms-officecmd:, any unknown protocols (CVE-2026-20841 in Windows Notepad —
//! exactly via protocols in markdown links), network paths \\server\... (leak of the
//! Windows password NTLM hash), hidden control characters (bidi text spoofing).

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkAction {
    /// Anchor within the current document (without '#', already decoded).
    Anchor(String),
    /// Another markdown file — open in this window.
    OpenMarkdown { path: PathBuf, anchor: Option<String> },
    /// Validated http(s) address. `warn` — a reason to ask the user again.
    Web { url: String, host: String, warn: Option<String> },
    Mail(String),
    /// Local non-markdown file or folder — only with confirmation.
    LocalFile(PathBuf),
    /// Internal command (vmd:...) — executed only on the built-in start page.
    Internal(String),
    Blocked(String),
}

/// Characters that must not appear in addresses: control characters and bidi overrides
/// (Trojan Source, CVE-2021-42574) — they are used to disguise the real address.
pub fn has_hidden_controls(s: &str) -> bool {
    s.chars().any(|c| {
        let u = c as u32;
        u < 0x20
            || u == 0x7F
            || (0x80..0xA0).contains(&u)
            || (0x202A..=0x202E).contains(&u)
            || (0x2066..=0x2069).contains(&u)
            || u == 0x200E
            || u == 0x200F
            || u == 0x061C
            || u == 0xFEFF
            // invisible: soft hyphen, zero-width, line/paragraph separators,
            // joiners, tags, variation selectors (finding of the 0.2.0 audit)
            || u == 0x00AD
            || u == 0x180E
            || (0x200B..=0x200D).contains(&u)
            || u == 0x2028
            || u == 0x2029
            || (0x2060..=0x2064).contains(&u)
            || (0xFE00..=0xFE0F).contains(&u)
            || (0xE0000..=0xE007F).contains(&u)
    })
}

const DANGEROUS_EXT: &[&str] = &[
    "exe", "com", "bat", "cmd", "scr", "pif", "cpl", "msi", "msp", "mst", "msc", "lnk", "url",
    "website", "ps1", "ps1xml", "psm1", "psd1", "psc1", "vbs", "vbe", "js", "jse", "mjs", "wsf",
    "wsh", "wsc", "hta", "jar", "reg", "inf", "scf", "chm", "hlp", "dll", "sys", "ocx", "drv",
    "appref-ms", "application", "gadget", "settingcontent-ms", "library-ms", "search-ms",
    "searchconnector-ms", "diagcab", "msix", "msixbundle", "appx", "appxbundle", "appinstaller",
    "iso", "img", "vhd", "vhdx", "xll", "xlam", "docm", "dotm", "xlsm", "xltm", "pptm", "potm",
    "ppam", "sldm", "one", "py", "pyw", "pyc", "pyz", "rb", "pl", "sh", "bash", "vb", "theme",
    "themepack", "deskthemepack", "rdp", "ica", "mht", "mhtml", "xbap", "cab", "msu", "job",
    "vsix", "vsto", "ws", "shb", "shs", "mof", "pif", "crt", "cer", "der", "p7b", "pfx",
    // Access (macros), scriptlets, IE settings, PowerShell shells, Java Web Start,
    // Windows Sandbox, Excel web queries/SYLK (DDE), other Office macro formats
    "ade", "adp", "accda", "accdb", "accde", "accdr", "accdt", "mdb", "mde", "mda", "mdt", "mdw",
    "mdz", "sct", "ins", "isp", "ps2", "psc2", "msh", "msh1", "msh2", "mshxml", "msh1xml",
    "msh2xml", "jnlp", "wsb", "iqy", "dqy", "rqy", "slk", "xla", "xlm", "xlsb", "xltm",
    "appcontent-ms", "printerexport", "msrcincident", "grp", "bas", "mst", "ocx", "plg", "prf",
    "prg", "pst", "scf", "vbp", "vhdx", "wiz", "xnk", "hpj", "csh", "ksh", "mcf", "nsh", "ops",
];

pub fn is_dangerous_ext(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(e) => DANGEROUS_EXT.contains(&e.to_ascii_lowercase().as_str()),
        None => false,
    }
}

/// Whitelist of types that may be opened with the default program from a document link
/// (after confirmation). Everything else — only "Show in Explorer". A blacklist is
/// not enough: 8.3 short names (`X~1.LIB` → `x.library-ms`) and Unicode case bypass it.
const OPENABLE_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "tif", "tiff", "heic", "avif",
    "pdf", "txt", "log", "csv", "tsv", "json", "yaml", "yml", "toml",
    "docx", "xlsx", "pptx", "odt", "ods", "odp",
    "mp3", "wav", "flac", "ogg", "m4a", "mp4", "mkv", "webm", "avi", "mov",
];

pub fn is_openable_ext(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(e) if e.is_ascii() => OPENABLE_EXT.contains(&e.to_ascii_lowercase().as_str()),
        _ => false,
    }
}

const MD_EXT: &[&str] = &["md", "markdown", "mdown", "mkd", "mkdn", "mdwn", "mdtxt", "mdtext"];

pub fn is_md_ext(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(e) => MD_EXT.contains(&e.to_ascii_lowercase().as_str()),
        None => false,
    }
}

/// Decoding %XX → bytes → UTF-8 (replacing broken sequences).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = (b[i + 1] as char).to_digit(16);
            let l = (b[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The URI scheme (lowercased), if any. A single letter is a drive (C:), not a scheme.
pub fn scheme_of(s: &str) -> Option<String> {
    let colon = s.find(':')?;
    let scheme = &s[..colon];
    if scheme.len() < 2 {
        return None;
    }
    let mut chars = scheme.chars();
    if !chars.next()?.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') {
        return None;
    }
    Some(scheme.to_ascii_lowercase())
}

pub fn classify(raw: &str, base_dir: Option<&Path>) -> LinkAction {
    let s = raw.trim();
    if s.is_empty() {
        return LinkAction::Blocked(tr!("Empty link", "Пустая ссылка").into());
    }
    if has_hidden_controls(s) {
        return LinkAction::Blocked(
            tr!(
                "The link contains hidden control characters — looks like a spoofed address",
                "Ссылка содержит скрытые управляющие символы — похоже на подмену адреса"
            )
            .into(),
        );
    }
    if let Some(frag) = s.strip_prefix('#') {
        return LinkAction::Anchor(percent_decode(frag));
    }
    if let Some(scheme) = scheme_of(s) {
        return match scheme.as_str() {
            "http" | "https" => validate_web(s),
            "mailto" => {
                if s.contains(['"', '<', '>', '\\', '`']) {
                    LinkAction::Blocked(tr!("Invalid email address", "Некорректный адрес почты").into())
                } else {
                    LinkAction::Mail(sanitize_mailto(s))
                }
            }
            "file" => {
                // file:///C:/x, file:/C:/x, file://localhost/C:/x — local;
                // file://server/share and file:////server — network (UNC) → blocked.
                let after = &s[5..];
                let rest = if let Some(r) = after.strip_prefix("//") {
                    let (auth, path) = match r.find('/') {
                        Some(i) => (&r[..i], &r[i..]),
                        None => (r, ""),
                    };
                    if !auth.is_empty() && !auth.eq_ignore_ascii_case("localhost") {
                        return LinkAction::Blocked(unc_reason());
                    }
                    path
                } else {
                    after
                };
                let rest = rest.trim_start_matches('/');
                if !looks_like_drive(rest) {
                    return LinkAction::Blocked(unc_reason());
                }
                local_path(rest, None)
            }
            "vmd" => LinkAction::Internal(s[4..].to_string()),
            other => LinkAction::Blocked(trf!(
                "The “{other}:” protocol is not allowed. Only http, https and mailto links can be opened from documents.",
                "Протокол «{other}:» запрещён. Из документов открываются только http, https и mailto."
            )),
        };
    }
    local_path(s, base_dir)
}

/// mailto: keep only the address and safe parameters (subject, body, cc).
/// Some mail programs treated parameters like `attach=` as "attach a file".
fn sanitize_mailto(s: &str) -> String {
    let s = &encode_url_chars(s);
    // Some programs read "&" in an address without "?" as the start of parameters
    // (mailto:a@b.c&attach=C:/x) — in the address it is allowed only as %26.
    let Some((addr, query)) = s.split_once('?') else { return s.replace('&', "%26") };
    let addr = addr.replace('&', "%26");
    let kept: Vec<&str> = query
        .split('&')
        .filter(|kv| {
            let key = kv.split('=').next().unwrap_or("").to_ascii_lowercase();
            matches!(key.as_str(), "subject" | "body" | "cc" | "bcc")
        })
        .collect();
    if kept.is_empty() {
        addr.to_string()
    } else {
        format!("{addr}?{}", kept.join("&"))
    }
}

fn looks_like_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn unc_reason() -> String {
    tr!(
        "Network paths (\\\\server\\...) are blocked: accessing them sends your Windows password hash (NTLM) to a foreign server.",
        "Сетевые пути (\\\\сервер\\...) заблокированы: обращение к ним отправляет хеш пароля Windows (NTLM) на чужой сервер."
    )
    .into()
}

const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
    "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "conin$", "conout$",
    // Windows also treats COM/LPT with superscript digits ¹ ² ³ as devices
    "com¹", "com²", "com³", "lpt¹", "lpt²", "lpt³",
];

/// Parses a local path from a link/image, with checks. Returns an absolute path.
pub fn resolve_local(raw: &str, base_dir: Option<&Path>) -> Result<(PathBuf, Option<String>), String> {
    let mut s = raw.to_string();
    let mut anchor = None;
    if let Some(i) = s.find('#') {
        let frag = s[i + 1..].to_string();
        if !frag.is_empty() {
            anchor = Some(percent_decode(&frag));
        }
        s.truncate(i);
    }
    if let Some(i) = s.find('?') {
        s.truncate(i);
    }
    let decoded = percent_decode(&s);
    if has_hidden_controls(&decoded) {
        return Err(tr!("The path contains control characters", "Путь содержит управляющие символы").into());
    }
    let p = decoded.replace('/', "\\");
    if p.is_empty() {
        return Err(tr!("Empty path", "Пустой путь").into());
    }
    if p.contains(['*', '?', '<', '>', '|', '"']) {
        return Err(tr!("Invalid characters in the path", "Недопустимые символы в пути").into());
    }
    if p.starts_with("\\\\") {
        return Err(unc_reason());
    }
    // A colon is allowed only as "C:" — otherwise it is an NTFS alternate stream (file.md:stream).
    let colon_ok = match p.find(':') {
        None => true,
        Some(1) => looks_like_drive(&p) && !p[2..].contains(':'),
        Some(_) => false,
    };
    if !colon_ok {
        return Err(tr!(
            "Invalid path (NTFS alternate streams and devices are not allowed)",
            "Недопустимый путь (альтернативные потоки NTFS и устройства запрещены)"
        )
        .into());
    }
    let path = if looks_like_drive(&p) {
        PathBuf::from(&p)
    } else if let Some(stripped) = p.strip_prefix('\\') {
        let base = base_dir.ok_or(tr!("No document folder", "Нет папки документа"))?;
        let root: PathBuf = base.components().take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir)).collect();
        root.join(stripped)
    } else {
        base_dir.ok_or(tr!("No document folder", "Нет папки документа"))?.join(&p)
    };
    let path = normalize(&path);
    if path.to_string_lossy().starts_with("\\\\") {
        return Err(unc_reason());
    }
    for c in path.components() {
        if let Component::Normal(n) = c {
            let n = n.to_string_lossy().to_ascii_lowercase();
            // Windows drops trailing dots and spaces from a name: "setup.exe." and "setup.exe "
            // open as setup.exe — this is how an extension is disguised.
            if n.ends_with('.') || n.ends_with(' ') || n.starts_with(' ') {
                return Err(tr!(
                    "Invalid name: a trailing dot or space hides the real file type",
                    "Недопустимое имя: точка или пробел в конце имени маскируют настоящий тип файла"
                )
                .into());
            }
            let stem = n.split('.').next().unwrap_or("").trim_end();
            if RESERVED.contains(&stem) {
                return Err(tr!(
                    "Windows device names (CON, NUL, COM1…) are not allowed",
                    "Имена устройств Windows (CON, NUL, COM1…) запрещены"
                )
                .into());
            }
        }
    }
    Ok((path, anchor))
}

fn local_path(raw: &str, base_dir: Option<&Path>) -> LinkAction {
    match resolve_local(raw, base_dir) {
        Err(e) => LinkAction::Blocked(e),
        Ok((path, anchor)) => {
            if is_md_ext(&path) {
                LinkAction::OpenMarkdown { path, anchor }
            } else if is_dangerous_ext(&path) {
                let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
                LinkAction::Blocked(trf!(
                    ".{ext} files can run programs — opening them from a document is blocked.",
                    "Файлы .{ext} могут запускать программы — открытие из документа заблокировано."
                ))
            } else {
                LinkAction::LocalFile(path)
            }
        }
    }
}

/// Does the path contain a symbolic link (it may lead to `\\server\…`)? The check goes
/// from the root: each component is read via FindFirstFileW, which does not follow the
/// component itself, and all previous ones are already checked. Junctions (mount points) lead
/// only to local volumes and are allowed; so are cloud placeholders (OneDrive).
pub fn has_symlink_component(path: &Path) -> bool {
    use windows::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW};
    const ATTR_REPARSE: u32 = 0x400;
    const TAG_SYMLINK: u32 = 0xA000_000C;
    let mut cur = PathBuf::new();
    for c in path.components() {
        cur.push(c.as_os_str());
        if let Component::Normal(_) = c {
            let w = crate::util::wide_path(&cur);
            let mut fd = WIN32_FIND_DATAW::default();
            unsafe {
                if let Ok(h) = FindFirstFileW(crate::util::pcwstr(&w), &mut fd) {
                    let _ = FindClose(h);
                    if fd.dwFileAttributes & ATTR_REPARSE != 0 && fd.dwReserved0 == TAG_SYMLINK {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// The real (long) file name: `X~1.LIB` → `x.library-ms`.
pub fn long_path(path: &Path) -> PathBuf {
    use windows::Win32::Storage::FileSystem::GetLongPathNameW;
    let w = crate::util::wide_path(path);
    unsafe {
        let n = GetLongPathNameW(crate::util::pcwstr(&w), None) as usize;
        if n == 0 {
            return path.to_path_buf();
        }
        let mut buf = vec![0u16; n + 1];
        let m = GetLongPathNameW(crate::util::pcwstr(&w), Some(&mut buf)) as usize;
        if m == 0 || m > n {
            return path.to_path_buf();
        }
        PathBuf::from(String::from_utf16_lossy(&buf[..m]))
    }
}

/// Checks a local file **at the moment of action** (click, image load): it exists,
/// has no symbolic links in the path, and its real name is not of a dangerous type. Returns the long path.
pub fn prepare_local(path: &Path) -> Result<PathBuf, String> {
    if has_symlink_component(path) {
        return Err(tr!(
            "The path contains a symbolic link — through it the document could reach a network resource.",
            "Путь содержит символическую ссылку — через неё документ мог бы обратиться к сетевому ресурсу."
        )
        .into());
    }
    if !path.exists() {
        return Err(trf!("File not found:\n{}", "Файл не найден:\n{}", path.display()));
    }
    let real = long_path(path);
    if is_dangerous_ext(&real) {
        let ext = real.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        return Err(trf!(
            ".{ext} files can run programs — opening them from a document is blocked.",
            "Файлы .{ext} могут запускать программы — открытие из документа заблокировано."
        ));
    }
    Ok(real)
}

/// Lexical normalization without touching the disk (so as not to touch the network).
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if can_pop {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn validate_web(s: &str) -> LinkAction {
    let Some(sep) = s.find("://") else {
        return LinkAction::Blocked(tr!("Invalid web address", "Некорректный веб-адрес").into());
    };
    let after = &s[sep + 3..];
    let auth_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..auth_end];
    let mut warn = None;
    let host_port = match authority.rfind('@') {
        Some(i) => {
            warn = Some(trf!(
                "The address contains “{}@” before the site name — a trick to hide the real address. The link actually leads to “{}”.",
                "Адрес содержит «{}@» перед именем сайта — так маскируют настоящий адрес. На самом деле ссылка ведёт на «{}».",
                &authority[..i],
                &authority[i + 1..]
            ));
            &authority[i + 1..]
        }
        None => authority,
    };
    let host = if host_port.starts_with('[') {
        host_port.split(']').next().map(|h| format!("{h}]")).unwrap_or_default()
    } else {
        host_port.split(':').next().unwrap_or("").to_string()
    };
    if host.is_empty() {
        return LinkAction::Blocked(tr!("The address has no site name", "В адресе нет имени сайта").into());
    }
    if host.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '<' | '>' | '\\' | '`' | '{' | '}' | '|' | '^')) {
        return LinkAction::Blocked(tr!("Invalid site name", "Некорректное имя сайта").into());
    }
    if warn.is_none() && !host.is_ascii() {
        warn = Some(trf!(
            "The site name “{host}” contains non-Latin letters. Look-alike characters are sometimes used to fake well-known addresses.",
            "Имя сайта «{host}» содержит не-латинские буквы. Так иногда подделывают известные адреса похожими символами."
        ));
    }
    LinkAction::Web { url: encode_url_chars(s), host: host.to_ascii_lowercase(), warn }
}

/// Encodes characters that must not reach the handler's command line as is.
fn encode_url_chars(s: &str) -> String {
    let mut url = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' => url.push_str("%20"),
            '"' => url.push_str("%22"),
            '<' => url.push_str("%3C"),
            '>' => url.push_str("%3E"),
            '\\' => url.push_str("%5C"),
            '^' => url.push_str("%5E"),
            '`' => url.push_str("%60"),
            '{' => url.push_str("%7B"),
            '|' => url.push_str("%7C"),
            '}' => url.push_str("%7D"),
            c => url.push(c),
        }
    }
    url
}

/// Removes invisible characters and maps look-alike dots to a regular dot.
fn normalize_display(t: &str) -> String {
    t.chars()
        .filter(|c| !matches!(*c as u32, 0x200B..=0x200F | 0x2060..=0x2064 | 0xFEFF | 0x00AD | 0x034F | 0x180E | 0x061C))
        .map(|c| match c {
            '\u{FF0E}' | '\u{3002}' | '\u{FF61}' | '\u{2024}' | '\u{FE52}' => '.',
            c => c,
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Does the word look like a site name; returns the "displayed" host.
fn domain_like(token: &str) -> Option<String> {
    let t = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '-' && c != ':' && c != '/');
    let explicit = t.starts_with("http://") || t.starts_with("https://") || t.starts_with("www.");
    let t = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://")).unwrap_or(t);
    // "README.md", "main.rs", "setup.py" are file names, not sites (unless there is http:// or www.).
    const FILE_EXT: &[&str] = &[
        "md", "txt", "rs", "py", "js", "ts", "tsx", "jsx", "html", "htm", "json", "yml", "yaml", "toml", "png", "jpg",
        "jpeg", "gif", "svg", "pdf", "exe", "zip", "sh", "ps1", "c", "h", "cpp", "hpp", "cs", "go", "java", "kt", "rb",
        "php", "css", "scss", "xml", "csv", "log", "ini", "cfg", "lock", "bat", "cmd", "dll", "so", "sql", "vue", "lua",
    ];
    if !explicit {
        let last = t.split(['/', '?', '#']).next().unwrap_or("").rsplit('.').next().unwrap_or("");
        if FILE_EXT.contains(&last) {
            return None;
        }
    }
    let shown = t.split(['/', '?', '#']).next().unwrap_or("");
    let shown = shown.rsplit('@').next().unwrap_or(shown);
    let shown = shown.split(':').next().unwrap_or(shown).trim_end_matches('.');
    let tld = shown.rsplit('.').next().unwrap_or("");
    if shown.contains('.') && tld.chars().count() >= 2 && tld.chars().all(|c| c.is_alphabetic()) && !shown.starts_with('.') {
        Some(shown.to_string())
    } else {
        None
    }
}

/// The link text looks like an address but leads to another site? → warning.
pub fn spoof_warning(link_text: &str, target_host: &str) -> Option<String> {
    let t = normalize_display(link_text);
    let strip_www = |h: &str| h.strip_prefix("www.").unwrap_or(h).to_string();
    let b = strip_www(&target_host.to_lowercase());
    // Every domain-like word of the link text must match the real site.
    for token in t.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '«' | '»' | '"' | ',' | ';')) {
        let Some(shown) = domain_like(token) else { continue };
        let a = strip_www(&shown);
        if a == b || b.ends_with(&format!(".{a}")) {
            continue;
        }
        return Some(trf!(
            "The link text shows “{shown}”, but the link actually leads to “{target_host}”.",
            "Текст ссылки показывает «{shown}», но на самом деле она ведёт на «{target_host}»."
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> PathBuf {
        PathBuf::from("D:\\docs\\proj")
    }

    fn blocked(a: &LinkAction) -> bool {
        matches!(a, LinkAction::Blocked(_))
    }

    #[test]
    fn blocks_script_and_protocol_handlers() {
        for u in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "  javascript:alert(1)",
            "vbscript:msgbox",
            "data:text/html,<script>",
            "ms-msdt:/id PCWDiagnostic",
            "search-ms:query=x&crumb=location:\\\\evil\\s",
            "ms-officecmd:abc",
            "ms-appinstaller:?source=http://evil/x.appx",
            "shell:startup",
            "ms-settings:",
            "steam://run/1",
        ] {
            assert!(blocked(&classify(u, Some(&base()))), "{u}");
        }
    }

    #[test]
    fn blocks_hidden_controls() {
        assert!(blocked(&classify("java\tscript:alert(1)", Some(&base()))));
        assert!(blocked(&classify("https://example.com/\u{202E}gpj.exe", Some(&base()))));
        // finding of the 0.2.0 audit: invisible zero-width characters, joiners, tags
        for u in ["https://exa\u{200B}mple.com/", "https://example.com/\u{2060}a", "https://example.com/\u{E0041}", "https://exa\u{2028}mple.com/", "https://exam\u{00AD}ple.com"] {
            assert!(blocked(&classify(u, Some(&base()))), "{u:?}");
        }
        // COM/LPT with superscript digits are devices too
        for u in ["COM¹", "lpt³.txt", "dir/COM².md"] {
            assert!(blocked(&classify(u, Some(&base()))), "{u}");
        }
        // "&" in a mailto: address without "?" does not turn into an attach parameter
        match classify("mailto:a@b.c&attach=C:/x", None) {
            LinkAction::Mail(m) => assert!(!m.contains('&'), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn blocks_unc_and_devices() {
        for u in [
            "\\\\evil\\share\\a.md",
            "//evil/share/a.md",
            "file://evil/share/a.md",
            "file:////evil/share/a.md",
            "\\\\?\\C:\\x.md",
            "\\\\.\\pipe\\x",
            "CON",
            "nul.md",
            "a.md:stream",
            "%5C%5Cevil%5Cshare%5Ca.png",
        ] {
            assert!(blocked(&classify(u, Some(&base()))), "{u}");
        }
    }

    #[test]
    fn blocks_executables() {
        for u in [
            "setup.exe", "run.bat", "x.LNK", "C:\\Windows\\notepad.exe", "a.hta", "s.ps1", "t.theme",
            // disguising the extension with trailing dots/spaces
            "setup.exe.", "setup.exe%20", "setup.exe...", "setup.exe%2E", "dir./x.md", "a.wsb", "q.iqy",
        ] {
            assert!(blocked(&classify(u, Some(&base()))), "{u}");
        }
    }

    #[test]
    fn mailto_params_are_filtered() {
        assert_eq!(classify("mailto:a@b.c?attach=C:/secret.txt&subject=Hi", None), LinkAction::Mail("mailto:a@b.c?subject=Hi".into()));
        assert_eq!(classify("mailto:a@b.c?attach=x", None), LinkAction::Mail("mailto:a@b.c".into()));
    }

    #[test]
    fn allows_normal_links() {
        assert_eq!(classify("#Раздел", None), LinkAction::Anchor("Раздел".into()));
        match classify("../other.md#sec", Some(&base())) {
            LinkAction::OpenMarkdown { path, anchor } => {
                assert_eq!(path, PathBuf::from("D:\\docs\\other.md"));
                assert_eq!(anchor.as_deref(), Some("sec"));
            }
            x => panic!("{x:?}"),
        }
        match classify("https://example.com/a b", None) {
            LinkAction::Web { url, host, warn } => {
                assert_eq!(url, "https://example.com/a%20b");
                assert_eq!(host, "example.com");
                assert!(warn.is_none());
            }
            x => panic!("{x:?}"),
        }
        assert!(matches!(classify("file:///C:/docs/a.md", None), LinkAction::OpenMarkdown { .. }));
        assert!(matches!(classify("img/photo.png", Some(&base())), LinkAction::LocalFile(_)));
        assert!(matches!(classify("mailto:a@b.c", None), LinkAction::Mail(_)));
    }

    #[test]
    fn warns_on_userinfo_and_spoof() {
        match classify("https://bank.com@evil.com/login", None) {
            LinkAction::Web { host, warn, .. } => {
                assert_eq!(host, "evil.com");
                assert!(warn.is_some());
            }
            x => panic!("{x:?}"),
        }
        assert!(spoof_warning("https://bank.com", "evil.com").is_some());
        assert!(spoof_warning("www.github.com", "github.com").is_none());
        assert!(spoof_warning("github.com", "docs.github.com").is_none());
        assert!(spoof_warning("нажми здесь", "evil.com").is_none());
        // bypasses from the audit: invisible space, look-alike dot, domain among words
        assert!(spoof_warning("github.com\u{200B}", "evil.example").is_some());
        assert!(spoof_warning("github\u{FF0E}com", "evil.example").is_some());
        assert!(spoof_warning("Войти на bank.com", "evil.example").is_some());
        assert!(spoof_warning("Документация на docs.rs", "docs.rs").is_none());
        assert!(spoof_warning("README.md", "github.com").is_none());
        assert!(spoof_warning("см. main.rs", "github.com").is_none());
        assert!(spoof_warning("https://bank.md", "evil.example").is_some());
    }

    #[test]
    fn openable_whitelist() {
        assert!(is_openable_ext(Path::new("a.PDF")));
        assert!(is_openable_ext(Path::new("photo.jpg")));
        for p in ["X~1.LIB", "x.library-ms", "a.html", "a.svg", "a.ſcr", "noext", "a.docm", "a.rtf"] {
            assert!(!is_openable_ext(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn short_names_resolve_to_long() {
        // A real file with a "dangerous" long name; if the volume has 8.3 names, check
        // that the short name is expanded and blocked.
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".toolcache").join("tmp").join("shortname-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let long = dir.join("examples-folder.library-ms");
        std::fs::write(&long, b"<x/>").unwrap();
        assert!(prepare_local(&long).is_err(), "a long name of a dangerous type is blocked");
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;
        let w = crate::util::wide_path(&long);
        let mut buf = vec![0u16; 1024];
        let n = unsafe { GetShortPathNameW(crate::util::pcwstr(&w), Some(&mut buf)) } as usize;
        let short = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
        if n > 0 && short != long {
            assert!(!is_dangerous_ext(&short), "the short name by itself looks safe: {}", short.display());
            assert!(prepare_local(&short).is_err(), "but is blocked after expansion: {}", short.display());
        } else {
            eprintln!("8.3 names are disabled on this volume — only the long name was checked");
        }
        let ok = dir.join("photo.png");
        std::fs::write(&ok, b"x").unwrap();
        assert!(prepare_local(&ok).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A unique scratch folder inside the project (never the system temp folder).
    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".toolcache").join("tmp").join(format!("links-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn extension_helpers() {
        // no extension at all
        assert!(!is_dangerous_ext(Path::new("README")));
        assert!(!is_md_ext(Path::new("README")));
        assert!(!is_openable_ext(Path::new("README")));
        // case-insensitive
        assert!(is_dangerous_ext(Path::new("A.EXE")));
        assert!(is_dangerous_ext(Path::new("x.Library-Ms")));
        for p in ["a.md", "a.MARKDOWN", "a.mdown", "a.mkd", "a.mkdn", "a.mdwn", "a.mdtxt", "a.mdtext"] {
            assert!(is_md_ext(Path::new(p)), "{p}");
        }
        assert!(!is_md_ext(Path::new("a.mdx")));
        // a non-ASCII extension is never openable (Unicode case tricks)
        assert!(!is_openable_ext(Path::new("a.рng")));
        assert!(is_openable_ext(Path::new("a.Mp4")));
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("%D0%A0%d1%8b"), "Ры");
        // invalid or truncated escapes stay as is
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        assert_eq!(percent_decode("%"), "%");
        assert_eq!(percent_decode("100%"), "100%");
        // a broken UTF-8 sequence becomes U+FFFD, not a panic
        assert_eq!(percent_decode("%FF"), "\u{FFFD}");
    }

    #[test]
    fn scheme_detection() {
        assert_eq!(scheme_of("HTTPS://x").as_deref(), Some("https"));
        assert_eq!(scheme_of("ms-settings:").as_deref(), Some("ms-settings"));
        assert_eq!(scheme_of("svn+ssh://x").as_deref(), Some("svn+ssh"));
        assert_eq!(scheme_of("C:\\x.md"), None, "a drive letter is not a scheme");
        assert_eq!(scheme_of("no colon"), None);
        assert_eq!(scheme_of("1ab:x"), None, "must start with a letter");
        assert_eq!(scheme_of("a b:x"), None, "no spaces");
        assert_eq!(scheme_of("dir/a.md:stream"), None, "no slashes");
        assert_eq!(scheme_of(":x"), None);
    }

    #[test]
    fn hidden_control_ranges() {
        for c in [
            '\u{0}', '\u{1F}', '\u{7F}', '\u{80}', '\u{9F}', '\u{AD}', '\u{61C}', '\u{180E}', '\u{200B}', '\u{200D}', '\u{200E}',
            '\u{200F}', '\u{2028}', '\u{2029}', '\u{202A}', '\u{202E}', '\u{2060}', '\u{2064}', '\u{2066}', '\u{2069}', '\u{FE00}',
            '\u{FE0F}', '\u{FEFF}', '\u{E0000}', '\u{E007F}',
        ] {
            assert!(has_hidden_controls(&format!("a{c}b")), "U+{:04X}", c as u32);
        }
        for s in ["https://example.com/a?b=c#d", "Привет мир", "日本語", "a\u{A0}b", "emoji 😀", "\u{2065}", "\u{FE10}"] {
            assert!(!has_hidden_controls(s), "{s:?}");
        }
    }

    #[test]
    fn classify_basic_branches() {
        assert!(blocked(&classify("", None)));
        assert!(blocked(&classify("   \t ", None)));
        // anchors are percent-decoded
        assert_eq!(classify("#%D0%A0%D0%B0%D0%B7", None), LinkAction::Anchor("Раз".into()));
        assert_eq!(classify("  #top  ", None), LinkAction::Anchor("top".into()));
        assert_eq!(classify("#", None), LinkAction::Anchor(String::new()));
        // internal commands are recognised (executed only on the start page)
        assert_eq!(classify("vmd:install", None), LinkAction::Internal("install".into()));
        assert_eq!(classify("VMD:uninstall", None), LinkAction::Internal("uninstall".into()));
        // unknown protocols are blocked and the message names the protocol
        match classify("Steam://run/1", None) {
            LinkAction::Blocked(m) => assert!(m.contains("steam:"), "{m}"),
            x => panic!("{x:?}"),
        }
    }

    #[test]
    fn mailto_sanitisation() {
        for u in ["mailto:a@b.c\"x", "mailto:<a@b.c>", "mailto:a@b.c\\x", "mailto:a`@b.c"] {
            assert!(blocked(&classify(u, None)), "{u}");
        }
        // only subject/body/cc/bcc survive, keys are case-insensitive, order is kept
        assert_eq!(
            classify("mailto:a@b.c?CC=x@y.z&attach=C:/a&Body=hi&bcc=q@w.e&foo=1", None),
            LinkAction::Mail("mailto:a@b.c?CC=x@y.z&Body=hi&bcc=q@w.e".into())
        );
        // spaces and special characters are encoded; "&" in the address becomes %26
        assert_eq!(classify("mailto:a&b@c.d?subject=Hi there|x", None), LinkAction::Mail("mailto:a%26b@c.d?subject=Hi%20there%7Cx".into()));
        assert_eq!(classify("mailto:a@b.c?", None), LinkAction::Mail("mailto:a@b.c".into()));
        assert_eq!(classify("MAILTO:a@b.c", None), LinkAction::Mail("MAILTO:a@b.c".into()));
    }

    #[test]
    fn file_urls() {
        let md = |u: &str| match classify(u, None) {
            LinkAction::OpenMarkdown { path, anchor } => (path, anchor),
            x => panic!("{u}: {x:?}"),
        };
        assert_eq!(md("file:///C:/docs/a.md").0, PathBuf::from("C:\\docs\\a.md"));
        assert_eq!(md("file://localhost/C:/docs/a.md#x"), (PathBuf::from("C:\\docs\\a.md"), Some("x".into())));
        assert_eq!(md("file://LOCALHOST/C:/a.md").0, PathBuf::from("C:\\a.md"));
        // without "//": file:/C:/x and file:C:/x
        assert_eq!(md("file:/C:/a.md").0, PathBuf::from("C:\\a.md"));
        assert_eq!(md("file:C:/a.md").0, PathBuf::from("C:\\a.md"));
        assert_eq!(classify("file:///C:/docs/report.pdf", None), LinkAction::LocalFile(PathBuf::from("C:\\docs\\report.pdf")));
        assert!(blocked(&classify("file:///C:/x/setup.exe", None)));
        // no drive → treated as a network path
        for u in ["file://localhost", "file://", "file:", "file:///", "file:///share/a.md", "file://localhost/share/a.md", "file:relative.md"] {
            assert!(blocked(&classify(u, None)), "{u}");
        }
    }

    #[test]
    fn local_path_resolution() {
        let b = base();
        let ok = |raw: &str| resolve_local(raw, Some(&b)).unwrap_or_else(|e| panic!("{raw}: {e}"));
        assert_eq!(ok("a.md"), (PathBuf::from("D:\\docs\\proj\\a.md"), None));
        assert_eq!(ok("./sub/../a.md"), (PathBuf::from("D:\\docs\\proj\\a.md"), None));
        // query is dropped, the fragment becomes the anchor (percent-decoded), an empty fragment is none
        assert_eq!(ok("a.md?x=1#%D0%AF"), (PathBuf::from("D:\\docs\\proj\\a.md"), Some("Я".into())));
        assert_eq!(ok("a.md#"), (PathBuf::from("D:\\docs\\proj\\a.md"), None));
        assert_eq!(ok("my%20file.md").0, PathBuf::from("D:\\docs\\proj\\my file.md"));
        // root-relative paths stay on the document's drive
        assert_eq!(ok("/other/a.md").0, PathBuf::from("D:\\other\\a.md"));
        assert_eq!(ok("\\other\\a.md").0, PathBuf::from("D:\\other\\a.md"));
        // ".." never climbs above the drive root
        assert_eq!(ok("../../../../x.md").0, PathBuf::from("D:\\x.md"));
        // absolute paths ignore the base
        assert_eq!(resolve_local("C:/x/y.md", None).unwrap().0, PathBuf::from("C:\\x\\y.md"));
        // relative paths need a document folder
        assert!(resolve_local("a.md", None).is_err());
        assert!(resolve_local("/a.md", None).is_err());
        assert!(matches!(classify("a.md", None), LinkAction::Blocked(_)));
    }

    #[test]
    fn local_path_rejections() {
        let b = base();
        let err = |raw: &str| resolve_local(raw, Some(&b)).expect_err(raw);
        // control characters hidden behind percent-encoding
        assert!(err("a%09b.md").contains("control characters"));
        assert!(err("a%E2%80%AEdm.exe").contains("control characters"));
        // nothing left after dropping the query/fragment
        assert!(err("?x=1").contains("Empty path"));
        assert!(err("#frag").contains("Empty path"));
        // wildcard and reserved characters (also when percent-encoded)
        for raw in ["a*.md", "a<b.md", "a>b.md", "a|b.md", "a\"b.md", "a%3Fb.md", "a%2Ab.md"] {
            assert!(err(raw).contains("Invalid characters"), "{raw}");
        }
        // NTFS streams: a colon anywhere but after the drive letter
        for raw in ["dir/a.md:stream", "C:/a.md:stream", "a.md%3A%24DATA", "C:x:y"] {
            assert!(err(raw).contains("alternate streams"), "{raw}");
        }
        // network paths, also hidden behind encoding or a forward-slash prefix
        for raw in ["\\\\srv\\s\\a.md", "//srv/s/a.md", "%2F%2Fsrv/s/a.md", "/\\srv\\a.md"] {
            assert!(err(raw).contains("Network paths"), "{raw}");
        }
        // trailing dots/spaces and leading spaces in any component
        for raw in ["setup.exe.", "setup.exe%20", "dir%20/a.md", "%20dir/a.md", "dir/ a.png"] {
            assert!(err(raw).contains("trailing dot or space"), "{raw}");
        }
        // device names, with or without an extension, in any component
        for raw in ["con", "PRN.txt", "aux.md", "Nul", "com1.png", "LPT9", "conin$", "CONOUT$", "dir/com3/a.md", "lpt¹.md", "con .md"] {
            assert!(err(raw).contains("device names") || err(raw).contains("trailing dot or space"), "{raw}");
        }
        // "console.md" and "community.png" are ordinary names
        assert!(resolve_local("console.md", Some(&b)).is_ok());
        assert!(resolve_local("com10.png", Some(&b)).is_ok());
    }

    /// A document opened from a network share must not resolve relative links onto the share.
    #[test]
    fn relative_links_from_a_network_folder_are_blocked() {
        let unc = PathBuf::from("\\\\server\\share\\docs");
        assert!(resolve_local("a.md", Some(&unc)).unwrap_err().contains("Network paths"));
        assert!(resolve_local("/a.png", Some(&unc)).unwrap_err().contains("Network paths"));
        assert!(blocked(&classify("img/a.png", Some(&unc))));
        // an absolute local path is still fine
        assert!(resolve_local("C:/a.md", Some(&unc)).is_ok());
    }

    #[test]
    fn local_link_kinds() {
        let b = base();
        assert!(matches!(classify("notes/x.MARKDOWN", Some(&b)), LinkAction::OpenMarkdown { .. }));
        assert_eq!(classify("report.pdf", Some(&b)), LinkAction::LocalFile(PathBuf::from("D:\\docs\\proj\\report.pdf")));
        assert_eq!(classify("folder", Some(&b)), LinkAction::LocalFile(PathBuf::from("D:\\docs\\proj\\folder")));
        // the block message names the extension as written
        match classify("tools/Run.CMD", Some(&b)) {
            LinkAction::Blocked(m) => assert!(m.contains(".CMD"), "{m}"),
            x => panic!("{x:?}"),
        }
        // dangerous types in the middle of the path are fine — only the final name counts
        assert!(matches!(classify("x.exe/readme.md", Some(&b)), LinkAction::OpenMarkdown { .. }));
    }

    #[test]
    fn lexical_normalisation() {
        assert_eq!(normalize(Path::new(".\\a\\.\\b")), PathBuf::from("a\\b"));
        assert_eq!(normalize(Path::new("D:\\a\\b\\..\\c")), PathBuf::from("D:\\a\\c"));
        assert_eq!(normalize(Path::new("D:\\..\\..\\c")), PathBuf::from("D:\\c"));
        assert_eq!(normalize(Path::new("..\\a")), PathBuf::from("a"), "a leading .. is dropped");
        assert_eq!(normalize(Path::new("\\\\srv\\share\\..\\x")), PathBuf::from("\\\\srv\\share\\x"), "the UNC prefix is kept");
        assert_eq!(normalize(Path::new("")), PathBuf::new());
    }

    #[test]
    fn web_addresses() {
        let web = |u: &str| match classify(u, None) {
            LinkAction::Web { url, host, warn } => (url, host, warn),
            x => panic!("{u}: {x:?}"),
        };
        // host is lowercased, port and path dropped from it
        let (url, host, warn) = web("HTTPS://Example.COM:8443/Path?q=1#f");
        assert_eq!((url.as_str(), host.as_str(), warn), ("HTTPS://Example.COM:8443/Path?q=1#f", "example.com", None));
        assert_eq!(web("http://example.com?x").1, "example.com");
        assert_eq!(web("http://example.com#x").1, "example.com");
        // IPv6 literal keeps its brackets
        assert_eq!(web("http://[::1]:8080/x").1, "[::1]");
        assert_eq!(web("http://[2001:db8::1]/").1, "[2001:db8::1]");
        // every special character is percent-encoded before reaching the handler
        assert_eq!(web("https://e.com/a b\"c<d>e\\f^g`h{i|j}k").0, "https://e.com/a%20b%22c%3Cd%3Ee%5Cf%5Eg%60h%7Bi%7Cj%7Dk");
        // user@host: warning, and the host is the part after the last "@"
        let (_, host, warn) = web("https://a@b@evil.example/x");
        assert_eq!(host, "evil.example");
        let warn = warn.unwrap();
        assert!(warn.contains("“a@b@”") && warn.contains("“evil.example”"), "{warn}");
        // non-Latin letters in the host: warning
        let (_, host, warn) = web("https://аpple.com/");
        assert_eq!(host, "аpple.com");
        assert!(warn.unwrap().contains("non-Latin"));
        // the userinfo warning wins over the non-Latin one
        assert!(web("https://x@рф.example/").2.unwrap().contains("@"));
    }

    #[test]
    fn invalid_web_addresses() {
        // no "://", no host, no host after userinfo, only a port
        for u in ["http:example.com", "https:///path", "https://user@/x", "http://:80/", "https://?q", "https://#f"] {
            assert!(blocked(&classify(u, None)), "{u}");
        }
        for u in ["https://exa mple.com/", "https://exa{mple.com", "https://a\"b.com", "https://a<b.com", "https://a`b.com", "https://a|b.com", "https://a^b.com", "https://a}b.com", "https://a\\b.com"] {
            match classify(u, None) {
                LinkAction::Blocked(m) => assert!(m.contains("Invalid site name"), "{u}: {m}"),
                x => panic!("{u}: {x:?}"),
            }
        }
    }

    #[test]
    fn spoof_detection_details() {
        // explicit scheme or www. → compared even with a "file" extension
        assert!(spoof_warning("http://bank.md/login", "evil.example").is_some());
        assert!(spoof_warning("www.bank.rs", "evil.example").is_some());
        // ports, paths, userinfo and trailing dots in the shown text are ignored
        assert!(spoof_warning("github.com:443", "github.com").is_none());
        assert!(spoof_warning("github.com/user/repo", "github.com").is_none());
        assert!(spoof_warning("github.com.", "github.com").is_none());
        assert!(spoof_warning("me@github.com", "github.com").is_none());
        assert!(spoof_warning("me@bank.com", "evil.example").is_some());
        // brackets, quotes and guillemets around the shown domain
        assert!(spoof_warning("(bank.com)", "evil.example").is_some());
        assert!(spoof_warning("«bank.com»", "evil.example").is_some());
        assert!(spoof_warning("\"bank.com\",", "evil.example").is_some());
        // case and www. on either side
        assert!(spoof_warning("WWW.GitHub.COM", "www.github.com").is_none());
        // not domains: numeric TLD, one-letter TLD, a leading dot, version numbers
        for t in ["1.2.3.4", "a.b", ".hidden", "v1.2", "see section 3.4", "e.g. this", "..."] {
            assert!(spoof_warning(t, "evil.example").is_none(), "{t}");
        }
        // a parent domain shown for a subdomain is fine, the reverse is not
        assert!(spoof_warning("docs.github.com", "github.com").is_some());
        assert!(spoof_warning("evilgithub.com", "github.com").is_some());
        // the first mismatching domain is reported
        let w = spoof_warning("github.com or bank.com", "github.com").unwrap();
        assert!(w.contains("“bank.com”") && w.contains("“github.com”"), "{w}");
        // look-alike dots: ideographic and halfwidth
        assert!(spoof_warning("bank\u{3002}com", "evil.example").is_some());
        assert!(spoof_warning("bank\u{FF61}com", "evil.example").is_some());
        assert!(spoof_warning("bank\u{2024}com", "evil.example").is_some());
        assert!(spoof_warning("ba\u{00AD}nk.com", "bank.com").is_none(), "a soft hyphen is removed before comparing");
    }

    #[test]
    fn prepare_local_checks() {
        let dir = scratch("prepare");
        // a missing file
        let missing = dir.join("missing.png");
        assert!(prepare_local(&missing).unwrap_err().contains("File not found"));
        // long_path of a missing file returns it unchanged
        assert_eq!(long_path(&missing), missing);
        // an existing file of an openable type: its long name is returned
        let ok = dir.join("Photo Long Name.png");
        std::fs::write(&ok, b"x").unwrap();
        assert_eq!(prepare_local(&ok).unwrap(), long_path(&ok));
        assert!(long_path(&ok).ends_with("Photo Long Name.png"));
        // a dangerous type is blocked even if it exists
        let bad = dir.join("tool.ps1");
        std::fs::write(&bad, b"x").unwrap();
        assert!(prepare_local(&bad).unwrap_err().contains(".ps1"));
        // a folder passes (it is only ever shown in Explorer)
        assert!(prepare_local(&dir).is_ok());
        // no symbolic links in an ordinary path
        assert!(!has_symlink_component(&ok));
        assert!(!has_symlink_component(&missing));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A symbolic link anywhere in the path is rejected. Creating one needs Developer Mode
    /// or the SeCreateSymbolicLink privilege; without them the check is skipped.
    #[test]
    fn symlinks_are_rejected() {
        let dir = scratch("symlink");
        let real_dir = dir.join("real");
        std::fs::create_dir_all(&real_dir).unwrap();
        let file = real_dir.join("a.png");
        std::fs::write(&file, b"x").unwrap();
        let link_file = dir.join("link.png");
        let link_dir = dir.join("linkdir");
        let made_file = std::os::windows::fs::symlink_file(&file, &link_file).is_ok();
        let made_dir = std::os::windows::fs::symlink_dir(&real_dir, &link_dir).is_ok();
        if made_file {
            assert!(has_symlink_component(&link_file));
            assert!(prepare_local(&link_file).unwrap_err().contains("symbolic link"));
        }
        if made_dir {
            let through = link_dir.join("a.png");
            assert!(has_symlink_component(&through), "a link in the middle of the path");
            assert!(prepare_local(&through).unwrap_err().contains("symbolic link"));
        }
        if !made_file && !made_dir {
            eprintln!("symbolic links cannot be created here (no Developer Mode) — check skipped");
        }
        assert!(!has_symlink_component(&file));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
