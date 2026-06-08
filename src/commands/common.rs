// SPDX-License-Identifier: EUPL-1.2
//! Shared CLI command helpers.

use std::path::{Path, PathBuf};

use inshellah::indexer::is_executable;
use inshellah::store::default_store_path;

pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in path_var.split(':') {
        let candidate = Path::new(dir).join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

pub fn split_colon_paths<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<PathBuf> {
    values
        .into_iter()
        .flat_map(|value| value.split(':'))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

pub fn completion_dirs(dir: Option<&str>) -> Vec<PathBuf> {
    dir.map(|d| d.split(':').map(PathBuf::from).collect())
        .unwrap_or_else(|| vec![default_store_path()])
}

pub fn parse_timeout_ms(timeout_ms: Option<&str>) -> Option<u64> {
    timeout_ms.and_then(|n| n.parse::<u64>().ok())
}
