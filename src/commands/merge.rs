// SPDX-License-Identifier: EUPL-1.2
//! `inshellah merge`.

use std::{
    fs,
    io,
    path::{
        Path,
        PathBuf,
    },
    slice,
};

use inshellah::indexer::cmd_merge;
use pound::Parse;

use super::{
    common::{
        absolutize,
        reject_empty,
    },
    index::Scrape,
};

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
    reject_empty(slice::from_ref(&args.dir), "--dir")?;
    let sources = read_sources(&args.sources)?;
    cmd_merge(
        &absolutize(&args.profile)?,
        &sources,
        &args.scrape.ignorelist()?,
        &args.scrape.help_only_list()?,
        &args.dir,
        args.scrape.timeout_ms,
        args.scrape.workers,
    )
}

fn read_sources(path: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    fs::read_to_string(path)
        .map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("cannot read --sources file {}, {err}", path.display()),
            )
        })?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (package, index) = line.split_once(' ').ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: expected `PACKAGE INDEX`, got {line:?}", path.display()),
                )
            })?;
            Ok((
                absolutize(Path::new(package))?,
                absolutize(Path::new(index.trim()))?,
            ))
        })
        .collect()
}
