# Changelog

**English** | [Русский](CHANGELOG.ru.md)

## 0.2.0 — 2026-11-19

### Look
- **Color emoji** in the view — ✅ ❌ 🟡 ⚠️ 🚀, sequences like 👨‍👩‍👧, keycaps 1️⃣. RichEdit draws through GDI, which has no color fonts, so the view paints the color version on top with DirectWrite/Direct2D (Segoe UI Emoji); each emoji is rendered once and cached. `d2d1.dll`/`dwrite.dll` are loaded only for a document with emoji and only after the first paint — opening speed is unchanged. In Visual and Code modes emoji use the emoji font, in monochrome.
- Emoji and symbols (✔ ★ ☐ ⌘) no longer show as empty boxes □: they get Segoe UI Emoji / Segoe UI Symbol explicitly (RichEdit's own font fallback found no glyph for many of them).
- **Dark theme in the colors of Claude's Markdown preview:** a soft slate-gray page instead of near-black, slightly dimmed body text, bright headings, calm blue links, gray table header and code backgrounds.

### Fixes
- “Make default app for .md” opened dozens of “Open with” windows. Now it opens Windows Settings → Default apps (Windows 11 — the vibe-md-viewer page, Windows 10 — “Choose default apps by file type”).
- The “Images from the internet” bar asked again after every re-render, and failures were only in the status bar. The answer is now remembered for the document, and a failed download is shown in the bar with the reason.
- Visual mode: after widening the window or switching the column width the old layout stayed next to the new one (the text looked duplicated). The editor is now repainted completely.
- Ctrl+Shift+X cut the selection instead of striking it through.
- The `#fragment` of an image address was sent to the server (`/a.png%23f` → 404).
- HTML in Markdown:
  - multi-line `<!-- … -->` comments and tags split across lines now work;
  - an unclosed `<iframe>`, `<svg>`, `<video>`… no longer hides the rest of the document, only up to the end of its block;
  - `<form>` content is hidden, as documented;
  - `<li>`, `<hr>` and `$$…$$` inside a table cell stay in the cell;
  - an unclosed `<a href>` in a cell ends with the cell;
  - the alignment of an unclosed `<p align>` ends with its block, and `</p>` no longer ends a `<div>`'s alignment;
  - `<code>`/`<kbd>` pills always get both paddings;
  - `<3` stays text;
  - loose task lists show no bullet.
- Email autolinks `<me@example.com>` open the mail client.
- Rust highlighting: `a != b` is no longer colored as a macro.
- `lang = ru` with spaces in `settings.ini` is recognized at startup.
- A table being built counts toward the 96 MB document limit, and the RTF stays valid when the limit is hit inside a table.

### English language
- **English (default) and Russian interface** — the **EN / RU** toolbar button switches the language immediately and remembers it (`lang=` in `settings.ini`). The installer speaks the language of the installed program.
- README and documentation in both languages: `README.md` / `docs/*.md` / `CHANGELOG.md` — English, `*.ru.md` — Russian; `AGENTS.md` — project rules for contributors and AI agents; the zip contains `README.txt` and `ПРОЧТИ.txt`.

### New name: vibe-md-viewer
- The program was renamed from vibe-mdViewer to **vibe-md-viewer**: exe `vibe-md-viewer.exe`, installation folder `C:\Program Files\vibe-md-viewer`, settings `%APPDATA%\vibe-md-viewer`, installer `vibe-md-viewer-<version>-setup.exe`.
- **Moving from 0.1.x happens automatically on Update:** the program is installed into the folder with the new name, the old `vibe-mdViewer` folder is removed (once the old version is closed), registry entries with the old name are removed, settings are moved. The `VibeMdViewer` keys and the ProgID did not change — the default-app choice for `.md` is kept, Windows won't ask again.

### Open source
- MIT license; licenses of the libraries included in the program — `THIRD-PARTY-NOTICES.txt` (`scripts\notices.ps1`), shipped in the zip and the release.
- GitHub Actions: every commit — unit tests, build, self-test on a live window; a `vX.Y.Z` tag — the installer is built from the tagged sources and released with checksums and a description from the CHANGELOG. Only official `actions/*` actions.
- The test document with inline `code` was replaced with a fictional one.

### Security (independent audit before publishing)
- An all-users installation goes only into `C:\Program Files\vibe-md-viewer`: in a custom folder a regular user could tamper with it (the parent folder on a second drive is writable for them). A custom folder is allowed only for a just-for-me installation.
- The installer copies the running image itself, not a file by path: putting your own setup.exe into “Downloads” during the UAC prompt no longer works.
- An image with a strong one-sided shrink (`width=1 height=10000` on a 16384×16000 image) no longer needs gigabytes of memory.
- Syntax highlighting: thousands of unclosed `#[` / `${` no longer freeze the window.
- **Autosave never overwrites changes made by other programs:** if the file changed on disk (git pull, another editor) while you have edits, autosave stops; Ctrl+S asks, F5 reloads. A file read with replaced broken characters is not saved automatically.
- Minor: an installation path with `..`, the fallback RTF guard, a path with a backtick on the start page, invisible characters and `COM¹` in links, `&` in `mailto:`, two more kinds of local network addresses.
- GitHub Actions: actions pinned by SHA, the token is not persisted, the build has no write permission, publishing is a separate job, build provenance for releases.

### Checks
- Migration tests: recognizing a 0.1.x installation, the new folder, deleting only “our” folder, cleaning up old registry entries.
- A test for every audit finding; self-test: autosave when another program changed the file, language switching.
- **258 unit tests** covering every module. They include a WinHTTP request against a local 127.0.0.1 server (never the internet), color emoji rendering, and every `classify` branch for links and paths.
- Self-test, 56 checks: added the “?” button (a real click), Visual-mode repaint after a width change, the images bar, keyboard shortcuts and toolbar commands, and color emoji (also after scrolling).
- **Code coverage 87.7% of lines** (was 67.6%). `scripts\coverage.ps1` measures unit tests + self-test on an instrumented build. CI publishes the report and fails below 85%. Not covered: system dialogs, UAC and installing into Program Files.
- Tests on a real RichEdit run one at a time (in parallel the pixel check occasionally saw an empty frame).

## 0.1.3 — 2026-11-18

### Smoothness
- **Scrolling at the monitor's refresh rate.** The message loop drives the frames and waits for the display update (`DwmFlush`): at 144 Hz — 144 even steps per second. Previously frames were driven by the Windows timer (~15.6 ms granularity): ~60 uneven frames, which looked like “low fps” on a 144 Hz monitor.
- A scroll frame is twice as cheap: inline `code` chips no longer ask RichEdit for the position of every chip on every frame — the geometry is computed once and shifted. A 2560×1380 window: 5.4 → 2.6 ms on average, the worst frame 13 → 6 ms (the budget at 144 Hz is 6.9 ms).

### Width
- **Fixed:** in full-width mode the text extended further right than code blocks and heading rules. Causes: a RichEdit table can't be wider than 22 inches (2112 px), and on a 2560 px monitor the text was wider; at zoom ≠ 100% RichEdit multiplies the margins by the zoom; the width was recalculated only if the window changed by more than 24 px; the scrollbar appeared after loading and made the text narrower. Now the column is at most 22 inches wide (the rest is split equally on both sides), margins are corrected for zoom, any width change is recalculated, and the scrollbar is always present. The right edge of text, code blocks and rules matches in every mode and at any zoom.
- A maximized window opens at the right size immediately — the document is rendered once, the width doesn't jump.
- Inline `code` wraps to the next line as a whole — no piece of the chip background left at the end of the line.

### Checks
- A test on a real RichEdit: margins at 50/100/150% zoom and the table width limit.
- Self-test: the right edge of blocks = the column edge (window 13 px wider, zoom 150% and 50%, narrow column); `code` chips after frame-by-frame scrolling match a full repaint; wheel scrolling runs at the display's frame rate (41 checks in total).
- `scripts\profile.ps1 -Size 2560x1380` — scroll frame time (average, 95th percentile, worst).

## 0.1.2 — 2026-11-18

### Speed
- **Chunked loading.** The part of the document with the wanted position is shown first; the rest loads in the background between events — the window doesn't freeze. A 275 KB document: opening 520 → 60 ms until shown; theme change 540 → 75 ms, width toggle 545 → 70 ms. The reading position is kept when the theme/width/zoom changes.
- Switching View ↔ Visual without edits is almost instant (the editor isn't recolored and the text isn't re-wrapped if the margins are the same).
- **Smooth scrolling** with the wheel, arrows, PgUp/PgDn, Space (by pixels, ~60 fps) instead of RichEdit's line-by-line jumps (a code block scrolled in one jump).
- Images are no longer decoded again on a theme change.

### Look
- Inline `code` — a background of the text height with rounded corners, like in VS Code (RichEdit used to fill the whole line height, and chips on adjacent lines merged).
- Paragraph spacing +2 px.
- There's space below the end of the document: the last paragraph scrolls up to the middle of the screen.
- **Fixed:** stripes with pieces of text/code at the top and bottom of the window (RichEdit margins weren't repainted on scroll).
- **Fixed:** at zoom ≠ 100% code blocks and tables were narrower than the text; zoom was reset to 100% on every re-render.
- **Fixed:** End didn't reach the end of a large document.

### Convenience
- Mouse text selection in the view (drag, Shift+click, double-click — word, Ctrl+A), copying without hidden characters.
- Zoom buttons `− 100% +` on the toolbar (100% — reset); Ctrl+wheel works with touchpads too.
- **Autosave** like in an IDE: a pause in typing, switching to view, leaving the window, closing. No more Open/Save buttons on the toolbar (Ctrl+O / drag-and-drop, Ctrl+S — save right away).

### Checks
- Test: a document loaded in chunks starting from the middle equals the fully loaded one character by character.
- Self-test: mouse selection, smooth scrolling, autosave, code chip markup (34 checks in total).
- `scripts\profile.ps1` / `--profile` — measuring heavy operations on a large document.

## 0.1.1 — 2026-11-18

### Look
- **Syntax highlighting** in code blocks — in the view and in the editors (Visual, Code): Rust, C/C++/C#, Java, JS/TS, Go, Python, Shell, PowerShell, SQL, HTML/XML, CSS, JSON/YAML/TOML/INI, Lua, Ruby, diff. Our own lightweight lexer (`highlight.rs`): one pass, linear time, blocks larger than 256 KB are shown without highlighting.
- Code blocks without a border, with padding, like on GitHub; line spacing 1.3.
- **Fixed:** in the dark theme, colors inside a code block weren't drawn — on a very dark cell background RichEdit draws all text in one “inverse” color. The code block background in the dark theme is slightly brighter than the threshold (checked by a test on a real RichEdit).
- Inline `code`: thinner side padding (a thin space in a proportional font instead of a whole Consolas character), its own calm background color, size relative to the surrounding text (larger in headings); lines with and without code have the same height.
- The space above a heading no longer stretches the `code` background in the heading upwards.
- **Full width:** text takes the whole window; the `→←` / `←→` toolbar button switches to a narrow reading column (remembered). The first window is ~85% of the screen, centered.

### Other
- `scripts\package.ps1` no longer deletes installers of previous versions from `dist\` (they're needed to test updating).

## 0.1.0 — 2026-11-18

The first version.

### Distribution and security audit
- Updating by running the new exe: the file in the installation folder is replaced (even if the old version is running), settings and the `.md` association are kept, versions are compared, the updated program starts.
- Installs like ordinary software: `C:\Program Files\vibe-mdViewer` + HKLM via UAC (only for the duration of the operation; the viewer itself runs with normal rights); without administrator rights — a just-for-me installation into `%LOCALAPPDATA%\Programs`. Settings of the installed program — `%APPDATA%\vibe-mdViewer`.
- The installer no longer overrides another default app for `.md` — the user chooses.
- **Fixed:** DLL planting next to the exe — `uxtheme`, `dwmapi`, `bcryptprimitives` (not KnownDLLs) were loaded from the program folder before our code started; dangerous for an installer run from “Downloads” with administrator rights. Now only KnownDLLs load at startup, the rest is delay-loaded and only from System32.
- The exe version (file properties) comes from `Cargo.toml`.
- `scripts\package.ps1`: a distribution zip with instructions and SHA-256.
- **Fixed:** RTF injection in the editing modes — a document starting with `{\rtf` was parsed by RichEdit as RTF (confirmed by a test). Text is now set and inserted only through TOM as plain text.
- **Fixed:** a crash on a table with more than 60 columns (out-of-bounds index).
- **Fixed:** a possible crash when opening find (re-entrant state borrow while the dialog was created).
- **Fixed:** extension masking with trailing dots/spaces (`setup.exe.`, `setup.exe%20`) bypassed the executable block.
- Images from the internet: redirects are checked manually (can't redirect to `192.168.x.x`), non-canonical IP notations (`127.1`, `0x7f.0.0.1`) are forbidden, the host name is checked via DNS.
- Saving: a temporary file with a unique name and `create_new` — a planted file or symbolic link is never overwritten.
- The memory limit per image lowered to 256 MB; RTF group nesting limited; panics while parsing a document are caught.
- `mailto:` — only the address, subject, body, cc (no `attach=`).
- OLE drag-and-drop into the editor disabled; deletion via `cmd /v:off`.
- Tests: “bombs” (deep nesting, 60,000 brackets, 300 columns…), registry and copying on install/update, an extended self-test (29 checks).

### Installer and convenience
- The setup wizard window: `vibe-mdViewer-<version>-setup.exe` (folder, “Open .md files with vibe-mdViewer”, “Launch after install”; for an installed program — Update / Reinstall / Uninstall).
- Dark theme by default.
- Start page: a visible “The program is not installed — Install…” bar; the Visual/Code buttons on the help page offer to open your own file instead of silently refusing.

### Fixes from the independent audit
- **High:** 8.3 short names (`X~1.LIB` → `x.library-ms`) bypassed the dangerous-type block. Now the type is checked by the long name at click time, and only an allow list of safe types can be opened; everything else — “Show in Explorer”. Paths with symbolic links are rejected.
- `width`/`height` on `<img>` can no longer cause a terabyte allocation.
- Limits against memory explosions: address length, `data:` budget, table cells, total RTF size; deduplication and cancellation of stale image jobs.
- Linear algorithms instead of quadratic ones: bare URLs, the HTML tokenizer, entities, RTF groups.
- Protected rights on the installation folder, `PING.EXE` by full path on uninstall.
- Panic guards in all window procedures; fixed crashes on Ctrl+click next to a no-break space and in the toolbar after the button set changed.
- The spoofed-address check is not bypassed by invisible characters and look-alike dots; an unclosed `<a href>` is closed at the paragraph boundary.
- No endless error dialogs if a modified file can't be read; unclosed `<form>`/`<template>` no longer hide the rest of the document.
- IPv4 inside IPv6 (`::a.b.c.d`, 6to4) is treated as a local network address; a 30 s total limit per image download; `ES_NOOLEDRAGDROP`.

### View
- A native Win32 + RichEdit window, no browser engine. Opening a document ≈ 80–100 ms.
- CommonMark + GFM: headings, lists, tasks, quotes, GitHub alerts, tables with alignment, code blocks, footnotes, definitions, front matter, formulas as code.
- A safe subset of HTML: `<img>`, `<a>`, `<br>`, `<b>/<i>/<s>/<u>`, `<kbd>/<code>`, `<sub>/<sup>`, `<details>/<summary>`, `align="center"`.
- PNG/JPEG/GIF/BMP/WebP/ICO images and `data:`; background decoding, caching, fitting to the width.
- Clickable links, anchors, navigation between `.md` files with Back history, bare URLs.
- Find, zoom, light/dark theme (follows the system), copying, context menu.
- Watching the file for changes on disk.

### Editing
- Visual mode: markup is hidden outside the cursor line, lossless saving.
- Code mode: Markdown highlighting and a live preview with synchronized scrolling.
- Formatting toolbar and shortcuts, list continuation on Enter, Tab/Shift+Tab.
- Atomic saving that keeps the encoding and line endings.

### Security
- Link allow list, blocking of protocols, UNC, devices, executables, bidi spoofing.
- Warnings about spoofed addresses, the real address visible before the click.
- Images from the internet — only with permission, without sending credentials, no local network addresses.
- Paste plain text only, no OLE/RTF from the clipboard.
- DLLs looked up only in System32, no DLLs from the network, CFG, static CRT.

### Installation
- A built-in installer: copy into `D:\Programs\vibe-mdViewer`, `.md` association in HKCU, the system app-choice dialog, an entry in “Uninstall programs”.
