//! Install, in-place update, uninstall — like regular software.
//!
//! * **For all users (default):** `C:\Program Files\vibe-md-viewer`, registration
//!   in HKLM, an entry in "Apps & features". Administrator rights are requested (UAC)
//!   **only** for the duration of install/update/uninstall: a second copy of the exe is started with
//!   `--elevated …`. The viewer itself always runs with normal rights.
//! * **For the current user only (if there are no administrator rights):** `%LOCALAPPDATA%\Programs\vibe-md-viewer`,
//!   registration in HKCU, no UAC — like the "user" installation of VS Code.
//! * **Update** — running the new exe: it finds the installation, replaces the program file
//!   (a running exe is renamed, which Windows allows) and updates the registry. The path and
//!   ProgID do not change — the "default program" choice is kept.
//! * Settings of the installed program — `%APPDATA%\vibe-md-viewer` (Program Files is not writable).
//! * Windows does not allow an app to silently make itself the "default": after the first install
//!   the system choice dialog opens. There are no workarounds for this protection, and there will be none.
//! * **Rename (0.2.0):** before 0.2.0 the program was called vibe-mdViewer (folder
//!   `vibe-mdViewer`, `vibe-md.exe`). Updating such an installation puts the program in the sibling
//!   folder `vibe-md-viewer` and removes the old folder and registry entries. The `VibeMdViewer` keys
//!   and the ProgID did not change — the "default program" choice for .md is kept.

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, HWND};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};

use crate::util::{self, pcwstr, wide};

pub const PROGID: &str = "VibeMdViewer.Markdown";
pub const EXE_NAME: &str = "vibe-md-viewer.exe";
pub const OLD_EXE_NAME: &str = "vibe-md-viewer.old.exe";
pub const DIR_NAME: &str = "vibe-md-viewer";
pub const EXTS: &[&str] = &[".md", ".markdown", ".mdown", ".mkd", ".mkdn", ".mdwn"];
const MARKER: &str = ".vibe-md-viewer-installed";
/// The former name (before 0.2.0) — for migrating a 0.1.x installation.
const LEGACY_DIR_NAME: &str = "vibe-mdViewer";
const LEGACY_EXE_NAME: &str = "vibe-md.exe";
const LEGACY_APP_NAME: &str = "vibe-mdViewer";
const LEGACY_MARKER: &str = ".vibe-md-installed";
const APP_KEY: &str = "Software\\VibeMdViewer";
const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\VibeMdViewer";
/// The name in `Software\RegisteredApplications` (Windows Settings → Default apps).
const REGISTERED_NAME: &str = "vibe-md-viewer";
pub const HELP_DOC: &str = "vibe-md-viewer.md";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const ERROR_CANCELLED_CODE: u32 = 1223;

// ---------- versions ----------

/// "1.2.3" / "1.2.3-beta" → (1, 2, 3). Unrecognized → (0, 0, 0).
pub fn parse_version(s: &str) -> (u32, u32, u32) {
    let mut it = s.trim().split(['.', '-', '+']).map(|p| p.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

pub fn compare_versions(a: &str, b: &str) -> Ordering {
    parse_version(a).cmp(&parse_version(b))
}

// ---------- installation scope ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Program Files + HKLM, administrator rights required.
    Machine,
    /// %LOCALAPPDATA%\Programs + HKCU, no administrator rights.
    User,
}

impl Scope {
    fn root(self) -> HKEY {
        match self {
            Scope::Machine => HKEY_LOCAL_MACHINE,
            Scope::User => HKEY_CURRENT_USER,
        }
    }

    pub fn default_dir(self) -> PathBuf {
        match self {
            Scope::Machine => program_files().join(DIR_NAME),
            Scope::User => std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("AppData").join("Local")))
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Programs")
                .join(DIR_NAME),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::Machine => tr!("for all users", "для всех пользователей"),
            Scope::User => tr!("for the current user only", "только для текущего пользователя"),
        }
    }
}

fn program_files() -> PathBuf {
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_ProgramFiles, KF_FLAG_DEFAULT};
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, None) {
            let s = p.to_string().unwrap_or_default();
            windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
            if !s.is_empty() {
                return PathBuf::from(s);
            }
        }
    }
    PathBuf::from(std::env::var("ProgramW6432").or_else(|_| std::env::var("ProgramFiles")).unwrap_or_else(|_| "C:\\Program Files".into()))
}

// ---------- registry ----------

/// Wrapper around a registry branch. `prefix` — for tests (a temporary branch in HKCU).
pub struct Reg {
    root: HKEY,
    prefix: String,
}

impl Reg {
    pub fn for_scope(scope: Scope) -> Reg {
        Reg { root: scope.root(), prefix: String::new() }
    }

    fn p(&self, sub: &str) -> String {
        format!("{}{}", self.prefix, sub)
    }

    fn open_create(&self, sub: &str) -> Result<HKEY, String> {
        let full = self.p(sub);
        let w = wide(&full);
        let mut hk = HKEY::default();
        let r = unsafe { RegCreateKeyExW(self.root, pcwstr(&w), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE | KEY_READ, None, &mut hk, None) };
        if r != ERROR_SUCCESS {
            return Err(trf!("Could not write to the registry: {full} (code {})", "Не удалось записать в реестр: {full} (код {})", r.0));
        }
        Ok(hk)
    }

    pub fn set_str(&self, sub: &str, name: Option<&str>, value: &str) -> Result<(), String> {
        let hk = self.open_create(sub)?;
        let wn = name.map(wide);
        let data: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
        let r = unsafe { RegSetValueExW(hk, wn.as_ref().map(|v| pcwstr(v)).unwrap_or(PCWSTR::null()), None, REG_SZ, Some(&data)) };
        unsafe {
            let _ = RegCloseKey(hk);
        }
        if r != ERROR_SUCCESS {
            return Err(trf!("Could not write to the registry: {}", "Не удалось записать в реестр: {}", self.p(sub)));
        }
        Ok(())
    }

    pub fn set_dword(&self, sub: &str, name: &str, value: u32) -> Result<(), String> {
        let hk = self.open_create(sub)?;
        let wn = wide(name);
        let r = unsafe { RegSetValueExW(hk, pcwstr(&wn), None, REG_DWORD, Some(&value.to_le_bytes())) };
        unsafe {
            let _ = RegCloseKey(hk);
        }
        if r != ERROR_SUCCESS {
            return Err(trf!("Could not write to the registry: {}", "Не удалось записать в реестр: {}", self.p(sub)));
        }
        Ok(())
    }

    pub fn get_str(&self, sub: &str, name: Option<&str>) -> Option<String> {
        let ws = wide(&self.p(sub));
        let wn = name.map(wide);
        let mut buf = vec![0u16; 2048];
        let mut size = (buf.len() * 2) as u32;
        let r = unsafe {
            RegGetValueW(
                self.root,
                pcwstr(&ws),
                wn.as_ref().map(|v| pcwstr(v)).unwrap_or(PCWSTR::null()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
                Some(&mut size),
            )
        };
        if r != ERROR_SUCCESS {
            return None;
        }
        buf.truncate((size as usize / 2).saturating_sub(1));
        Some(String::from_utf16_lossy(&buf))
    }

    pub fn delete_tree(&self, sub: &str) {
        let w = wide(&self.p(sub));
        unsafe {
            let _ = RegDeleteTreeW(self.root, pcwstr(&w));
            let _ = RegDeleteKeyW(self.root, pcwstr(&w));
        }
    }

    pub fn delete_value(&self, sub: &str, name: &str) {
        let ws = wide(&self.p(sub));
        let wn = wide(name);
        unsafe {
            let _ = RegDeleteKeyValueW(self.root, pcwstr(&ws), pcwstr(&wn));
        }
    }
}

// ---------- state ----------

#[derive(Clone, Debug)]
pub struct Installed {
    pub scope: Scope,
    /// Where it is installed now.
    pub dir: PathBuf,
    pub version: String,
}

impl Installed {
    /// A 0.1.x installation under the former name (vibe-mdViewer folder).
    pub fn is_legacy(&self) -> bool {
        has_name(&self.dir, LEGACY_DIR_NAME)
    }

    /// Where to put the update: always the scope's standard folder — for all users
    /// only Program Files (see do_install); the old vibe-mdViewer folder (or a custom
    /// 0.1.x installation folder) moves there.
    pub fn target_dir(&self) -> PathBuf {
        self.scope.default_dir()
    }
}

fn has_name(dir: &Path, name: &str) -> bool {
    dir.file_name().map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false)
}

/// The installed copy: first "for all users", then "for the current user".
pub fn installed() -> Option<Installed> {
    for scope in [Scope::Machine, Scope::User] {
        if let Some(inst) = installed_at(&Reg::for_scope(scope), scope) {
            return Some(inst);
        }
    }
    None
}

/// The installation registered in `reg`, if its folder still has the program.
fn installed_at(reg: &Reg, scope: Scope) -> Option<Installed> {
    let dir = reg.get_str(APP_KEY, Some("InstallDir")).map(PathBuf::from)?;
    if dir.join(EXE_NAME).exists() || (has_name(&dir, LEGACY_DIR_NAME) && dir.join(LEGACY_EXE_NAME).exists()) {
        let version = reg.get_str(APP_KEY, Some("Version")).unwrap_or_else(|| "0.0.0".into());
        return Some(Installed { scope, dir, version });
    }
    None
}

/// Are we running from the installation folder (the installer marker lies next to us).
pub fn running_installed() -> bool {
    util::exe_dir().map(|d| d.join(MARKER).exists() || d.join(LEGACY_MARKER).exists()).unwrap_or(false)
}

/// The installed copy is older than this one (and we are not running from it) → can update.
pub fn update_available() -> Option<Installed> {
    if running_installed() {
        return None;
    }
    installed().filter(|i| compare_versions(VERSION, &i.version) == Ordering::Greater)
}

/// Removes the ".old" copy left over after updating over a running exe.
pub fn cleanup_old() {
    if let Some(d) = util::exe_dir() {
        let _ = fs::remove_file(d.join(OLD_EXE_NAME));
    }
}

/// Opens a file only to read attributes, without hindering others (for the "same file" comparison).
fn open_for_id(p: &Path) -> Option<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_READ_ATTRIBUTES; FILE_SHARE_READ | WRITE | DELETE
    fs::OpenOptions::new().access_mode(0x80).share_mode(7).open(p).ok()
}

/// File identifier (volume + index): the same for all paths and handles of one file.
fn file_id(f: &fs::File) -> Option<(u32, u32, u32)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle()), &mut info) }.ok()?;
    Some((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
}

/// Our exe to copy into the installation folder is the running image itself, not the file at a path.
/// A running exe can be renamed: a program with user rights could, while the
/// elevated copy waits for UAC confirmation, move setup.exe out of "Downloads" and plant its own
/// file under the same name — and that file would end up in Program Files. Therefore:
/// 1) we open the file the image is mapped from (the name comes from the mapping itself, which follows
///    renames), denying others write and delete access — now it cannot be renamed
///    or modified;
/// 2) we take the image name once more and compare the file identifier: it is the same file.
fn open_own_image() -> Result<fs::File, String> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::ProcessStatus::K32GetMappedFileNameW;
    use windows::Win32::System::Threading::GetCurrentProcess;
    let base = unsafe { GetModuleHandleW(PCWSTR::null()) }.map_err(|_| tr!("Could not locate the program image", "Не удалось найти образ программы").to_string())?;
    let mapped = || -> Result<String, String> {
        let mut buf = vec![0u16; 2048];
        let n = unsafe { K32GetMappedFileNameW(GetCurrentProcess(), base.0 as *const core::ffi::c_void, &mut buf) } as usize;
        if n == 0 || n >= buf.len() {
            return Err(tr!("Could not determine the program file", "Не удалось узнать файл программы").into());
        }
        // \Device\HarddiskVolumeN\... → a path that CreateFileW can open.
        Ok(format!("\\\\?\\GLOBALROOT{}", String::from_utf16_lossy(&buf[..n])))
    };
    // FILE_SHARE_READ: others may only read (no writing, renaming or deleting).
    let f = fs::OpenOptions::new().read(true).share_mode(1).open(mapped()?).map_err(|e| trf!("Could not open the program file: {e}", "Не удалось открыть файл программы: {e}"))?;
    let now = open_for_id(Path::new(&mapped()?)).ok_or(tr!("Could not verify the program file", "Не удалось проверить файл программы"))?;
    match (file_id(&f), file_id(&now)) {
        (Some(a), Some(b)) if a == b => Ok(f),
        _ => Err(tr!(
            "The program file was replaced during installation — installation stopped.",
            "Файл программы подменён во время установки — установка остановлена."
        )
        .into()),
    }
}

/// The installation folder must be "ours": an absolute path, the last component is vibe-md-viewer,
/// not a drive root and not the Windows system folder. This limits the damage even if the elevated
/// copy is handed a foreign path (and uninstall removes only such a folder).
pub fn validate_install_dir(dir: &Path) -> Result<(), String> {
    validate_dir_shape(dir, &[DIR_NAME])
}

/// May the folder be removed as our installation: correct path shape (new or former name),
/// with the installer marker and the program exe inside.
fn is_our_install_dir(dir: &Path) -> bool {
    validate_dir_shape(dir, &[DIR_NAME, LEGACY_DIR_NAME]).is_ok()
        && (dir.join(MARKER).exists() || dir.join(LEGACY_MARKER).exists())
        && (dir.join(EXE_NAME).exists() || dir.join(LEGACY_EXE_NAME).exists())
}

fn validate_dir_shape(dir: &Path, names: &[&str]) -> Result<(), String> {
    let s = dir.to_string_lossy();
    if dir.components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::CurDir)) {
        return Err(tr!("The installation folder path must not contain “..” or “.”", "Путь к папке установки не должен содержать «..» и «.»").into());
    }
    if s.contains('"') || s.contains('%') || s.contains('!') || s.ends_with('\\') {
        return Err(tr!("The installation folder path contains invalid characters", "Путь к папке установки содержит недопустимые символы").into());
    }
    if !dir.is_absolute() || s.starts_with("\\\\") {
        return Err(tr!("The installation folder must be a local absolute path", "Папка установки должна быть локальным абсолютным путём").into());
    }
    if !names.iter().any(|n| has_name(dir, n)) {
        return Err(trf!("The installation folder must be named {DIR_NAME}", "Папка установки должна называться {DIR_NAME}"));
    }
    if dir.components().count() < 3 {
        return Err(tr!("Cannot install to the root of a drive", "Нельзя устанавливать в корень диска").into());
    }
    if let Some(win) = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir")) {
        let win = PathBuf::from(win).to_string_lossy().to_ascii_lowercase();
        if s.to_ascii_lowercase().starts_with(&win) {
            return Err(tr!("Cannot install to the Windows system folder", "Нельзя устанавливать в системную папку Windows").into());
        }
    }
    Ok(())
}

/// Copies the exe into the installation folder. If an exe is already there (possibly running),
/// it is renamed to vibe-md-viewer.old.exe and deleted immediately or on the next start.
#[cfg(test)]
pub fn install_files(src_exe: &Path, dir: &Path) -> Result<(), String> {
    let mut src = fs::File::open(src_exe).map_err(|e| trf!("Could not open {}: {e}", "Не удалось открыть {}: {e}", src_exe.display()))?;
    install_files_from(&mut src, dir)
}

/// The same from an open file (in the installer — a handle to the running exe itself, open_own_image).
fn install_files_from(src: &mut fs::File, dir: &Path) -> Result<(), String> {
    use std::io::{Seek, SeekFrom};
    validate_install_dir(dir)?;
    fs::create_dir_all(dir).map_err(|e| trf!("Could not create the folder {}: {e}", "Не удалось создать папку {}: {e}", dir.display()))?;
    let dest = dir.join(EXE_NAME);
    let same = file_id(src).is_some() && file_id(src) == open_for_id(&dest).as_ref().and_then(file_id);
    if !same {
        let old = dir.join(OLD_EXE_NAME);
        let had_old = dest.exists();
        if had_old {
            let _ = fs::remove_file(&old);
            fs::rename(&dest, &old).map_err(|e| trf!("Could not replace the installed program: {e}", "Не удалось заменить установленную программу: {e}"))?;
        }
        let copied = src.seek(SeekFrom::Start(0)).and_then(|_| {
            let mut out = fs::File::create(&dest)?;
            std::io::copy(src, &mut out)?;
            out.sync_all()
        });
        if let Err(e) = copied {
            let _ = fs::remove_file(&dest);
            if had_old {
                let _ = fs::rename(&old, &dest); // rollback
            }
            return Err(trf!("Could not copy the program: {e}", "Не удалось скопировать программу: {e}"));
        }
        if had_old {
            let _ = fs::remove_file(&old); // succeeds if the old version is not running
        }
    }
    fs::write(dir.join(MARKER), format!("vibe-md-viewer {VERSION}\n")).map_err(|e| e.to_string())?;
    let _ = fs::write(dir.join(HELP_DOC), crate::app::welcome_md().replace("{{STATUS}}", &trf!("Version {VERSION}.", "Версия {VERSION}.")));
    Ok(())
}

pub fn register_at(reg: &Reg, scope: Scope, exe: &Path, dir: &Path) -> Result<(), String> {
    let exe_s = exe.to_string_lossy().to_string();
    let open_cmd = format!("\"{exe_s}\" \"%1\"");
    let edit_cmd = format!("\"{exe_s}\" --mode visual \"%1\"");
    let icon = format!("\"{exe_s}\",0");
    let classes = "Software\\Classes";

    let pk = format!("{classes}\\{PROGID}");
    reg.set_str(&pk, None, tr!("Markdown Document", "Документ Markdown"))?;
    reg.set_str(&pk, Some("FriendlyTypeName"), tr!("Markdown Document", "Документ Markdown"))?;
    reg.set_str(&format!("{pk}\\DefaultIcon"), None, &icon)?;
    reg.set_str(&format!("{pk}\\shell"), None, "open")?;
    reg.set_str(&format!("{pk}\\shell\\open"), Some("FriendlyAppName"), "vibe-md-viewer")?;
    reg.set_str(&format!("{pk}\\shell\\open\\command"), None, &open_cmd)?;
    reg.set_str(&format!("{pk}\\shell\\edit"), None, tr!("Edit in vibe-md-viewer", "Редактировать в vibe-md-viewer"))?;
    reg.set_str(&format!("{pk}\\shell\\edit\\command"), None, &edit_cmd)?;

    let ak = format!("{classes}\\Applications\\{EXE_NAME}");
    reg.set_str(&ak, Some("FriendlyAppName"), "vibe-md-viewer")?;
    reg.set_str(&format!("{ak}\\DefaultIcon"), None, &icon)?;
    reg.set_str(&format!("{ak}\\shell\\open\\command"), None, &open_cmd)?;

    let cap = format!("{APP_KEY}\\Capabilities");
    reg.set_str(&cap, Some("ApplicationName"), "vibe-md-viewer")?;
    reg.set_str(&cap, Some("ApplicationDescription"), tr!("Fast Markdown viewer and editor", "Быстрый просмотрщик и редактор Markdown"))?;
    reg.set_str(&cap, Some("ApplicationIcon"), &icon)?;
    for ext in EXTS {
        reg.set_str(&format!("{ak}\\SupportedTypes"), Some(ext), "")?;
        reg.set_str(&format!("{classes}\\{ext}\\OpenWithProgids"), Some(PROGID), "")?;
        reg.set_str(&format!("{cap}\\FileAssociations"), Some(ext), PROGID)?;
        // We do not override another default program: the user makes the choice in the system window.
        let cur = reg.get_str(&format!("{classes}\\{ext}"), None).unwrap_or_default();
        if cur.is_empty() {
            reg.set_str(&format!("{classes}\\{ext}"), None, PROGID)?;
        }
        if reg.get_str(&format!("{classes}\\{ext}"), Some("Content Type")).is_none() {
            reg.set_str(&format!("{classes}\\{ext}"), Some("Content Type"), "text/markdown")?;
        }
        if reg.get_str(&format!("{classes}\\{ext}"), Some("PerceivedType")).is_none() {
            reg.set_str(&format!("{classes}\\{ext}"), Some("PerceivedType"), "text")?;
        }
    }
    reg.set_str("Software\\RegisteredApplications", Some(REGISTERED_NAME), &cap)?;
    reg.set_str(APP_KEY, Some("InstallDir"), &dir.to_string_lossy())?;
    reg.set_str(APP_KEY, Some("Version"), VERSION)?;
    reg.set_str(APP_KEY, Some("Scope"), if scope == Scope::Machine { "machine" } else { "user" })?;

    reg.set_str(UNINSTALL_KEY, Some("DisplayName"), "vibe-md-viewer")?;
    reg.set_str(UNINSTALL_KEY, Some("DisplayVersion"), VERSION)?;
    reg.set_str(UNINSTALL_KEY, Some("Publisher"), "vibe-tools")?;
    reg.set_str(UNINSTALL_KEY, Some("DisplayIcon"), &icon)?;
    reg.set_str(UNINSTALL_KEY, Some("InstallLocation"), &dir.to_string_lossy())?;
    reg.set_str(UNINSTALL_KEY, Some("UninstallString"), &format!("\"{exe_s}\" --uninstall"))?;
    reg.set_dword(UNINSTALL_KEY, "NoModify", 1)?;
    reg.set_dword(UNINSTALL_KEY, "NoRepair", 1)?;
    let size_kb = fs::metadata(exe).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
    reg.set_dword(UNINSTALL_KEY, "EstimatedSize", size_kb)?;
    Ok(())
}

pub fn unregister_at(reg: &Reg) {
    let classes = "Software\\Classes";
    for ext in EXTS {
        reg.delete_value(&format!("{classes}\\{ext}\\OpenWithProgids"), PROGID);
        if reg.get_str(&format!("{classes}\\{ext}"), None).as_deref() == Some(PROGID) {
            let _ = reg.set_str(&format!("{classes}\\{ext}"), None, "");
        }
    }
    reg.delete_tree(&format!("{classes}\\{PROGID}"));
    reg.delete_tree(&format!("{classes}\\Applications\\{EXE_NAME}"));
    reg.delete_value("Software\\RegisteredApplications", REGISTERED_NAME);
    reg.delete_tree(APP_KEY);
    reg.delete_tree(UNINSTALL_KEY);
    unregister_legacy(reg);
}

/// Entries under the former name (0.1.x) that the new registration does not overwrite.
/// The ProgID and VibeMdViewer keys are the same — register_at updates them.
fn unregister_legacy(reg: &Reg) {
    reg.delete_tree(&format!("Software\\Classes\\Applications\\{LEGACY_EXE_NAME}"));
    reg.delete_value("Software\\RegisteredApplications", LEGACY_APP_NAME);
}

// ---------- operations without UI (including in the elevated copy) ----------

fn notify_assoc() {
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

/// Permissions like Program Files: only SYSTEM and administrators may modify, users may
/// read and execute. The owner is the Administrators group. Otherwise, in a folder on a non-system
/// drive any local user could replace the exe (and their code would run for other users).
fn secure_dir(dir: &Path) -> Result<(), String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows::Win32::Security::{SetFileSecurityW, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
    let w = util::wide_path(dir);
    let apply = |sddl: &str, info| -> bool {
        let s = wide(sddl);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(pcwstr(&s), SDDL_REVISION_1, &mut sd, None).is_err() {
                return false;
            }
            let ok = SetFileSecurityW(pcwstr(&w), info, sd).as_bool();
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            ok
        }
    };
    let dacl = "D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)";
    let full = format!("O:BA{dacl}");
    if apply(&full, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION) {
        return Ok(());
    }
    if apply(dacl, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION) {
        return Ok(());
    }
    Err(trf!("Could not set access permissions on the folder {}", "Не удалось задать права доступа на папку {}", dir.display()))
}

fn do_install(scope: Scope, dir: &Path) -> Result<(), String> {
    validate_install_dir(dir)?;
    // For all users — Program Files only: all parent folders belong to
    // administrators, a regular user can replace neither the folder itself nor the path to it
    // (by renaming a parent, via a junction), and the elevated copy will not write or
    // delete anything foreign. A custom folder — only for a "current user" install (no rights).
    let machine_dir = Scope::Machine.default_dir();
    if scope == Scope::Machine && !same_dir(dir, &machine_dir) {
        return Err(trf!(
            "For all users, the program can only be installed to {}",
            "Для всех пользователей программа устанавливается только в {}",
            machine_dir.display()
        ));
    }
    let mut src = match open_own_image() {
        Ok(f) => f,
        // Without elevation there is nothing to tamper with: the user's copy is under their control anyway.
        Err(_) if scope == Scope::User => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            fs::File::open(&exe).map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e),
    };
    // We do not install into a foreign non-empty folder (it could have been prepared in advance).
    if dir.exists() && !dir.join(MARKER).exists() && fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(trf!(
            "The folder {} already exists and is not empty. Choose another folder.",
            "Папка {} уже существует и не пуста. Выберите другую папку.",
            dir.display()
        ));
    }
    if scope == Scope::Machine {
        fs::create_dir_all(dir).map_err(|e| trf!("Could not create the folder {}: {e}", "Не удалось создать папку {}: {e}", dir.display()))?;
        secure_dir(dir)?;
    }
    let reg = Reg::for_scope(scope);
    // The previous folder — from the registry of the same scope (for "all users" that is HKLM, only
    // an administrator can write there, so the elevated copy can trust the path).
    let prev = reg.get_str(APP_KEY, Some("InstallDir")).map(PathBuf::from);
    install_files_from(&mut src, dir)?;
    register_at(&reg, scope, &dir.join(EXE_NAME), dir)?;
    // Migration from 0.1.x: the old entries and the folder with the former name are no longer needed.
    unregister_legacy(&reg);
    if let Some(old) = prev {
        if !same_dir(&old, dir) && may_delete(scope, &old) {
            // The old version may still be open — the folder is removed once it is closed (up to 10 min).
            let _ = schedule_dir_delete(&old, 300);
        }
    }
    notify_assoc();
    Ok(())
}

/// A folder may be deleted only if it is our installation and — for the elevated copy — it is directly
/// in Program Files: there all parent folders belong to administrators and the path cannot be redirected
/// to a foreign folder. 0.1.x "all users" installations in a custom folder are not deleted (they stay on disk).
fn may_delete(scope: Scope, dir: &Path) -> bool {
    is_our_install_dir(dir) && (scope == Scope::User || dir.parent().map(|p| same_dir(p, &program_files())).unwrap_or(false))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().trim_end_matches('\\').eq_ignore_ascii_case(b.to_string_lossy().trim_end_matches('\\'))
}

fn do_uninstall(scope: Scope) -> Result<(), String> {
    let reg = Reg::for_scope(scope);
    let dir = reg.get_str(APP_KEY, Some("InstallDir")).map(PathBuf::from);
    unregister_at(&reg);
    notify_assoc();
    if let Some(dir) = dir {
        if may_delete(scope, &dir) {
            schedule_dir_delete(&dir, 30)?;
        }
    }
    Ok(())
}

pub fn is_elevated() -> bool {
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elev as *mut _ as *mut core::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elev.TokenIsElevated != 0
    }
}

enum ElevError {
    Cancelled,
    Failed,
}

/// Starts a copy of this exe with administrator rights and waits for it.
fn run_elevated(owner: Option<HWND>, params: &str) -> Result<(), ElevError> {
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    let exe = std::env::current_exe().map_err(|_| ElevError::Failed)?;
    let file = util::wide_path(&exe);
    let verb = wide("runas");
    let par = wide(params);
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        hwnd: owner.unwrap_or_default(),
        lpVerb: pcwstr(&verb),
        lpFile: pcwstr(&file),
        lpParameters: pcwstr(&par),
        nShow: 1,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut sei) {
            return Err(if (e.code().0 as u32 & 0xFFFF) == ERROR_CANCELLED_CODE { ElevError::Cancelled } else { ElevError::Failed });
        }
        if sei.hProcess.is_invalid() {
            return Err(ElevError::Failed);
        }
        WaitForSingleObject(sei.hProcess, INFINITE);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(sei.hProcess, &mut code);
        let _ = CloseHandle(sei.hProcess);
        if code == 0 {
            Ok(())
        } else {
            Err(ElevError::Failed)
        }
    }
}

/// Entry point of the elevated copy: `--elevated install|update|uninstall [--target <folder>]`.
/// No questions to the user (except an error message). Exit code 0 means success.
pub fn elevated_main(op: &str, target: Option<PathBuf>) -> i32 {
    if !is_elevated() {
        return 5;
    }
    let r = match op {
        "install" | "update" => match target {
            Some(dir) => do_install(Scope::Machine, &dir),
            None => Err(tr!("Installation folder not specified", "Не указана папка установки").into()),
        },
        "uninstall" => do_uninstall(Scope::Machine),
        _ => Err(tr!("Unknown operation", "Неизвестная операция").into()),
    };
    match r {
        Ok(()) => 0,
        Err(e) => {
            util::error(None, &e);
            1
        }
    }
}

// ---------- UI scenarios (normal rights) ----------

pub enum Outcome {
    Installed { dir: PathBuf, scope: Scope },
    Updated { dir: PathBuf, from: String },
}

/// Install/update in scope `scope` (for Machine — via UAC).
fn run_install(owner: Option<HWND>, scope: Scope, dir: &Path, op: &str) -> Result<(), ElevError> {
    if scope == Scope::User || is_elevated() {
        return do_install(scope, dir).map_err(|e| {
            util::error(owner, &e);
            ElevError::Failed
        });
    }
    run_elevated(owner, &format!("--elevated {op} --target \"{}\"", dir.display()))
}

/// Updates the existing installation with this copy.
pub fn update_installed(owner: Option<HWND>, inst: &Installed) -> Result<bool, String> {
    match run_install(owner, inst.scope, &inst.target_dir(), "update") {
        Ok(()) => Ok(true),
        Err(ElevError::Cancelled) => Ok(false),
        Err(ElevError::Failed) => Err(tr!("Update failed", "Обновление не выполнено").into()),
    }
}

/// Note for updating a 0.1.x installation: the program moves to the folder with the new name.
fn rename_note(inst: &Installed) -> String {
    if inst.is_legacy() {
        trf!(
            "\n\nThe program is now called vibe-md-viewer and will move to the folder\n{}\n(the old folder will be removed once the old version is closed).",
            "\n\nПрограмма теперь называется vibe-md-viewer и переедет в папку\n{}\n(прежняя папка удалится, когда старая версия будет закрыта).",
            inst.target_dir().display()
        )
    } else {
        String::new()
    }
}

/// Offers an update when a newer exe is started. true — updated.
pub fn offer_update(owner: Option<HWND>, inst: &Installed) -> Result<bool, String> {
    let admin = if inst.scope == Scope::Machine {
        tr!("\n\nWindows will ask you to confirm administrator rights.", "\n\nWindows попросит подтвердить права администратора.")
    } else {
        ""
    };
    let msg = trf!(
        "Version {ver} is installed in\n{dir}\n\nUpdate to version {VERSION}?\nYour settings and the .md file association will be kept.{note}{admin}\n\nNo — just run this copy without installing.",
        "Установлена версия {ver} в папке\n{dir}\n\nОбновить до версии {VERSION}?\nНастройки и привязка файлов .md сохранятся.{note}{admin}\n\nНет — просто запустить эту копию без установки.",
        ver = inst.version,
        dir = inst.dir.display(),
        note = rename_note(inst)
    );
    if !util::ask_yes_no(owner, &msg, false) {
        return Ok(false);
    }
    update_installed(owner, inst)
}

pub fn install_interactive(owner: Option<HWND>) -> Result<Option<Outcome>, String> {
    // Already installed → update / reinstall.
    if let Some(inst) = installed() {
        let (msg, warn) = match compare_versions(VERSION, &inst.version) {
            Ordering::Greater => (
                trf!(
                    "Update vibe-md-viewer {ver} → {VERSION}?\n\nFolder: {dir}\nYour settings and the .md association will be kept.{note}",
                    "Обновить vibe-md-viewer {ver} → {VERSION}?\n\nПапка: {dir}\nНастройки и привязка .md сохранятся.{note}",
                    ver = inst.version,
                    dir = inst.target_dir().display(),
                    note = rename_note(&inst)
                ),
                false,
            ),
            Ordering::Equal => (
                trf!(
                    "Version {VERSION} is already installed in\n{}\n\nReinstall (replace the program file)?",
                    "Версия {VERSION} уже установлена в\n{}\n\nПереустановить (заменить файл программы)?",
                    inst.dir.display()
                ),
                false,
            ),
            Ordering::Less => (
                trf!(
                    "A newer version {ver} is installed in\n{dir}\n\nReplace it with the older version {VERSION}?",
                    "В папке {dir}\nустановлена более новая версия {ver}.\n\nЗаменить её более старой {VERSION}?",
                    dir = inst.dir.display(),
                    ver = inst.version
                ),
                true,
            ),
        };
        if !util::ask_yes_no(owner, &msg, warn) {
            return Ok(None);
        }
        return match update_installed(owner, &inst)? {
            true => Ok(Some(Outcome::Updated { dir: inst.target_dir(), from: inst.version.clone() })),
            false => Ok(None),
        };
    }

    // Fresh install: Program Files for all users (a custom folder is not allowed, see do_install).
    let dir = Scope::Machine.default_dir();
    let msg = trf!(
        "Install vibe-md-viewer {VERSION} to the folder:\n\n{}\n\nWindows will ask you to confirm administrator rights.",
        "Установить vibe-md-viewer {VERSION} в папку:\n\n{}\n\nWindows попросит подтвердить права администратора.",
        dir.display()
    );
    if !util::ask_yes_no(owner, &msg, false) {
        return Ok(None);
    }
    let Some((dir, scope)) = install_fresh(owner, &dir)? else { return Ok(None) };
    prompt_default_with_hint(owner);
    Ok(Some(Outcome::Installed { dir, scope }))
}

/// Fresh install into `dir` for all users (UAC). If there are no rights or the request is declined —
/// offers to install for the current user only. Returns the actual folder and scope.
pub fn install_fresh(owner: Option<HWND>, dir: &Path) -> Result<Option<(PathBuf, Scope)>, String> {
    validate_install_dir(dir)?;
    match run_install(owner, Scope::Machine, dir, "install") {
        Ok(()) => Ok(Some((dir.to_path_buf(), Scope::Machine))),
        Err(_) => {
            let udir = Scope::User.default_dir();
            let q = trf!(
                "Installation for all users failed (no administrator rights, or the request was declined).\n\nInstall just for you to the folder\n{}?\nThis does not require administrator rights.",
                "Установка для всех пользователей не выполнена (нет прав администратора или запрос отклонён).\n\nУстановить только для вас в папку\n{}?\nПрава администратора для этого не нужны.",
                udir.display()
            );
            if !util::ask_yes_no(owner, &q, false) {
                return Ok(None);
            }
            do_install(Scope::User, &udir)?;
            Ok(Some((udir, Scope::User)))
        }
    }
}

/// Explanation + Windows Settings → Default apps (choosing the "default program").
pub fn prompt_default_with_hint(owner: Option<HWND>) {
    util::info(
        owner,
        tr!(
            "Windows Settings → Default apps will open now.\n\nWindows 11: on the vibe-md-viewer page, set it for .md.\nWindows 10: click “Choose default apps by file type”, find .md and select vibe-md-viewer.\n\nThen .md files will open with a double-click.",
            "Сейчас откроются «Параметры Windows → Приложения по умолчанию».\n\nWindows 11: на странице vibe-md-viewer назначьте его для .md.\nWindows 10: нажмите «Выбор стандартных приложений для типов файлов», найдите .md и выберите vibe-md-viewer.\n\nТогда файлы .md будут открываться двойным щелчком."
        ),
    );
    prompt_default(owner);
}

/// Windows Settings → Default apps; on Windows 11 right on our app's page
/// (`registeredAppMachine`/`registeredAppUser` = our RegisteredApplications name; Windows 10
/// ignores the parameter and opens the general page). Windows doesn't let apps make themselves
/// the default silently — the user chooses there. The old SHOpenWithDialog didn't show up when
/// called from our window and left a hidden OpenWith.exe process behind on every click.
pub fn prompt_default(owner: Option<HWND>) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let uri = wide(&settings_uri(installed().map(|i| i.scope).unwrap_or(Scope::Machine)));
    let verb = wide("open");
    unsafe {
        ShellExecuteW(owner, pcwstr(&verb), pcwstr(&uri), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// The Settings page with our app's default-app choices (see prompt_default).
fn settings_uri(scope: Scope) -> String {
    let key = if scope == Scope::Machine { "registeredAppMachine" } else { "registeredAppUser" };
    format!("ms-settings:defaultapps?{key}={REGISTERED_NAME}")
}

pub fn uninstall_interactive(owner: Option<HWND>) -> Result<bool, String> {
    let Some(inst) = installed() else {
        util::info(owner, tr!("vibe-md-viewer is not installed.", "vibe-md-viewer не установлен."));
        return Ok(false);
    };
    let q = trf!(
        "Uninstall vibe-md-viewer ({scope}) and its .md file association?\n\nFolder: {dir}",
        "Удалить vibe-md-viewer ({scope}) и его привязку к файлам .md?\n\nПапка: {dir}",
        scope = inst.scope.label(),
        dir = inst.dir.display()
    );
    if !util::ask_yes_no(owner, &q, true) {
        return Ok(false);
    }
    if inst.scope == Scope::User || is_elevated() {
        do_uninstall(inst.scope)?;
    } else {
        match run_elevated(owner, "--elevated uninstall") {
            Ok(()) => {}
            Err(ElevError::Cancelled) => return Ok(false),
            Err(ElevError::Failed) => return Err(tr!("Uninstall failed", "Удаление не выполнено").into()),
        }
    }
    util::info(
        owner,
        tr!(
            "vibe-md-viewer has been uninstalled. Your settings remain in %APPDATA%\\vibe-md-viewer.",
            "vibe-md-viewer удалён. Ваши настройки остались в %APPDATA%\\vibe-md-viewer."
        ),
    );
    Ok(true)
}

/// A folder with a running exe cannot be deleted immediately — a background cmd tries `tries` times
/// (≈ every 2 seconds) until all copies of the program are closed.
fn schedule_dir_delete(dir: &Path, tries: u32) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    if !is_our_install_dir(dir) {
        return Err(trf!("{} is not the program's installation folder", "{} — не папка установки программы", dir.display()));
    }
    let s = dir.to_string_lossy().to_string();
    let mut sysdir = vec![0u16; 260];
    let n = unsafe { windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut sysdir)) } as usize;
    let system32 = PathBuf::from(String::from_utf16_lossy(&sysdir[..n]));
    let cmd = system32.join("cmd.exe");
    // ping — by full path, current folder — System32: cmd looks for commands in the current
    // folder first, and a planted ping.bat must not run.
    let ping = system32.join("PING.EXE");
    let ps = ping.to_string_lossy();
    if ps.contains('"') || ps.contains('%') {
        return Err(tr!("Unexpected system folder path", "Неожиданный путь к системной папке").into());
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(cmd)
        .raw_arg(format!(
            "/d /v:off /s /c \"for /l %i in (1,1,{tries}) do @(if exist \"{s}\" (rmdir /s /q \"{s}\" 2>nul & \"{ps}\" -n 2 127.0.0.1 >nul))\""
        ))
        .env("NoDefaultCurrentDirectoryInExePath", "1")
        .creation_flags(CREATE_NO_WINDOW)
        .current_dir(&system32)
        .spawn()
        .map_err(|e| trf!("Could not delete the folder: {e}", "Не удалось удалить папку: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("1.2.3"), (1, 2, 3));
        assert_eq!(parse_version("0.10.0-beta"), (0, 10, 0));
        assert_eq!(compare_versions("0.2.0", "0.1.9"), Ordering::Greater);
        assert_eq!(compare_versions("0.1.10", "0.1.9"), Ordering::Greater);
        assert_eq!(compare_versions("0.1.0", "0.1.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.1.0", "1.0.0"), Ordering::Less);
    }

    #[test]
    fn default_dirs_and_validation() {
        let m = Scope::Machine.default_dir();
        assert!(m.to_string_lossy().to_ascii_lowercase().contains("program files"), "{}", m.display());
        assert!(m.ends_with(DIR_NAME));
        assert!(Scope::User.default_dir().to_string_lossy().contains("Programs"));
        assert!(validate_install_dir(&m).is_ok());
        assert!(validate_install_dir(Path::new("D:\\Apps\\vibe-md-viewer")).is_ok());
        for bad in ["C:\\", "C:\\Windows\\vibe-md-viewer", "C:\\WINDOWS\\System32\\vibe-md-viewer", "C:\\Program Files\\Other", "relative\\vibe-md-viewer", "\\\\srv\\share\\x\\vibe-md-viewer", "C:\\a\"b\\vibe-md-viewer"] {
            assert!(validate_install_dir(Path::new(bad)).is_err(), "{bad}");
        }
    }

    /// A fresh folder name under .toolcache\tmp (unique per process: several test runs may overlap).
    fn test_dir(name: &str) -> PathBuf {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".toolcache").join("tmp").join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    /// A temporary registry branch HKCU\Software\VibeMdViewer-Test-<pid>-<name>\ (delete with `drop_test_reg`).
    fn test_reg(name: &str) -> (Reg, String) {
        let base = format!("Software\\VibeMdViewer-Test-{}-{name}", std::process::id());
        (Reg { root: HKEY_CURRENT_USER, prefix: format!("{base}\\") }, base)
    }

    fn drop_test_reg(base: &str) {
        let root = Reg { root: HKEY_CURRENT_USER, prefix: String::new() };
        root.delete_tree(base);
        assert!(root.get_str(base, None).is_none() && root.get_str(&format!("{base}\\{APP_KEY}"), Some("InstallDir")).is_none());
    }

    fn get_dword(reg: &Reg, sub: &str, name: &str) -> Option<u32> {
        let ws = wide(&reg.p(sub));
        let wn = wide(name);
        let mut v = 0u32;
        let mut size = 4u32;
        let r = unsafe { RegGetValueW(reg.root, pcwstr(&ws), pcwstr(&wn), RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut core::ffi::c_void), Some(&mut size)) };
        (r == ERROR_SUCCESS).then_some(v)
    }

    #[test]
    fn version_edge_cases() {
        for (s, v) in [
            ("", (0, 0, 0)),
            ("garbage", (0, 0, 0)),
            ("1", (1, 0, 0)),
            ("1.2", (1, 2, 0)),
            (" 1.2.3 \n", (1, 2, 3)),
            ("1.2.3+build.5", (1, 2, 3)),
            ("1.2.3.4", (1, 2, 3)),
            ("1..3", (1, 0, 3)),
            ("v1.2.3", (0, 2, 3)),
            ("4294967296.1.1", (0, 1, 1)),
            ("4294967295.0.0", (u32::MAX, 0, 0)),
        ] {
            assert_eq!(parse_version(s), v, "{s:?}");
        }
        assert_eq!(compare_versions("1.2.3-beta", "1.2.3"), Ordering::Equal, "pre-release tags are ignored");
        assert_eq!(compare_versions("garbage", "0.0.1"), Ordering::Less);
        assert_eq!(compare_versions("10.0.0", "9.99.99"), Ordering::Greater, "numeric, not lexicographic");
        assert_eq!(compare_versions(VERSION, VERSION), Ordering::Equal);
        assert_ne!(parse_version(VERSION), (0, 0, 0), "the crate version parses");
    }

    #[test]
    fn scopes() {
        assert_eq!(Scope::Machine.root(), HKEY_LOCAL_MACHINE);
        assert_eq!(Scope::User.root(), HKEY_CURRENT_USER);
        assert_eq!(Scope::Machine.label(), "for all users");
        assert_eq!(Scope::User.label(), "for the current user only");
        let r = Reg::for_scope(Scope::User);
        assert_eq!((r.root, r.prefix.as_str()), (HKEY_CURRENT_USER, ""));
        assert_eq!(r.p(APP_KEY), APP_KEY);
        let (t, base) = test_reg("prefix");
        assert_eq!(t.p("Software\\X"), format!("{base}\\Software\\X"));
        assert!(program_files().is_absolute());
        assert_eq!(Scope::Machine.default_dir(), program_files().join(DIR_NAME));
        let user = Scope::User.default_dir();
        assert!(user.ends_with(Path::new("Programs").join(DIR_NAME)), "{}", user.display());
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            assert_eq!(user, PathBuf::from(local).join("Programs").join(DIR_NAME));
        }
    }

    #[test]
    fn install_dir_shape_edge_cases() {
        for ok in ["D:\\Apps\\vibe-md-viewer", "D:\\Apps\\VIBE-MD-VIEWER", "D:\\vibe-md-viewer", "C:\\Program Files (x86)\\vibe-md-viewer"] {
            assert!(validate_install_dir(Path::new(ok)).is_ok(), "{ok}");
        }
        let err = |p: &str| validate_install_dir(Path::new(p)).unwrap_err();
        assert!(err("D:\\Apps\\..\\vibe-md-viewer").contains("“..”"));
        assert!(err(".\\vibe-md-viewer").contains("“..”"), "a leading . is a CurDir component");
        for bad in ["D:\\100%\\vibe-md-viewer", "D:\\a!b\\vibe-md-viewer", "D:\\Apps\\vibe-md-viewer\\", "D:\\a\"b\\vibe-md-viewer"] {
            assert!(err(bad).contains("invalid characters"), "{bad}");
        }
        for bad in ["vibe-md-viewer", "C:vibe-md-viewer", "\\Apps\\vibe-md-viewer", "\\\\?\\C:\\Apps\\vibe-md-viewer", "\\\\server\\share\\vibe-md-viewer"] {
            assert!(err(bad).contains("local absolute path"), "{bad}");
        }
        for bad in ["D:\\Apps\\vibe-md-viewer.exe", "D:\\Apps\\vibe-md-viewer2", "D:\\Apps\\vibe-mdViewer"] {
            assert!(err(bad).contains("must be named vibe-md-viewer"), "{bad}");
        }
        if let Some(win) = std::env::var_os("SystemRoot") {
            let p = PathBuf::from(win).join("Temp").join(DIR_NAME);
            assert!(validate_install_dir(&p).unwrap_err().contains("Windows system folder"), "{}", p.display());
        }
        // The former name is accepted only where a 0.1.x installation is recognized (uninstall/migration).
        assert!(validate_dir_shape(Path::new("D:\\Apps\\vibe-mdViewer"), &[DIR_NAME, LEGACY_DIR_NAME]).is_ok());
    }

    #[test]
    fn names_and_same_dir() {
        assert!(has_name(Path::new("C:\\x\\Vibe-MD-Viewer"), DIR_NAME));
        assert!(!has_name(Path::new("C:\\"), DIR_NAME), "a drive root has no name");
        assert!(!has_name(Path::new("C:\\x\\vibe-md-viewer.exe"), DIR_NAME));
        assert!(same_dir(Path::new("C:\\A\\\\"), Path::new("c:\\a")), "trailing separators are ignored");
        assert!(!same_dir(Path::new("C:\\A"), Path::new("C:\\B")));
        assert!(!same_dir(Path::new("C:\\A\\vibe"), Path::new("C:\\A\\vibe-md-viewer")));
        let pf = Installed { scope: Scope::Machine, dir: program_files().join(DIR_NAME), version: "0.2.0".into() };
        assert!(!pf.is_legacy());
        assert_eq!(pf.target_dir(), pf.dir);
        let user_legacy = Installed { scope: Scope::User, dir: PathBuf::from("D:\\Tools\\VIBE-MDVIEWER"), version: "0.1.0".into() };
        assert!(user_legacy.is_legacy(), "case-insensitive");
        assert_eq!(user_legacy.target_dir(), Scope::User.default_dir());
    }

    #[test]
    fn rename_note_only_for_legacy_installs() {
        let cur = Installed { scope: Scope::User, dir: Scope::User.default_dir(), version: "0.2.0".into() };
        assert_eq!(rename_note(&cur), "");
        let old = Installed { scope: Scope::User, dir: PathBuf::from("D:\\Tools\\vibe-mdViewer"), version: "0.1.3".into() };
        let note = rename_note(&old);
        assert!(note.starts_with("\n\nThe program is now called vibe-md-viewer"), "{note}");
        assert!(note.contains(&Scope::User.default_dir().display().to_string()), "{note}");
    }

    #[test]
    fn our_install_dir_needs_marker_and_exe() {
        let root = test_dir("ours-test");
        let dir = root.join(DIR_NAME);
        fs::create_dir_all(&dir).unwrap();
        assert!(!is_our_install_dir(&dir), "empty folder");
        fs::write(dir.join(MARKER), b"m").unwrap();
        assert!(!is_our_install_dir(&dir), "marker without the program");
        fs::write(dir.join(EXE_NAME), b"exe").unwrap();
        assert!(is_our_install_dir(&dir));
        assert!(may_delete(Scope::User, &dir));
        assert!(!may_delete(Scope::Machine, &dir), "not directly in Program Files");
        // The same folder reached through ".." is refused by the path shape check.
        let indirect = root.join("x").join("..").join(DIR_NAME);
        assert!(!is_our_install_dir(&indirect));
        assert!(schedule_dir_delete(&indirect, 1).is_err(), "nothing is started for a foreign path");
        // Program without the marker → not ours.
        fs::remove_file(dir.join(MARKER)).unwrap();
        assert!(!is_our_install_dir(&dir));
        let e = schedule_dir_delete(&dir, 1).unwrap_err();
        assert!(e.contains("is not the program's installation folder"), "{e}");
        // A former marker with the new exe also counts (partially migrated installation).
        fs::write(dir.join(LEGACY_MARKER), b"m").unwrap();
        assert!(is_our_install_dir(&dir));
        assert!(!may_delete(Scope::Machine, Path::new("C:\\nonexistent\\vibe-md-viewer")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn install_files_edge_cases() {
        let root = test_dir("install-edge-test");
        let dir = root.join(DIR_NAME);
        let e = install_files(&root.join("missing.exe"), &dir).unwrap_err();
        assert!(e.starts_with("Could not open"), "{e}");
        assert!(!dir.exists());

        // A stale .old copy from an earlier update is replaced and then removed.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(EXE_NAME), b"old version").unwrap();
        fs::write(dir.join(OLD_EXE_NAME), b"stale").unwrap();
        let src = root.join("new.exe");
        fs::write(&src, b"new version").unwrap();
        install_files(&src, &dir).unwrap();
        assert_eq!(fs::read(dir.join(EXE_NAME)).unwrap(), b"new version");
        assert!(!dir.join(OLD_EXE_NAME).exists());
        assert_eq!(fs::read_to_string(dir.join(MARKER)).unwrap(), format!("vibe-md-viewer {VERSION}\n"));
        let help = fs::read_to_string(dir.join(HELP_DOC)).unwrap();
        assert!(help.contains(&format!("Version {VERSION}.")) && !help.contains("{{STATUS}}"));

        // Installing the installed exe over itself (same file): nothing is copied or renamed.
        fs::remove_file(dir.join(MARKER)).unwrap();
        install_files(&dir.join(EXE_NAME), &dir).unwrap();
        assert_eq!(fs::read(dir.join(EXE_NAME)).unwrap(), b"new version");
        assert!(!dir.join(OLD_EXE_NAME).exists() && dir.join(MARKER).exists());

        // An invalid folder is refused before anything is created.
        let bad = root.join("x").join("..").join(DIR_NAME);
        assert!(install_files(&src, &bad).is_err());
        assert!(!root.join("x").exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// The copy fails (the source handle cannot be read): the installed program is restored.
    #[test]
    fn failed_copy_rolls_back() {
        let root = test_dir("rollback-test");
        let src_path = root.join("src.exe");
        fs::create_dir_all(&root).unwrap();
        fs::write(&src_path, b"new version").unwrap();
        let write_only = || fs::OpenOptions::new().write(true).open(&src_path).unwrap();

        let dir = root.join("upd").join(DIR_NAME);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(EXE_NAME), b"installed").unwrap();
        let e = install_files_from(&mut write_only(), &dir).unwrap_err();
        assert!(e.starts_with("Could not copy the program"), "{e}");
        assert_eq!(fs::read(dir.join(EXE_NAME)).unwrap(), b"installed", "the previous exe is put back");
        assert!(!dir.join(OLD_EXE_NAME).exists() && !dir.join(MARKER).exists());

        // Fresh folder: no half-written exe is left behind.
        let fresh = root.join("new").join(DIR_NAME);
        assert!(install_files_from(&mut write_only(), &fresh).is_err());
        assert!(!fresh.join(EXE_NAME).exists() && !fresh.join(MARKER).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn installed_from_registry() {
        let (reg, base) = test_reg("installed");
        let root = test_dir("installed-test");
        assert!(installed_at(&reg, Scope::User).is_none(), "no registration");

        // Registered, but the folder has no program (removed by hand) → not installed.
        let dir = root.join(DIR_NAME);
        fs::create_dir_all(&dir).unwrap();
        reg.set_str(APP_KEY, Some("InstallDir"), &dir.to_string_lossy()).unwrap();
        assert!(installed_at(&reg, Scope::User).is_none());

        fs::write(dir.join(EXE_NAME), b"exe").unwrap();
        let i = installed_at(&reg, Scope::User).expect("installed");
        assert_eq!((i.scope, i.dir.as_path(), i.version.as_str()), (Scope::User, dir.as_path(), "0.0.0"), "no Version value");
        reg.set_str(APP_KEY, Some("Version"), "0.1.7").unwrap();
        assert_eq!(installed_at(&reg, Scope::Machine).unwrap().version, "0.1.7");

        // 0.1.x: the former folder name with the former exe name.
        let legacy = root.join(LEGACY_DIR_NAME);
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join(LEGACY_EXE_NAME), b"old").unwrap();
        reg.set_str(APP_KEY, Some("InstallDir"), &legacy.to_string_lossy()).unwrap();
        let i = installed_at(&reg, Scope::User).expect("legacy install");
        assert!(i.is_legacy());
        // The former exe name in a folder with another name does not count.
        let other = root.join("Other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join(LEGACY_EXE_NAME), b"old").unwrap();
        reg.set_str(APP_KEY, Some("InstallDir"), &other.to_string_lossy()).unwrap();
        assert!(installed_at(&reg, Scope::User).is_none());

        drop_test_reg(&base);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn registry_values_and_errors() {
        let (reg, base) = test_reg("values");
        // Default value, named value, empty and non-ASCII strings.
        reg.set_str("K", None, "default").unwrap();
        reg.set_str("K", Some("name"), "Значение ✓").unwrap();
        reg.set_str("K", Some("empty"), "").unwrap();
        assert_eq!(reg.get_str("K", None).as_deref(), Some("default"));
        assert_eq!(reg.get_str("K", Some("name")).as_deref(), Some("Значение ✓"));
        assert_eq!(reg.get_str("K", Some("empty")).as_deref(), Some(""));
        assert_eq!(reg.get_str("K", Some("missing")), None);
        assert_eq!(reg.get_str("Missing", None), None);
        // A DWORD is not read as a string.
        reg.set_dword("K", "n", 42).unwrap();
        assert_eq!(get_dword(&reg, "K", "n"), Some(42));
        assert_eq!(reg.get_str("K", Some("n")), None);
        reg.delete_value("K", "name");
        assert_eq!(reg.get_str("K", Some("name")), None);
        reg.delete_value("K", "never-existed");
        reg.delete_tree("Missing\\Deeper");

        // A key name component over 255 characters cannot be created.
        let long = "k".repeat(300);
        let e = reg.set_str(&long, None, "x").unwrap_err();
        assert!(e.starts_with("Could not write to the registry") && e.contains(&base), "{e}");
        assert!(reg.set_dword(&long, "n", 1).is_err());
        // A value name over 16383 characters cannot be written.
        let huge = "v".repeat(20000);
        assert!(reg.set_str("K", Some(&huge), "x").unwrap_err().starts_with("Could not write to the registry"));
        assert!(reg.set_dword("K", &huge, 1).unwrap_err().starts_with("Could not write to the registry"));

        reg.delete_tree("K");
        assert_eq!(reg.get_str("K", None), None);
        drop_test_reg(&base);
    }

    #[test]
    fn registry_register_for_current_user() {
        let (reg, base) = test_reg("user");
        let root = test_dir("register-user-test");
        let dir = root.join(DIR_NAME);
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(EXE_NAME);
        fs::write(&exe, vec![0u8; 5000]).unwrap();
        let classes = "Software\\Classes";
        // An empty default value means "nobody" → taken; an existing Content Type is kept.
        reg.set_str(&format!("{classes}\\.mkd"), None, "").unwrap();
        reg.set_str(&format!("{classes}\\.md"), Some("Content Type"), "text/x-custom").unwrap();

        register_at(&reg, Scope::User, &exe, &dir).unwrap();
        assert_eq!(reg.get_str(APP_KEY, Some("Scope")).as_deref(), Some("user"));
        assert_eq!(reg.get_str(APP_KEY, Some("InstallDir")).as_deref(), Some(&*dir.to_string_lossy()));
        assert_eq!(get_dword(&reg, UNINSTALL_KEY, "EstimatedSize"), Some(4), "5000 bytes → 4 KB");
        assert_eq!(get_dword(&reg, UNINSTALL_KEY, "NoModify"), Some(1));
        assert_eq!(get_dword(&reg, UNINSTALL_KEY, "NoRepair"), Some(1));
        let exe_s = exe.to_string_lossy();
        assert_eq!(reg.get_str(UNINSTALL_KEY, Some("UninstallString")), Some(format!("\"{exe_s}\" --uninstall")));
        assert_eq!(reg.get_str(&format!("{classes}\\{PROGID}\\shell\\edit\\command"), None), Some(format!("\"{exe_s}\" --mode visual \"%1\"")));
        assert_eq!(reg.get_str(&format!("{classes}\\{PROGID}\\DefaultIcon"), None), Some(format!("\"{exe_s}\",0")));
        assert_eq!(reg.get_str(&format!("{classes}\\.mkd"), None).as_deref(), Some(PROGID));
        assert_eq!(reg.get_str(&format!("{classes}\\.md"), Some("Content Type")).as_deref(), Some("text/x-custom"));
        assert_eq!(reg.get_str(&format!("{classes}\\.mdwn"), Some("Content Type")).as_deref(), Some("text/markdown"));
        assert_eq!(reg.get_str(&format!("{classes}\\.mdwn"), Some("PerceivedType")).as_deref(), Some("text"));
        for ext in EXTS {
            assert_eq!(reg.get_str(&format!("{APP_KEY}\\Capabilities\\FileAssociations"), Some(ext)).as_deref(), Some(PROGID), "{ext}");
            assert!(reg.get_str(&format!("{classes}\\Applications\\{EXE_NAME}\\SupportedTypes"), Some(ext)).is_some(), "{ext}");
        }
        let i = installed_at(&reg, Scope::User).expect("registered and present");
        assert_eq!((i.dir.as_path(), i.version.as_str()), (dir.as_path(), VERSION));

        unregister_at(&reg);
        assert!(installed_at(&reg, Scope::User).is_none());
        assert_eq!(reg.get_str(&format!("{classes}\\.mkd"), None).as_deref(), Some(""), "our default is cleared");
        assert!(reg.get_str(&format!("{classes}\\Applications\\{EXE_NAME}\\shell\\open\\command"), None).is_none());
        drop_test_reg(&base);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn process_state_without_side_effects() {
        // Tests run from target\...\deps: no installer marker next to the exe.
        assert!(!running_installed());
        assert_eq!(is_elevated(), is_elevated());
        // Without administrator rights the elevated entry point refuses at once (no UI, no changes).
        if !is_elevated() {
            assert_eq!(elevated_main("not-an-operation", None), 5);
        }
    }

    #[test]
    fn cleanup_old_removes_the_leftover_copy() {
        let old = util::exe_dir().unwrap().join(OLD_EXE_NAME);
        fs::write(&old, b"old").unwrap();
        cleanup_old();
        assert!(!old.exists());
        cleanup_old(); // nothing to remove — no error
    }

    #[test]
    fn install_and_update_files() {
        let root = test_dir("install-test");
        let src_dir = root.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let v1 = src_dir.join("v1.exe");
        let v2 = src_dir.join("v2.exe");
        fs::write(&v1, b"version-1").unwrap();
        fs::write(&v2, b"version-2").unwrap();
        let dir = root.join("Programs").join(DIR_NAME);

        install_files(&v1, &dir).unwrap();
        assert_eq!(fs::read(dir.join(EXE_NAME)).unwrap(), b"version-1");
        assert!(dir.join(MARKER).exists() && dir.join(HELP_DOC).exists());

        install_files(&v2, &dir).unwrap();
        assert_eq!(fs::read(dir.join(EXE_NAME)).unwrap(), b"version-2");
        assert!(!dir.join(OLD_EXE_NAME).exists(), "the old copy is removed if not running");

        // a "running" old exe: the file is open without delete sharing — like the Windows loader does
        {
            use std::os::windows::fs::OpenOptionsExt;
            let _lock = fs::OpenOptions::new().read(true).share_mode(0x1 | 0x4 /* READ | DELETE */).open(dir.join(EXE_NAME)).unwrap();
            install_files(&v1, &dir).unwrap();
        }
        assert_eq!(fs::read(dir.join(EXE_NAME)).unwrap(), b"version-1");
        assert!(install_files(&v1, &root.join("wrong-name")).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    /// Migration of a 0.1.x installation (vibe-mdViewer folder, vibe-md.exe) to the new name.
    #[test]
    fn legacy_install_is_recognized_and_moved() {
        let pf = program_files();
        let old = Installed { scope: Scope::Machine, dir: pf.join(LEGACY_DIR_NAME), version: "0.1.3".into() };
        assert!(old.is_legacy());
        assert_eq!(old.target_dir(), pf.join(DIR_NAME));
        // a custom "all users" folder from 0.1.x — moves to Program Files (a custom folder is no longer allowed)
        let custom = Installed { scope: Scope::Machine, dir: PathBuf::from("D:\\Programs\\vibe-mdViewer"), version: "0.1.3".into() };
        assert_eq!(custom.target_dir(), pf.join(DIR_NAME));
        let cur = Installed { scope: Scope::User, dir: Scope::User.default_dir(), version: "0.2.0".into() };
        assert!(!cur.is_legacy());
        assert_eq!(cur.target_dir(), cur.dir);
        // for all users — Program Files only; the elevated copy may delete only there
        assert!(may_delete(Scope::User, Path::new("D:\\nonexistent\\vibe-md-viewer")) == false, "no marker and exe — no");
        // Installing into a folder with the former name is not allowed — only migrating from it.
        assert!(validate_install_dir(Path::new("C:\\Program Files\\vibe-mdViewer")).is_err());

        // Only "our" folder may be deleted: correct name + marker + exe.
        let root = test_dir("legacy-test");
        let legacy = root.join(LEGACY_DIR_NAME);
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join(LEGACY_EXE_NAME), b"old").unwrap();
        assert!(!is_our_install_dir(&legacy), "no installer marker — not ours");
        fs::write(legacy.join(LEGACY_MARKER), b"vibe-mdViewer 0.1.3").unwrap();
        assert!(is_our_install_dir(&legacy));
        assert!(may_delete(Scope::User, &legacy), "our own \"current user\" installation — may be deleted");
        assert!(!may_delete(Scope::Machine, &legacy), "as administrator — only inside Program Files");
        let other = root.join("Something");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join(LEGACY_EXE_NAME), b"x").unwrap();
        fs::write(other.join(LEGACY_MARKER), b"x").unwrap();
        assert!(!is_our_install_dir(&other), "foreign folder name");
        assert!(schedule_dir_delete(&other, 1).is_err());
        assert!(same_dir(Path::new("C:\\A\\vibe-md-viewer\\"), Path::new("c:\\a\\VIBE-MD-VIEWER")));
        let _ = fs::remove_dir_all(&root);
    }

    /// The installer copies the running image itself: the file is found via the mapping and matches
    /// the exe on disk (finding of the 0.2.0 audit — setup.exe replaced during the UAC prompt).
    #[test]
    fn own_image_is_opened_by_mapping() {
        let mut f = open_own_image().expect("image found");
        let exe = std::env::current_exe().unwrap();
        assert_eq!(f.metadata().unwrap().len(), fs::metadata(&exe).unwrap().len());
        assert_eq!(file_id(&f), open_for_id(&exe).as_ref().and_then(file_id));
        // While the file is open this way, the running exe cannot be renamed.
        let moved = exe.with_extension("moved.exe");
        assert!(fs::rename(&exe, &moved).is_err(), "renaming must be denied");
        let mut head = [0u8; 2];
        std::io::Read::read_exact(&mut f, &mut head).unwrap();
        assert_eq!(&head, b"MZ");
    }

    #[test]
    fn registry_register_update_unregister() {
        // Temporary branch HKCU\Software\VibeMdViewer-Test-<pid>\..., deleted at the end.
        // The logic is the same as for HKLM (only the root differs).
        let base = format!("Software\\VibeMdViewer-Test-{}", std::process::id());
        let reg = Reg { root: HKEY_CURRENT_USER, prefix: format!("{base}\\") };
        let classes = "Software\\Classes";
        // .md already belongs to another program, .markdown belongs to nobody
        reg.set_str(&format!("{classes}\\.md"), None, "OtherApp.md").unwrap();

        // Entries of the previous version 0.1.x (name vibe-mdViewer).
        reg.set_str(&format!("{classes}\\Applications\\{LEGACY_EXE_NAME}\\shell\\open\\command"), None, "\"C:\\Program Files\\vibe-mdViewer\\vibe-md.exe\" \"%1\"").unwrap();
        reg.set_str("Software\\RegisteredApplications", Some(LEGACY_APP_NAME), "Software\\VibeMdViewer\\Capabilities").unwrap();

        let dir = PathBuf::from("C:\\Program Files\\vibe-md-viewer");
        let exe = dir.join(EXE_NAME);
        register_at(&reg, Scope::Machine, &exe, &dir).unwrap();
        unregister_legacy(&reg); // as in do_install
        assert!(reg.get_str(&format!("{classes}\\Applications\\{LEGACY_EXE_NAME}\\shell\\open\\command"), None).is_none(), "the old name is removed from \"Open with\"");
        assert!(reg.get_str("Software\\RegisteredApplications", Some(LEGACY_APP_NAME)).is_none());
        assert!(reg.get_str("Software\\RegisteredApplications", Some(REGISTERED_NAME)).is_some());
        // "Make default" opens Settings right on this app (Windows 11 deep link).
        assert_eq!(settings_uri(Scope::Machine), "ms-settings:defaultapps?registeredAppMachine=vibe-md-viewer");
        assert_eq!(settings_uri(Scope::User), "ms-settings:defaultapps?registeredAppUser=vibe-md-viewer");
        assert!(reg.get_str(&format!("{classes}\\Applications\\{EXE_NAME}\\shell\\open\\command"), None).is_some());
        assert_eq!(
            reg.get_str(&format!("{classes}\\{PROGID}\\shell\\open\\command"), None).as_deref(),
            Some("\"C:\\Program Files\\vibe-md-viewer\\vibe-md-viewer.exe\" \"%1\"")
        );
        assert_eq!(reg.get_str(&format!("{classes}\\.md"), None).as_deref(), Some("OtherApp.md"), "another program is not overridden");
        assert_eq!(reg.get_str(&format!("{classes}\\.markdown"), None).as_deref(), Some(PROGID));
        assert!(reg.get_str(&format!("{classes}\\.md\\OpenWithProgids"), Some(PROGID)).is_some());
        assert_eq!(reg.get_str(APP_KEY, Some("Version")).as_deref(), Some(VERSION));
        assert_eq!(reg.get_str(UNINSTALL_KEY, Some("DisplayVersion")).as_deref(), Some(VERSION));
        assert_eq!(reg.get_str(APP_KEY, Some("Scope")).as_deref(), Some("machine"));

        // Repeated registration (update) is idempotent.
        register_at(&reg, Scope::Machine, &exe, &dir).unwrap();
        assert_eq!(reg.get_str(&format!("{classes}\\.md"), None).as_deref(), Some("OtherApp.md"));

        unregister_at(&reg);
        assert_eq!(reg.get_str(&format!("{classes}\\.md"), None).as_deref(), Some("OtherApp.md"));
        assert!(reg.get_str(&format!("{classes}\\.md\\OpenWithProgids"), Some(PROGID)).is_none());
        assert_ne!(reg.get_str(&format!("{classes}\\.markdown"), None).as_deref(), Some(PROGID));
        assert!(reg.get_str(&format!("{classes}\\{PROGID}\\shell\\open\\command"), None).is_none());
        assert!(reg.get_str(APP_KEY, Some("Version")).is_none());
        assert!(reg.get_str(UNINSTALL_KEY, Some("DisplayName")).is_none());

        Reg { root: HKEY_CURRENT_USER, prefix: String::new() }.delete_tree(&base);
        assert!(Reg { root: HKEY_CURRENT_USER, prefix: String::new() }.get_str(&format!("{base}\\{classes}\\.md"), None).is_none());
    }
}
