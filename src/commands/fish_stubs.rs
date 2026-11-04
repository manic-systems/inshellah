// SPDX-License-Identifier: EUPL-1.2
//! `inshellah fish-stubs`.

use std::{
    env,
    fs,
    io::{
        self,
        Write as _,
    },
    path::Path,
};

use inshellah::{
    executable::is_executable,
    store::write_file,
};

use super::common::Dirs;

/// bumped whenever `stub_for` changes, so stubs written by an older inshellah
/// are replaced rather than kept because they exist.
const STUB_VERSION: &str = "2";
const VERSION_FILE: &str = ".version";

/// fish has no catch-all completer, so every command on PATH gets a stub that
/// fish autoloads by name. prints the stub dir. stubs are best effort, a full
/// disk or another shell racing the same upgrade still leaves the ones that
/// exist usable.
///
/// # Errors
/// fails when the stub dir cannot be created.
pub fn run(dirs: Dirs) -> io::Result<()> {
    let dir = dirs.resolve_lenient().swap_remove(0).join("fish");
    fs::create_dir_all(&dir)?;
    let version_file = dir.join(VERSION_FILE);
    if fs::read_to_string(&version_file).ok().as_deref() != Some(STUB_VERSION) {
        remove_stubs(&dir);
        drop(write_file(&version_file, STUB_VERSION));
    }
    let path = env::var_os("PATH").unwrap_or_default();
    for bin_dir in env::split_paths(&path).filter(|bin_dir| bin_dir.is_absolute()) {
        let Ok(entries) = fs::read_dir(&bin_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str().filter(|name| stubbable(name)) else {
                continue;
            };
            let stub = dir.join(format!("{name}.fish"));
            if stub.exists() || !is_executable(&entry.path()) {
                continue;
            }
            drop(write_file(&stub, &stub_for(name)));
        }
    }
    writeln!(io::stdout().lock(), "{}", dir.display())
}

fn remove_stubs(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "fish") {
            drop(fs::remove_file(path));
        }
    }
}

fn stubbable(name: &str) -> bool {
    !name.is_empty() && !name.starts_with(['.', '-']) && !name.contains(char::is_control)
}

fn stub_for(name: &str) -> String {
    let quoted = format!("'{}'", name.replace('\\', r"\\").replace('\'', r"\'"));
    format!("__inshellah_load {quoted}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_quotes_the_command_name() {
        assert_eq!(stub_for("it's"), "__inshellah_load 'it\\'s'\n");
        assert!(!stubbable("-x"));
        assert!(!stubbable(".hidden"));
        assert!(stubbable("git"));
    }
}
