# vibe-md-viewer

**English** | [Русский](README.ru.md)

[![CI](https://github.com/SiTiROS/vibe-md-viewer/actions/workflows/ci.yml/badge.svg)](https://github.com/SiTiROS/vibe-md-viewer/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/SiTiROS/vibe-md-viewer)](https://github.com/SiTiROS/vibe-md-viewer/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A fast native Markdown viewer and editor for Windows 10/11.
A single small `exe` (≈ 2.7 MB), no browser engine (no Edge / WebView2 / Electron), no internet access by default. Written in Rust on top of Win32 and RichEdit.

**Download:** [latest release](https://github.com/SiTiROS/vibe-md-viewer/releases/latest) → `vibe-md-viewer-<version>-setup.exe`. The exe is built from the tagged sources by GitHub Actions; SHA-256 checksums and a build provenance attestation are published next to it.

- **Speed.** The window with the rendered document appears in about 50–100 ms (measurements — [docs/BUILD.md](docs/BUILD.md#startup-time)).
- **Images and links.** Local images are shown immediately, images from the internet only with your permission. Links are clickable; the real address is shown at the bottom of the window before you click.
- **Three modes:** view, visual editing (like a word processor), code (source + live preview).
- **Security.** HTML and scripts in `.md` files are never executed; dangerous protocols, network paths and executables in links are blocked. Details — [docs/SECURITY.md](docs/SECURITY.md).
- **Two languages.** English (default) and Russian interface — the **EN / RU** button on the toolbar.

## Installation

Only Windows 10/11 x64 is needed. The installer is `vibe-md-viewer-<version>-setup.exe` from the [releases](https://github.com/SiTiROS/vibe-md-viewer/releases) page (or build it yourself: `scripts\package.ps1` → `dist\`).

The installer is the same exe: if the file name contains “setup”, it opens the setup wizard (“Open .md files with vibe-md-viewer”, “Launch after install”, Install / Update / Uninstall buttons). The same wizard opens from the “Install…” button on the yellow bar of the start page, or with `--setup`.

It installs like ordinary software: into Program Files, without shortcuts, services or autostart.

1. Run `vibe-md-viewer-<version>-setup.exe`.
2. Click **Install**:
   - Windows asks for administrator rights (UAC) — only for the duration of the installation;
   - the program copies itself to `C:\Program Files\vibe-md-viewer` (for all users — only there, so a regular user cannot tamper with it) and registers for `.md`, `.markdown` and similar extensions for all users, plus an entry in “Apps & features”;
   - without administrator rights (e.g. a work PC) it offers a just-for-me installation into `%LOCALAPPDATA%\Programs\vibe-md-viewer`, without UAC;
   - then Windows Settings → Default apps opens: on Windows 11 the vibe-md-viewer page (set it for `.md`), on Windows 10 click “Choose default apps by file type”, find `.md` and pick **vibe-md-viewer**. The same from the start page: “Make default app for .md”.

The viewer itself always runs with normal rights — elevated rights are needed only by the installer copy, for a few seconds.

Windows does not let programs silently make themselves the default app — this is a system protection, so the last step is one click by the user.

The exe is not code-signed, so after downloading it on another computer SmartScreen shows “Unknown publisher” → “More info” → “Run anyway”. You can verify that a release file was built by GitHub Actions from this repository: `gh attestation verify vibe-md-viewer-<version>-setup.exe -R SiTiROS/vibe-md-viewer`.

From the command line: `vibe-md-viewer.exe --install`. Uninstall: “Settings → Apps” or `vibe-md-viewer.exe --uninstall`.

## Updating

**Just run the new version's `setup.exe`** and click **Update** (or run the new `vibe-md-viewer.exe` — it asks “Update 0.1.3 → 0.2.0?” by itself). For a Program Files installation that is a single UAC prompt:

- the program file in the installation folder is replaced (if the old version is running, it is renamed to `vibe-md-viewer.old.exe`, keeps working until closed and is deleted on the next start);
- registry entries are updated; the path and program ID stay the same — your default-app choice for `.md` is kept, Windows does not ask again;
- settings (`%APPDATA%\vibe-md-viewer\settings.ini`) are not touched;
- the updated program starts right after the update.

No need to uninstall and reinstall. There is no auto-update over the internet and none is planned — the program never goes online by itself.

**From 0.1.x (vibe-mdViewer) to 0.2.0:** the program was renamed to vibe-md-viewer. **Update** installs it into the `vibe-md-viewer` folder next to the old one, removes the old `vibe-mdViewer` folder (once the old version is closed) and the registry entries with the old name, and moves the settings from `%APPDATA%\vibe-mdViewer` to `%APPDATA%\vibe-md-viewer`. The default-app choice for `.md` is kept.

### What is written where

| What | Where |
| --- | --- |
| Program | `C:\Program Files\vibe-md-viewer\vibe-md-viewer.exe` (without administrator rights — `%LOCALAPPDATA%\Programs\vibe-md-viewer`) |
| Settings | `%APPDATA%\vibe-md-viewer\settings.ini` (a portable copy without installation — `data\settings.ini` next to the exe) |
| `.md` association, “Apps & features” entry | `HKLM\Software\Classes`, `HKLM\Software\VibeMdViewer`, `HKLM\...\Uninstall\VibeMdViewer` (for a just-for-me installation — the same under HKCU) |
| Shortcuts, caches, services, autostart | **nothing** |

The installer never overrides another program that is the default for `.md` — the user chooses in the system window. Settings in `%APPDATA%\vibe-md-viewer` are kept on uninstall.

## Keyboard shortcuts

| Action | Keys |
| --- | --- |
| View / visual / code | Ctrl+1 / Ctrl+2 / Ctrl+3, Esc — back to view |
| Open a file | Ctrl+O or drag the file into the window |
| Save | **automatic** — a second after you stop typing, when switching to view, when leaving the window and on close; Ctrl+S — right away, Ctrl+Shift+S — save as. If another program changed the file in the meantime (git pull, another editor), autosave stops and overwrites nothing — Ctrl+S asks, F5 reloads |
| Find / next | Ctrl+F / F3 (Shift+F3 — previous) |
| Reload the file | F5 |
| Back through links | Alt+← or Backspace (in view) |
| Zoom | the −  100%  + buttons on the right of the toolbar (100% — reset), Ctrl+wheel, Ctrl+plus / minus / 0 |
| Select text in view | drag with the mouse (Shift+click — extend), double-click — word, Ctrl+A — all, Ctrl+C — copy |
| Scrolling | wheel, arrows, PgUp/PgDn, Space — smooth, at the monitor's refresh rate (60/144 Hz…); Home/End — start/end; the last paragraph can be scrolled up to the middle of the screen |
| Bold / italic / strikethrough | Ctrl+B / Ctrl+I / Ctrl+Shift+X |
| Heading 1–6 / plain text | Ctrl+Shift+1…6 / Ctrl+Shift+0 |
| Bulleted / numbered list | Ctrl+Shift+8 / Ctrl+Shift+7 |
| Quote / code / link | Ctrl+Q / Ctrl+E / Ctrl+K |
| Open a link in the editor | Ctrl+click |
| Interface language | the **EN / RU** button on the right of the toolbar (English by default; remembered as `lang=` in `settings.ini`) |
| Full width / narrow column | the `→←` / `←→` button on the right of the toolbar (remembered); text, code blocks and rules always end at the same right edge; on a very wide monitor the column is at most 22 inches wide (2112 px at 100%) and centered |

In lists Enter continues the list, Enter on an empty item ends it, Tab / Shift+Tab change the level.

## Modes

- **View** — the formatted document: headings, lists, tasks, quotes, GitHub alerts (`> [!NOTE]`), tables, code blocks with syntax highlighting (Rust, C/C++/C#, Java, JS/TS, Go, Python, Shell, PowerShell, SQL, HTML/XML, CSS, JSON/YAML/TOML/INI, Lua, Ruby, diff), images, color emoji ✅ 🟡 ❌, footnotes, front matter, a safe subset of HTML (`<img>`, `<br>`, `<b>`, `<kbd>`, `<details>`, `align="center"`…). Full-width text or a narrow column — a toggle on the toolbar.
- **Visual** — editing “as a user”: Markdown markup characters are hidden and appear only on the line with the cursor. The file always stays plain Markdown and is saved without losses.
- **Code** — the Markdown source with markup highlighting on the left and a live preview on the right.

## Building from source

Rust stable (`x86_64-pc-windows-msvc`) and Visual Studio Build Tools with the “Desktop development with C++” workload:

```powershell
cargo test --release          # unit tests (some on a real RichEdit control)
cargo build --release         # target\release\vibe-md-viewer.exe
powershell -ExecutionPolicy Bypass -File scripts\selftest.ps1 -Exe target\release\vibe-md-viewer.exe
```

Details (caches inside the project folder, measurements, distribution package) — [docs/BUILD.md](docs/BUILD.md). Every commit is checked by [CI](.github/workflows/ci.yml): tests, build, self-test on a live window. Releases are built from a `vX.Y.Z` tag ([release.yml](.github/workflows/release.yml)).

## Documentation

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — how it works.
- [docs/SECURITY.md](docs/SECURITY.md) — threat model, research of vulnerabilities in Markdown apps, measures taken.
- [docs/BUILD.md](docs/BUILD.md) — build, tests, measurements, command-line options.
- [CHANGELOG.md](CHANGELOG.md) — version history.
- [AGENTS.md](AGENTS.md) — project rules for contributors and AI coding agents.

Russian versions: `*.ru.md` next to each document.

## Security

Found a vulnerability? Report it privately: **Security → Report a vulnerability** in this repository (not in public issues). Threat model and measures — [docs/SECURITY.md](docs/SECURITY.md).

## License

[MIT](LICENSE). Licenses of the libraries included in the program (all MIT / Apache-2.0 / BSD / Zlib) are in the release's `THIRD-PARTY-NOTICES.txt` (generated by `scripts\notices.ps1`).
