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

fn main() {
    security::harden_process();
    // UI language — before any window (including the installer's).
    i18n::set(settings::initial_lang());

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
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        let s = a.to_string_lossy().to_string();
        match s.as_str() {
            "--install" => {
                match install::install_interactive(None) {
                    Ok(Some(outcome)) => util::info(None, &app::install_message(&outcome)),
                    Ok(None) => {}
                    Err(e) => util::error(None, &e),
                }
                return;
            }
            "--uninstall" => {
                if let Err(e) = install::uninstall_interactive(None) {
                    util::error(None, &e);
                }
                return;
            }
            // Elevated copy started by the installer via UAC (see install.rs).
            "--elevated" => {
                let op = args.next().map(|v| v.to_string_lossy().to_string()).unwrap_or_default();
                let mut target = None;
                if args.next().map(|v| v == "--target").unwrap_or(false) {
                    target = args.next().map(PathBuf::from);
                }
                std::process::exit(install::elevated_main(&op, target));
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
            "--help" | "-h" | "/?" => {
                util::info(None, usage());
                return;
            }
            _ if s.starts_with("--") => {}
            _ => {
                if file.is_none() {
                    file = Some(PathBuf::from(a));
                }
            }
        }
    }
    // Installer: vibe-md-viewer-<version>-setup.exe or the --setup switch → setup wizard window.
    if setup_flag || (setup::is_setup_exe() && file.is_none() && !selftest && timing.is_none() && profile.is_none()) {
        std::process::exit(setup::run(snapshot));
    }
    // A newer exe was started while an older installation exists → offer to update it.
    // Only on a normal start without a file (double-click on the downloaded exe).
    let service = snapshot.is_some() || timing.is_some() || selftest || exit_after_paint || profile.is_some();
    if file.is_none() && !service {
        if let Some(inst) = install::update_available() {
            match install::offer_update(None, &inst) {
                Ok(true) => {
                    let dir = inst.target_dir();
                    util::info(None, &app::install_message(&install::Outcome::Updated { dir: dir.clone(), from: inst.version.clone() }));
                    // Launch the updated program from this (normal, non-elevated) process.
                    let _ = std::process::Command::new(dir.join(install::EXE_NAME)).current_dir(&dir).spawn();
                    return;
                }
                Ok(false) => {}
                Err(e) => util::error(None, &e),
            }
        }
    }
    let code = app::run(app::Options { file, mode, snapshot, timing, exit_after_paint, size, scroll, selftest, profile });
    std::process::exit(code);
}
