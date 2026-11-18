//! Images: path resolution under the security policy, decoding in background threads.
//!
//! Untrusted image bytes are parsed only by safe Rust decoders (the `image` crate:
//! png, zune-jpeg, gif, image-webp…) with size limits (protection against "bombs").
//! RichEdit receives a clean PNG/JPEG of the needed size that we re-encoded ourselves,
//! so the Windows system codecs never see the original file.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use image::{DynamicImage, GenericImageView, ImageEncoder};

use crate::links;

pub const MAX_LOCAL_BYTES: u64 = 60 * 1024 * 1024;
const MAX_DIM: u32 = 16_384;
/// Limit for width/height taken from the document (CSS pixels).
const MAX_WANT: u32 = 10_000;
/// Size limit of the re-encoded image (pixels).
const MAX_OUT_PIXELS: u64 = 16_000_000;

#[derive(Debug, Clone)]
pub enum Source {
    Local(PathBuf),
    Remote(String),
    Data(Vec<u8>),
}

const IMG_EXT: &[&str] = &["png", "jpg", "jpeg", "jfif", "gif", "bmp", "webp", "ico"];

/// Parse an image src. Error = the image will not be loaded (alt is shown instead).
pub fn resolve(src: &str, base_dir: Option<&Path>) -> Result<Source, String> {
    let s = src.trim();
    if s.is_empty() {
        return Err(tr!("empty address", "пустой адрес").into());
    }
    if links::has_hidden_controls(s) {
        return Err(tr!("the address contains control characters", "адрес содержит управляющие символы").into());
    }
    match links::scheme_of(s).as_deref() {
        Some("http") | Some("https") => match links::classify(s, None) {
            links::LinkAction::Web { url, host, .. } => {
                if crate::net::is_private_host(&host) {
                    Err(tr!("local network addresses are not allowed", "адреса локальной сети запрещены").into())
                } else {
                    Ok(Source::Remote(url))
                }
            }
            links::LinkAction::Blocked(r) => Err(r),
            _ => Err(tr!("invalid address", "некорректный адрес").into()),
        },
        Some("data") => parse_data_uri(s).map(Source::Data),
        Some("file") => match links::classify(s, None) {
            links::LinkAction::LocalFile(p) => check_local(p),
            links::LinkAction::Blocked(r) => Err(r),
            _ => Err(tr!("unsupported file", "неподдерживаемый файл").into()),
        },
        Some(other) => Err(trf!("the “{other}:” protocol is not allowed", "протокол «{other}:» запрещён")),
        None => {
            let (p, _) = links::resolve_local(s, base_dir)?;
            check_local(p)
        }
    }
}

fn check_local(p: PathBuf) -> Result<Source, String> {
    let ext = p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
    if ext == "svg" || ext == "svgz" {
        return Err(tr!("SVG is not supported yet", "SVG пока не поддерживается").into());
    }
    if !IMG_EXT.contains(&ext.as_str()) {
        return Err(trf!("a .{ext} file is not an image", "тип файла .{ext} не является картинкой"));
    }
    Ok(Source::Local(p))
}

fn parse_data_uri(s: &str) -> Result<Vec<u8>, String> {
    let rest = &s[5..];
    let comma = rest.find(',').ok_or(tr!("invalid data:", "некорректный data:"))?;
    let meta = rest[..comma].to_ascii_lowercase();
    if !meta.starts_with("image/") || meta.starts_with("image/svg") {
        return Err(tr!("data: is allowed only for raster images", "data: разрешён только для растровых картинок").into());
    }
    if !meta.ends_with(";base64") {
        return Err(tr!("data: supports only base64", "data: поддерживается только base64").into());
    }
    let data = &rest[comma + 1..];
    if data.len() > 40 * 1024 * 1024 {
        return Err(tr!("image is too large", "слишком большая картинка").into());
    }
    base64_decode(data).ok_or_else(|| tr!("corrupt base64", "битый base64").into())
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for &c in s.as_bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[derive(Debug, Clone)]
pub struct Job {
    pub gen: u64,
    pub id: usize,
    pub source: Source,
    /// Maximum width in device pixels.
    pub max_w: u32,
    pub want_w: Option<u32>,
    pub want_h: Option<u32>,
    pub dpi: u32,
}

pub struct Done {
    pub gen: u64,
    pub id: usize,
    pub result: Result<String, String>,
}

pub struct Pool {
    tx: Option<Sender<Job>>,
    results: Arc<Mutex<Vec<Done>>>,
    notify: isize,
    msg: u32,
    /// Current render "generation": jobs from previous renders are skipped ("Code" mode
    /// redraws the preview often — stale images must not pile up in the queue).
    current_gen: Arc<std::sync::atomic::AtomicU64>,
}

impl Pool {
    pub fn new(notify_hwnd: isize, msg: u32) -> Pool {
        Pool {
            tx: None,
            results: Arc::new(Mutex::new(Vec::new())),
            notify: notify_hwnd,
            msg,
            current_gen: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    pub fn set_gen(&self, gen: u64) {
        self.current_gen.store(gen, std::sync::atomic::Ordering::Relaxed);
    }

    /// Threads are created lazily: a document without images pays nothing for them.
    fn ensure(&mut self) -> &Sender<Job> {
        if self.tx.is_none() {
            let (tx, rx) = channel::<Job>();
            let rx = Arc::new(Mutex::new(rx));
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(2, 4);
            for _ in 0..threads {
                let rx: Arc<Mutex<Receiver<Job>>> = rx.clone();
                let results = self.results.clone();
                let hwnd = self.notify;
                let msg = self.msg;
                let current = self.current_gen.clone();
                let _ = std::thread::Builder::new().stack_size(1 << 20).spawn(move || loop {
                    let job = {
                        let Ok(guard) = rx.lock() else { return };
                        match guard.recv() {
                            Ok(j) => j,
                            Err(_) => return,
                        }
                    };
                    if job.gen < current.load(std::sync::atomic::Ordering::Relaxed) {
                        continue; // stale — the document has already been redrawn
                    }
                    let result = std::panic::catch_unwind(|| process(&job))
                        .unwrap_or_else(|_| Err(tr!("internal decoder error", "внутренняя ошибка декодера").into()));
                    if let Ok(mut r) = results.lock() {
                        r.push(Done { gen: job.gen, id: job.id, result });
                    }
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                            Some(windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void)),
                            msg,
                            windows::Win32::Foundation::WPARAM(0),
                            windows::Win32::Foundation::LPARAM(0),
                        );
                    }
                });
            }
            self.tx = Some(tx);
        }
        self.tx.as_ref().unwrap()
    }

    pub fn submit(&mut self, job: Job) {
        let _ = self.ensure().send(job);
    }

    pub fn take(&self) -> Vec<Done> {
        self.results.lock().map(|mut r| std::mem::take(&mut *r)).unwrap_or_default()
    }
}

fn read_local(p: &Path) -> Result<Vec<u8>, String> {
    let m = std::fs::metadata(p).map_err(|_| tr!("file not found", "файл не найден").to_string())?;
    if !m.is_file() {
        return Err(tr!("not a file", "это не файл").into());
    }
    if m.len() > MAX_LOCAL_BYTES {
        return Err(tr!("file is too large", "файл слишком большой").into());
    }
    std::fs::read(p).map_err(|e| trf!("could not read: {e}", "не удалось прочитать: {e}"))
}

fn process(job: &Job) -> Result<String, String> {
    let bytes = match &job.source {
        Source::Local(p) => {
            // Checked at read time: no symbolic links in the path (they may lead to the network).
            if crate::links::has_symlink_component(p) {
                return Err(tr!("the path contains a symbolic link", "путь содержит символическую ссылку").into());
            }
            read_local(p)?
        }
        Source::Remote(u) => crate::net::fetch(u, crate::net::MAX_REMOTE_BYTES)?,
        Source::Data(d) => return to_pict(d, job),
    };
    to_pict(&bytes, job)
}

fn looks_like_svg(b: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&b[..b.len().min(512)]).to_ascii_lowercase();
    head.contains("<svg")
}

/// The intermediate buffer of resize_exact is f32 RGBA (16 bytes per pixel) sized "source
/// width × new height": 16384×10000 is 2.6 GB, and four such images in the threads crash
/// the process. Above this pixel count we shrink via "thumbnail": integer averaging
/// without an intermediate buffer.
const MAX_RESIZE_TMP: u64 = 8 * 1024 * 1024;

fn use_thumbnail(w: u32, h: u32, pw: u32, ph: u32) -> bool {
    (w / pw.max(1) >= 2 && h / ph.max(1) >= 2) || (w as u64 * ph as u64).max(pw as u64 * h as u64) > MAX_RESIZE_TMP
}

/// Decode → shrink to fit the width → re-encode → a {\pict ...} group for RTF.
pub fn to_pict(bytes: &[u8], job: &Job) -> Result<String, String> {
    let fmt = match image::guess_format(bytes) {
        Ok(f) => f,
        Err(_) => {
            return Err(if looks_like_svg(bytes) {
                tr!("SVG is not supported yet", "SVG пока не поддерживается").into()
            } else {
                tr!("unknown image format", "неизвестный формат картинки").into()
            })
        }
    };
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), fmt);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIM);
    limits.max_image_height = Some(MAX_DIM);
    // Up to 256 MB per image (≈ 8000×8000 RGBA): 4 threads cannot exhaust memory with "bombs".
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| trf!("could not decode: {e}", "не удалось декодировать: {e}"))?;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(tr!("empty image", "пустая картинка").into());
    }
    let dpi = job.dpi.clamp(96, 960);
    let k = dpi as f64 / 96.0;
    // Desired size in CSS pixels; width/height values from the document are capped.
    let want_w = job.want_w.filter(|&v| v > 0).map(|v| v.min(MAX_WANT));
    let want_h = job.want_h.filter(|&v| v > 0).map(|v| v.min(MAX_WANT));
    let (cw, ch) = match (want_w, want_h) {
        (Some(a), Some(b)) => (a as f64, b as f64),
        (Some(a), None) => (a as f64, h as f64 * a as f64 / w as f64),
        (None, Some(b)) => (w as f64 * b as f64 / h as f64, b as f64),
        _ => (w as f64, h as f64),
    };
    let mut dw = cw * k;
    let mut dh = ch * k;
    let maxw = job.max_w.max(32) as f64;
    if dw > maxw {
        dh = dh * maxw / dw;
        dw = maxw;
    }
    // The display height is capped too (≈ 20 screens).
    let maxh = MAX_WANT as f64 * k;
    if dh > maxh {
        dw = dw * maxh / dh;
        dh = maxh;
    }
    let dw = dw.round().clamp(1.0, 1e6) as u32;
    let dh = dh.round().clamp(1.0, 1e6) as u32;
    // Pixels are only ever reduced (each side separately); RichEdit stretches them when drawing.
    // Otherwise width=100 height=4000000000 would force allocating terabytes of memory.
    let mut pw = dw.min(w);
    let mut ph = dh.min(h);
    let area = pw as u64 * ph as u64;
    if area > MAX_OUT_PIXELS {
        let s = (MAX_OUT_PIXELS as f64 / area as f64).sqrt();
        pw = ((pw as f64 * s) as u32).max(1);
        ph = ((ph as f64 * s) as u32).max(1);
    }
    let img: DynamicImage = if (pw, ph) != (w, h) {
        if use_thumbnail(w, h, pw, ph) {
            img.thumbnail_exact(pw, ph)
        } else {
            img.resize_exact(pw, ph, image::imageops::FilterType::Triangle)
        }
    } else {
        img
    };
    let mut out = Vec::new();
    let blip = if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        image::codecs::png::PngEncoder::new_with_quality(
            &mut out,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::Adaptive,
        )
        .write_image(rgba.as_raw(), pw, ph, image::ExtendedColorType::Rgba8)
        .map_err(|e| trf!("encoding error: {e}", "ошибка кодирования: {e}"))?;
        "pngblip"
    } else {
        let rgb = img.to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .write_image(rgb.as_raw(), pw, ph, image::ExtendedColorType::Rgb8)
            .map_err(|e| trf!("encoding error: {e}", "ошибка кодирования: {e}"))?;
        "jpegblip"
    };
    let tw = (dw as i64 * 1440 / dpi as i64).max(15);
    let th = (dh as i64 * 1440 / dpi as i64).max(15);
    let mut rtf = String::with_capacity(out.len() * 2 + 128);
    rtf.push_str(&format!("{{\\pict\\{blip}\\picw{pw}\\pich{ph}\\picwgoal{tw}\\pichgoal{th} "));
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for b in out {
        rtf.push(HEX[(b >> 4) as usize] as char);
        rtf.push(HEX[(b & 15) as usize] as char);
    }
    rtf.push('}');
    Ok(rtf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_policy() {
        let base = PathBuf::from("D:\\docs");
        assert!(matches!(resolve("img/a.png", Some(&base)), Ok(Source::Local(_))));
        assert!(resolve("\\\\evil\\share\\a.png", Some(&base)).is_err());
        assert!(resolve("//evil/share/a.png", Some(&base)).is_err());
        assert!(resolve("file://evil/share/a.png", Some(&base)).is_err());
        assert!(resolve("a.exe", Some(&base)).is_err());
        assert!(resolve("http://192.168.0.1/reboot", Some(&base)).is_err());
        assert!(resolve("javascript:alert(1)", Some(&base)).is_err());
        assert!(resolve("x.svg", Some(&base)).is_err());
        assert!(matches!(resolve("https://example.com/a.png", None), Ok(Source::Remote(_))));
        assert!(matches!(resolve("data:image/png;base64,iVBORw0KGgo=", None), Ok(Source::Data(_))));
        assert!(resolve("data:text/html;base64,PHNjcmlwdD4=", None).is_err());
    }

    #[test]
    fn decode_tiny_png() {
        // 1x1 red PNG
        let mut png = Vec::new();
        let img = image::RgbaImage::from_pixel(4, 2, image::Rgba([255, 0, 0, 128]));
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(img.as_raw(), 4, 2, image::ExtendedColorType::Rgba8)
            .unwrap();
        let job = Job { gen: 0, id: 0, source: Source::Data(vec![]), max_w: 800, want_w: None, want_h: None, dpi: 96 };
        let rtf = to_pict(&png, &job).unwrap();
        assert!(rtf.starts_with("{\\pict\\pngblip\\picw4\\pich2"));
    }

    /// Audit finding: width=100 height=4000000000 caused a terabyte allocation → crash.
    #[test]
    fn huge_requested_size_is_safe() {
        let mut png = Vec::new();
        let img = image::RgbaImage::from_pixel(200, 1, image::Rgba([0, 0, 255, 255]));
        image::codecs::png::PngEncoder::new(&mut png).write_image(img.as_raw(), 200, 1, image::ExtendedColorType::Rgba8).unwrap();
        for (w, h) in [(Some(100), Some(4_000_000_000)), (Some(4_000_000_000), Some(4_000_000_000)), (None, Some(u32::MAX)), (Some(u32::MAX), None)] {
            let job = Job { gen: 0, id: 0, source: Source::Data(vec![]), max_w: 800, want_w: w, want_h: h, dpi: 96 };
            let t = std::time::Instant::now();
            let rtf = to_pict(&png, &job).unwrap();
            assert!(rtf.len() < 2_000_000 && t.elapsed().as_secs() < 5, "{w:?}x{h:?}");
        }
    }

    /// Audit finding 0.2.0: squeezing one side narrowly (width=1 height=10000 for a
    /// 16384×16000 image) went through resize_exact with a multi-gigabyte intermediate buffer.
    #[test]
    fn one_sided_shrink_has_bounded_memory() {
        assert!(use_thumbnail(16384, 16000, 1, 10000), "a 16384×10000×16-byte buffer is not allowed");
        assert!(use_thumbnail(4000, 2000, 1000, 500), "ordinary strong downscale");
        assert!(!use_thumbnail(1000, 500, 900, 450), "slight downscale uses the quality filter");
        let mut png = Vec::new();
        let img = image::GrayImage::from_pixel(4096, 2100, image::Luma([128]));
        image::codecs::png::PngEncoder::new(&mut png).write_image(img.as_raw(), 4096, 2100, image::ExtendedColorType::L8).unwrap();
        let job = Job { gen: 0, id: 0, source: Source::Data(vec![]), max_w: 800, want_w: Some(1), want_h: Some(2100), dpi: 96 };
        let t = std::time::Instant::now();
        assert!(to_pict(&png, &job).is_ok());
        assert!(t.elapsed().as_secs() < 5);
    }

    #[test]
    fn garbage_is_rejected() {
        let job = Job { gen: 0, id: 0, source: Source::Data(vec![]), max_w: 800, want_w: None, want_h: None, dpi: 96 };
        assert!(to_pict(b"\x89PNG\r\n\x1a\nGARBAGE", &job).is_err());
        assert!(to_pict(b"<svg xmlns=...>", &job).is_err());
    }
}
