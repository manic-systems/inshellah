// SPDX-License-Identifier: EUPL-1.2
//! Shared CLI command helpers.

use std::{
    io,
    path::{
        self,
        Path,
        PathBuf,
    },
};

use inshellah::{
    indexer::read_bin_dir,
    store::default_store_path,
};
use pound::Parse;

#[derive(Parse, Debug)]
pub struct Dirs {
    /// colon-separated completion dirs, the first one the writable cache
    #[pound(
        long = "dir",
        value_name = "PATHS",
        delimiter = ':',
        env = "INSHELLAH_DIR"
    )]
    paths: Vec<PathBuf>,
}

/// # Errors
/// fails when any entry of `paths` is empty, which would otherwise name the
/// working directory or shift the remaining entries out of their slots.
pub fn reject_empty(paths: &[PathBuf], source: &str) -> io::Result<()> {
    if paths.iter().any(|path| path.as_os_str().is_empty()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{source} contains an empty directory entry"),
        ));
    }
    Ok(())
}

/// children run in a fixed cwd, so every path they may see is made absolute
/// against ours first.
///
/// # Errors
/// fails when the working directory cannot be read to resolve `path`.
pub fn absolutize(path: &Path) -> io::Result<PathBuf> {
    path::absolute(path).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("cannot resolve {}, {err}", path.display()),
        )
    })
}

/// # Errors
/// fails when `prefix` is not a directory or its `bin` cannot be listed.
pub fn validate_prefix(prefix: &Path) -> io::Result<()> {
    if !prefix.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("prefix {} is not a directory", prefix.display()),
        ));
    }
    read_bin_dir(&prefix.join("bin")).map(drop)
}

impl Dirs {
    /// # Errors
    /// fails when any `--dir` or `INSHELLAH_DIR` entry is empty.
    pub fn resolve(self) -> io::Result<Vec<PathBuf>> {
        reject_empty(&self.paths, "--dir or INSHELLAH_DIR")?;
        Ok(if self.paths.is_empty() {
            vec![default_store_path()]
        } else {
            self.paths
        })
    }

    /// `resolve` for the completer, which drops empty entries since a failing
    /// completer leaves the shell with nothing to offer.
    pub fn resolve_lenient(mut self) -> Vec<PathBuf> {
        self.paths.retain(|path| !path.as_os_str().is_empty());
        if self.paths.is_empty() {
            vec![default_store_path()]
        } else {
            self.paths
        }
    }
}
