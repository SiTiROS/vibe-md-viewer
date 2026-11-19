# Architecture

**English** | [Русский](ARCHITECTURE.ru.md)

## The stack and why

| Decision | Why |
| --- | --- |
| **Rust** | Native-code speed without a runtime, plus memory safety in the riskiest code (parsing untrusted `.md`, converters, threads). |
| **Win32 + RichEdit** (`msftedit.dll`) | Built into Windows: formatted text, tables, images, editing, undo. The window starts in tens of milliseconds. |
| No WebView2/Edge/Electron | WebView2 is the Edge engine (the owner wants to be able to remove Edge), and it starts several times slower than a native window. A browser engine is also the main attack surface of Markdown apps (see [SECURITY.md](SECURITY.md)). |
| **pulldown-cmark** | A fast, battle-tested (rustdoc, mdBook) CommonMark + GFM parser. It reports source ranges — the editor highlighting is built on them. |
| **image** (Rust decoders only) | PNG/JPEG/GIF/BMP/WebP/ICO are decoded by safe Rust code with limits. Windows system codecs never see untrusted bytes. |
| Static CRT, CFG | The exe does not need the VC++ Redistributable; Control Flow Guard is enabled by a compiler flag. |

Alternatives were measured or estimated when the stack was chosen (C++ Win32, C# WPF/WinForms/Avalonia, WebView2, Electron). Result: Rust + RichEdit is as fast as C++ with stronger safety guarantees.

## Modules (`src/`)

| File | Purpose |
| --- | --- |
| `main.rs` | Argument parsing, install/uninstall, starting the window. |
| `security.rs` | Process hardening in the very first millisecond: DLLs only from System32, no DLLs from network paths. |
| `app.rs` | Main window: modes, layout, links, images, find, saving, keyboard shortcuts. |
| `render.rs` | Markdown → RTF for the view (with full escaping). |
| `html.rs` | A tiny tokenizer for inline HTML (formatting only, nothing is executed). |
| `styler.rs` | Splits the source into formatting “runs” for the Visual and Code modes. |
| `highlight.rs` | Syntax highlighting of code blocks (view and editors): one pass, no regular expressions, linear time, 256 KB limit per block. |
| `editor.rs` | Applies the runs to the editor through TOM with incremental updates. |
| `links.rs` | **Link and path policy** (allow list). |
| `images.rs` | Resolving image `src` by policy, a decoding thread pool, re-encoding into clean PNG/JPEG. |
| `net.rs` | Downloading images from the internet via WinHTTP (only with permission). |
| `richedit.rs` | A thin wrapper over RichEdit messages. |
| `emoji.rs` | Color emoji in the view: RichEdit draws through GDI (no color fonts), so each emoji cluster is rendered once with DirectWrite/Direct2D (Segoe UI Emoji) into a cached bitmap and blended over RichEdit's monochrome glyph. |
| `ui.rs` | Custom GDI bars: toolbar, info bar, status bar. |
| `doc.rs` | Reading/writing: encodings (UTF-8/BOM, UTF-16, ANSI), line endings, atomic save. |
| `install.rs` | Installation, registry, opening Windows Settings → Default apps, update, uninstall. |
| `i18n.rs` | Interface language: English/Russian, the `tr!` and `trf!` macros. |
| `settings.rs`, `theme.rs`, `dialogs.rs`, `util.rs` | Settings, theme colors, system dialogs, Win32 helpers. |

## From a double-click to pixels on screen

1. `security::harden_process()` — DLL loading policy.
2. `msftedit.dll` is loaded from System32; the window and two RichEdit controls are created (the view is read-only; the editor stays hidden until the first switch to an editing mode).
3. `doc::load` — reads the file (64 MB limit), detects the encoding, normalizes line breaks.
4. `render::render` — one pulldown-cmark pass → an RTF string. All document text is escaped.
5. **Chunked loading** (see below): `EM_SETTEXTEX` gets only the part of the RTF around the wanted position, the window is shown and painted (startup time is measured here), the remaining parts load in the background.
6. Images: each one is checked by the policy (`images::resolve`), then queued to a thread pool. Threads are created lazily: a document without images doesn't pay for them.
7. A finished image (already scaled to the column width and re-encoded) is inserted exactly at its hidden marker via TOM `ITextRange2::SetText2(tomConvertRTF)`.

## Chunked loading

RichEdit parses RTF slowly — about 0.5 µs per byte (a 275 KB document with hundreds of code blocks is 1 MB of RTF ≈ 500 ms), while the same text without formatting loads in 50 ms: the parser itself is expensive and can't be sped up. Therefore:

- `render` marks chunk boundaries (~40 KB of RTF) — only between ordinary top-level paragraphs when everything is closed (not inside tables, quotes or groups), so every chunk is valid RTF on its own (`Rendered::splits`, together with the source offset);
- `render_view` loads the chunk with the wanted position (`Anchor`: the chunk's source offset + character index + on-screen position) plus enough to have two screens below it; the rest loads one chunk per `T_LOAD` timer tick (WM_TIMER arrives only when there is no input and no painting — the window stays responsive);
- chunks above the screen are inserted at the start with scroll compensation (the reading position doesn't jump), chunks below at the end; inserting via TOM `SetText2(tomConvertRTF)` doesn't touch the selection;
- when the whole text is replaced, RichEdit “eats” the last `\par` (it merges with the final paragraph), but not when inserting via TOM; so only the first loaded chunk gets an extra tiny empty paragraph (test `chunked_load_equals_full_load`: a document assembled from chunks starting in the middle equals the fully loaded one character by character);
- find, jumping to a section, Ctrl+A, Home/End, snapshots and the self-test load everything first (`finish_loading`); images and captions from not-yet-loaded chunks wait in `pending_*`.

Changing the theme, width or zoom, and editing in Code mode re-render the document “at the same place” (`view_anchor`). Measured (`scripts\profile.ps1`, 275 KB): opening — 60 ms until shown + 110 ms in the background (was 520 ms in one piece), theme change — 75 ms (was 540).

## Hidden markers in the view

RichEdit can't attach data to text, so the renderer inserts **hidden** (`\v`) Private Use Area characters:

| Characters | Meaning |
| --- | --- |
| `U+E000 + id` … `U+F8FE` | start/end of link `id` |
| `U+E800 + id` | heading anchor `id` (for `#section`) |
| `U+EC00 + id` … `U+F8FD` | the place of image `id`; between the markers — a hidden caption `[🖼 alt]` shown if the image can't be displayed |
| `U+202F` … `U+202F` | visible narrow no-break spaces padding inline `code` — the view draws the chip background between them; they are no-break so the chip wraps to the next line as a whole (such spaces from the document itself are replaced with a thin `U+2009` of the same width) |

After loading, the control text is read with `GT_RAWTEXT` (character positions match the internal ones, including table control characters) and a position index is built. Hover and click → `EM_CHARFROMPOS` → look up the link in the index → the `links::classify` policy.

The same characters coming from the document itself are replaced with `U+FFFD`, so a marker can't be forged.

Why not RichEdit `HYPERLINK` fields: they can't be controlled as strictly, and we keep all link handling to ourselves.

## Formatting RichEdit can't do

RichEdit doesn't draw paragraph borders, therefore:

- the bar on the left of a quote is a one-row table with a left cell border (one row per paragraph; together they look like a solid bar);
- the rule under H1/H2 and the horizontal rule are tables with a bottom/top border;
- a code block is a one-cell table with a background (`\clcbpat`) and padding, without borders; highlight colors are `\cfN` groups (color table indices 12–18);
- the grey cell grid is turned off with `SES_HIDEGRIDLINES`.

RichEdit pitfalls found by measurement (tests in `render.rs`):

- **A dark cell background eats text colors.** If the brightness of `\clcbpat` is below ~20% (measured: `46,52,60` — colors are there, `44,50,58` — gone), RichEdit draws all the cell text in one color, the “inverse” of the background. So the code block background in the dark theme is `47,54,64`; the test `dark_code_block_keeps_syntax_colors` draws a block with a real RichEdit and looks for pixels of the keyword color. Character background (`\highlight`) is not affected.
- **Character background (`\highlight`) fills the whole line height** at any line spacing, together with the paragraph's `\sb` — inline `code` chips on adjacent lines merged into a block. So the view draws the chip background itself (`paint_chips`): after RichEdit has painted, only in the updated region (otherwise it would be applied twice), as a translucent rectangle of the font height from the baseline (TOM `GetPoint(TA_BASELINE)`), with rounded corners; the text under it barely changes color. Asking RichEdit for a position costs ~50 µs, so a chip's geometry is computed once and stored in document coordinates (`chip_geo`): a scroll frame only has to shift it. The cache is reset when the text, margins, zoom, text length or document height change, and when an image caption is revealed; the self-test compares the picture after frame-by-frame scrolling with a full repaint. Editors still use `\highlight` (`code_inline_bg`).
- **Top/bottom `EM_SETRECT` margins are not repainted on scroll** — stripes of foreign text remained in them. In the view the top padding and the space after the end are paragraphs inside the document; in the editor the margin strips are repainted on every scroll change (`fix_margin_strips`).
- **`EM_SETRECT` with the same margins still re-wraps all the text** — tens of ms on a large document on every mode switch. `set_margins` skips the call if nothing changed.
- **Zoom (`EM_SETZOOM`) multiplies the margins too** — `EM_SETRECT`/`EM_SETMARGINS` set margins “before zoom”. Below 100% the required margins can be wider than the window itself: `EM_SETRECT` rejects such a rectangle (resets the margins), `EM_SETMARGINS` accepts it. So left/right margins use `EM_SETMARGINS`, top/bottom use `EM_SETRECT`.
- **A table is never wider than 22 inches** (31680 twips = 2112 px at 96 dpi and 100%): `\cellx` beyond that is silently clipped. Code blocks, rules and quotes are tables, so the text column is never wider than this limit (see “Column width”).
- **The scrollbar appears only for a long document** and makes the text narrower while tables keep their width — the view and the editor always have it (`ES_DISABLENOSCROLL`).
- **Replacing the whole text (`EM_SETTEXTEX`) resets zoom to 100%** — zoom is restored from the settings afterwards. Table and code block widths take zoom into account, otherwise they'd be narrower or wider than the text at zoom ≠ 100%.
- **`SB_BOTTOM` doesn't reach the end of a large document** — Home/End and the wheel scroll by pixels themselves (`EM_SETSCROLLPOS`; 32-bit coordinates verified on a 196,000 px document).
- **A space in Consolas is a full character wide**, including the thin one (U+2009). The chip padding uses a narrow space in Segoe UI.
- **RichEdit draws invisible “joiner” characters (U+2060) as a dot** — wrapping is prevented with a no-break space instead.

The test `richedit_layout_assumptions` checks the margins at 50/100/150% zoom and the table width limit on a real RichEdit.

## Column width

One function, `App::column`, computes both the RichEdit margins and the table/image width for the renderer, so the right edge of text, code blocks and rules is the same (the self-test checks it by pixels: the initial window, a window 13 px wider, zoom 150% and 50%, the other column mode).

- `settings.wide` (default `true`): margins of 3% of the window width (at least 28 px); otherwise a 900 px column (× zoom), centered.
- The column is at most 22 inches × zoom wide (the RichEdit table limit): on a large monitor the extra space is split equally on both sides. 2560 px at 100% → a 2112 px column.
- Margins are computed in screen pixels and divided by the zoom (RichEdit multiplies them back).
- Any width change (even 1 px) triggers a re-render 160 ms after the window resize (`T_RESIZE`).
- A maximized window gets the maximized size while still hidden (`presize_maximized`) — the document is rendered once and at the right width.

The first window without a saved position takes 85×88% of the work area, centered.

## Scrolling, selection, zoom

- Wheel, arrows, PgUp/PgDn, Space — smooth scrolling: the target accumulates, the position approaches it exponentially (~60 ms, based on elapsed time). The message loop drives the frames: while scrolling it processes pending messages, makes a step (`EM_SETSCROLLPOS` + `UpdateWindow`) and waits for the next display frame (`DwmFlush`) — as many steps as the monitor has hertz (60/144/…), evenly spaced. The Windows timer (~15.6 ms granularity) gave ~60 uneven frames, which looked like “low fps” on 144 Hz. The `T_SMOOTH` timer (30 ms) remains as a fallback for modal loops (menus, window dragging). A frame at 2560×1380: 2.6 ms on average, 95% under 3.6 ms (the frame budget at 144 Hz is 6.9 ms). Touchpads with small steps work the same way.
- Ctrl+wheel — zoom (steps accumulate up to a “notch”), and the `−  100%  +` toolbar buttons.
- Drag selection in the view is implemented by the app (`WM_LBUTTONDOWN`/`WM_MOUSEMOVE`/`WM_LBUTTONUP`, scrolling beyond the window edge): a read-only RichEdit didn't do it. RichEdit gets the focus, and the blinking caret is hidden with `HideCaret` after every message. Double-click (word) is RichEdit's own. A click on a link without dragging follows the link.

## Autosave

Like in an IDE: 1.2 s after you stop typing (`T_AUTOSAVE`), when switching to view, when leaving the window, and when closing or opening another file — without questions. Quietly: a write error goes to the status bar, no dialog. The write is the same atomic save (`doc::save`). Turned off with `autosave=0` in `settings.ini` (then the Save button and the question on close come back).

Autosave never overwrites someone else's changes: if the file changed on disk while you have unsaved edits (git pull, another editor), autosave stops; Ctrl+S asks whether to write your version, F5 reloads. A file that was read with replaced broken characters (invalid UTF-8/UTF-16, NUL) is not saved automatically either.

## Interface language

English (default) and Russian. Strings live right in the code as pairs: `tr!("English", "Русский")` → `&str`, `trf!("Saved {n}", "Сохранено {n}")` → `String` (named placeholders — word order may differ between languages). The language is a process-wide flag (`i18n::set`) chosen before the first window: from `lang=` in the app's own `settings.ini`, and for a downloaded installer — from the installed program's settings (`settings::initial_lang`). The **EN / RU** toolbar button switches the language immediately: bars, status bar, start page (`assets/welcome.en.md` / `welcome.ru.md`). Code comments, test and self-test messages are in English.

## Editing modes

The editor always contains **the Markdown itself**: lossless saving, no reverse conversion.

- After each edit (`EN_CHANGE` → a deferred `WM_APP_RESTYLE`) the text is re-parsed by `styler::analyze` (pulldown-cmark with ranges) into runs with bit flags (`STRONG`, `MARKER`, `LINK`, `URL`, heading level…).
- `editor::Styler` compares the new runs with the old ones (common text prefix/suffix) and applies only the changed ones through TOM. Formatting changes don't go into the undo history (`tomSuspend`); painting is frozen (`Freeze`).
- “Visual”: proportional font, large headings, markers (`**`, `#`, `](url)`, `> `) hidden everywhere except on the cursor line; unordered list markers are replaced with real paragraph bullets.
- “Code”: monospace font, everything visible, markup dimmed; a preview on the right re-renders 350 ms after you stop typing, scrolling is synchronized.
- Syntax highlighting in both modes: `styler` puts the token kind (`highlight::Kind`) into bits 26–28 of the flags (`fl::SYN_MASK`) inside code block bodies, `editor` picks the theme color (`syn_*`) from them, comments are italic.
- “Modified” = hash of the editor text ≠ hash of the loaded/saved text (undoing back to the original clears the mark).

## Installation and updates (`install.rs`)

The same exe is both the program and the installer. It installs like ordinary software.

| Scope | Folder | Registry | Rights |
| --- | --- | --- | --- |
| **All users (default)** | only `%ProgramFiles%\vibe-md-viewer` (`SHGetKnownFolderPath(FOLDERID_ProgramFiles)`); a custom folder is not allowed | HKLM | UAC — only for the duration of the operation |
| Just me (no admin rights / UAC declined) | `%LOCALAPPDATA%\Programs\vibe-md-viewer` | HKCU | normal |

Elevation: the normal copy starts **itself** via `ShellExecuteExW(verb="runas")` with `--elevated install|update|uninstall --target "<folder>"` and waits for the exit code. The elevated copy asks nothing and only performs the operation (`elevated_main`). The “Open with” window and starting the updated program always happen from the **normal** process, so the viewer never runs with administrator rights. The manifest is `asInvoker`.

The installation folder is validated (`validate_install_dir`): an absolute local path without `..`, the last component is `vibe-md-viewer`, not inside `%SystemRoot%`, no `" % !`; for all users — exactly `Program Files\vibe-md-viewer` (otherwise a regular user could tamper with the parent folder). Uninstall deletes only a folder that contains the installer marker and our exe (`is_our_install_dir`), and on behalf of an administrator only directly inside Program Files (`may_delete`).

The elevated copy copies **the running image itself**, not a file by path (`open_own_image`): the file name comes from the image mapping (`K32GetMappedFileNameW`, which follows renames), the file is opened denying write and delete to others, and the file ID is verified again. So setup.exe in “Downloads” can't be swapped while the UAC prompt is shown.

| Situation at start | What happens |
| --- | --- |
| Not installed | Start page with an “Install” button (Program Files, UAC; if declined — offer a just-for-me installation). |
| An **older** version is installed, a new exe is started without a file | Immediately asks “Update X → Y?”. Yes → (UAC) → install files + `register_at` → the updated program starts from the normal process. |
| The same/a newer version is installed | Normal start; the start page shows “Reinstall” / “a newer version is installed”. |
| Started from the installation folder | “Make default app for .md”, “Uninstall”. Settings — `%APPDATA%\vibe-md-viewer`. |

Updating the file: the existing `vibe-md-viewer.exe` is renamed to `vibe-md-viewer.old.exe` (a running exe can be renamed but not deleted), the new one is copied, the old one is deleted right away or on the next start (`cleanup_old`). If copying fails, the rename is rolled back.

Registry: the ProgID `VibeMdViewer.Markdown` and the path don't change between versions, so the user's default-app choice (UserChoice) stays valid. Another `.md` handler is never overridden (we add `OpenWithProgids`, and `(Default)` only if it's empty). The registry logic is the same for HKLM and HKCU and is tested on a temporary branch `HKCU\Software\VibeMdViewer-Test-<pid>` that is deleted after the test.

Deleting a folder with a running exe: a background `cmd /d /v:off` keeps trying for up to 30 seconds until all copies of the program are closed.

**Rename vibe-mdViewer → vibe-md-viewer (0.2.0).** `installed()` also recognizes a 0.1.x installation (folder `vibe-mdViewer` with `vibe-md.exe`); updating it installs into the standard `vibe-md-viewer` folder (`Installed::target_dir`). After installing, `do_install` removes the entries with the old name (`Applications\vibe-md.exe`, `RegisteredApplications\vibe-mdViewer`) and queues deletion of the old folder — the path comes from `InstallDir` in the same registry scope (for all users that's HKLM, writable only by administrators); the folder is deleted only if `may_delete` (our installer marker and exe, and directly inside Program Files when elevated), retrying for up to 10 minutes while the old version is open. The keys `Software\VibeMdViewer`, `Uninstall\VibeMdViewer` and the ProgID `VibeMdViewer.Markdown` did not change (it's the same name in registry style), so the default-app choice is kept. Settings: on the first start of the installed 0.2.0 the folder `%APPDATA%\vibe-mdViewer` is renamed to `%APPDATA%\vibe-md-viewer` (`settings::data_dir`). Tests: `legacy_install_is_recognized_and_moved`, the registry test on a temporary branch.

## Current limitations

- SVG is not displayed (planned via the safe Rust renderer resvg).
- Animated GIFs show the first frame.
- Mermaid diagrams and LaTeX formulas are shown as code (that needs a browser engine or a separate renderer).
- Mixed-DPI monitors: the app is “system DPI aware” — on a second monitor with a different scale the picture may be slightly blurry.
