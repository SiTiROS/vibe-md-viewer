vibe-md-viewer {{VERSION}} — a fast Markdown (.md) viewer and editor for Windows
================================================================================

Requirements: Windows 10 or 11, 64-bit. Nothing else is needed (no Edge, no .NET,
no extra libraries).


INSTALLATION
1. Run vibe-md-viewer-{{VERSION}}-setup.exe.
   If Windows shows "Windows protected your PC" (the program is not signed with a
   paid certificate), click "More info" → "Run anyway".
2. In the setup window click "Install" and confirm the administrator prompt.
   The program installs into C:\Program Files\vibe-md-viewer.
   Without administrator rights a just-for-me installation is offered.
3. Windows Settings → Default apps opens. Windows 11: on the vibe-md-viewer page
   set it for .md. Windows 10: click "Choose default apps by file type", find .md
   and pick vibe-md-viewer.

Done: .md files open with a double-click.


UPDATING
Run the new version's setup.exe and click "Update". No need to uninstall the old
version; settings and the .md association are kept.


UNINSTALLING
"Settings" → "Apps" → vibe-md-viewer → "Uninstall"
(or run setup.exe and click "Uninstall").


KEYBOARD SHORTCUTS
Ctrl+1 / Ctrl+2 / Ctrl+3 — view / visual editing / code
Ctrl+O open (or drag a file into the window), Ctrl+F find, F5 reload, Alt+← back
Edits are saved automatically (pause in typing, switching to view, leaving the window); Ctrl+S — right away
Ctrl+B / Ctrl+I — bold / italic, Ctrl+K — link
− 100% + on the right of the toolbar or Ctrl+wheel — zoom
☀/☾ in the top right corner — light/dark theme (dark by default)
→← / ←→ next to it — full-width text or a narrow column (full width by default)
EN / RU — interface language (English by default)


SECURITY
HTML and scripts inside .md are never executed. Links to programs, dangerous
protocols and network paths are blocked. Images from the internet are loaded only
with your permission. The real link address is shown at the bottom of the window
before you click.