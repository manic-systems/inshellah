// SPDX-License-Identifier: EUPL-1.2
//! `inshellah index`.

use std::collections::HashSet;
use std::io;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::process;
use std::thread::available_parallelism;

use pound::Parse;

use inshellah::config::DEFAULT_TIMEOUT_MS;
use inshellah::indexer::{cmd_index, load_ignorelist};
use inshellah::store::default_store_path;

#[derive(Parse, Debug)]
pub struct Args {
    /// directory holding bin/ and share/man/
    #[pound(value_name = "PREFIX")]
    pub prefixes: Vec<PathBuf>,
    /// extra colon-separated prefixes
    #[pound(long = "prefix", value_name = "PATHS", delimiter = ':')]
    pub extra_prefixes: Vec<PathBuf>,
    /// output directory
    #[pound(long, default = { default_store_path().display() })]
    pub dir: PathBuf,
    #[pound(flatten)]
    pub scrape: Scrape,
}

#[derive(Parse, Debug)]
pub struct Scrape {
    /// file listing commands to skip entirely
    #[pound(long)]
    pub ignore: Option<PathBuf>,
    /// file listing commands to scrape with --help, skipping manpages
    #[pound(long)]
    pub help_only: Option<PathBuf>,
    /// per-subprocess timeout in milliseconds
    #[pound(long, value_name = "MS", default = { DEFAULT_TIMEOUT_MS })]
    pub timeout_ms: u64,
    /// parallel scrape workers
    #[pound(long, min = 1, default = { available_parallelism().map_or(4, NonZero::get) })]
    pub workers: usize,
}

impl Scrape {
    pub fn ignorelist(&self) -> HashSet<String> {
        self.ignore
            .as_deref()
            .map(load_ignorelist)
            .unwrap_or_default()
    }

    pub fn help_only_list(&self) -> HashSet<String> {
        self.help_only
            .as_deref()
            .map(load_ignorelist)
            .unwrap_or_default()
    }
}

#[expect(
    clippy::exit,
    reason = "a missing PREFIX is a usage error, reported without the `index failed` prefix"
)]
pub fn run(args: Args) -> io::Result<()> {
    let prefixes: Vec<PathBuf> = args
        .prefixes
        .into_iter()
        .chain(args.extra_prefixes)
        .collect();
    if prefixes.is_empty() {
        eprintln!("error: index requires at least one PREFIX");
        process::exit(1);
    }

    let bindirs: Vec<PathBuf> = prefixes.iter().map(|prefix| prefix.join("bin")).collect();
    let mandirs: Vec<PathBuf> = prefixes
        .iter()
        .map(|prefix| man_dir_of_prefix(prefix))
        .collect();
    cmd_index(
        &bindirs,
        &mandirs,
        &args.scrape.ignorelist(),
        &args.scrape.help_only_list(),
        &args.dir,
        args.scrape.timeout_ms,
        args.scrape.workers,
    )
}

fn man_dir_of_prefix(prefix: &Path) -> PathBuf {
    prefix.join("share/man")
}
