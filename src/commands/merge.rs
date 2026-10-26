// SPDX-License-Identifier: EUPL-1.2
//! `inshellah merge`.

use std::{
    fs,
    io,
    path::{
        Path,
        PathBuf,
    },
};

use inshellah::indexer::cmd_merge;
use pound::Parse;

use super::index::Scrape;

#[derive(Parse, Debug)]
pub struct Args {
    /// profile whose bin/ links into the package indexes
    #[pound(value_name = "PROFILE")]
    pub profile: PathBuf,
    /// output directory
    #[pound(long)]
    pub dir:     PathBuf,
    /// file of `PACKAGE INDEX` lines, in priority order
    #[pound(long)]
    pub sources: PathBuf,
    #[pound(flatten)]
    pub scrape:  Scrape,
}

pub fn run(args: &Args) -> io::Result<()> {
    let sources = read_sources(&args.sources)?;
    cmd_merge(
        &args.profile,
        &sources,
        &args.scrape.ignorelist(),
        &args.scrape.help_only_list(),
        &args.dir,
        args.scrape.timeout_ms,
        args.scrape.workers,
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
