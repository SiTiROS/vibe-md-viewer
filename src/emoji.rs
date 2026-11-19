//! Color emoji in the view. RichEdit draws through GDI, and GDI has no color fonts: emoji come
//! out as monochrome outlines (render.rs puts them into Segoe UI Emoji so that at least the
//! glyph exists). The view paints the color version on top: DirectWrite + Direct2D render each
//! emoji cluster once (Segoe UI Emoji, color layers) into a premultiplied 32-bit bitmap, and a
//! paint only covers RichEdit's outline and blends the cached bitmap.
//!
//! d2d1.dll and dwrite.dll are delay-loaded (build.rs): a document without emoji never loads them.

use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::w;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;

use crate::theme;

/// A rendered emoji cluster.
struct Glyph {
    bmp: HBITMAP,
    w: i32,
    h: i32,
    /// Baseline and the start of the advance inside the bitmap, the advance width (DirectWrite).
    baseline: i32,
    left: i32,
    advance: f32,
    /// Bounds of the non-transparent pixels inside the bitmap.
    ink: RECT,
}

impl Drop for Glyph {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.bmp.into());
        }
    }
}

/// Rendered clusters kept at most (each is a small bitmap); then the cache starts over.
const MAX_CACHE: usize = 1024;

pub struct Painter {
    dw: IDWriteFactory,
    rt: ID2D1DCRenderTarget,
    formats: HashMap<u32, IDWriteTextFormat>,
    cache: HashMap<(Box<[u16]>, u32, u32), Option<Glyph>>,
}

impl Painter {
    /// None if Direct2D/DirectWrite are unavailable: the view then keeps RichEdit's outlines.
    pub fn new() -> Option<Painter> {
        unsafe {
            let f: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = f.CreateDCRenderTarget(&props).ok()?;
            // ClearType needs an opaque target; on a transparent one — grayscale.
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            Some(Painter { dw, rt, formats: HashMap::new(), cache: HashMap::new() })
        }
    }

    /// Paints the emoji cluster `text` over RichEdit's monochrome glyph. `x0..x1` is the
    /// cluster's advance in the window and `base` its baseline (from RichEdit), `px` the font
    /// size in pixels, `fg` the text color (for glyphs that have no color layers).
    ///
    /// # Safety
    /// `hdc` is a valid device context of the view.
    pub unsafe fn paint(&mut self, hdc: HDC, text: &[u16], px: f32, fg: u32, x0: i32, x1: i32, base: i32) {
        let Some(g) = self.glyph(text, px, fg) else { return };
        // Centered in RichEdit's advance (GDI and DirectWrite round widths differently).
        let x = x0 + (((x1 - x0) as f32 - g.advance) / 2.0).round() as i32 - g.left;
        let y = base - g.baseline;
        // Cover the outline: the ink box of the color glyph (same font, same outlines) one pixel
        // larger, across the whole advance, in the color just above it (page, table cell, code
        // block or selection — whatever RichEdit painted there).
        let (top, bottom) = (y + g.ink.top - 1, y + g.ink.bottom + 1);
        let (left, right) = (x0.min(x + g.ink.left - 1), x1.max(x + g.ink.right + 1));
        let under = GetPixel(hdc, (x0 + x1) / 2, top - 1);
        if under.0 != CLR_INVALID {
            let brush = CreateSolidBrush(under);
            FillRect(hdc, &RECT { left, top, right, bottom }, brush);
            let _ = DeleteObject(brush.into());
        }
        let mem = CreateCompatibleDC(Some(hdc));
        let old = SelectObject(mem, g.bmp.into());
        let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let _ = GdiAlphaBlend(hdc, x, y, g.w, g.h, mem, 0, 0, g.w, g.h, bf);
        SelectObject(mem, old);
        let _ = DeleteDC(mem);
    }

    fn glyph(&mut self, text: &[u16], px: f32, fg: u32) -> Option<&Glyph> {
        let key = (Box::<[u16]>::from(text), (px * 4.0).round() as u32, fg);
        if !self.cache.contains_key(&key) {
            if self.cache.len() >= MAX_CACHE {
                self.cache.clear();
            }
            let g = unsafe { self.render(text, px, fg) };
            self.cache.insert(key.clone(), g);
        }
        self.cache.get(&key).and_then(|g| g.as_ref())
    }

    unsafe fn render(&mut self, text: &[u16], px: f32, fg: u32) -> Option<Glyph> {
        if text.is_empty() || text.len() > 64 || !(2.0..=1000.0).contains(&px) {
            return None;
        }
        let sk = (px * 4.0).round() as u32;
        let format = match self.formats.get(&sk) {
            Some(f) => f.clone(),
            None => {
                let f = self
                    .dw
                    .CreateTextFormat(w!("Segoe UI Emoji"), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, px, w!(""))
                    .ok()?;
                let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                self.formats.insert(sk, f.clone());
                f
            }
        };
        let layout = self.dw.CreateTextLayout(text, &format, 100_000.0, 100_000.0).ok()?;
        let mut m = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut m).ok()?;
        let mut lines = [DWRITE_LINE_METRICS::default(); 1];
        let mut n = 0u32;
        layout.GetLineMetrics(Some(&mut lines), &mut n).ok()?;
        // Room for ink outside the advance and the line box.
        let pad = (px * 0.25).ceil() as i32 + 1;
        let (w, h) = (m.widthIncludingTrailingWhitespace.ceil() as i32 + 2 * pad, m.height.ceil() as i32 + 2 * pad);
        if w <= 2 * pad || h <= 2 * pad || w > 4096 || h > 4096 {
            return None;
        }
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() {
            let _ = DeleteObject(bmp.into());
            return None;
        }
        let dc = CreateCompatibleDC(None);
        let old = SelectObject(dc, bmp.into());
        let drawn = (|| -> windows::core::Result<()> {
            self.rt.BindDC(dc, &RECT { left: 0, top: 0, right: w, bottom: h })?;
            self.rt.BeginDraw();
            self.rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            let c = D2D1_COLOR_F { r: theme::r(fg) as f32 / 255.0, g: theme::g(fg) as f32 / 255.0, b: theme::b(fg) as f32 / 255.0, a: 1.0 };
            let brush = self.rt.CreateSolidColorBrush(&c, None)?;
            let rect = D2D_RECT_F { left: pad as f32, top: pad as f32, right: w as f32, bottom: h as f32 };
            self.rt.DrawText(text, &format, &rect, &brush, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, DWRITE_MEASURING_MODE_NATURAL);
            self.rt.EndDraw(None, None)
        })();
        let _ = GdiFlush();
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
        let ink = drawn.ok().and_then(|_| ink_bounds(std::slice::from_raw_parts(bits as *const u32, (w * h) as usize), w, h));
        let Some(ink) = ink else {
            let _ = DeleteObject(bmp.into());
            return None;
        };
        Some(Glyph { bmp, w, h, baseline: pad + lines[0].baseline.round() as i32, left: pad, advance: m.widthIncludingTrailingWhitespace, ink })
    }
}

/// Bounds of the pixels with non-zero alpha in a `w`×`h` BGRA bitmap; None if all are transparent.
fn ink_bounds(px: &[u32], w: i32, h: i32) -> Option<RECT> {
    let mut r = RECT { left: w, top: h, right: 0, bottom: 0 };
    for y in 0..h {
        for x in 0..w {
            if px[(y * w + x) as usize] >> 24 != 0 {
                r.left = r.left.min(x);
                r.right = r.right.max(x + 1);
                r.top = r.top.min(y);
                r.bottom = r.bottom.max(y + 1);
            }
        }
    }
    (r.right > r.left).then_some(r)
}

/// GetPixel's "no pixel" value (outside the clip or the window).
const CLR_INVALID: u32 = 0xFFFF_FFFF;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_bounds_finds_opaque_pixels() {
        let mut px = vec![0u32; 6 * 4];
        assert!(ink_bounds(&px, 6, 4).is_none());
        px[6 + 2] = 0x8000_0000;
        px[2 * 6 + 4] = 0x0100_0000;
        let r = ink_bounds(&px, 6, 4).unwrap();
        assert_eq!((r.left, r.top, r.right, r.bottom), (2, 1, 5, 3));
    }

    /// The emoji comes out in color: a yellow circle has yellow pixels, a check mark box green
    /// ones. Segoe UI Emoji differs between Windows versions (Windows 10 vs. Windows 11 /
    /// Server 2025 on CI — other shades), so the check is a hue family, not an exact color;
    /// the monochrome fallback (gray `fg`) matches neither.
    #[test]
    fn renders_color_layers() {
        let Some(mut p) = Painter::new() else { panic!("Direct2D/DirectWrite unavailable") };
        let fg = theme::rgb(200, 200, 200);
        let yellow = |r: i32, g: i32, b: i32| r >= 180 && g >= 140 && b <= 130 && r - b >= 100 && (r - g).abs() <= 90;
        let green = |r: i32, g: i32, b: i32| g >= 100 && g - r >= 30 && g - b >= 30;
        let families: [(&str, &dyn Fn(i32, i32, i32) -> bool); 2] = [("🟡", &yellow), ("✅", &green)];
        for (text, family) in families {
            let t: Vec<u16> = text.encode_utf16().collect();
            let g = p.glyph(&t, 32.0, fg).unwrap_or_else(|| panic!("{text}: not rendered"));
            assert!(g.w > 20 && g.h > 20 && g.advance > 20.0, "{text}: size {}x{} adv {}", g.w, g.h, g.advance);
            let mut bits = vec![0u32; (g.w * g.h) as usize];
            unsafe {
                let dc = CreateCompatibleDC(None);
                let mut bi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: g.w,
                        biHeight: -g.h,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                GetDIBits(dc, g.bmp, 0, g.h as u32, Some(bits.as_mut_ptr() as *mut c_void), &mut bi, DIB_RGB_COLORS);
                let _ = DeleteDC(dc);
            }
            // Opaque pixels (premultiplied = straight when opaque) of the expected hue family.
            let rgb = |p: u32| (((p >> 16) & 0xFF) as i32, ((p >> 8) & 0xFF) as i32, (p & 0xFF) as i32);
            let opaque: Vec<u32> = bits.iter().copied().filter(|p| p >> 24 == 255).collect();
            let hits = opaque.iter().filter(|&&p| {
                let (r, gg, b) = rgb(p);
                family(r, gg, b)
            });
            let hits = hits.count();
            if hits <= 30 {
                // For the failure message: the most frequent opaque colors.
                let mut freq: std::collections::HashMap<u32, usize> = Default::default();
                for p in &opaque {
                    *freq.entry(p & 0xFF_FFFF).or_default() += 1;
                }
                let mut top: Vec<_> = freq.into_iter().collect();
                top.sort_by(|a, b| b.1.cmp(&a.1));
                let top: Vec<String> = top.iter().take(6).map(|(c, n)| format!("#{c:06X}×{n}")).collect();
                panic!("{text}: {hits} pixels of the expected hue; most frequent colors: {}", top.join(" "));
            }
            // The same request comes from the cache.
            let n = p.cache.len();
            p.glyph(&t, 32.0, fg);
            assert_eq!(p.cache.len(), n);
        }
        // Out-of-range sizes and empty text are refused.
        assert!(p.glyph(&[], 32.0, fg).is_none());
        assert!(p.glyph(&[0x41], 0.5, fg).is_none());
    }
}
