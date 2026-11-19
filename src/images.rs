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
            assert!(rtf.len() < 2_000_000 && t.elapsed().as_secs() < if cfg!(debug_assertions) { 60 } else { 5 }, "{w:?}x{h:?}");
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
        assert!(t.elapsed().as_secs() < if cfg!(debug_assertions) { 60 } else { 5 });
    }

    #[test]
    fn garbage_is_rejected() {
        let job = Job { gen: 0, id: 0, source: Source::Data(vec![]), max_w: 800, want_w: None, want_h: None, dpi: 96 };
        assert!(to_pict(b"\x89PNG\r\n\x1a\nGARBAGE", &job).is_err());
        assert!(to_pict(b"<svg xmlns=...>", &job).is_err());
    }

    fn job(max_w: u32, want_w: Option<u32>, want_h: Option<u32>, dpi: u32) -> Job {
        Job { gen: 0, id: 0, source: Source::Data(vec![]), max_w, want_w, want_h, dpi }
    }

    fn encode(img: &DynamicImage, fmt: image::ImageFormat) -> Vec<u8> {
        let mut c = Cursor::new(Vec::new());
        img.write_to(&mut c, fmt).unwrap_or_else(|e| panic!("{fmt:?}: {e}"));
        c.into_inner()
    }

    fn rgb(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 7) as u8, (y * 13) as u8, 200])))
    }

    fn rgba(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 7) as u8, (y * 13) as u8, 50, 128])))
    }

    /// The parts of `{\pict\<blip>\picwW\pichH\picwgoalA\pichgoalB <hex>}`.
    struct Pict {
        blip: String,
        w: u32,
        h: u32,
        wgoal: i64,
        hgoal: i64,
        bytes: Vec<u8>,
    }

    fn parse_pict(rtf: &str) -> Pict {
        let body = rtf.strip_prefix("{\\pict\\").and_then(|r| r.strip_suffix('}')).unwrap_or_else(|| panic!("not a pict group: {}", &rtf[..rtf.len().min(80)]));
        let (head, hex) = body.split_once(' ').unwrap();
        let num = |key: &str| -> i64 {
            let i = head.find(key).unwrap_or_else(|| panic!("{key} in {head}")) + key.len();
            head[i..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap()
        };
        assert!(hex.len() % 2 == 0 && hex.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "lowercase hex payload");
        let bytes = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        Pict {
            blip: head.split('\\').next().unwrap().to_string(),
            w: num("\\picw") as u32,
            h: num("\\pich") as u32,
            wgoal: num("\\picwgoal"),
            hgoal: num("\\pichgoal"),
            bytes,
        }
    }

    /// Every supported format decodes; the result is our own PNG (with alpha) or JPEG
    /// (without), whose real pixel size matches \picw/\pich.
    #[test]
    fn to_pict_decodes_every_format() {
        use image::ImageFormat as F;
        let cases: Vec<(&str, Vec<u8>, &str)> = vec![
            ("png rgba", encode(&rgba(6, 4), F::Png), "pngblip"),
            ("png rgb", encode(&rgb(6, 4), F::Png), "jpegblip"),
            ("png gray", encode(&DynamicImage::ImageLuma8(image::GrayImage::from_pixel(6, 4, image::Luma([90]))), F::Png), "jpegblip"),
            ("png gray+alpha", encode(&DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_pixel(6, 4, image::LumaA([90, 10]))), F::Png), "pngblip"),
            ("jpeg", encode(&rgb(6, 4), F::Jpeg), "jpegblip"),
            ("gif", encode(&rgba(6, 4), F::Gif), "pngblip"),
            ("bmp rgb", encode(&rgb(6, 4), F::Bmp), "jpegblip"),
            ("webp rgb", encode(&rgb(6, 4), F::WebP), "jpegblip"),
            ("webp rgba", encode(&rgba(6, 4), F::WebP), "pngblip"),
            ("ico", encode(&rgba(6, 4), F::Ico), "pngblip"),
        ];
        for (name, bytes, blip) in cases {
            let rtf = to_pict(&bytes, &job(800, None, None, 96)).unwrap_or_else(|e| panic!("{name}: {e}"));
            let p = parse_pict(&rtf);
            assert_eq!(p.blip, blip, "{name}");
            assert_eq!((p.w, p.h, p.wgoal, p.hgoal), (6, 4, 90, 60), "{name}: 1 px = 15 twips at 96 dpi");
            let magic: &[u8] = if blip == "pngblip" { b"\x89PNG" } else { b"\xFF\xD8\xFF" };
            assert!(p.bytes.starts_with(magic), "{name}: re-encoded payload");
            let back = image::load_from_memory(&p.bytes).unwrap();
            assert_eq!(back.dimensions(), (6, 4), "{name}");
            assert_eq!(back.color().has_alpha(), blip == "pngblip", "{name}");
        }
    }

    #[test]
    fn unknown_and_svg_bytes() {
        let j = job(800, None, None, 96);
        assert_eq!(to_pict(b"hello world, not an image", &j).unwrap_err(), "unknown image format");
        assert_eq!(to_pict(b"", &j).unwrap_err(), "unknown image format");
        assert_eq!(to_pict(b"<?xml version=\"1.0\"?>\n<SVG width=\"1\"/>", &j).unwrap_err(), "SVG is not supported yet");
        // "<svg" beyond the first 512 bytes is not looked at
        let late = format!("{}<svg/>", " ".repeat(600));
        assert_eq!(to_pict(late.as_bytes(), &j).unwrap_err(), "unknown image format");
        assert!(to_pict(b"\x89PNG\r\n\x1a\n", &j).unwrap_err().starts_with("could not decode"));
    }

    /// Display size in twips (1 CSS px = 15 twips), pixels only ever reduced, DPI scaling.
    #[test]
    fn sizes_and_dpi_scaling() {
        let img = encode(&rgb(10, 5), image::ImageFormat::Png);
        let size = |j: Job| {
            let p = parse_pict(&to_pict(&img, &j).unwrap());
            let real = image::load_from_memory(&p.bytes).unwrap().dimensions();
            assert_eq!(real, (p.w, p.h), "payload pixels match \\picw/\\pich");
            (p.w, p.h, p.wgoal, p.hgoal)
        };
        // natural size
        assert_eq!(size(job(800, None, None, 96)), (10, 5, 150, 75));
        // 200% DPI: the same size in twips, the pixels are not enlarged
        assert_eq!(size(job(800, None, None, 192)), (10, 5, 150, 75));
        // DPI is clamped to 96..=960
        assert_eq!(size(job(800, None, None, 0)), (10, 5, 150, 75));
        assert_eq!(size(job(800, None, None, 100_000)), (10, 5, 150, 75));
        // width only / height only keep the aspect ratio
        assert_eq!(size(job(800, Some(4), None, 96)), (4, 2, 60, 30));
        assert_eq!(size(job(800, None, Some(10), 96)), (10, 5, 300, 150), "enlarged only by the display size");
        // both given: the aspect ratio follows the document
        assert_eq!(size(job(800, Some(8), Some(1), 96)), (8, 1, 120, 15));
        // zero means "not given"
        assert_eq!(size(job(800, Some(0), Some(0), 96)), (10, 5, 150, 75));
        // the column width limits the display width (in device pixels)
        assert_eq!(size(job(6, None, None, 96)).0, 10, "max_w below 32 is raised to 32");
        let big = encode(&rgb(100, 50), image::ImageFormat::Png);
        let p = parse_pict(&to_pict(&big, &job(40, None, None, 96)).unwrap());
        assert_eq!((p.w, p.h, p.wgoal, p.hgoal), (40, 20, 600, 300), "strong downscale (thumbnail)");
        let p = parse_pict(&to_pict(&big, &job(90, None, None, 96)).unwrap());
        assert_eq!((p.w, p.h), (90, 45), "slight downscale (filter)");
        let p = parse_pict(&to_pict(&big, &job(90, None, None, 192)).unwrap());
        assert_eq!((p.w, p.h, p.wgoal, p.hgoal), (90, 45, 675, 337), "at 200% the column holds 45 CSS px");
        // a tiny display size still gets at least 15 twips per side
        let wide = encode(&rgb(200, 1), image::ImageFormat::Png);
        let p = parse_pict(&to_pict(&wide, &job(800, Some(1), None, 192)).unwrap());
        assert_eq!((p.wgoal, p.hgoal), (15, 15));
    }

    /// The display height is capped at ≈ 20 screens (10000 CSS px), keeping the aspect ratio.
    #[test]
    fn display_height_is_capped() {
        let tall = encode(&rgb(1, 2), image::ImageFormat::Png);
        // width 10000 → height 20000 → capped to 10000, width halved
        let p = parse_pict(&to_pict(&tall, &job(1_000_000, Some(10_000), None, 96)).unwrap());
        assert_eq!((p.w, p.h, p.wgoal, p.hgoal), (1, 2, 75_000, 150_000));
        // at 200% the cap is in CSS pixels too
        let p = parse_pict(&to_pict(&tall, &job(1_000_000, Some(10_000), None, 192)).unwrap());
        assert_eq!((p.wgoal, p.hgoal), (75_000, 150_000));
    }

    /// A source above 16 Mpix is re-encoded at ≤ 16 Mpix, keeping the aspect ratio.
    #[test]
    fn output_pixels_are_capped() {
        let src = DynamicImage::ImageLuma8(image::GrayImage::from_pixel(4096, 4000, image::Luma([77])));
        let png = encode(&src, image::ImageFormat::Png);
        let t = std::time::Instant::now();
        let p = parse_pict(&to_pict(&png, &job(100_000, None, None, 96)).unwrap());
        assert!((p.w as u64) * (p.h as u64) <= MAX_OUT_PIXELS, "{}x{}", p.w, p.h);
        assert!(p.w < 4096 && p.h < 4000 && p.w > 4000 && p.h > 3900, "{}x{}", p.w, p.h);
        // the display size is not reduced, only the pixels
        assert_eq!((p.wgoal, p.hgoal), (4096 * 15, 4000 * 15));
        assert_eq!(image::load_from_memory(&p.bytes).unwrap().dimensions(), (p.w, p.h));
        assert!(t.elapsed().as_secs() < if cfg!(debug_assertions) { 60 } else { 10 });
    }

    #[test]
    fn data_uri_parsing() {
        // "hello" in standard base64; whitespace and padding are ignored
        assert_eq!(parse_data_uri("data:image/png;base64,aGVsbG8="), Ok(b"hello".to_vec()));
        assert_eq!(parse_data_uri("data:image/png;base64,aGVs\r\n bG8\t="), Ok(b"hello".to_vec()));
        assert_eq!(parse_data_uri("data:IMAGE/GIF;BASE64,aGVsbG8"), Ok(b"hello".to_vec()));
        assert_eq!(parse_data_uri("data:image/png;base64,"), Ok(vec![]));
        // URL-safe alphabet decodes like the standard one
        assert_eq!(base64_decode("+/8="), Some(vec![0xFB, 0xFF]));
        assert_eq!(base64_decode("-_8="), Some(vec![0xFB, 0xFF]));
        assert_eq!(base64_decode("AAECAwQFBgcICQ"), Some((0..10).collect()));
        assert_eq!(parse_data_uri("data:image/png;base64").unwrap_err(), "invalid data:");
        assert_eq!(parse_data_uri("data:image/png,raw").unwrap_err(), "data: supports only base64");
        assert_eq!(parse_data_uri("data:image/png;charset=x,raw").unwrap_err(), "data: supports only base64");
        for s in ["data:text/plain;base64,aGk=", "data:;base64,aGk=", "data:image/svg+xml;base64,PHN2Zz4=", "data:image/SVG;base64,aGk=", "data:,x"] {
            assert_eq!(parse_data_uri(s).unwrap_err(), "data: is allowed only for raster images", "{s}");
        }
        for s in ["data:image/png;base64,aGk$", "data:image/png;base64,a%3D", "data:image/png;base64,aGk=\u{0}", "data:image/png;base64,ПР"] {
            assert_eq!(parse_data_uri(s).unwrap_err(), "corrupt base64", "{s:?}");
        }
        // the size is checked before decoding
        let huge = format!("data:image/png;base64,{}", "A".repeat(40 * 1024 * 1024 + 1));
        assert_eq!(parse_data_uri(&huge).unwrap_err(), "image is too large");
    }

    #[test]
    fn resolve_every_source_kind() {
        let base = PathBuf::from("D:\\docs");
        let r = |s: &str| resolve(s, Some(&base));
        // empty and hidden characters
        assert_eq!(r("").unwrap_err(), "empty address");
        assert_eq!(r("  \t ").unwrap_err(), "empty address");
        assert_eq!(r("img/a\u{202E}gnp.exe").unwrap_err(), "the address contains control characters");
        // web: normalised address, private hosts and malformed addresses refused
        match r(" https://example.com/a b.png ") {
            Ok(Source::Remote(u)) => assert_eq!(u, "https://example.com/a%20b.png"),
            x => panic!("{x:?}"),
        }
        for s in ["http://localhost/a.png", "https://10.0.0.1/a.png", "http://[::1]/a.png", "http://router/a.png", "https://127.1/a.png", "http://nas.local/a.png"] {
            assert_eq!(r(s).unwrap_err(), "local network addresses are not allowed", "{s}");
        }
        for s in ["https:///a.png", "http:a.png", "https://exa mple.com/a.png"] {
            assert!(r(s).is_err(), "{s}");
        }
        // data:
        assert!(matches!(r("data:image/gif;base64,R0lGODlh"), Ok(Source::Data(d)) if d.starts_with(b"GIF89a")));
        assert!(r("data:image/png,raw").is_err());
        // file: URLs — only local image files
        match r("file:///C:/img/a.PNG") {
            Ok(Source::Local(p)) => assert_eq!(p, PathBuf::from("C:\\img\\a.PNG")),
            x => panic!("{x:?}"),
        }
        assert_eq!(r("file:///C:/notes/a.md").unwrap_err(), "unsupported file");
        assert!(r("file:///C:/x/a.exe").unwrap_err().contains(".exe"));
        assert!(r("file://server/share/a.png").unwrap_err().contains("Network paths"));
        assert_eq!(r("file:///C:/docs/a.txt").unwrap_err(), "a .txt file is not an image");
        // any other protocol
        assert_eq!(r("ftp://example.com/a.png").unwrap_err(), "the “ftp:” protocol is not allowed");
        assert_eq!(r("ms-appx:///a.png").unwrap_err(), "the “ms-appx:” protocol is not allowed");
        // local paths: every image extension, case-insensitive
        for ext in ["png", "JPG", "jpeg", "jfif", "gif", "bmp", "WebP", "ico"] {
            match r(&format!("img/a.{ext}")) {
                Ok(Source::Local(p)) => assert_eq!(p, PathBuf::from(format!("D:\\docs\\img\\a.{ext}"))),
                x => panic!("{ext}: {x:?}"),
            }
        }
        assert_eq!(r("a.svgz").unwrap_err(), "SVG is not supported yet");
        assert_eq!(r("a.SVG").unwrap_err(), "SVG is not supported yet");
        assert_eq!(r("a.tiff").unwrap_err(), "a .tiff file is not an image");
        assert_eq!(r("noext").unwrap_err(), "a . file is not an image");
        assert!(r("a.png:stream").is_err());
        assert!(r("con.png").unwrap_err().contains("device names"));
        assert!(r("%5C%5Cevil%5Cs%5Ca.png").unwrap_err().contains("Network paths"));
        assert_eq!(resolve("a.png", None).unwrap_err(), "No document folder");
    }

    /// A unique scratch folder inside the project (never the system temp folder).
    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".toolcache").join("tmp").join(format!("images-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn process_reads_each_source() {
        let dir = scratch("process");
        let png = encode(&rgba(3, 2), image::ImageFormat::Png);
        let file = dir.join("a.png");
        std::fs::write(&file, &png).unwrap();
        let run = |source: Source| process(&Job { source, ..job(800, None, None, 96) });
        // a local file and the same bytes from data: give the same picture
        let local = run(Source::Local(file.clone())).unwrap();
        assert_eq!(local, run(Source::Data(png.clone())).unwrap());
        assert!(local.starts_with("{\\pict\\pngblip\\picw3\\pich2"));
        // missing, a folder, too large
        assert_eq!(run(Source::Local(dir.join("missing.png"))).unwrap_err(), "file not found");
        assert_eq!(run(Source::Local(dir.clone())).unwrap_err(), "not a file");
        let big = dir.join("big.png");
        std::fs::File::create(&big).unwrap().set_len(MAX_LOCAL_BYTES + 1).unwrap();
        assert_eq!(run(Source::Local(big)).unwrap_err(), "file is too large");
        // a file that is not an image
        let txt = dir.join("fake.png");
        std::fs::write(&txt, b"just text").unwrap();
        assert_eq!(run(Source::Local(txt)).unwrap_err(), "unknown image format");
        // a remote address of the local network is refused before any connection is made
        assert_eq!(run(Source::Remote("http://127.0.0.1/a.png".into())).unwrap_err(), "local network addresses are not allowed");
        assert_eq!(run(Source::Remote("ftp://example.com/a.png".into())).unwrap_err(), "only http and https are allowed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Background decoding: results arrive through `take`, stale generations are skipped.
    #[test]
    fn pool_decodes_in_background_and_skips_stale_jobs() {
        let png = encode(&rgb(4, 4), image::ImageFormat::Png);
        // hwnd 0: the notification is posted to the worker thread's own queue and ignored
        let mut pool = Pool::new(0, 0x8000 + 1);
        assert!(pool.take().is_empty());
        let wait_for = |pool: &Pool, n: usize| {
            let mut got = Vec::new();
            let t = std::time::Instant::now();
            while got.len() < n && t.elapsed().as_secs() < 30 {
                got.extend(pool.take());
                std::thread::yield_now();
            }
            got
        };
        pool.set_gen(5);
        // a job from an older render is skipped, a current one is decoded
        pool.submit(Job { gen: 4, id: 1, source: Source::Data(png.clone()), ..job(800, None, None, 96) });
        pool.submit(Job { gen: 5, id: 2, source: Source::Data(png.clone()), ..job(800, None, None, 96) });
        pool.submit(Job { gen: 6, id: 3, source: Source::Data(b"not an image".to_vec()), ..job(800, None, None, 96) });
        let mut done = wait_for(&pool, 2);
        done.sort_by_key(|d| d.id);
        assert_eq!(done.iter().map(|d| (d.gen, d.id)).collect::<Vec<_>>(), vec![(5, 2), (6, 3)]);
        assert!(done[0].result.as_ref().unwrap().starts_with("{\\pict\\jpegblip\\picw4\\pich4"));
        assert_eq!(done[1].result.as_ref().unwrap_err(), "unknown image format");
        // the skipped job never shows up; results are handed out only once
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(pool.take().is_empty());
    }
}
