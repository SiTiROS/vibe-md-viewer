//! Process hardening settings applied at the very start of main().
//!
//! * Dynamically loaded DLLs are searched only in System32 — protection against DLLs planted
//!   next to the opened .md or next to the exe (DLL planting / search-order hijacking).
//!   Imports that aren't KnownDLLs are delay-loaded, so they also go through this rule (build.rs).
//! * Loading DLLs from network resources (NoRemoteImages) and with a low integrity
//!   label is forbidden; system DLLs are preferred over same-named ones in the exe directory.
//! * The current directory is excluded from the DLL search.

use windows::core::PCWSTR;
use windows::Win32::System::LibraryLoader::{
    SetDefaultDllDirectories, SetDllDirectoryW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::System::SystemServices::PROCESS_MITIGATION_IMAGE_LOAD_POLICY;
use windows::Win32::System::Threading::{ProcessImageLoadPolicy, SetProcessMitigationPolicy};

pub fn harden_process() {
    unsafe {
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
        let empty: [u16; 1] = [0];
        let _ = SetDllDirectoryW(PCWSTR::from_raw(empty.as_ptr()));

        // NoRemoteImages(1) | NoLowMandatoryLabelImages(2) | PreferSystem32Images(4)
        let mut pol = PROCESS_MITIGATION_IMAGE_LOAD_POLICY::default();
        pol.Anonymous.Flags = 0b111;
        let _ = SetProcessMitigationPolicy(
            ProcessImageLoadPolicy,
            &pol as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<PROCESS_MITIGATION_IMAGE_LOAD_POLICY>(),
        );
    }
}
