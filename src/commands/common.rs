// SPDX-License-Identifier: EUPL-1.2
//! Shared CLI command helpers.

use std::path::PathBuf;

use inshellah::store::default_store_path;
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

impl Dirs {
    pub fn resolve(self) -> Vec<PathBuf> {
        if self.paths.is_empty() {
            vec![default_store_path()]
        } else {
            self.paths
        }
    }
}
