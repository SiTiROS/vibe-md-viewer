//! Settings:
//! * installed program (Program Files / %LOCALAPPDATA%\Programs) — like regular software:
//!   `%APPDATA%\vibe-md-viewer\settings.ini` (Program Files is not writable; before 0.2.0 —
//!   `%APPDATA%\vibe-mdViewer`, the folder is renamed on first start);
//! * portable copy (the exe just sits in a folder, e.g. during development on D:) —
//!   next to the exe: `<exe folder>\data\settings.ini`.

use std::fs;
use std::path::PathBuf;

use crate::i18n::Lang;
use crate::theme::ThemePref;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteImages {
    /// Ask (default): show the "load images from the internet" bar.
    Ask,
    Always,
    Never,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub theme: ThemePref,
    pub remote_images: RemoteImages,
    /// Text zoom, percent.
    pub zoom: u32,
    /// x, y, w, h, maximized
    pub window: Option<(i32, i32, i32, i32, bool)>,
    /// Text across the full window width (true, default) or in a narrow ~900 px column.
    pub wide: bool,
    /// Autosave edits (typing pause, leaving the window, closing) — like in an IDE.
    pub autosave: bool,
    /// UI language: English by default, `lang=ru` — Russian (EN/RU button).
    pub lang: Lang,
    pub dirty: bool,
}

impl Default for Settings {
    fn default() -> Self {
        // Dark theme by default (toggled with the ☀/☾ button, remembered).
        Settings { theme: ThemePref::Dark, remote_images: RemoteImages::Ask, zoom: 100, window: None, wide: true, autosave: true, lang: Lang::En, dirty: false }
    }
}

pub fn data_dir() -> Option<PathBuf> {
    if crate::install::running_installed() {
        let appdata = PathBuf::from(std::env::var_os("APPDATA")?);
        let dir = appdata.join("vibe-md-viewer");
        // Before 0.2.0 the program was called vibe-mdViewer — settings move to the folder with the new name.
        let legacy = appdata.join("vibe-mdViewer");
        if !dir.exists() && legacy.is_dir() && !legacy.is_symlink() {
            let _ = fs::rename(&legacy, &dir);
        }
        Some(dir)
    } else {
        Some(crate::util::exe_dir()?.join("data"))
    }
}

fn file() -> Option<PathBuf> {
    Some(data_dir()?.join("settings.ini"))
}

/// Language at startup: from our own settings; a downloaded installer has none — then from the
/// installed program's settings (the update runs in the language the user chose).
pub fn initial_lang() -> Lang {
    let read = |p: PathBuf| -> Option<Lang> {
        let text = fs::read_to_string(p).ok()?;
        text.trim_start_matches('\u{feff}').lines().filter_map(|l| l.trim().strip_prefix("lang=")).map(Lang::parse).last()
    };
    if let Some(l) = file().and_then(read) {
        return l;
    }
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    appdata.and_then(|a| read(a.join("vibe-md-viewer").join("settings.ini"))).unwrap_or(Lang::En)
}

impl Settings {
    pub fn load() -> Settings {
        let mut s = Settings::default();
        let Some(path) = file() else { return s };
        let Ok(text) = fs::read_to_string(path) else { return s };
        // Notepad and PowerShell may write the file with a BOM — otherwise the first line is not read.
        for line in text.trim_start_matches('\u{feff}').lines() {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "theme" => {
                    s.theme = match v {
                        "light" => ThemePref::Light,
                        "system" => ThemePref::System,
                        _ => ThemePref::Dark,
                    }
                }
                "remote_images" => {
                    s.remote_images = match v {
                        "always" => RemoteImages::Always,
                        "never" => RemoteImages::Never,
                        _ => RemoteImages::Ask,
                    }
                }
                "wide" => s.wide = v != "0" && v != "false",
                "autosave" => s.autosave = v != "0" && v != "false",
                "lang" => s.lang = Lang::parse(v),
                "zoom" => s.zoom = v.parse::<u32>().unwrap_or(100).clamp(50, 300),
                "window" => {
                    let p: Vec<i32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if p.len() == 5 && p[2] > 200 && p[3] > 150 {
                        s.window = Some((p[0], p[1], p[2], p[3], p[4] != 0));
                    }
                }
                _ => {}
            }
        }
        s
    }

    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(dir) = data_dir() else { return };
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let theme = match self.theme {
            ThemePref::System => "system",
            ThemePref::Light => "light",
            ThemePref::Dark => "dark",
        };
        let ri = match self.remote_images {
            RemoteImages::Ask => "ask",
            RemoteImages::Always => "always",
            RemoteImages::Never => "never",
        };
        let mut out = String::from("# vibe-md-viewer settings\n");
        out += &format!("theme={theme}\nremote_images={ri}\nzoom={}\nwide={}\nautosave={}\nlang={}\n", self.zoom, self.wide as i32, self.autosave as i32, self.lang.code());
        if let Some((x, y, w, h, m)) = self.window {
            out += &format!("window={x},{y},{w},{h},{}\n", m as i32);
        }
        if crate::doc::atomic_write(&dir.join("settings.ini"), out.as_bytes()).is_ok() {
            self.dirty = false;
        }
    }
}
