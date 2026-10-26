// SPDX-License-Identifier: EUPL-1.2
//! Shared CLI command helpers.

use std::env;
use std::path::{Path, PathBuf};

use pound::Parse;

use inshellah::indexer::probe::is_executable;
use inshellah::store::default_store_path;

pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var("PATH").ok()?;
    for dir in path_var.split(':') {
        let candidate = Path::new(dir).join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

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

impl Dirs {
    pub fn resolve(self) -> Vec<PathBuf> {
        if self.paths.is_empty() {
            vec![default_store_path()]
        } else {
            self.paths
        }
    }
}
