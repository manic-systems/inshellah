// SPDX-License-Identifier: EUPL-1.2
//! executable-file checks and command lookup.

use std::{
    env,
    ffi::{
        CString,
        OsStr,
    },
    fs,
    os::unix::ffi::OsStrExt as _,
    path::{
        Path,
        PathBuf,
    },
};

/// whether `path` is a regular file the current user may execute.
pub fn is_executable(path: &Path) -> bool {
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: c_path is a valid NUL-terminated string that outlives the call.
    fs::metadata(path).is_ok_and(|meta| meta.is_file())
        && unsafe { libc::access(c_path.as_ptr(), libc::X_OK) } == 0
}

fn find_in_path_value(name: &OsStr, path: Option<&OsStr>) -> Option<PathBuf> {
    env::split_paths(path?)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// resolve a bare command name through PATH.
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    find_in_path_value(name.as_ref(), env::var_os("PATH").as_deref())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{
            PermissionsExt as _,
            symlink,
        },
        process,
    };

    use super::*;

    fn write_file(path: &Path, mode: u32) {
        fs::write(path, "#!/bin/sh\n").expect("write file");
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set permissions");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("inshellah-{name}-{}", process::id()));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn executable_requires_a_regular_file_and_execute_bit() {
        let root = temp_dir("exe-check");
        let file = root.join("tool");
        write_file(&file, 0o644);
        assert!(!is_executable(&file));

        write_file(&file, 0o700);
        assert!(is_executable(&file));
        assert!(!is_executable(&root));
        assert!(!is_executable(&root.join("missing")));

        let link = root.join("link");
        symlink(&file, &link).expect("create symlink");
        assert!(is_executable(&link));
        drop(fs::remove_dir_all(&root));
    }

    #[test]
    fn path_lookup_uses_the_first_executable_match() {
        let root = temp_dir("path-order");
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).expect("create first");
        fs::create_dir_all(&second).expect("create second");
        write_file(&first.join("tool"), 0o644);
        write_file(&second.join("tool"), 0o755);
        let path = env::join_paths([&first, &second]).expect("join paths");

        assert_eq!(
            find_in_path_value(OsStr::new("tool"), Some(&path)),
            Some(second.join("tool"))
        );
        assert!(find_in_path_value(OsStr::new("tool"), None).is_none());
        drop(fs::remove_dir_all(&root));
    }

    #[test]
    fn path_lookup_skips_files_the_user_cannot_execute() {
        // SAFETY: geteuid has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let root = temp_dir("path-eacces");
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).expect("create first");
        fs::create_dir_all(&second).expect("create second");
        write_file(&first.join("tool"), 0o001);
        write_file(&second.join("tool"), 0o755);
        assert!(!is_executable(&first.join("tool")));
        let path = env::join_paths([&first, &second]).expect("join paths");

        assert_eq!(
            find_in_path_value(OsStr::new("tool"), Some(&path)),
            Some(second.join("tool"))
        );
        drop(fs::remove_dir_all(&root));
    }
}
