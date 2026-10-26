// SPDX-License-Identifier: EUPL-1.2
//! inshellah CLI.

mod commands;

use std::{
    path::PathBuf,
    process,
};

use commands::common::Dirs;
use inshellah::config::Config;
use pound::Parse;

/// nushell completions engine
#[derive(Parse, Debug)]
#[pound(
    name = "inshellah",
    help_if_empty,
    after_help = "Environment, read by `complete`:
  INSHELLAH_DIR             default for --dir
  INSHELLAH_FLAG_TRIGGERS   chars that surface flags, \"-\" by default
  INSHELLAH_FLAG_ON_EMPTY   1 to also surface flags on an empty token
  INSHELLAH_MAX_COMPLETIONS cap on candidates returned, 0 for none
  INSHELLAH_TIMEOUT_MS      default --help resolve timeout
  INSHELLAH_CACHE_TTL_SECS  rescrape user-cached sets older than this, 604800 by default, 0 for \
                  never

Read by `index`:
  INSHELLAH_MAX_INDEX_NODES cap on subcommand nodes per root command, 10000 by default"
)]
enum Cli {
    /// index completions into a directory of JSON/nu files
    Index {
        #[pound(flatten)]
        args: commands::index::Args,
    },
    /// merge per-package indexes for a profile whose bin/ links into them
    ///
    /// manpages under PROFILE/share/man are then applied profile-wide.
    Merge {
        #[pound(flatten)]
        args: commands::merge::Args,
    },
    /// parse a manpage and emit nushell extern
    Manpage { file: PathBuf },
    /// batch-process manpages under a directory
    ManpageDir { dir: PathBuf },
    /// nushell external completer, printing JSON candidates
    ///
    /// a command missing from the index is resolved through its --help and
    /// cached in the first --dir.
    Complete {
        #[pound(flatten)]
        dirs:       Dirs,
        /// --help resolve timeout in milliseconds
        #[pound(long, value_name = "MS")]
        timeout_ms: Option<u64>,
        /// the command line being completed
        #[pound(trailing, value_name = "SPAN")]
        spans:      Vec<String>,
    },
    /// print stored completion data
    Query {
        #[pound(flatten)]
        dirs: Dirs,
        #[pound(value_name = "CMD", min_values = 1)]
        cmd:  Vec<String>,
    },
    /// list indexed commands
    Dump {
        #[pound(flatten)]
        dirs: Dirs,
    },
    /// report flag and subcommand gaps between a manpage and --help
    #[pound(required_group = target)]
    Diff {
        /// audit every command under a prefix
        #[pound(long, group = target)]
        scan:       Option<PathBuf>,
        /// extra colon-separated man directories
        #[pound(long, value_name = "PATHS", delimiter = ':')]
        dir:        Vec<PathBuf>,
        /// --help scrape timeout in milliseconds
        #[pound(long, value_name = "MS")]
        timeout_ms: Option<u64>,
        #[pound(value_name = "CMD", group = target)]
        cmd:        Vec<String>,
    },
    /// delete the writable user cache, or CMD's entries in it
    Purge {
        #[pound(flatten)]
        dirs: Dirs,
        #[pound(value_name = "CMD")]
        cmd:  Vec<String>,
    },
    /// re-resolve CMD into the user cache from the binary on PATH
    Refresh {
        #[pound(flatten)]
        dirs:       Dirs,
        /// --help resolve timeout in milliseconds
        #[pound(long, value_name = "MS")]
        timeout_ms: Option<u64>,
        #[pound(value_name = "CMD", min_values = 1)]
        cmd:        Vec<String>,
    },
    /// generate nushell completions for inshellah
    Completions,
}

fn main() {
    // rust ignores SIGPIPE, so a broken-pipe write becomes a BrokenPipe error
    // that `println!` panics on. restore the default so piping into `head`
    // exits quietly.
    // SAFETY: runs first in main, before any thread exists, and SIG_DFL is a
    // valid handler.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    match Cli::parse() {
        Cli::Index { args } => {
            if let Err(err) = commands::index::run(args) {
                eprintln!("index failed: {err}");
                process::exit(1);
            }
        },
        Cli::Merge { args } => {
            if let Err(err) = commands::merge::run(&args) {
                eprintln!("merge failed: {err}");
                process::exit(1);
            }
        },
        Cli::Manpage { file } => commands::manpage::run(&file),
        Cli::ManpageDir { dir } => commands::manpage_dir::run(&dir),
        Cli::Complete {
            dirs,
            timeout_ms: timeout_override,
            spans,
        } => {
            let cfg = Config::from_env();
            let timeout_ms = timeout_override.unwrap_or(cfg.timeout_ms);
            let (user_dir, system_dirs) =
                commands::complete::default_user_dir_and_system_dirs(dirs.resolve());
            let mandirs = commands::complete::mandirs_for_system_dirs(&system_dirs);
            if let Err(err) =
                commands::complete::run(&spans, &user_dir, &system_dirs, &mandirs, timeout_ms, &cfg)
            {
                eprintln!("complete failed: {err}");
                process::exit(1);
            }
        },
        Cli::Query { dirs, cmd } => {
            if let Err(err) = commands::query::run(&cmd.join(" "), &dirs.resolve()) {
                eprintln!("{err}");
                process::exit(1);
            }
        },
        Cli::Dump { dirs } => commands::dump::run(&dirs.resolve()),
        Cli::Diff {
            scan,
            dir,
            timeout_ms: timeout_override,
            cmd,
        } => {
            let timeout_ms = timeout_override.unwrap_or_else(|| Config::from_env().timeout_ms);
            match scan {
                Some(prefix) => commands::diff::run_scan(&prefix, timeout_ms),
                None => {
                    if let Err(err) = commands::diff::run(&cmd, &dir, timeout_ms) {
                        eprintln!("error: {err}");
                        process::exit(1);
                    }
                },
            }
        },
        Cli::Purge { dirs, cmd } => {
            // only the writable user dir is purged, never the system overlays
            let mut resolved = dirs.resolve();
            if let Err(err) = commands::cache::purge(&resolved.swap_remove(0), &cmd) {
                eprintln!("purge failed: {err}");
                process::exit(1);
            }
        },
        Cli::Refresh {
            dirs,
            timeout_ms: timeout_override,
            cmd,
        } => {
            let timeout_ms = timeout_override.unwrap_or_else(|| Config::from_env().timeout_ms);
            let (user_dir, system_dirs) =
                commands::complete::default_user_dir_and_system_dirs(dirs.resolve());
            let mandirs = commands::complete::mandirs_for_system_dirs(&system_dirs);
            if let Err(err) = commands::cache::refresh(&user_dir, &mandirs, &cmd, timeout_ms) {
                eprintln!("refresh failed: {err}");
                process::exit(1);
            }
        },
        Cli::Completions => {
            if let Err(err) = commands::completions::run(Cli::SPEC) {
                eprintln!("completions failed: {err}");
                process::exit(1);
            }
        },
    }
}
