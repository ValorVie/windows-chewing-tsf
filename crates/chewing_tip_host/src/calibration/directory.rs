// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::{OsString, c_void},
    os::windows::ffi::OsStringExt,
    path::PathBuf,
};
use windows::Win32::{
    System::Com::CoTaskMemFree,
    UI::Shell::{FOLDERID_Profile, FOLDERID_RoamingAppData},
};
use windows_core::{GUID, HRESULT, PWSTR};

#[link(name = "shell32")]
unsafe extern "system" {
    #[link_name = "SHGetKnownFolderPath"]
    fn known_folder_path(
        id: *const GUID,
        flags: u32,
        token: *mut c_void,
        path: *mut *mut u16,
    ) -> HRESULT;
}

#[derive(Clone, Copy)]
pub(super) enum KnownFolder {
    Profile,
    Roaming,
}

pub(super) fn resolve() -> Result<PathBuf, ()> {
    resolve_with(
        std::env::var("CHEWING_USER_PATH").ok().map(PathBuf::from),
        query,
    )
}
pub(super) fn query(folder: KnownFolder) -> Result<PathBuf, ()> {
    let id = match folder {
        KnownFolder::Profile => FOLDERID_Profile,
        KnownFolder::Roaming => FOLDERID_RoamingAppData,
    };
    let mut allocated = std::ptr::null_mut();
    // Retain the raw out pointer so it is freed even when the API returns an
    // error, as required by SHGetKnownFolderPath's allocation contract.
    let status = unsafe { known_folder_path(&id, 0, std::ptr::null_mut(), &mut allocated) };
    let path = if status.is_ok() && !allocated.is_null() {
        let raw = PWSTR(allocated);
        Some(PathBuf::from(OsString::from_wide(unsafe { raw.as_wide() })))
    } else {
        None
    };
    unsafe { CoTaskMemFree(Some(allocated.cast())) };
    path.ok_or(())
}
pub(super) fn resolve_with(
    override_path: Option<PathBuf>,
    mut known: impl FnMut(KnownFolder) -> Result<PathBuf, ()>,
) -> Result<PathBuf, ()> {
    if let Some(path) = override_path {
        return Ok(path);
    }
    if let Ok(profile) = known(KnownFolder::Profile) {
        let legacy = profile.join("ChewingTextService");
        if legacy.is_dir() {
            return Ok(legacy);
        }
    }
    known(KnownFolder::Roaming).map(|path| path.join("Chewing"))
}
