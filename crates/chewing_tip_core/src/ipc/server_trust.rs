// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::OsString,
    fs,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf, Prefix},
};

use scoped_error::expect_error;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HMODULE},
        Globalization::{CSTR_EQUAL, CompareStringOrdinal},
        System::{
            LibraryLoader::{
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
                GetModuleHandleExW,
            },
            Registry::KEY_WOW64_64KEY,
            Threading::{
                OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};
use windows_registry::LOCAL_MACHINE;

use super::{IpcError, named_pipe::verify_trust};

pub(super) fn attest_server(pid: u32) -> Result<(), IpcError> {
    expect_error("Failed to attest scoring host", || {
        let executable = process_image_path(pid)?;
        if verify_trust(&executable) {
            return Ok(());
        }
        let module = own_module_path()?;
        let program_files = protected_program_files()?;
        if !unsigned_host_is_trusted(&executable, &module, &program_files) {
            return Err("unsigned host does not match the protected TIP installation".into());
        }
        Ok(())
    })
}

fn process_image_path(pid: u32) -> Result<PathBuf, IpcError> {
    expect_error("Failed to query server image", || {
        let mut buffer = vec![0u16; 32768];
        let mut size = buffer.len() as u32;
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)? };
        let queried = unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_FORMAT(0),
                PWSTR(buffer.as_mut_ptr()),
                &mut size,
            )
        };
        let closed = unsafe { CloseHandle(handle) };
        queried?;
        closed?;
        Ok(PathBuf::from(OsString::from_wide(
            buffer
                .get(..size as usize)
                .ok_or("invalid server image length")?,
        )))
    })
}

fn own_module_path() -> Result<PathBuf, IpcError> {
    expect_error(
        "Failed to locate the module containing the TIP client",
        || {
            let mut module = HMODULE::default();
            unsafe {
                GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                        | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                    PCWSTR(own_module_path as *const () as *const u16),
                    &mut module,
                )?;
            }
            let mut buffer = vec![0u16; 32768];
            let length = unsafe { GetModuleFileNameW(Some(module), &mut buffer) } as usize;
            if length == 0 || length >= buffer.len() {
                return Err("module path is missing or truncated".into());
            }
            Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
        },
    )
}

fn protected_program_files() -> Result<PathBuf, IpcError> {
    expect_error("Failed to locate protected Program Files", || {
        // Read the administrator-owned 64-bit registry view, including in an x86 TIP.
        // Process environment variables must not become trust anchors.
        let key = LOCAL_MACHINE
            .options()
            .read()
            .access(KEY_WOW64_64KEY.0)
            .open(r"SOFTWARE\Microsoft\Windows\CurrentVersion")?;
        Ok(PathBuf::from(key.get_string("ProgramFilesDir")?))
    })
}

fn unsigned_host_is_trusted(server: &Path, module: &Path, program_files: &Path) -> bool {
    let checked = (|| -> Option<bool> {
        if !server.is_absolute() || !module.is_absolute() || !program_files.is_absolute() {
            return None;
        }
        // Resolve actual files, including any junctions, before comparing paths.
        let module = fs::canonicalize(module).ok()?;
        if !module.extension()?.to_str()?.eq_ignore_ascii_case("dll") {
            return None;
        }
        let root = normalized_local_path(&fs::canonicalize(program_files).ok()?)?;
        let expected = fs::canonicalize(module.parent()?.join("chewing_tip_host.exe")).ok()?;
        let server = normalized_local_path(&fs::canonicalize(server).ok()?)?;
        let module = normalized_local_path(&module)?;
        let expected = normalized_local_path(&expected)?;
        Some(
            is_below(&module, &root)
                && is_below(&expected, &root)
                && is_below(&server, &root)
                && ordinal_equal(&server, &expected),
        )
    })();
    checked.unwrap_or(false)
}

fn normalized_local_path(path: &Path) -> Option<Vec<u16>> {
    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else {
        return None;
    };
    let skip = match prefix.kind() {
        Prefix::Disk(_) => 0,
        Prefix::VerbatimDisk(_) => 4,
        _ => return None,
    };
    if components.next()? != Component::RootDir {
        return None;
    }
    let mut wide = path
        .as_os_str()
        .encode_wide()
        .skip(skip)
        .collect::<Vec<_>>();
    while wide.last() == Some(&(b'\\' as u16)) {
        wide.pop();
    }
    Some(wide)
}

fn ordinal_equal(left: &[u16], right: &[u16]) -> bool {
    unsafe { CompareStringOrdinal(left, right, true) == CSTR_EQUAL }
}

fn is_below(path: &[u16], root: &[u16]) -> bool {
    path.len() > root.len() + 1
        && path[root.len()] == b'\\' as u16
        && ordinal_equal(&path[..root.len()], root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use interprocess::os::windows::named_pipe::{PipeListenerOptions, pipe_mode::Bytes};
    use std::{
        fs,
        path::PathBuf,
        sync::mpsc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    struct Files {
        base: PathBuf,
        program_files: PathBuf,
        module: PathBuf,
        host: PathBuf,
    }

    impl Files {
        fn new() -> Self {
            let base = std::env::temp_dir().join(format!(
                "c2-auth-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let program_files = base.join("Program Files");
            let install = program_files.join("ChewingTextService");
            fs::create_dir_all(&install).unwrap();
            let module = install.join("chewing_tip_x64.dll");
            let host = install.join("chewing_tip_host.exe");
            fs::write(&module, b"synthetic DLL path fixture").unwrap();
            fs::write(&host, b"synthetic host path fixture").unwrap();
            Self {
                base,
                program_files,
                module,
                host,
            }
        }

        fn file(&self, relative: &str) -> PathBuf {
            let path = self.base.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"synthetic path fixture").unwrap();
            path
        }
    }

    impl Drop for Files {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.base).unwrap();
        }
    }

    #[test]
    fn exact_unsigned_host_beside_tip_in_program_files_is_accepted() {
        let f = Files::new();
        assert!(unsigned_host_is_trusted(
            &f.host,
            &f.module,
            &f.program_files
        ));
    }

    #[test]
    fn case_and_verbatim_prefix_preserve_equivalent_paths() {
        let f = Files::new();
        let host = PathBuf::from(format!(
            r"\\?\{}",
            f.host.to_str().unwrap().to_ascii_uppercase()
        ));
        let module = PathBuf::from(f.module.to_str().unwrap().to_ascii_lowercase());
        assert!(unsigned_host_is_trusted(&host, &module, &f.program_files));
        assert!(unsigned_host_is_trusted(
            &f.host,
            &f.module,
            &f.program_files
        ));
    }

    #[test]
    fn different_host_path_is_rejected() {
        let f = Files::new();
        for path in [
            "Program Files/Other/chewing_tip_host.exe",
            "Program Files/ChewingTextService/other.exe",
        ] {
            assert!(!unsigned_host_is_trusted(
                &f.file(path),
                &f.module,
                &f.program_files
            ));
        }
    }

    #[test]
    fn case_and_verbatim_prefix_do_not_hide_a_different_host() {
        let f = Files::new();
        let wrong = f.file("Program Files/Other/chewing_tip_host.exe");
        let wrong = PathBuf::from(format!(
            r"\\?\{}",
            wrong.to_str().unwrap().to_ascii_uppercase()
        ));
        assert!(!unsigned_host_is_trusted(
            &wrong,
            &f.module,
            &f.program_files
        ));
    }

    #[test]
    fn matching_pair_outside_program_files_is_rejected() {
        let f = Files::new();
        for folder in ["Writable", "Program Files-user"] {
            let module = f.file(&format!("{folder}/chewing_tip_x64.dll"));
            let host = f.file(&format!("{folder}/chewing_tip_host.exe"));
            assert!(!unsigned_host_is_trusted(&host, &module, &f.program_files));
        }
    }

    #[test]
    fn parent_traversal_does_not_bypass_program_files() {
        let f = Files::new();
        let host = f.file("Writable/chewing_tip_host.exe");
        let traversed = f
            .program_files
            .join("..")
            .join("Writable")
            .join(host.file_name().unwrap());
        assert!(!unsigned_host_is_trusted(
            &traversed,
            &f.module,
            &f.program_files
        ));
    }

    #[test]
    fn application_exe_cannot_supply_the_tip_module_directory() {
        let f = Files::new();
        let exe = f.file("Program Files/ChewingTextService/application.exe");
        assert!(!unsigned_host_is_trusted(&f.host, &exe, &f.program_files));
    }

    #[test]
    fn missing_or_relative_trust_root_is_rejected() {
        let f = Files::new();
        for root in [
            Path::new(""),
            Path::new("Program Files"),
            f.base.join("missing").as_path(),
        ] {
            assert!(!unsigned_host_is_trusted(&f.host, &f.module, root));
        }
    }

    #[test]
    fn debug_build_rejects_a_real_unsigned_server_in_scratch() {
        let name = format!(
            r"\\.\pipe\c2-auth-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let listener = PipeListenerOptions::new()
            .path(name.as_str())
            .create_duplex::<Bytes>()
            .unwrap();
        let (release, wait) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let _connection = listener.accept().unwrap();
            let _ = wait.recv_timeout(Duration::from_secs(5));
        });
        let result = crate::ipc::named_pipe::connect_and_attest(&name, Duration::from_millis(100));
        let _ = release.send(());
        server.join().unwrap();
        assert!(
            result.is_err(),
            "debug must not bypass host identity verification"
        );
    }
}
