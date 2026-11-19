# Build, tests, measurements

**English** | [Русский](BUILD.ru.md)

## Requirements

- Rust stable with the `x86_64-pc-windows-msvc` target (rustup).
- Visual Studio 2022 Build Tools / Community with the “Desktop development with C++” workload (`link.exe`, `rc.exe`, Windows SDK are needed).
- Nothing else: no Node.js, no .NET, no WebView2.

## Rule: nothing on drive C:

By default the tools put caches into the user profile. The `scripts\env.ps1` script redirects everything into the project folder:

| Variable | Where |
| --- | --- |
| `CARGO_HOME` | `.toolcache\cargo` — crate registry and sources |
| `CARGO_TARGET_DIR` | `target\` |
| `TEMP`, `TMP` | `.toolcache\tmp` — compiler and linker temporary files |

The Rust toolchain itself (already installed in the profile) is only used; nothing is downloaded into it. Run any `cargo` command after `. .\scripts\env.ps1`.

The `.ps1` scripts are saved as UTF-8 **with BOM** — otherwise Windows PowerShell 5.1 breaks on non-ASCII text (e.g. the Cyrillic `ПРОЧТИ.txt` name).

## Commands

```powershell
# release → dist\vibe-md-viewer.exe (≈2.7 MB)
powershell -ExecutionPolicy Bypass -File scripts\build.ps1
# the same + tests
powershell -ExecutionPolicy Bypass -File scripts\build.ps1 -Test

# development
. .\scripts\env.ps1
cargo build           # target\debug\vibe-md-viewer.exe
cargo test            # unit tests

# end-to-end self-test on a live window (the window flashes for a second)
powershell -ExecutionPolicy Bypass -File scripts\selftest.ps1

# startup time
powershell -ExecutionPolicy Bypass -File scripts\bench.ps1 [-File path.md] [-Runs 15]
# a large document (~270 KB): theme, modes, width, zoom, scroll frame time
powershell -ExecutionPolicy Bypass -File scripts\profile.ps1 [-Size 2560x1380]

# distribution package: build + tests → dist\vibe-md-viewer-<version>-setup.exe, .zip (+ .sha256.txt, licenses)
powershell -ExecutionPolicy Bypass -File scripts\package.ps1

# real download of an image from the internet (network!)
cargo test --release fetch_public -- --ignored

# redraw the icon assets\app.ico
powershell -ExecutionPolicy Bypass -File scripts\make-icon.ps1
```

## Releasing a new version

1. Bump `version` in `Cargo.toml` (the only place: the version goes into the exe properties via `build.rs` and into the update logic).
2. Add a `## X.Y.Z — date` section to `CHANGELOG.md` (it becomes the release description) and to `CHANGELOG.ru.md`.
3. Commit and tag: `git tag vX.Y.Z` → `git push origin main vX.Y.Z`.
4. GitHub Actions (`.github/workflows/release.yml`) checks that the tag matches the version, builds and tests (`scripts\package.ps1`, self-test), signs the build provenance, and publishes the release: `vibe-md-viewer-X.Y.Z-setup.exe`, zip, checksums, `THIRD-PARTY-NOTICES.txt`.

Locally the same — `scripts\package.ps1` → `dist\`.

## CI

`.github/workflows/ci.yml` — on every push to `main` and every pull request, runner `windows-latest`: `cargo test --release --locked` (including tests on a real RichEdit), `cargo build --release --locked`, the self-test on a live window (`scripts\selftest.ps1`), the exe as a build artifact for 14 days. Only official `actions/*` actions pinned by commit SHA; the token is read-only and not persisted. In CI the cargo caches are the standard ones (the “nothing on C:” rule is about the developer's machine: there always `. .\scripts\env.ps1`).

Users update by running the new exe (see README, “Updating”). Versions are compared numerically `a.b.c`: an update is offered only if the new version is greater than the installed one.

## Code coverage

`scripts\coverage.ps1` builds the program with coverage counters (`-C instrument-coverage`, into `target\cov`), runs the unit tests, the live-window self-test and the service modes (`--profile` on a large document, snapshots, startup timing, the setup wizard window — nothing is installed), and prints line coverage per file; `-Html` also writes an HTML report to `.toolcache\coverage\html`, `-MinLines N` fails below N%.

It needs `llvm-profdata.exe` and `llvm-cov.exe` of the same LLVM version as `rustc` (`rustc -vV`). In CI they come from the rustup component `llvm-tools`. Locally, so that nothing is installed on drive C:, they are put into `.toolcache\llvm-tools`: take `llvm-tools-<version>-x86_64-pc-windows-msvc.tar.xz` for your `rustc` version from the channel manifest `https://static.rust-lang.org/dist/channel-rust-<version>.toml` and extract only these two files.

Current line coverage is **87.7%**. The core modules are at 96–100%: render, html, highlight, styler, editor, richedit, links, images, doc, settings, theme and emoji.

What is not covered, and why:
- modal windows and system dialogs (message boxes, file open/save, UAC): `dialogs.rs` is at 0%, the setup wizard at 56%;
- installing into Program Files/HKLM with administrator rights (`install.rs` 72%);
- real downloads from the internet;
- failure branches of Win32 calls.

Everything else is covered by unit tests or by the self-test. 100% is not reachable without a person clicking through UAC and dialogs.

The CI job `coverage` runs the same script with `-MinLines 85` and shows the summary on the run page; the HTML report is an artifact.

## Command-line options

| Option | Purpose |
| --- | --- |
| `file.md` | open the file |
| `--mode view\|visual\|code` | mode at start (`visual` is used by the “Edit” verb in Explorer) |
| `--install`, `--uninstall` | install / uninstall (with dialogs) |
| `--setup` | the setup wizard (also when the exe name contains “setup”) |
| `--register` | re-register the current exe for `.md` without copying |
| `--timing FILE` | append the time to first paint and a stage breakdown to the file |
| `--exit-after-paint` | exit right after the first paint (for measurements) |
| `--snapshot PNG` | wait for images, save a window snapshot and exit |
| `--size WxH`, `--scroll Y` | window size and scroll position for the snapshot |
| `--selftest` | end-to-end self-test on the opened file (log `<name>.selftest.log`), exit code 0/3 |
| `--profile FILE` | measurements on a real window: opening (until shown / background loading), scrolling, theme, width, zoom, mode switches; checks that the reading position is kept |

## Startup time

Measured from process creation to a fully painted window with the document (images load afterwards). Developer machine, release build, median of 15 runs:

| Document | To first paint |
| --- | --- |
| `tests\fixtures\showcase.md` (all kinds of markup, 2 images), window 1100×860 (`--size 1100x860`) | ≈ 80 ms |
| the same, default window (85% of the screen) — `scripts\bench.ps1` | ≈ 88 ms (2560×1440 screen, 0.1.3) |
| `docs\ARCHITECTURE.md` (≈ 8 KB, many tables) | ≈ 100 ms |

Paint time grows with the window area: the large default window (since 0.1.1) adds ≈ 20 ms. Syntax highlighting doesn't affect speed (0.1.0 and 0.1.1 with the same window — 81.9 and 81.5 ms).

Breakdown (ms from process start): before `main` ≈ 10, window and bars ≈ 10, creating RichEdit ≈ 12, parsing and loading the document ≈ 14, showing and painting ≈ 30.

Scrolling (`scripts\profile.ps1 -Size 2560x1380`, a 268 KB document): a frame takes 2.6 ms on average, 95% of frames under 3.6 ms, the worst ≈ 6 ms; the frame budget at 144 Hz is 6.9 ms.

What is already done for speed:

- rarely used DLLs (WinHTTP, shell32, comdlg32, ole32, oleaut32; d2d1 and dwrite for color emoji — only after the first paint of a document with emoji) are delay-loaded (`/DELAYLOAD`, see `build.rs`);
- the source editor is created only on the first switch to an editing mode;
- image decoding threads are created lazily;
- static CRT, LTO, `codegen-units = 1`.

## Repository layout

```
src/                sources (see docs/ARCHITECTURE.md)
assets/             icon, manifest, resources, start page welcome.en.md / welcome.ru.md, README.txt / ПРОЧТИ.txt for the zip
scripts/            env / build / bench / selftest / profile / package / notices / make-icon
tests/fixtures/     showcase.md (showcase), attack.md (attacks), inline-code.md (lots of inline `code` and cpp blocks), img/
docs/               documentation (*.md — English, *.ru.md — Russian)
dist/               built exe (not in git)
.toolcache/         tool caches, snapshots, temporary files (not in git)
```
