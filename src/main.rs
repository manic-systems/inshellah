// SPDX-License-Identifier: EUPL-1.2
//! inshellah CLI.

mod commands;

use std::path::PathBuf;

use pound::Parse;

use inshellah::config::Config;
use inshellah::store::default_store_path;

use commands::common::{completion_dirs, parse_timeout_ms};

fn usage() {
    eprintln!(
        "inshellah - nushell completions engine

Usage:
  inshellah index PREFIX... [--dir PATH] [--ignore FILE] [--help-only FILE]
                            [--prefix PATH[:PATH...]] [--timeout-ms N] [--workers N]
      Index completions into a directory of JSON/nu files.
      PREFIX is a directory containing bin/ and share/man/.
      Default dir: $XDG_CACHE_HOME/inshellah
      --ignore FILE     skip listed commands entirely
      --help-only FILE  skip manpages for listed commands, use --help instead
      --prefix PATHS    extra scrape prefixes, colon-separated (in addition
                        to the positional PREFIX args)
      --timeout-ms N    per-subprocess timeout in milliseconds (default 1200)
      --workers N       parallel scrape workers (default: cpu count)
      (env INSHELLAH_MAX_INDEX_NODES caps subcommand nodes per root command;
       default 10000, bounds runaway recursion on pathological trees)
  inshellah complete CMD [ARGS...] [--dir PATH[:PATH...]] [--timeout-ms N]
      Nushell custom completer. Outputs JSON completion candidates.
      Falls back to --help resolution if command is not indexed.
      --dir takes colon-separated paths. The first path is the writable
      user cache; additional paths are read-only system directories.
  inshellah query CMD [--dir PATH[:PATH...]]
      Print stored completion data for CMD.
  inshellah dump [--dir PATH[:PATH...]]
      List indexed commands.
  inshellah diff CMD [SUB...] [--dir EXTRA_MANDIR] [--timeout-ms N]
      Audit source divergence: parse CMD's manpage and --help separately
      and report subcommand/flag gaps between them (dev tool).
  inshellah purge [--dir PATH[:PATH...]]
      Delete the on-the-fly user cache (.json/.nu files). Only the first
      --dir (the writable user cache) is cleared; system dirs are untouched.
      Default dir: $XDG_CACHE_HOME/inshellah
  inshellah manpage FILE            Parse a manpage and emit nushell extern
  inshellah manpage-dir DIR         Batch-process manpages under DIR
  inshellah completions             Generate nushell completions for inshellah

Configuration (environment, read by `complete`):
  INSHELLAH_FLAG_TRIGGERS   chars that surface flags (default \"-\"; e.g. \"-+\")
  INSHELLAH_FLAG_ON_EMPTY   1 to also surface flags on an empty token
  INSHELLAH_MAX_COMPLETIONS cap on candidates returned (0 = no cap)
  INSHELLAH_TIMEOUT_MS      default --help resolve timeout (--timeout-ms wins)
  INSHELLAH_CACHE_TTL_SECS  rescrape user-cached sets older than N seconds (default 604800; 0 = never)
"
    );
}

#[derive(Parse, Debug)]
#[pound(name = "inshellah")]
enum Cli {
    /// index completions into a directory of JSON/nu files
    Index {
        #[pound(positional, value_name = "PREFIX")]
        prefixes: Vec<PathBuf>,
        #[pound(long)]
        dir: Option<PathBuf>,
        #[pound(long)]
        ignore: Option<PathBuf>,
        #[pound(long)]
        help_only: Option<PathBuf>,
        #[pound(long = "prefix", value_name = "PATHS")]
        extra_prefixes: Vec<String>,
        #[pound(long)]
        timeout_ms: Option<String>,
        #[pound(long)]
        workers: Option<String>,
    },
    /// parse a manpage and emit nushell extern
    Manpage { file: PathBuf },
    /// batch-process manpages under a directory
    ManpageDir { dir: PathBuf },
    /// nushell custom completer
    Complete {
        #[pound(long)]
        dir: Option<String>,
        #[pound(long)]
        timeout_ms: Option<String>,
        #[pound(positional, value_name = "SPAN")]
        spans: Vec<String>,
    },
    /// print stored completion data
    Query {
        #[pound(long)]
        dir: Option<String>,
        #[pound(positional, value_name = "CMD")]
        cmd: Vec<String>,
    },
    /// list indexed commands
    Dump {
        #[pound(long)]
        dir: Option<String>,
    },
    /// audit source divergence
    Diff {
        #[pound(long)]
        scan: Option<PathBuf>,
        #[pound(long)]
        dir: Option<String>,
        #[pound(long)]
        timeout_ms: Option<String>,
        #[pound(positional, value_name = "CMD")]
        cmd: Vec<String>,
    },
    /// delete the on-the-fly user cache
    Purge {
        #[pound(long)]
        dir: Option<String>,
    },
    /// generate nushell completions for inshellah
    Completions,
    #[pound(hidden)]
    Help,
}

const COMPLETE_DASH_ARG_SENTINEL: &str = "__INSHELLAH_COMPLETE_DASH_ARG__";
const COMPLETE_DOUBLE_DASH_SENTINEL: &str = "__INSHELLAH_LITERAL_DOUBLE_DASH__";

fn normalize_cli_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut args: Vec<String> = args.into_iter().collect();
    if args.first().is_some_and(|arg| arg == "complete") {
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--dir" || args[i] == "--timeout-ms" {
                i += 2;
                continue;
            }
            if args[i].starts_with("--dir=") || args[i].starts_with("--timeout-ms=") {
                i += 1;
                continue;
            }
            if args[i] == "--" {
                args[i] = COMPLETE_DOUBLE_DASH_SENTINEL.to_string();
            } else if args[i].starts_with('-') {
                args[i] = format!("{COMPLETE_DASH_ARG_SENTINEL}{}", args[i]);
            }
            i += 1;
        }
    }
    args
}

fn restore_complete_spans(spans: &mut [String]) {
    for span in spans {
        if span == COMPLETE_DOUBLE_DASH_SENTINEL {
            *span = "--".to_string();
        } else if let Some(rest) = span.strip_prefix(COMPLETE_DASH_ARG_SENTINEL) {
            *span = rest.to_string();
        }
    }
}

fn main() {
    // rust ignores SIGPIPE, so a broken-pipe write becomes a BrokenPipe error
    // that `println!` panics on. restore the default so piping into `head` exits
    // quietly.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if raw_args.is_empty() {
        usage();
        std::process::exit(1);
    }
    if raw_args
        .first()
        .is_some_and(|arg| arg == "-h" || arg == "--help")
    {
        usage();
        return;
    }
    let args = normalize_cli_args(raw_args);
    match Cli::parse_from(args.iter().map(String::as_str)) {
        Cli::Index {
            prefixes,
            dir,
            ignore,
            help_only,
            extra_prefixes,
            timeout_ms,
            workers,
        } => {
            let args = commands::index::Args::from_parts(
                prefixes,
                dir,
                ignore,
                help_only,
                extra_prefixes,
                timeout_ms.as_deref(),
                workers.as_deref(),
            );
            if let Err(e) = commands::index::run(args) {
                eprintln!("index failed: {e}");
                std::process::exit(1);
            }
        }
        Cli::Manpage { file } => {
            if let Err(e) = commands::manpage::run(&file) {
                eprintln!("manpage failed: {e}");
                std::process::exit(1);
            }
        }
        Cli::ManpageDir { dir } => {
            if let Err(e) = commands::manpage_dir::run(&dir) {
                eprintln!("manpage-dir failed: {e}");
                std::process::exit(1);
            }
        }
        Cli::Complete {
            dir,
            timeout_ms,
            mut spans,
        } => {
            restore_complete_spans(&mut spans);
            let cfg = Config::from_env();
            let dirs = completion_dirs(dir.as_deref());
            let timeout_override = parse_timeout_ms(timeout_ms.as_deref());
            let timeout_ms = timeout_override.unwrap_or(cfg.timeout_ms);
            let (user_dir, system_dirs) =
                commands::complete::default_user_dir_and_system_dirs(dirs);
            let mandirs = commands::complete::mandirs_for_system_dirs(&system_dirs);
            commands::complete::run(&spans, &user_dir, &system_dirs, &mandirs, timeout_ms, &cfg);
        }
        Cli::Query { dir, cmd } => {
            let dirs = completion_dirs(dir.as_deref());
            if cmd.is_empty() {
                eprintln!("error: query requires a CMD argument");
                std::process::exit(1);
            }
            let cmd = cmd.join(" ");
            if let Err(e) = commands::query::run(&cmd, &dirs) {
                eprintln!("query failed: {e}");
                std::process::exit(1);
            }
        }
        Cli::Dump { dir } => {
            let dirs = completion_dirs(dir.as_deref());
            commands::dump::run(&dirs);
        }
        Cli::Diff {
            scan,
            dir,
            timeout_ms,
            cmd,
        } => {
            let cfg = Config::from_env();
            if let Some(prefix) = scan {
                commands::diff::run_scan(&prefix, cfg.timeout_ms);
            } else {
                let dirs = completion_dirs(dir.as_deref());
                let timeout_override = parse_timeout_ms(timeout_ms.as_deref());
                if cmd.is_empty() {
                    eprintln!("error: diff requires a CMD argument");
                    std::process::exit(1);
                }
                commands::diff::run(&cmd, &dirs, timeout_override.unwrap_or(cfg.timeout_ms));
            }
        }
        Cli::Purge { dir } => {
            let dirs = completion_dirs(dir.as_deref());
            // only the writable user dir is purged, never the system overlays
            let user_dir = dirs.first().cloned().unwrap_or_else(default_store_path);
            commands::purge::run(&user_dir);
        }
        Cli::Completions => commands::completions::run(),
        Cli::Help => usage(),
    }
}
