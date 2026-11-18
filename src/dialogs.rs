//! System dialogs for opening/saving a file and choosing a folder (IFileDialog).

use std::path::{Path, PathBuf};

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;

use crate::util::wide;

pub fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }
}

fn result_path(item: &IShellItem) -> Option<PathBuf> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
        s.map(PathBuf::from)
    }
}

fn set_folder(dlg: &IFileDialog, dir: Option<&Path>) {
    let Some(dir) = dir else { return };
    let w = crate::util::wide_path(dir);
    unsafe {
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR::from_raw(w.as_ptr()), None) {
            let _ = dlg.SetFolder(&item);
        }
    }
}

pub fn open_markdown(owner: HWND, dir: Option<&Path>) -> Option<PathBuf> {
    com_init();
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let n1 = wide("Markdown");
        let s1 = wide("*.md;*.markdown;*.mdown;*.mkd;*.mkdn;*.mdwn;*.txt");
        let n2 = wide(tr!("All files", "Все файлы"));
        let s2 = wide("*.*");
        let specs = [
            COMDLG_FILTERSPEC { pszName: PCWSTR::from_raw(n1.as_ptr()), pszSpec: PCWSTR::from_raw(s1.as_ptr()) },
            COMDLG_FILTERSPEC { pszName: PCWSTR::from_raw(n2.as_ptr()), pszSpec: PCWSTR::from_raw(s2.as_ptr()) },
        ];
        let _ = dlg.SetFileTypes(&specs);
        let _ = dlg.SetOptions(FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST);
        set_folder(&dlg.cast().ok()?, dir);
        dlg.Show(Some(owner)).ok()?;
        result_path(&dlg.GetResult().ok()?)
    }
}

pub fn open_image(owner: HWND, dir: Option<&Path>) -> Option<PathBuf> {
    com_init();
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let n1 = wide(tr!("Images", "Картинки"));
        let s1 = wide("*.png;*.jpg;*.jpeg;*.gif;*.bmp;*.webp");
        let specs = [COMDLG_FILTERSPEC { pszName: PCWSTR::from_raw(n1.as_ptr()), pszSpec: PCWSTR::from_raw(s1.as_ptr()) }];
        let _ = dlg.SetFileTypes(&specs);
        let _ = dlg.SetOptions(FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM);
        set_folder(&dlg.cast().ok()?, dir);
        dlg.Show(Some(owner)).ok()?;
        result_path(&dlg.GetResult().ok()?)
    }
}

pub fn save_markdown(owner: HWND, dir: Option<&Path>, name: &str) -> Option<PathBuf> {
    com_init();
    unsafe {
        let dlg: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let n1 = wide("Markdown");
        let s1 = wide("*.md");
        let specs = [COMDLG_FILTERSPEC { pszName: PCWSTR::from_raw(n1.as_ptr()), pszSpec: PCWSTR::from_raw(s1.as_ptr()) }];
        let _ = dlg.SetFileTypes(&specs);
        let ext = wide("md");
        let _ = dlg.SetDefaultExtension(PCWSTR::from_raw(ext.as_ptr()));
        let wn = wide(name);
        let _ = dlg.SetFileName(PCWSTR::from_raw(wn.as_ptr()));
        let _ = dlg.SetOptions(FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST);
        set_folder(&dlg.cast().ok()?, dir);
        dlg.Show(Some(owner)).ok()?;
        result_path(&dlg.GetResult().ok()?)
    }
}
