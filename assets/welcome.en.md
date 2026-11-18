# vibe-md-viewer

A fast Markdown viewer and editor for Windows. No browser engine, no Edge, no internet.

{{STATUS}}

## How to use

- Open a `.md` file by double-clicking it, with **Ctrl+O**, or by dragging it into the window.
- **Ctrl+1** — view, **Ctrl+2** — visual editing, **Ctrl+3** — code mode (source + live preview).
- Edits are **saved automatically** — a second after you stop typing, when you switch to view, and when you leave the window (Ctrl+S saves right away).
- **Ctrl+F** — find, **F5** — reload the file, **Alt+←** — back through links.
- Zoom — the **− 100% +** buttons on the right of the toolbar, **Ctrl+mouse wheel**, or **Ctrl+plus / minus / 0**.
- The **→←** button switches between full-width text and a narrow column. Text in the view can be selected with the mouse and copied.
- The **EN / RU** button switches the interface language.

## Editing

| Action | Keys |
| --- | --- |
| Bold / italic / strikethrough | Ctrl+B / Ctrl+I / Ctrl+Shift+X |
| Heading 1–6, plain text | Ctrl+Shift+1…6, Ctrl+Shift+0 |
| Bulleted / numbered list | Ctrl+Shift+8 / Ctrl+Shift+7 |
| Quote | Ctrl+Q |
| Code | Ctrl+E |
| Link | Ctrl+K |

In visual mode the markup characters (`**`, `#`, `](address)`) are hidden and appear only on the line with the cursor. The file is always saved as plain markdown — nothing is lost.

## Security

- HTML and scripts inside `.md` are **never** executed.
- Links with `javascript:`, `vbscript:`, `data:`, `ms-*:`, `search-ms:` and any unknown protocol are blocked.
- Network paths `\\server\...` are blocked (protection against Windows password hash theft).
- Executable files (`.exe`, `.bat`, `.lnk`, `.ps1`…) are never opened from links.
- Images from the internet are loaded only with your permission.
- Before you open a link, its real address is shown at the bottom of the window.

Details — in `docs/SECURITY.md` in the repository.
