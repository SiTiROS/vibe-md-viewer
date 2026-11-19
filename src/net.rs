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
        fetch_with(&session, url, max, check_host)
    }
}

/// The request loop of `fetch`. Every step — the address itself and each redirect — is
/// requested without its `#fragment`, must not go from https to http and must pass `check`
/// (always `check_host` in the program; tests on the loopback interface pass their own).
unsafe fn fetch_with(session: &H, url: &str, max: usize, check: fn(&str, u16) -> Result<(), String>) -> Result<Vec<u8>, String> {
    let mut current = url.to_string();
    let mut was_secure = current.to_ascii_lowercase().starts_with("https://");
    // Overall time limit per image: a slow server cannot occupy the threads for long.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    for _ in 0..=MAX_REDIRECTS {
        let p = crack(without_fragment(&current))?;
        if was_secure && !p.secure {
            return Err(tr!("redirect from https to http is not allowed", "редирект с https на http запрещён").into());
        }
        was_secure = p.secure;
        check(&p.host, p.port)?;
        match fetch_once(session, &p, max, deadline)? {
            Step::Body(b) => return Ok(b),
            Step::Redirect(loc) => current = resolve_location(&current, &loc),
        }
    }
    Err(tr!("too many redirects", "слишком много перенаправлений").into())
}

/// The address without its `#fragment`. The fragment is for the client only; WinHTTP would send
/// it to the server percent-encoded as part of the path (`a.png#f` → `GET /a.png%23f`). Cutting
/// at the first `#` also matches where `links::classify` ends the host.
fn without_fragment(url: &str) -> &str {
    url.split_once('#').map_or(url, |(u, _)| u)
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

    #[test]
    fn private_ip_ranges() {
        let v4 = |s: &str| is_private_ip(IpAddr::V4(s.parse().unwrap()));
        for ip in [
            "0.1.2.3", "10.255.0.1", "100.64.0.0", "100.127.255.255", "127.255.255.254", "169.254.169.254", "172.16.0.1", "172.31.9.9",
            "192.168.0.1", "198.18.0.1", "198.19.255.255", "224.0.0.251", "239.1.1.1", "240.0.0.1", "255.255.255.255",
        ] {
            assert!(v4(ip), "{ip}");
        }
        for ip in ["1.1.1.1", "8.8.8.8", "100.63.255.255", "100.128.0.0", "172.32.0.1", "198.17.0.1", "198.20.0.1", "223.255.255.255", "11.0.0.1"] {
            assert!(!v4(ip), "{ip}");
        }
        let v6 = |s: &str| is_private_ip(IpAddr::V6(s.parse().unwrap()));
        for ip in [
            "::", "::1", "ff02::1", "fc00::1", "fdff::1", "fe80::1", "febf::1", "fec0::1", "feff::1",
            // IPv4 inside IPv6: mapped, compatible, translated, 6to4, NAT64
            "::ffff:10.0.0.1", "::ffff:169.254.1.1", "::10.0.0.1", "::0.0.0.2", "::ffff:0:192.168.1.1", "2002:0a00:0001::", "64:ff9b::8.8.8.8",
            "64:ff9b:1::1",
        ] {
            assert!(v6(ip), "{ip}");
        }
        for ip in ["2001:4860:4860::8888", "2606:4700::1111", "::ffff:8.8.8.8", "::8.8.8.8", "::ffff:0:8.8.8.8", "2002:0808:0808::1", "fe00::1", "2001:db8::1"] {
            assert!(!v6(ip), "{ip}");
        }
    }

    #[test]
    fn private_host_names() {
        for h in [
            "", "[]", ".", "LOCALHOST", "a.localhost", "printer.local", "nas.lan", "svc.internal", "x.home.arpa", "wiki.intranet", "dc.corp", "pc.home",
            "NAS.LOCAL.", "intranet", "[::]", "[fe80::1]",
            // non-canonical IPv4 notations
            "1.2.3", "1.2.3.4.5", "0x0a.0.0.1", "0x", "012.0.0.1", "4294967295",
        ] {
            assert!(is_private_host(h), "{h:?}");
        }
        for h in ["example.com", "EXAMPLE.COM.", "1.1.1.1", "[2001:4860:4860::8888]", "0x1.example", "local.example.com", "my-corp.com", "home.example"] {
            assert!(!is_private_host(h), "{h}");
        }
        assert!(is_numeric_host("1.2"));
        assert!(is_numeric_host("0xff.0x0"));
        assert!(!is_numeric_host(""));
        assert!(!is_numeric_host("1..2"));
        assert!(!is_numeric_host("1.2.x"));
        assert!(!is_numeric_host("0xfg"));
    }

    /// IP literals are checked without DNS; names are never resolved in this test.
    #[test]
    fn host_check_without_dns() {
        assert!(check_host("8.8.8.8", 443).is_ok());
        assert!(check_host("[2001:4860:4860::8888]", 443).is_ok());
        for h in ["127.0.0.1", "localhost", "[::1]", "10.0.0.1", "router", "0x7f.1"] {
            assert_eq!(check_host(h, 80).unwrap_err(), "local network addresses are not allowed", "{h}");
        }
    }

    fn object(p: &Parsed) -> String {
        assert_eq!(p.object.last(), Some(&0), "NUL-terminated");
        String::from_utf16(&p.object[..p.object.len() - 1]).unwrap()
    }

    #[test]
    fn url_cracking() {
        let p = crack("http://example.com").unwrap();
        assert_eq!((p.secure, p.host.as_str(), p.port, object(&p).as_str()), (false, "example.com", 80, "/"));
        let p = crack("https://Example.com:8443/a/b.png?x=1&y=2").unwrap();
        assert_eq!((p.secure, p.host.as_str(), p.port, object(&p).as_str()), (true, "Example.com", 8443, "/a/b.png?x=1&y=2"));
        let p = crack("HTTPS://example.com/").unwrap();
        assert_eq!((p.secure, p.port), (true, 443));
        let p = crack("http://example.com:8080").unwrap();
        assert_eq!((p.port, object(&p).as_str()), (8080, "/"));
        let p = crack("http://[2001:db8::1]:81/x").unwrap();
        assert_eq!(p.port, 81);
        assert!(p.host.contains("2001:db8::1"), "{}", p.host);
        assert!(is_private_host(&crack("http://[::1]/").unwrap().host), "a bracketed or bare IPv6 loopback is private");
        for u in ["ftp://example.com/a.png", "file:///C:/a.png", "example.com/a.png", "javascript:alert(1)", ""] {
            assert_eq!(crack(u).err().as_deref(), Some("only http and https are allowed"), "{u}");
        }
    }

    #[test]
    fn location_resolution_details() {
        // absolute addresses (any case) are taken as is, surrounding spaces trimmed
        assert_eq!(resolve_location("https://a.com/x", "  HTTPS://B.com/y  "), "HTTPS://B.com/y");
        assert_eq!(resolve_location("http://a.com/x", "https://b.com"), "https://b.com");
        // scheme-relative keeps the current scheme
        assert_eq!(resolve_location("http://a.com/x", "//b.com/y"), "http://b.com/y");
        // root-relative replaces the path, query and fragment
        assert_eq!(resolve_location("https://a.com:8443/x/y?q=1#f", "/z"), "https://a.com:8443/z");
        assert_eq!(resolve_location("https://a.com?q=1", "/z"), "https://a.com/z");
        // path-relative: resolved against the current folder; the query of the base is dropped
        assert_eq!(resolve_location("https://a.com/x/y.png?q=/1", "z.png"), "https://a.com/x/z.png");
        assert_eq!(resolve_location("https://a.com", "z.png"), "https://a.com/z.png");
        assert_eq!(resolve_location("https://a.com/", "z.png"), "https://a.com/z.png");
        assert_eq!(resolve_location("https://a.com#frag", "z.png"), "https://a.com/z.png");
        assert_eq!(resolve_location("https://a.com/dir/", "?p=2"), "https://a.com/dir/?p=2");
    }

    /// Addresses rejected before any connection is made (no network is touched).
    #[test]
    fn fetch_rejects_before_connecting() {
        assert_eq!(fetch("ftp://example.com/a.png", 1024).unwrap_err(), "only http and https are allowed");
        assert_eq!(fetch("file:///C:/a.png", 1024).unwrap_err(), "only http and https are allowed");
        for u in ["http://127.0.0.1/", "https://localhost:8443/a.png", "http://[::1]/a.png", "http://10.1.2.3/", "http://192.168.0.1/", "http://router/", "http://2130706433/"] {
            assert_eq!(fetch(u, 1024).unwrap_err(), "local network addresses are not allowed", "{u}");
        }
        assert!(fetch("http://", 1024).is_err());
    }

    /// A one-shot HTTP server on the loopback interface. Returns the port and a handle that
    /// yields the raw request (empty if nobody connected within the timeout).
    fn serve_once(response: Vec<u8>) -> (u16, std::thread::JoinHandle<String>) {
        let (port, h) = serve(vec![response]);
        (port, std::thread::spawn(move || h.join().unwrap().pop().unwrap_or_default()))
    }

    /// Like `serve_once`, for several connections in a row: one response per connection.
    /// Yields the requests received (stops waiting after 10 s without a connection).
    fn serve(responses: Vec<Vec<u8>>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        l.set_nonblocking(true).unwrap();
        let h = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for response in responses {
                let t = std::time::Instant::now();
                let mut s = loop {
                    match l.accept() {
                        Ok((s, _)) => break s,
                        Err(_) if t.elapsed().as_secs() < 10 => std::thread::sleep(std::time::Duration::from_millis(5)),
                        Err(_) => return requests,
                    }
                };
                s.set_nonblocking(false).unwrap();
                s.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
                let mut req = Vec::new();
                let mut buf = [0u8; 4096];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = s.write_all(&response);
                let _ = s.flush();
                requests.push(String::from_utf8_lossy(&req).into_owned());
            }
            requests
        });
        (port, h)
    }

    /// Host check for the loopback tests: only the test server is allowed.
    fn loopback_only(host: &str, _port: u16) -> Result<(), String> {
        if host == "127.0.0.1" {
            Ok(())
        } else {
            Err(format!("blocked {host}"))
        }
    }

    /// The whole request loop of `fetch` against the loopback server.
    fn fetch_loopback(responses: Vec<Vec<u8>>, path: &str) -> (Result<Vec<u8>, String>, Vec<String>) {
        let (port, server) = serve(responses);
        let session = loopback_session();
        let r = unsafe { fetch_with(&session, &format!("http://127.0.0.1:{port}{path}"), 1024, loopback_only) };
        (r, server.join().unwrap())
    }

    #[test]
    fn fragment_is_cut() {
        assert_eq!(without_fragment("https://x/a.png#f"), "https://x/a.png");
        assert_eq!(without_fragment("https://x/a.png?q=1#f#g"), "https://x/a.png?q=1");
        assert_eq!(without_fragment("https://x/a.png#"), "https://x/a.png");
        assert_eq!(without_fragment("https://x/a.png"), "https://x/a.png");
        // the host ends at "#", as in links::classify — "@" after it is not userinfo
        let p = crack(without_fragment("http://a.example#@127.0.0.1/x")).unwrap();
        assert_eq!((p.host.as_str(), object(&p).as_str()), ("a.example", "/"));
    }

    /// Regression: `https://x/a.png#f` requested `/a.png%23f`. Neither the first address nor
    /// a redirect address sends its fragment.
    #[test]
    fn fragment_is_never_sent() {
        let (r, reqs) = fetch_loopback(
            vec![response("HTTP/1.1 302 Found\r\nLocation: /b.png?q=1#loc-frag", b""), response("HTTP/1.1 200 OK", b"img")],
            "/a.png?x=1#frag",
        );
        assert_eq!(r.as_deref(), Ok(&b"img"[..]));
        assert_eq!(reqs.len(), 2);
        assert!(reqs[0].starts_with("GET /a.png?x=1 HTTP/1.1\r\n"), "{}", reqs[0]);
        assert!(reqs[1].starts_with("GET /b.png?q=1 HTTP/1.1\r\n"), "{}", reqs[1]);
        for req in &reqs {
            assert!(!req.contains("frag") && !req.contains("%23"), "{req}");
        }
    }

    /// Every redirect is resolved, re-checked and counted.
    #[test]
    fn redirects_are_checked_at_every_step() {
        // a relative address is resolved against the current one
        let (r, reqs) = fetch_loopback(vec![response("HTTP/1.1 301 Moved\r\nLocation: next.png", b""), response("HTTP/1.1 200 OK", b"ok")], "/dir/a.png");
        assert_eq!(r.as_deref(), Ok(&b"ok"[..]));
        assert!(reqs[1].starts_with("GET /dir/next.png HTTP/1.1\r\n"), "{}", reqs[1]);
        // a redirect to another host goes through the host check again, before connecting
        let (r, reqs) = fetch_loopback(vec![response("HTTP/1.1 302 Found\r\nLocation: http://10.0.0.1/x.png", b"")], "/");
        assert_eq!(r.unwrap_err(), "blocked 10.0.0.1");
        assert_eq!(reqs.len(), 1);
        // a scheme-relative redirect to another host, with a fragment
        let (r, reqs) = fetch_loopback(vec![response("HTTP/1.1 303 See Other\r\nLocation: //10.0.0.2/x.png#f", b"")], "/");
        assert_eq!(r.unwrap_err(), "blocked 10.0.0.2");
        assert_eq!(reqs.len(), 1);
        // at most MAX_REDIRECTS redirects
        let hops = (0..=MAX_REDIRECTS).map(|_| response("HTTP/1.1 307 Again\r\nLocation: /again", b"")).collect();
        let (r, reqs) = fetch_loopback(hops, "/");
        assert_eq!(r.unwrap_err(), "too many redirects");
        assert_eq!(reqs.len(), MAX_REDIRECTS + 1);
        // the first address is checked too: nothing is requested
        let session = loopback_session();
        let r = unsafe { fetch_with(&session, "http://localhost:1/a.png#f", 1024, loopback_only) };
        assert_eq!(r.unwrap_err(), "blocked localhost");
    }

    /// A WinHTTP session without any proxy (no WPAD lookups): only the loopback server is contacted.
    fn loopback_session() -> H {
        let ua = wide("vibe-md-viewer-test");
        let s = H(unsafe { WinHttpOpen(PCWSTR::from_raw(ua.as_ptr()), WINHTTP_ACCESS_TYPE_NO_PROXY, PCWSTR::null(), PCWSTR::null(), 0) });
        assert!(!s.0.is_null());
        unsafe {
            let _ = WinHttpSetTimeouts(s.0, 5000, 5000, 5000, 5000);
        }
        s
    }

    fn response(head: &str, body: &[u8]) -> Vec<u8> {
        let mut r = format!("{head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        r.extend_from_slice(body);
        r
    }

    /// Runs one request of `fetch` (`fetch_once`) against the loopback server.
    fn once(resp: Vec<u8>, path: &str, max: usize, deadline_s: u64) -> (Result<Step, String>, String) {
        let (port, server) = serve_once(resp);
        let session = loopback_session();
        let p = crack(&format!("http://127.0.0.1:{port}{path}")).unwrap();
        assert_eq!((p.host.as_str(), p.port), ("127.0.0.1", port));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(deadline_s);
        let r = unsafe { fetch_once(&session, &p, max, deadline) };
        (r, server.join().unwrap())
    }

    /// The request/response handling of one step, on a local loopback server (no internet,
    /// no proxy). The address checks are bypassed on purpose: `fetch` itself refuses 127.0.0.1.
    #[test]
    fn single_request_handling() {
        // 200: the body is returned; the request carries no cookies, credentials or Referer
        let (r, req) = once(response("HTTP/1.1 200 OK", b"\x89PNG-body"), "/img.png?x=1", 1024, 30);
        match r {
            Ok(Step::Body(b)) => assert_eq!(b, b"\x89PNG-body"),
            Ok(Step::Redirect(l)) => panic!("redirect {l}"),
            Err(e) => panic!("{e}"),
        }
        assert!(req.starts_with("GET /img.png?x=1 HTTP/1.1\r\n"), "{req}");
        let lower = req.to_ascii_lowercase();
        assert!(lower.contains("\r\naccept: image/*\r\n"), "{req}");
        assert!(lower.contains("\r\nuser-agent: vibe-md-viewer-test\r\n"), "{req}");
        for h in ["cookie:", "referer:", "authorization:"] {
            assert!(!lower.contains(h), "{h} in {req}");
        }
        // every redirect status hands the Location back to `fetch` (which re-checks it)
        for code in [301, 302, 303, 307, 308] {
            let (r, _) = once(response(&format!("HTTP/1.1 {code} Moved\r\nLocation: /next.png"), b""), "/a.png", 1024, 30);
            match r {
                Ok(Step::Redirect(l)) => assert_eq!(l, "/next.png", "{code}"),
                Ok(Step::Body(_)) => panic!("{code}: body"),
                Err(e) => panic!("{code}: {e}"),
            }
        }
        let err = |r: Result<Step, String>| match r {
            Err(e) => e,
            Ok(_) => panic!("expected an error"),
        };
        // a redirect without an address
        assert_eq!(err(once(response("HTTP/1.1 302 Found", b""), "/", 1024, 30).0), "redirect without an address");
        // a redirect address with a hidden control character
        assert_eq!(err(once(response("HTTP/1.1 302 Found\r\nLocation: /a\tb.png", b""), "/", 1024, 30).0), "suspicious redirect address");
        // any other status
        assert_eq!(err(once(response("HTTP/1.1 404 Not Found", b"nope"), "/", 1024, 30).0), "server responded 404");
        assert_eq!(err(once(response("HTTP/1.1 304 Not Modified", b""), "/", 1024, 30).0), "server responded 304");
        // the server closes the connection without answering
        assert!(err(once(Vec::new(), "/", 1024, 30).0).starts_with("network error"));
        // the size limit and the overall deadline
        assert_eq!(err(once(response("HTTP/1.1 200 OK", &[7u8; 100]), "/", 10, 30).0), "image is too large");
        assert_eq!(err(once(response("HTTP/1.1 200 OK", b"data"), "/", 1024, 0).0), "the server responds too slowly");
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
