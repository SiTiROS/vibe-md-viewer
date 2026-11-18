//! Downloading images from the internet via the system WinHTTP — only with the user's
//! explicit permission (see the remote_images setting).
//!
//! * Windows credentials are never sent automatically (AUTOLOGON HIGH).
//! * Cookies are disabled, no Referer is sent.
//! * Only http/https, a size limit and timeouts.
//! * Local network addresses and localhost are forbidden: a document cannot "knock"
//!   on the router or internal services. Checked: IP literals (including non-standard
//!   notations like `127.1` and `0x7f.0.0.1`), the result of DNS name resolution and **every
//!   redirect** (WinHTTP automatic redirects are off; we handle them ourselves).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Networking::WinHttp::*;

use crate::util::wide;

pub const MAX_REMOTE_BYTES: usize = 25 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const WINHTTP_DISABLE_REDIRECTS_: u32 = 0x0000_0002;
const WINHTTP_QUERY_LOCATION_: u32 = 33;

struct H(*mut core::ffi::c_void);
impl Drop for H {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let o = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || o[0] >= 240
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            let s = ip.segments();
            // Deprecated "IPv4-compatible" ::a.b.c.d and 6to4 2002:AABB:CCDD:: — check the embedded IPv4.
            if s[..6].iter().all(|&x| x == 0) && (s[6] != 0 || s[7] > 1) {
                return is_private_ip(IpAddr::V4(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8)));
            }
            // IPv4-translated ::ffff:0:a.b.c.d
            if s[..4].iter().all(|&x| x == 0) && s[4] == 0xffff && s[5] == 0 {
                return is_private_ip(IpAddr::V4(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8)));
            }
            if s[0] == 0x2002 {
                return is_private_ip(IpAddr::V4(Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8)));
            }
            ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80
                // deprecated site-local fec0::/10; NAT64 64:ff9b::/96 and 64:ff9b:1::/48
                || (s[0] & 0xffc0) == 0xfec0
                || (s[0] == 0x64 && s[1] == 0xff9b)
        }
    }
}

/// The host consists only of digit/hex labels — it is an IP address notation. Only the
/// canonical a.b.c.d notation is allowed (otherwise `127.1`, `2130706433`, `0x7f.0.0.1` bypass the check).
fn is_numeric_host(h: &str) -> bool {
    !h.is_empty()
        && h.split('.').all(|l| {
            !l.is_empty() && (l.chars().all(|c| c.is_ascii_digit()) || (l.starts_with("0x") && l[2..].chars().all(|c| c.is_ascii_hexdigit())))
        })
}

/// A local network / special-purpose host or a non-standard IP notation is forbidden (no DNS).
pub fn is_private_host(host: &str) -> bool {
    let h = host.trim_matches(|c| c == '[' || c == ']').trim_end_matches('.').to_ascii_lowercase();
    if h.is_empty()
        || h == "localhost"
        || [".localhost", ".local", ".lan", ".internal", ".home.arpa", ".intranet", ".corp", ".home"].iter().any(|s| h.ends_with(s))
    {
        return true;
    }
    if let Ok(ip) = h.parse::<Ipv4Addr>() {
        // Rust accepts only the canonical a.b.c.d notation without leading zeros.
        return is_private_ip(IpAddr::V4(ip));
    }
    if let Ok(ip) = h.parse::<Ipv6Addr>() {
        return is_private_ip(IpAddr::V6(ip));
    }
    if is_numeric_host(&h) {
        return true;
    }
    // Single-label names (no dot) are intranet.
    !h.contains('.')
}

/// Full host check before a request: literals + DNS resolution.
fn check_host(host: &str, port: u16) -> Result<(), String> {
    if is_private_host(host) {
        return Err(tr!("local network addresses are not allowed", "адреса локальной сети запрещены").into());
    }
    let h = host.trim_matches(|c| c == '[' || c == ']');
    let addrs = (h, port).to_socket_addrs().map_err(|_| tr!("could not find the server", "не удалось найти сервер").to_string())?;
    for a in addrs {
        if is_private_ip(a.ip()) {
            return Err(tr!("the server name points to a local network address", "имя сервера указывает на адрес локальной сети").into());
        }
    }
    Ok(())
}

struct Parsed {
    secure: bool,
    host: String,
    port: u16,
    object: Vec<u16>,
}

fn crack(url: &str) -> Result<Parsed, String> {
    let lower = url.to_ascii_lowercase();
    let secure = lower.starts_with("https://");
    if !secure && !lower.starts_with("http://") {
        return Err(tr!("only http and https are allowed", "разрешены только http и https").into());
    }
    let wurl: Vec<u16> = url.encode_utf16().collect();
    let mut host = vec![0u16; 512];
    let mut path = vec![0u16; 8192];
    let mut extra = vec![0u16; 8192];
    let mut uc = URL_COMPONENTS {
        dwStructSize: std::mem::size_of::<URL_COMPONENTS>() as u32,
        lpszHostName: PWSTR(host.as_mut_ptr()),
        dwHostNameLength: host.len() as u32,
        lpszUrlPath: PWSTR(path.as_mut_ptr()),
        dwUrlPathLength: path.len() as u32,
        lpszExtraInfo: PWSTR(extra.as_mut_ptr()),
        dwExtraInfoLength: extra.len() as u32,
        ..Default::default()
    };
    unsafe { WinHttpCrackUrl(&wurl, 0, &mut uc) }.map_err(|_| tr!("invalid address", "некорректный адрес").to_string())?;
    let host_s = String::from_utf16_lossy(&host[..uc.dwHostNameLength as usize]);
    let mut object: Vec<u16> = path[..uc.dwUrlPathLength as usize].to_vec();
    object.extend_from_slice(&extra[..uc.dwExtraInfoLength as usize]);
    if object.is_empty() {
        object.push(b'/' as u16);
    }
    object.push(0);
    Ok(Parsed { secure, host: host_s, port: uc.nPort, object })
}

/// Absolute address from the Location header.
fn resolve_location(base: &str, loc: &str) -> String {
    let l = loc.trim();
    let lower = l.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return l.to_string();
    }
    let scheme_end = base.find("://").map(|i| i + 3).unwrap_or(0);
    let scheme = &base[..scheme_end];
    if let Some(rest) = l.strip_prefix("//") {
        return format!("{scheme}{rest}");
    }
    let after = &base[scheme_end..];
    let host_end = after.find(['/', '?', '#']).map(|i| scheme_end + i).unwrap_or(base.len());
    let origin = &base[..host_end];
    if l.starts_with('/') {
        return format!("{origin}{l}");
    }
    let path = &base[host_end..];
    let path = path.split(['?', '#']).next().unwrap_or("");
    let dir = match path.rfind('/') {
        Some(i) => &path[..=i],
        None => "/",
    };
    format!("{origin}{dir}{l}")
}

enum Step {
    Body(Vec<u8>),
    Redirect(String),
}

pub fn fetch(url: &str, max: usize) -> Result<Vec<u8>, String> {
    unsafe {
        let ua = wide(concat!("vibe-md-viewer/", env!("CARGO_PKG_VERSION")));
        let session = H(WinHttpOpen(PCWSTR::from_raw(ua.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if session.0.is_null() {
            return Err(tr!("WinHTTP is unavailable", "WinHTTP недоступен").into());
        }
        let _ = WinHttpSetTimeouts(session.0, 5000, 7000, 10000, 15000);
        let policy = WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH.to_ne_bytes();
        let _ = WinHttpSetOption(Some(session.0), WINHTTP_OPTION_AUTOLOGON_POLICY, Some(&policy));

        let mut current = url.to_string();
        let mut was_secure = current.to_ascii_lowercase().starts_with("https://");
        // Overall time limit per image: a slow server cannot occupy the threads for long.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        for _ in 0..=MAX_REDIRECTS {
            let p = crack(&current)?;
            if was_secure && !p.secure {
                return Err(tr!("redirect from https to http is not allowed", "редирект с https на http запрещён").into());
            }
            was_secure = p.secure;
            check_host(&p.host, p.port)?;
            match fetch_once(&session, &p, max, deadline)? {
                Step::Body(b) => return Ok(b),
                Step::Redirect(loc) => current = resolve_location(&current, &loc),
            }
        }
        Err(tr!("too many redirects", "слишком много перенаправлений").into())
    }
}

unsafe fn fetch_once(session: &H, p: &Parsed, max: usize, deadline: std::time::Instant) -> Result<Step, String> {
    let host = wide(&p.host);
    let connect = H(WinHttpConnect(session.0, PCWSTR::from_raw(host.as_ptr()), p.port, 0));
    if connect.0.is_null() {
        return Err(tr!("could not connect", "не удалось подключиться").into());
    }
    let verb = wide("GET");
    let flags = if p.secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
    let req = H(WinHttpOpenRequest(connect.0, PCWSTR::from_raw(verb.as_ptr()), PCWSTR::from_raw(p.object.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags));
    if req.0.is_null() {
        return Err(tr!("could not create the request", "не удалось создать запрос").into());
    }
    let features = (WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS_).to_ne_bytes();
    let _ = WinHttpSetOption(Some(req.0), WINHTTP_OPTION_DISABLE_FEATURE, Some(&features));
    let policy = WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH.to_ne_bytes();
    let _ = WinHttpSetOption(Some(req.0), WINHTTP_OPTION_AUTOLOGON_POLICY, Some(&policy));
    let headers: Vec<u16> = "Accept: image/*\r\n".encode_utf16().collect();
    WinHttpSendRequest(req.0, Some(&headers), None, 0, 0, 0).map_err(|e| trf!("network error: {e}", "ошибка сети: {e}"))?;
    WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|e| trf!("network error: {e}", "ошибка сети: {e}"))?;
    let mut code: u32 = 0;
    let mut size = 4u32;
    let mut index = 0u32;
    let _ = WinHttpQueryHeaders(
        req.0,
        WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
        PCWSTR::null(),
        Some(&mut code as *mut u32 as *mut core::ffi::c_void),
        &mut size,
        &mut index,
    );
    if matches!(code, 301 | 302 | 303 | 307 | 308) {
        let mut buf = vec![0u16; 4096];
        let mut bytes = (buf.len() * 2) as u32;
        let mut idx = 0u32;
        WinHttpQueryHeaders(req.0, WINHTTP_QUERY_LOCATION_, PCWSTR::null(), Some(buf.as_mut_ptr() as *mut _), &mut bytes, &mut idx)
            .map_err(|_| tr!("redirect without an address", "перенаправление без адреса").to_string())?;
        let loc = String::from_utf16_lossy(&buf[..(bytes as usize / 2).min(buf.len())]);
        if crate::links::has_hidden_controls(&loc) {
            return Err(tr!("suspicious redirect address", "подозрительный адрес перенаправления").into());
        }
        return Ok(Step::Redirect(loc));
    }
    if code != 200 {
        return Err(trf!("server responded {code}", "сервер ответил {code}"));
    }
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let mut read = 0u32;
        WinHttpReadData(req.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut read).map_err(|e| trf!("read error: {e}", "ошибка чтения: {e}"))?;
        if read == 0 {
            break;
        }
        out.extend_from_slice(&buf[..read as usize]);
        if out.len() > max {
            return Err(tr!("image is too large", "картинка слишком большая").into());
        }
        if std::time::Instant::now() > deadline {
            return Err(tr!("the server responds too slowly", "сервер отвечает слишком медленно").into());
        }
    }
    Ok(Step::Body(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_hosts() {
        for h in [
            "localhost", "192.168.1.1", "10.0.0.5", "127.0.0.1", "169.254.1.1", "[::1]", "[fd00::1]", "router", "nas.local",
            "127.1", "2130706433", "0x7f.0.0.1", "0177.0.0.1", "127.000.000.001", "[::ffff:127.0.0.1]", "localhost.",
            "0.0.0.0", "224.0.0.1", "100.64.0.1", "[::127.0.0.1]", "[::192.168.1.1]", "[2002:c0a8:0101::1]", "[2002:7f00:1::]",
            // audit finding 0.2.0: IPv4-translated, site-local, NAT64 local-use
            "[::ffff:0:7f00:1]", "[::ffff:0:c0a8:101]", "[fec0::1]", "[64:ff9b:1::a00:1]",
        ] {
            assert!(is_private_host(h), "{h}");
        }
        for h in ["github.com", "8.8.8.8", "raw.githubusercontent.com", "123abc.example.com"] {
            assert!(!is_private_host(h), "{h}");
        }
    }

    #[test]
    fn location_resolution() {
        assert_eq!(resolve_location("https://a.com/x/y.png", "/z.png"), "https://a.com/z.png");
        assert_eq!(resolve_location("https://a.com/x/y.png", "z.png"), "https://a.com/x/z.png");
        assert_eq!(resolve_location("https://a.com/x/y.png", "//b.com/q"), "https://b.com/q");
        assert_eq!(resolve_location("https://a.com/x", "http://192.168.0.1/"), "http://192.168.0.1/");
    }

    /// Real download from the internet: `cargo test fetch_public -- --ignored`
    #[test]
    #[ignore]
    fn fetch_public() {
        let b = fetch("https://github.githubassets.com/favicons/favicon.png", 1 << 20).expect("download");
        assert!(b.starts_with(b"\x89PNG"), "is a PNG");
        let job = crate::images::Job { gen: 0, id: 0, source: crate::images::Source::Data(vec![]), max_w: 800, want_w: None, want_h: None, dpi: 96 };
        assert!(crate::images::to_pict(&b, &job).unwrap().starts_with("{\\pict"));
        let e = fetch("https://github.githubassets.com/favicons/no-such-file-xyz.png", 1 << 20).unwrap_err();
        assert!(e.contains("404"), "{e}");
        assert!(fetch("http://127.0.0.1/", 1024).is_err());
    }
}
