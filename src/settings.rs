//! Settings:
//! * installed program (Program Files / %LOCALAPPDATA%\Programs) — like regular software:
//!   `%APPDATA%\vibe-md-viewer\settings.ini` (Program Files is not writable; before 0.2.0 —
//!   `%APPDATA%\vibe-mdViewer`, the folder is renamed on first start);
//! * portable copy (the exe just sits in a folder, e.g. during development on D:) —
//!   next to the exe: `<exe folder>\data\settings.ini`.

use std::fs;
use std::path::{Path, PathBuf};

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
        Some(installed_data_dir(&appdata))
    } else {
        Some(crate::util::exe_dir()?.join("data"))
    }
}

/// `<appdata>\vibe-md-viewer`. Before 0.2.0 the program was called vibe-mdViewer — settings move
/// to the folder with the new name.
fn installed_data_dir(appdata: &Path) -> PathBuf {
    let dir = appdata.join("vibe-md-viewer");
    let legacy = appdata.join("vibe-mdViewer");
    if !dir.exists() && legacy.is_dir() && !legacy.is_symlink() {
        let _ = fs::rename(&legacy, &dir);
    }
    dir
}

fn file() -> Option<PathBuf> {
    Some(data_dir()?.join("settings.ini"))
}

/// `key=value` lines of settings.ini, trimmed. A BOM (Notepad and PowerShell may write one —
/// otherwise the first line is not read), comments (`#`, `;`) and lines without `=` are skipped.
fn entries(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.trim_start_matches('\u{feff}').lines().filter_map(|line| {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') {
            return None;
        }
        let (k, v) = line.split_once('=')?;
        Some((k.trim(), v.trim()))
    })
}

/// `lang=` from the text of settings.ini (the last one wins), read exactly like `Settings::parse`.
fn lang_in(text: &str) -> Option<Lang> {
    entries(text).filter(|(k, _)| *k == "lang").map(|(_, v)| Lang::parse(v)).last()
}

/// Language at startup: from our own settings; a downloaded installer has none — then from the
/// installed program's settings (the update runs in the language the user chose).
pub fn initial_lang() -> Lang {
    let read = |p: PathBuf| -> Option<Lang> { lang_in(&fs::read_to_string(p).ok()?) };
    if let Some(l) = file().and_then(read) {
        return l;
    }
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    appdata.and_then(|a| read(a.join("vibe-md-viewer").join("settings.ini"))).unwrap_or(Lang::En)
}

impl Settings {
    pub fn load() -> Settings {
        let Some(path) = file() else { return Settings::default() };
        Settings::load_from(&path)
    }

    /// Settings from a file; defaults if it is missing or unreadable.
    fn load_from(path: &Path) -> Settings {
        let Ok(text) = fs::read_to_string(path) else { return Settings::default() };
        Settings::parse(&text)
    }

    /// Parses the text of settings.ini: unknown keys are skipped, invalid values give defaults.
    fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for (k, v) in entries(text) {
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
        self.save_in(&dir);
    }

    /// Writes settings.ini into `dir` (creating it); `dirty` is cleared only on success.
    fn save_in(&mut self, dir: &Path) {
        if fs::create_dir_all(dir).is_err() {
            return;
        }
        if crate::doc::atomic_write(&dir.join("settings.ini"), self.serialize().as_bytes()).is_ok() {
            self.dirty = false;
        }
    }

    /// The text of settings.ini.
    fn serialize(&self) -> String {
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
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under .toolcache\tmp (unique per process: several test runs may overlap).
    fn tmp_dir(name: &str) -> PathBuf {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".toolcache").join("tmp").join(format!("settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// All fields except `dirty`, for comparisons.
    fn fields(s: &Settings) -> (ThemePref, RemoteImages, u32, Option<(i32, i32, i32, i32, bool)>, bool, bool, Lang) {
        (s.theme, s.remote_images, s.zoom, s.window, s.wide, s.autosave, s.lang)
    }

    #[test]
    fn defaults() {
        let s = Settings::default();
        assert_eq!(fields(&s), (ThemePref::Dark, RemoteImages::Ask, 100, None, true, true, Lang::En));
        assert!(!s.dirty);
        assert_eq!(fields(&Settings::parse("")), fields(&s));
    }

    #[test]
    fn parses_every_key() {
        let s = Settings::parse(
            "theme=light\nremote_images=always\nzoom=150\nwide=0\nautosave=false\nlang=ru\nwindow=10,20,800,600,1\n",
        );
        assert_eq!(fields(&s), (ThemePref::Light, RemoteImages::Always, 150, Some((10, 20, 800, 600, true)), false, false, Lang::Ru));
        assert!(!s.dirty);
        let s = Settings::parse("theme=system\nremote_images=never\nwide=1\nautosave=yes\nlang=en");
        assert_eq!((s.theme, s.remote_images, s.wide, s.autosave, s.lang), (ThemePref::System, RemoteImages::Never, true, true, Lang::En));
    }

    #[test]
    fn invalid_values_fall_back_to_defaults() {
        // The later line wins, and an unknown value gives the default.
        let s = Settings::parse("theme=light\ntheme=blue\nremote_images=never\nremote_images=sometimes\nlang=ru\nlang=de");
        assert_eq!((s.theme, s.remote_images, s.lang), (ThemePref::Dark, RemoteImages::Ask, Lang::En));
        for (v, zoom) in [("abc", 100), ("", 100), ("-5", 100), ("10", 50), ("49", 50), ("50", 50), ("300", 300), ("1000", 300), ("120", 120)] {
            assert_eq!(Settings::parse(&format!("zoom={v}")).zoom, zoom, "zoom={v}");
        }
    }

    #[test]
    fn window_is_validated() {
        let w = |v: &str| Settings::parse(&format!("window={v}")).window;
        assert_eq!(w("10,20,800,600,1"), Some((10, 20, 800, 600, true)));
        assert_eq!(w(" -1920 , -8 , 1024 , 768 , 0 "), Some((-1920, -8, 1024, 768, false)));
        assert_eq!(w("0,0,201,151,2"), Some((0, 0, 201, 151, true)));
        for bad in ["0,0,200,600,0", "0,0,800,150,0", "0,0,800,600", "0,0,800,600,0,0", "a,b,c,d,e", "", "0,0,-800,600,0"] {
            assert_eq!(w(bad), None, "window={bad}");
        }
    }

    #[test]
    fn bom_comments_whitespace_and_crlf() {
        let s = Settings::parse("\u{feff}theme=light\r\n# zoom=200\r\n; lang=ru\r\n  zoom = 175  \r\nno equals sign\r\n=orphan\r\nunknown=1\r\n");
        assert_eq!(s.theme, ThemePref::Light, "the first line is read despite the BOM");
        assert_eq!(s.zoom, 175);
        assert_eq!(s.lang, Lang::En, "comments are skipped");
    }

    #[test]
    fn serialize_format_and_round_trip() {
        let s = Settings::default();
        assert_eq!(s.serialize(), "# vibe-md-viewer settings\ntheme=dark\nremote_images=ask\nzoom=100\nwide=1\nautosave=1\nlang=en\n");
        let variants = [
            Settings { theme: ThemePref::Light, remote_images: RemoteImages::Always, zoom: 50, window: Some((-5, 7, 1200, 900, true)), wide: false, autosave: false, lang: Lang::Ru, dirty: true },
            Settings { theme: ThemePref::System, remote_images: RemoteImages::Never, zoom: 300, window: Some((0, 0, 640, 480, false)), wide: true, autosave: false, lang: Lang::En, dirty: false },
            Settings { theme: ThemePref::Dark, remote_images: RemoteImages::Ask, zoom: 125, window: None, wide: false, autosave: true, lang: Lang::Ru, dirty: false },
        ];
        for v in &variants {
            let text = v.serialize();
            assert!(text.starts_with("# vibe-md-viewer settings\n") && text.ends_with('\n'), "{text}");
            assert_eq!(fields(&Settings::parse(&text)), fields(v), "{text}");
        }
        assert!(variants[0].serialize().contains("\nwindow=-5,7,1200,900,1\n"));
        assert!(!variants[2].serialize().contains("window="));
    }

    #[test]
    fn save_and_load_files() {
        let dir = tmp_dir("save");
        let target = dir.join("data");
        let mut s = Settings { theme: ThemePref::Light, zoom: 130, lang: Lang::Ru, window: Some((1, 2, 900, 700, false)), dirty: true, ..Settings::default() };
        s.save_in(&target);
        assert!(!s.dirty, "dirty is cleared after a successful write");
        let path = target.join("settings.ini");
        assert_eq!(fs::read_to_string(&path).unwrap(), s.serialize());
        assert_eq!(fields(&Settings::load_from(&path)), fields(&s));
        let leftovers: Vec<_> = fs::read_dir(&target).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).filter(|n| n != "settings.ini").collect();
        assert!(leftovers.is_empty(), "no temporary files left: {leftovers:?}");

        // Missing or unreadable (not UTF-8) file → defaults.
        assert_eq!(fields(&Settings::load_from(&dir.join("missing.ini"))), fields(&Settings::default()));
        let bad = dir.join("bad.ini");
        fs::write(&bad, b"theme=light\n\xFF\xFE\n").unwrap();
        assert_eq!(fields(&Settings::load_from(&bad)), fields(&Settings::default()));

        // The folder cannot be created (a file is in the way) → dirty stays set.
        let blocker = dir.join("blocker");
        fs::write(&blocker, b"x").unwrap();
        let mut s2 = Settings { dirty: true, ..Settings::default() };
        s2.save_in(&blocker.join("data"));
        assert!(s2.dirty);
        // Nothing to save → save() returns before touching the disk.
        let mut clean = Settings::default();
        clean.save();
        assert!(!clean.dirty);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lang_from_settings_text() {
        assert_eq!(lang_in(""), None);
        assert_eq!(lang_in("theme=light\nzoom=120"), None);
        assert_eq!(lang_in("\u{feff}lang=ru\n"), Some(Lang::Ru), "BOM");
        assert_eq!(lang_in("lang=ru\r\nlang=en\r\n"), Some(Lang::En), "the last one wins");
        assert_eq!(lang_in("  lang=RU  "), Some(Lang::Ru));
        assert_eq!(lang_in("lang=xx"), Some(Lang::En));
    }

    /// The startup language is read like every other key: spaces around "=" are allowed.
    #[test]
    fn lang_in_agrees_with_parse() {
        assert_eq!(lang_in("lang = ru"), Some(Lang::Ru));
        assert_eq!(lang_in("\tlang\t=\tru\t"), Some(Lang::Ru));
        assert_eq!(lang_in("\u{feff} lang = ru\r\nzoom = 120\r\n"), Some(Lang::Ru));
        assert_eq!(lang_in("lang = ru\nlang= en"), Some(Lang::En), "the last one wins");
        // comments, other keys and lines without "=" are not a language
        for t in ["# lang = ru", "; lang=ru", "language=ru", "LANG=ru", "lang ru", "xlang=ru", "=ru"] {
            assert_eq!(lang_in(t), None, "{t:?}");
        }
        // whatever settings.ini contains, the startup language equals the parsed one
        for t in [
            "", "lang = ru", "  lang=RU  ", "lang =\n", "lang=ru\n# lang=en", "lang=ru\n; lang = en\nlang", "theme = light\nlang\t= ru\nzoom=1",
            "\u{feff}lang = ru", "lang = ru\r\nlang = xx", "lang==ru", "lang = ru = en",
        ] {
            assert_eq!(lang_in(t).unwrap_or(Lang::En), Settings::parse(t).lang, "{t:?}");
        }
    }

    #[test]
    fn data_dir_of_a_portable_copy() {
        // Tests run from target\...\deps without the installer marker → a portable copy.
        if !crate::install::running_installed() {
            assert_eq!(data_dir(), crate::util::exe_dir().map(|d| d.join("data")));
            assert_eq!(file(), crate::util::exe_dir().map(|d| d.join("data").join("settings.ini")));
        }
    }

    #[test]
    fn installed_data_dir_moves_legacy_settings() {
        let root = tmp_dir("appdata");
        // No folders yet → the new name, nothing is created.
        let a = root.join("empty");
        fs::create_dir_all(&a).unwrap();
        assert_eq!(installed_data_dir(&a), a.join("vibe-md-viewer"));
        assert!(!a.join("vibe-md-viewer").exists());

        // Only the former folder → renamed.
        let b = root.join("legacy");
        fs::create_dir_all(b.join("vibe-mdViewer")).unwrap();
        fs::write(b.join("vibe-mdViewer").join("settings.ini"), "lang=ru\n").unwrap();
        let dir = installed_data_dir(&b);
        assert_eq!(dir, b.join("vibe-md-viewer"));
        assert_eq!(fs::read_to_string(dir.join("settings.ini")).unwrap(), "lang=ru\n");
        assert!(!b.join("vibe-mdViewer").exists());

        // Both folders → the new one is kept, the former one is not touched.
        let c = root.join("both");
        fs::create_dir_all(c.join("vibe-mdViewer")).unwrap();
        fs::create_dir_all(c.join("vibe-md-viewer")).unwrap();
        fs::write(c.join("vibe-mdViewer").join("settings.ini"), "old").unwrap();
        assert_eq!(installed_data_dir(&c), c.join("vibe-md-viewer"));
        assert!(c.join("vibe-mdViewer").join("settings.ini").exists());
        assert!(!c.join("vibe-md-viewer").join("settings.ini").exists());

        // The former name is a file, not a folder → not renamed.
        let d = root.join("file");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("vibe-mdViewer"), "x").unwrap();
        assert_eq!(installed_data_dir(&d), d.join("vibe-md-viewer"));
        assert!(d.join("vibe-mdViewer").is_file() && !d.join("vibe-md-viewer").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
