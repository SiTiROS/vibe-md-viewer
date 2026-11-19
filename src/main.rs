//! vibe-md-viewer — a fast native Markdown viewer and editor for Windows.
//! Rust + Win32 + RichEdit (ships with Windows). No browser engine.

#![windows_subsystem = "windows"]

// First: the tr!/trf! macros (two-language UI strings) are needed by every module.
#[macro_use]
mod i18n;
mod app;
mod dialogs;
mod doc;
mod editor;
mod emoji;
mod highlight;
mod html;
mod images;
mod install;
mod links;
mod net;
mod render;
mod richedit;
mod security;
mod settings;
mod setup;
mod styler;
mod theme;
mod ui;
mod util;

/// Helpers shared by tests of several modules.
#[cfg(test)]
pub mod testutil {
    /// Tests on a real RichEdit control run one at a time: in parallel (windows in different
    /// threads) the pixel checks occasionally saw an empty frame.
    pub fn richedit_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
}

use std::path::PathBuf;

/// Command-line help (in the UI language; the switches themselves are not translated).
fn usage() -> &'static str {
    tr!(
        "vibe-md-viewer.exe [file.md] [--mode view|visual|code]\n\n\
--setup        setup wizard (also when the exe is named …setup….exe)\n\
--install      install and register for .md\n\
--uninstall    uninstall\n\
--elevated ... internal installer mode (started automatically via UAC)\n\
--timing FILE  append the startup time (ms) to the file\n\
--exit-after-paint  exit right after the first paint (for benchmarks)\n\
--snapshot PNG save a window snapshot and exit (for layout checks)\n\
--profile FILE measure theme/mode/zoom/scroll switching (scripts\\profile.ps1)",
        "vibe-md-viewer.exe [файл.md] [--mode view|visual|code]\n\n\
--setup        окно установщика (так же — если exe называется …setup….exe)\n\
--install      установить и зарегистрировать для .md\n\
--uninstall    удалить\n\
--elevated ... служебный режим установщика (запускается сам через UAC)\n\
--timing ФАЙЛ  дописать время запуска (мс) в файл\n\
--exit-after-paint  выйти сразу после первой отрисовки (для замеров)\n\
--snapshot PNG сохранить снимок окна и выйти (для проверки вёрстки)\n\
--profile ФАЙЛ замер смены темы/режимов/масштаба/прокрутки (scripts\\profile.ps1)"
    )
}

/// Exit code from `main` (not `std::process::exit`, which on Windows is an immediate
/// ExitProcess: destructors and atexit handlers don't run — e.g. the coverage counters of an
/// instrumented build were never written).
fn code(c: i32) -> std::process::ExitCode {
    std::process::ExitCode::from(c.clamp(0, 255) as u8)
}

/// What the command line asks for.
enum Cli {
    Install,
    Uninstall,
    /// `--elevated <op> [--target <folder>]`: the installer copy started via UAC.
    Elevated { op: String, target: Option<PathBuf> },
    Help,
    /// Open the viewer; `setup` — the `--setup` switch (setup wizard).
    Run { opts: app::Options, setup: bool },
}

/// Parses the arguments (without the exe name). An action switch (--install, --uninstall,
/// --elevated, --help) ends parsing; switches with a value take the next argument whatever it is.
fn parse_args(mut args: impl Iterator<Item = std::ffi::OsString>) -> Cli {
    let mut file: Option<PathBuf> = None;
    let mut mode = app::Mode::View;
    let mut snapshot = None;
    let mut timing = None;
    let mut exit_after_paint = false;
    let mut size = None;
    let mut scroll = None;
    let mut selftest = false;
    let mut profile = None;
    let mut setup_flag = false;
    while let Some(a) = args.next() {
        let s = a.to_string_lossy().to_string();
        match s.as_str() {
            "--install" => return Cli::Install,
            "--uninstall" => return Cli::Uninstall,
            // Elevated copy started by the installer via UAC (see install.rs).
            "--elevated" => {
                let op = args.next().map(|v| v.to_string_lossy().to_string()).unwrap_or_default();
                let mut target = None;
                if args.next().map(|v| v == "--target").unwrap_or(false) {
                    target = args.next().map(PathBuf::from);
                }
                return Cli::Elevated { op, target };
            }
            "--mode" => {
                mode = match args.next().map(|v| v.to_string_lossy().to_string()).as_deref() {
                    Some("visual") => app::Mode::Visual,
                    Some("code") => app::Mode::Code,
                    _ => app::Mode::View,
                }
            }
            "--snapshot" => snapshot = args.next().map(PathBuf::from),
            "--timing" => timing = args.next().map(PathBuf::from),
            "--exit-after-paint" => exit_after_paint = true,
            "--selftest" => selftest = true,
            "--profile" => profile = args.next().map(PathBuf::from),
            "--setup" => setup_flag = true,
            "--scroll" => scroll = args.next().and_then(|v| v.to_string_lossy().parse().ok()),
            "--size" => {
                size = args.next().and_then(|v| {
                    let v = v.to_string_lossy().to_string();
                    let (w, h) = v.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                })
            }
            "--help" | "-h" | "/?" => return Cli::Help,
            _ if s.starts_with("--") => {}
            _ => {
                if file.is_none() {
                    file = Some(PathBuf::from(a));
                }
            }
        }
    }
    Cli::Run { opts: app::Options { file, mode, snapshot, timing, exit_after_paint, size, scroll, selftest, profile }, setup: setup_flag }
}

fn main() -> std::process::ExitCode {
    security::harden_process();
    // UI language — before any window (including the installer's).
    i18n::set(settings::initial_lang());

    let (opts, setup_flag) = match parse_args(std::env::args_os().skip(1)) {
        Cli::Install => {
            match install::install_interactive(None) {
                Ok(Some(outcome)) => util::info(None, &app::install_message(&outcome)),
                Ok(None) => {}
                Err(e) => util::error(None, &e),
            }
            return code(0);
        }
        Cli::Uninstall => {
            if let Err(e) = install::uninstall_interactive(None) {
                util::error(None, &e);
            }
            return code(0);
        }
        Cli::Elevated { op, target } => return code(install::elevated_main(&op, target)),
        Cli::Help => {
            util::info(None, usage());
            return code(0);
        }
        Cli::Run { opts, setup } => (opts, setup),
    };
    // Installer: vibe-md-viewer-<version>-setup.exe or the --setup switch → setup wizard window.
    if setup_flag || (setup::is_setup_exe() && opts.file.is_none() && !opts.selftest && opts.timing.is_none() && opts.profile.is_none()) {
        return code(setup::run(opts.snapshot));
    }
    // A newer exe was started while an older installation exists → offer to update it.
    // Only on a normal start without a file (double-click on the downloaded exe).
    let service = opts.snapshot.is_some() || opts.timing.is_some() || opts.selftest || opts.exit_after_paint || opts.profile.is_some();
    if opts.file.is_none() && !service {
        if let Some(inst) = install::update_available() {
            match install::offer_update(None, &inst) {
                Ok(true) => {
                    let dir = inst.target_dir();
                    util::info(None, &app::install_message(&install::Outcome::Updated { dir: dir.clone(), from: inst.version.clone() }));
                    // Launch the updated program from this (normal, non-elevated) process.
                    let _ = std::process::Command::new(dir.join(install::EXE_NAME)).current_dir(&dir).spawn();
                    return code(0);
                }
                Ok(false) => {}
                Err(e) => util::error(None, &e),
            }
        }
    }
    code(app::run(opts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn parse(args: &[&str]) -> Cli {
        parse_args(args.iter().map(OsString::from))
    }

    /// Arguments that must open the viewer.
    fn run(args: &[&str]) -> (app::Options, bool) {
        match parse(args) {
            Cli::Run { opts, setup } => (opts, setup),
            _ => panic!("expected a normal start for {args:?}"),
        }
    }

    #[test]
    fn no_arguments() {
        let (o, setup) = run(&[]);
        assert!(!setup);
        assert_eq!(o.mode, app::Mode::View);
        assert!(o.file.is_none() && o.snapshot.is_none() && o.timing.is_none() && o.profile.is_none());
        assert!(o.size.is_none() && o.scroll.is_none() && !o.selftest && !o.exit_after_paint);
    }

    #[test]
    fn file_and_mode() {
        let (o, _) = run(&["notes.md", "--mode", "visual"]);
        assert_eq!((o.file.as_deref(), o.mode), (Some(std::path::Path::new("notes.md")), app::Mode::Visual));
        assert_eq!(run(&["--mode", "code", "a.md"]).0.mode, app::Mode::Code);
        assert_eq!(run(&["--mode", "VISUAL"]).0.mode, app::Mode::View, "values are case-sensitive");
        assert_eq!(run(&["--mode"]).0.mode, app::Mode::View, "missing value");
        // The first file wins; unknown switches are skipped without taking a value.
        let (o, _) = run(&["--unknown", "first.md", "second.md", "--mode", "code", "--mode", "view"]);
        assert_eq!(o.file.as_deref(), Some(std::path::Path::new("first.md")));
        assert_eq!(o.mode, app::Mode::View, "the last --mode wins");
        // A path with spaces and Cyrillic, and one that is not valid Unicode, is kept as is.
        assert_eq!(run(&["C:\\Мои документы\\файл.md"]).0.file, Some(PathBuf::from("C:\\Мои документы\\файл.md")));
        use std::os::windows::ffi::OsStringExt;
        let odd = OsString::from_wide(&[0xD800, 0x61, 0x2E, 0x6D, 0x64]);
        match parse_args([odd.clone()].into_iter()) {
            Cli::Run { opts, .. } => assert_eq!(opts.file, Some(PathBuf::from(odd))),
            _ => panic!("expected a normal start"),
        }
    }

    #[test]
    fn service_switches() {
        let (o, setup) = run(&[
            "--snapshot", "shot.png", "--size", "800x600", "--scroll", "-120", "--timing", "t.txt", "--exit-after-paint", "--selftest",
            "--profile", "p.log", "--setup", "doc.md",
        ]);
        assert!(setup);
        assert_eq!(o.snapshot, Some(PathBuf::from("shot.png")));
        assert_eq!(o.size, Some((800, 600)));
        assert_eq!(o.scroll, Some(-120));
        assert_eq!(o.timing, Some(PathBuf::from("t.txt")));
        assert_eq!(o.profile, Some(PathBuf::from("p.log")));
        assert!(o.exit_after_paint && o.selftest);
        assert_eq!(o.file, Some(PathBuf::from("doc.md")));
        for bad in ["800", "800x", "x600", "axb", "800x600x2", "800X600", ""] {
            assert_eq!(run(&["--size", bad]).0.size, None, "--size {bad}");
        }
        assert_eq!(run(&["--scroll", "down"]).0.scroll, None);
        // A value is taken even when it looks like a switch or a file.
        let (o, _) = run(&["--snapshot", "--selftest"]);
        assert_eq!(o.snapshot, Some(PathBuf::from("--selftest")));
        assert!(!o.selftest);
        assert!(run(&["--snapshot"]).0.snapshot.is_none() && run(&["--timing"]).0.timing.is_none() && run(&["--profile"]).0.profile.is_none());
    }

    #[test]
    fn action_switches() {
        assert!(matches!(parse(&["--install"]), Cli::Install));
        assert!(matches!(parse(&["a.md", "--uninstall", "--install"]), Cli::Uninstall), "the first action wins");
        for h in ["--help", "-h", "/?"] {
            assert!(matches!(parse(&[h, "--install"]), Cli::Help), "{h}");
        }
        // A switch value is never an action: `--mode --install` is just an unknown mode.
        assert!(matches!(parse(&["--mode", "--install"]), Cli::Run { .. }));
        assert!(matches!(parse(&["--snapshot", "--uninstall"]), Cli::Run { .. }));
    }

    #[test]
    fn elevated_switch() {
        let el = |args: &[&str]| match parse(args) {
            Cli::Elevated { op, target } => (op, target),
            _ => panic!("expected --elevated for {args:?}"),
        };
        assert_eq!(el(&["--elevated", "install", "--target", "C:\\Program Files\\vibe-md-viewer"]), (String::from("install"), Some(PathBuf::from("C:\\Program Files\\vibe-md-viewer"))));
        assert_eq!(el(&["--elevated", "uninstall"]), (String::from("uninstall"), None));
        assert_eq!(el(&["--elevated"]), (String::new(), None));
        assert_eq!(el(&["--elevated", "update", "--other", "C:\\x"]), (String::from("update"), None), "only --target gives a folder");
        assert_eq!(el(&["--elevated", "update", "--target"]), (String::from("update"), None));
        assert_eq!(el(&["file.md", "--elevated", "install", "--target", "D:\\x", "--install"]).1, Some(PathBuf::from("D:\\x")));
    }

    #[test]
    fn usage_lists_the_switches() {
        let u = usage();
        for s in ["--mode view|visual|code", "--setup", "--install", "--uninstall", "--elevated", "--timing", "--exit-after-paint", "--snapshot", "--profile"] {
            assert!(u.contains(s), "{s}");
        }
    }

    #[test]
    fn exit_codes_are_clamped() {
        use std::process::ExitCode;
        assert_eq!(code(0), ExitCode::SUCCESS);
        assert_eq!(code(1), ExitCode::FAILURE);
        assert_eq!(code(5), ExitCode::from(5));
        assert_eq!(code(-1), ExitCode::from(0));
        assert_eq!(code(1000), ExitCode::from(255));
    }
}
