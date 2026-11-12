// SPDX-License-Identifier: EUPL-1.2
//! `inshellah index`.

use std::{
    collections::HashSet,
    hash::RandomState,
    io,
    num::NonZero,
    path::{
        Path,
        PathBuf,
    },
    process,
    slice,
    thread::available_parallelism,
};

use inshellah::{
    config::DEFAULT_TIMEOUT_MS,
    indexer::{
        Scrape as ScrapeOpts,
        cmd_index,
        load_ignorelist,
    },
    shell::Shell,
    store::default_store_path,
};
use pound::Parse;

use super::common::{
    absolutize,
    reject_empty,
    validate_prefix,
};

#[derive(Parse, Debug)]
pub struct Args {
    /// directory holding bin/ and share/man/
    #[pound(value_name = "PREFIX")]
    pub prefixes:       Vec<PathBuf>,
    /// extra colon-separated prefixes
    #[pound(long = "prefix", value_name = "PATHS", delimiter = ':')]
    pub extra_prefixes: Vec<PathBuf>,
    /// output directory
    #[pound(long, default = { default_store_path().display() })]
    pub dir:            PathBuf,
    #[pound(flatten)]
    pub scrape:         Scrape,
}

#[derive(Parse, Debug)]
pub struct Scrape {
    /// file listing commands to skip entirely
    #[pound(long)]
    pub ignore:     Option<PathBuf>,
    /// file of commands, and their subcommands, to scrape with --help only
    #[pound(long)]
    pub help_only:  Option<PathBuf>,
    /// per-subprocess timeout in milliseconds
    #[pound(long, value_name = "MS", default = { DEFAULT_TIMEOUT_MS })]
    pub timeout_ms: u64,
    /// parallel scrape workers
    #[pound(long, min = 1, default = { available_parallelism().map_or(4, NonZero::get) })]
    pub workers:    usize,
    /// shells the index serves, nushell when none are given
    #[pound(long = "shell", value_name = "SHELL", delimiter = ',')]
    pub shells:     Vec<Shell>,
}

impl Scrape {
    pub fn shells(&self) -> &[Shell] {
        if self.shells.is_empty() {
            &[Shell::Nushell]
        } else {
            &self.shells
        }
    }

    /// # Errors
    /// fails when --ignore or --help-only cannot be read.
    pub fn with_opts<T, Run>(&self, run: Run) -> io::Result<T>
    where
        Run: FnOnce(&ScrapeOpts<'_, RandomState>) -> io::Result<T>,
    {
        let ignorelist = self.ignorelist()?;
        let help_only = self.help_only_list()?;
        run(&ScrapeOpts {
            ignorelist: &ignorelist,
            help_only:  &help_only,
            timeout_ms: self.timeout_ms,
            workers:    self.workers,
            shells:     self.shells(),
        })
    }

    pub fn ignorelist(&self) -> io::Result<HashSet<String>> {
        load_command_list(self.ignore.as_deref(), "--ignore")
    }

    pub fn help_only_list(&self) -> io::Result<HashSet<String>> {
        load_command_list(self.help_only.as_deref(), "--help-only")
    }
}

fn load_command_list(path: Option<&Path>, option: &str) -> io::Result<HashSet<String>> {
    path.map_or_else(
        || Ok(HashSet::new()),
        |list| {
            load_ignorelist(list).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("cannot read {option} file {}, {err}", list.display()),
                )
            })
        },
    )
}

#[expect(
    clippy::exit,
    reason = "a missing PREFIX is a usage error, reported without the `index failed` prefix"
)]
pub fn run(args: Args) -> io::Result<()> {
    let typed: Vec<PathBuf> = args
        .prefixes
        .into_iter()
        .chain(args.extra_prefixes)
        .collect();
    if typed.is_empty() {
        eprintln!("error: index requires at least one PREFIX");
        process::exit(1);
    }

    reject_empty(&typed, "PREFIX or --prefix")?;
    let prefixes = typed
        .iter()
        .map(|prefix| absolutize(prefix))
        .collect::<io::Result<Vec<_>>>()?;
    for prefix in &prefixes {
        validate_prefix(prefix)?;
    }
    reject_empty(slice::from_ref(&args.dir), "--dir")?;
    let bindirs: Vec<PathBuf> = prefixes.iter().map(|prefix| prefix.join("bin")).collect();
    let mandirs: Vec<PathBuf> = prefixes
        .iter()
        .map(|prefix| man_dir_of_prefix(prefix))
        .collect();
    args.scrape
        .with_opts(|opts| cmd_index(&bindirs, &mandirs, &args.dir, opts))
}

fn man_dir_of_prefix(prefix: &Path) -> PathBuf {
    prefix.join("share/man")
}
