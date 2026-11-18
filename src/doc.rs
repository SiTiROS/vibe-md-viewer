//! Reading and saving .md: encoding and line-ending detection, atomic write.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use windows::Win32::Globalization::{
    MultiByteToWideChar, WideCharToMultiByte, CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS,
    WC_NO_BEST_FIT_CHARS,
};

/// Hard file size limit: protects against "bombs" of hundreds of megabytes.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    /// The system single-byte encoding (usually Windows-1251).
    Ansi,
}

impl Encoding {
    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 BOM",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Ansi => "ANSI",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    Crlf,
    Lf,
    Cr,
}

impl Eol {
    pub fn label(self) -> &'static str {
        match self {
            Eol::Crlf => "CRLF",
            Eol::Lf => "LF",
            Eol::Cr => "CR",
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Eol::Crlf => "\r\n",
            Eol::Lf => "\n",
            Eol::Cr => "\r",
        }
    }
}

#[derive(Clone, Debug)]
pub struct LoadedFile {
    /// Text with normalized line breaks ("\n").
    pub text: String,
    pub encoding: Encoding,
    pub eol: Eol,
    /// Something was replaced while reading (invalid UTF-8/UTF-16 bytes, NUL): saving
    /// would write a different file — autosave is disabled for it.
    pub lossy: bool,
}

pub fn load(path: &Path) -> Result<LoadedFile, String> {
    let meta = fs::metadata(path).map_err(|e| trf!("Could not open the file: {e}", "Не удалось открыть файл: {e}"))?;
    if !meta.is_file() {
        return Err(tr!("This is not a file", "Это не файл").into());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(trf!(
            "The file is too large ({} MB). The limit is {} MB.",
            "Файл слишком большой ({} МБ). Предел — {} МБ.",
            meta.len() / (1024 * 1024),
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    let bytes = fs::read(path).map_err(|e| trf!("Could not read the file: {e}", "Не удалось прочитать файл: {e}"))?;
    Ok(decode(&bytes))
}

pub fn decode(bytes: &[u8]) -> LoadedFile {
    let mut lossy = false;
    let (raw, encoding) = if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        lossy = std::str::from_utf8(rest).is_err();
        (String::from_utf8_lossy(rest).into_owned(), Encoding::Utf8Bom)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let u: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        lossy = rest.len() % 2 != 0 || String::from_utf16(&u).is_err();
        (String::from_utf16_lossy(&u), Encoding::Utf16Le)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let u: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        lossy = rest.len() % 2 != 0 || String::from_utf16(&u).is_err();
        (String::from_utf16_lossy(&u), Encoding::Utf16Be)
    } else {
        match std::str::from_utf8(bytes) {
            Ok(s) => (s.to_owned(), Encoding::Utf8),
            Err(_) => (ansi_to_string(bytes), Encoding::Ansi),
        }
    };
    let eol = detect_eol(&raw);
    let mut text = normalize_eol(&raw);
    // NUL breaks text controls — replace it.
    if text.contains('\0') {
        text = text.replace('\0', "\u{FFFD}");
        lossy = true;
    }
    LoadedFile { text, encoding, eol, lossy }
}

fn detect_eol(s: &str) -> Eol {
    let b = s.as_bytes();
    for i in 0..b.len() {
        match b[i] {
            b'\r' => {
                return if b.get(i + 1) == Some(&b'\n') { Eol::Crlf } else { Eol::Cr };
            }
            b'\n' => return Eol::Lf,
            _ => {}
        }
    }
    Eol::Crlf
}

pub fn normalize_eol(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_owned();
    }
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn ansi_to_string(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    unsafe {
        let n = MultiByteToWideChar(CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), bytes, None);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut buf = vec![0u16; n as usize];
        let n2 = MultiByteToWideChar(CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), bytes, Some(&mut buf));
        buf.truncate(n2.max(0) as usize);
        String::from_utf16_lossy(&buf)
    }
}

/// Encodes to ANSI; None if there are characters missing from the code page.
fn string_to_ansi(s: &str) -> Option<Vec<u8>> {
    let w: Vec<u16> = s.encode_utf16().collect();
    if w.is_empty() {
        return Some(Vec::new());
    }
    unsafe {
        let mut used_default = windows::core::BOOL(0);
        let n = WideCharToMultiByte(CP_ACP, WC_NO_BEST_FIT_CHARS, &w, None, None, Some(&mut used_default));
        if n <= 0 || used_default.as_bool() {
            return None;
        }
        let mut buf = vec![0u8; n as usize];
        let mut used2 = windows::core::BOOL(0);
        let n2 = WideCharToMultiByte(CP_ACP, WC_NO_BEST_FIT_CHARS, &w, Some(&mut buf), None, Some(&mut used2));
        if n2 <= 0 || used2.as_bool() {
            return None;
        }
        buf.truncate(n2 as usize);
        Some(buf)
    }
}

/// The save result is the actual encoding (ANSI may switch to UTF-8).
pub fn save(path: &Path, text_lf: &str, encoding: Encoding, eol: Eol) -> Result<Encoding, String> {
    let text = if eol == Eol::Lf { text_lf.to_owned() } else { text_lf.replace('\n', eol.as_str()) };
    let (bytes, used) = match encoding {
        Encoding::Utf8 => (text.into_bytes(), Encoding::Utf8),
        Encoding::Utf8Bom => {
            let mut v = vec![0xEF, 0xBB, 0xBF];
            v.extend_from_slice(text.as_bytes());
            (v, Encoding::Utf8Bom)
        }
        Encoding::Utf16Le => {
            let mut v = vec![0xFF, 0xFE];
            for u in text.encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            (v, Encoding::Utf16Le)
        }
        Encoding::Utf16Be => {
            let mut v = vec![0xFE, 0xFF];
            for u in text.encode_utf16() {
                v.extend_from_slice(&u.to_be_bytes());
            }
            (v, Encoding::Utf16Be)
        }
        Encoding::Ansi => match string_to_ansi(&text) {
            Some(b) => (b, Encoding::Ansi),
            None => (text.into_bytes(), Encoding::Utf8),
        },
    };
    atomic_write(path, &bytes)?;
    Ok(used)
}

/// Write to a temporary file alongside and atomically replace the original:
/// on a power failure/error the original file does not end up truncated.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        REPLACE_FILE_FLAGS,
    };
    let dir = path.parent().ok_or(tr!("Invalid path", "Неверный путь"))?;
    let name = path.file_name().ok_or(tr!("Invalid path", "Неверный путь"))?.to_string_lossy().into_owned();
    // Unique name + create_new: a pre-planted file or symbolic link with that name
    // will not be overwritten (writing "through" a link into someone else's file is impossible).
    let nonce = format!("{:x}{:x}", std::process::id(), crate::util::now_filetime());
    let tmp: PathBuf = dir.join(format!(".{name}.{nonce}.vmd-tmp"));
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| trf!("Could not write the file: {e}", "Не удалось записать файл: {e}"))?;
        let res = f.write_all(bytes).and_then(|_| f.sync_all());
        if let Err(e) = res {
            drop(f);
            let _ = fs::remove_file(&tmp);
            return Err(trf!("Write error: {e}", "Ошибка записи: {e}"));
        }
    }
    let wt = crate::util::wide_path(&tmp);
    let wp = crate::util::wide_path(path);
    unsafe {
        if path.exists() {
            if ReplaceFileW(
                crate::util::pcwstr(&wp),
                crate::util::pcwstr(&wt),
                windows::core::PCWSTR::null(),
                REPLACE_FILE_FLAGS(0),
                None,
                None,
            )
            .is_ok()
            {
                return Ok(());
            }
        }
        match MoveFileExW(
            crate::util::pcwstr(&wt),
            crate::util::pcwstr(&wp),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        ) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(trf!("Could not save: {e}", "Не удалось сохранить: {e}"))
            }
        }
    }
}

/// File modification time (for tracking external edits).
pub fn mtime(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_bom_and_eol() {
        let f = decode(b"\xEF\xBB\xBFa\r\nb\r\n");
        assert_eq!(f.encoding, Encoding::Utf8Bom);
        assert_eq!(f.eol, Eol::Crlf);
        assert_eq!(f.text, "a\nb\n");
        let f = decode(b"x\ny");
        assert_eq!(f.encoding, Encoding::Utf8);
        assert_eq!(f.eol, Eol::Lf);
    }

    #[test]
    fn utf16() {
        let f = decode(&[0xFF, 0xFE, b'h', 0, b'i', 0]);
        assert_eq!(f.text, "hi");
        assert_eq!(f.encoding, Encoding::Utf16Le);
    }
}
