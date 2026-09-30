// SPDX-License-Identifier: EUPL-1.2
//! `inshellah merge`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use inshellah::config::DEFAULT_TIMEOUT_MS;
use inshellah::indexer::{cmd_merge, load_ignorelist};

use super::index::default_workers;

pub struct Args {
    pub profile: PathBuf,
    pub dir: PathBuf,
    pub sources: PathBuf,
    pub ignore: Option<PathBuf>,
    pub help_only: Option<PathBuf>,
    pub timeout_ms: Option<String>,
    pub workers: Option<String>,
}

pub fn run(args: Args) -> io::Result<()> {
    let sources = read_sources(&args.sources)?;
    let load = |path: &Option<PathBuf>| path.as_deref().map(load_ignorelist).unwrap_or_default();
    cmd_merge(
        &args.profile,
        &sources,
        &load(&args.ignore),
        &load(&args.help_only),
        &args.dir,
        args.timeout_ms
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(DEFAULT_TIMEOUT_MS),
        args.workers
            .and_then(|n| n.parse::<usize>().ok())
            .map(|n| n.max(1))
            .unwrap_or_else(default_workers),
    )
}

fn read_sources(path: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (package, index) = line.split_once(' ').ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: expected `PACKAGE INDEX`, got {line:?}", path.display()),
                )
            })?;
            Ok((PathBuf::from(package), PathBuf::from(index.trim())))
        })
        .collect()
}
