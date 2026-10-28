// SPDX-License-Identifier: EUPL-1.2
//! executable-file checks and command lookup.

use std::{
    env,
    ffi::{
        CString,
        OsStr,
    },
    fs,
    os::unix::{
        ffi::OsStrExt as _,
        fs::PermissionsExt as _,
    },
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
    // access(X_OK) passes any file for root on macOS and some BSDs.
    if !fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    {
        return false;
    }
    // SAFETY: c_path is a valid NUL-terminated string that outlives the call.
    unsafe { libc::access(c_path.as_ptr(), libc::X_OK) == 0 }
}

fn find_in_path_value(name: &OsStr, path: Option<&OsStr>) -> Option<PathBuf> {
    env::split_paths(path?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// resolve a bare command name through PATH, skipping empty and relative
/// entries.
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    find_in_path_value(name.as_ref(), env::var_os("PATH").as_deref())
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::fs::symlink,
        process,
        sync::atomic::{
            AtomicU64,
            Ordering,
        },
    };

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let dir = env::temp_dir().join(format!(
                "inshellah-{name}-{}-{}",
                process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    fn write_file(path: &Path, mode: u32) {
        fs::write(path, "#!/bin/sh\n").expect("write file");
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set permissions");
    }

    #[test]
    fn executable_requires_a_regular_file_and_execute_bit() {
        let guard = TempDir::new("exe-check");
        let root = &guard.0;
        let file = root.join("tool");
        write_file(&file, 0o644);
        assert!(!is_executable(&file));

        write_file(&file, 0o700);
        assert!(is_executable(&file));
        assert!(!is_executable(root));
        assert!(!is_executable(&root.join("missing")));

        let link = root.join("link");
        symlink(&file, &link).expect("create symlink");
        assert!(is_executable(&link));
    }

    #[test]
    fn path_lookup_uses_the_first_executable_match() {
        let guard = TempDir::new("path-order");
        let first = guard.0.join("first");
        let second = guard.0.join("second");
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
    }

    #[test]
    fn path_lookup_skips_empty_and_relative_entries() {
        let guard = TempDir::new("path-relative");
        let dir = guard.0.join("bin");
        fs::create_dir_all(&dir).expect("create bin");
        write_file(&dir.join("tool"), 0o755);
        let depth = env::current_dir().expect("cwd").components().count() - 1;
        let relative =
            PathBuf::from("../".repeat(depth)).join(dir.strip_prefix("/").expect("absolute"));
        assert!(is_executable(&relative.join("tool")));
        let path = env::join_paths([Path::new(""), &relative]).expect("join paths");

        assert!(find_in_path_value(OsStr::new("tool"), Some(&path)).is_none());
    }

    #[test]
    fn path_lookup_skips_files_the_user_cannot_execute() {
        // SAFETY: geteuid has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let guard = TempDir::new("path-eacces");
        let first = guard.0.join("first");
        let second = guard.0.join("second");
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
    }
}
