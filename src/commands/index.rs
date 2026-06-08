// SPDX-License-Identifier: EUPL-1.2
//! `inshellah index`.

use std::path::{Path, PathBuf};

use inshellah::config::DEFAULT_TIMEOUT_MS;
use inshellah::indexer::{cmd_index, load_ignorelist};
use inshellah::store::default_store_path;

use super::common::split_colon_paths;

pub struct Args {
    pub prefixes: Vec<PathBuf>,
    pub dir: Option<PathBuf>,
    pub ignore: Option<PathBuf>,
    pub help_only: Option<PathBuf>,
    pub timeout_ms: u64,
    pub workers: usize,
}

impl Args {
    pub fn from_parts(
        mut prefixes: Vec<PathBuf>,
        dir: Option<PathBuf>,
        ignore: Option<PathBuf>,
        help_only: Option<PathBuf>,
        extra_prefixes: Vec<String>,
        timeout_ms: Option<&str>,
        workers: Option<&str>,
    ) -> Self {
        prefixes.extend(split_colon_paths(extra_prefixes.iter().map(String::as_str)));
        Self {
            prefixes,
            dir,
            ignore,
            help_only,
            timeout_ms: timeout_ms
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(DEFAULT_TIMEOUT_MS),
            workers: workers
                .and_then(|n| n.parse::<usize>().ok())
                .map(|n| n.max(1))
                .unwrap_or_else(default_workers),
        }
    }
}

pub fn run(args: Args) -> std::io::Result<()> {
    if args.prefixes.is_empty() {
        eprintln!("error: index requires at least one PREFIX");
        std::process::exit(1);
    }
    let dir = args.dir.unwrap_or_else(default_store_path);
    let ignorelist = args
        .ignore
        .as_deref()
        .map(load_ignorelist)
        .unwrap_or_default();
    let help_only = args
        .help_only
        .as_deref()
        .map(load_ignorelist)
        .unwrap_or_default();
    let bindirs: Vec<PathBuf> = args.prefixes.iter().map(|p| p.join("bin")).collect();
    let mandirs: Vec<PathBuf> = args.prefixes.iter().map(|p| man_dir_of_prefix(p)).collect();
    cmd_index(
        &bindirs,
        &mandirs,
        &ignorelist,
        &help_only,
        &dir,
        args.timeout_ms,
        args.workers,
    )
}

fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

fn man_dir_of_prefix(prefix: &Path) -> PathBuf {
    prefix.join("share/man")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pound::Parse;

    #[derive(Parse, Debug)]
    #[pound(name = "inshellah index")]
    struct IndexCli {
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
    }

    impl From<IndexCli> for Args {
        fn from(parsed: IndexCli) -> Self {
            Args::from_parts(
                parsed.prefixes,
                parsed.dir,
                parsed.ignore,
                parsed.help_only,
                parsed.extra_prefixes,
                parsed.timeout_ms.as_deref(),
                parsed.workers.as_deref(),
            )
        }
    }

    fn parse_index_args(args: &[String]) -> Args {
        IndexCli::parse_from(args.iter().map(String::as_str)).into()
    }

    #[test]
    fn index_prefix_flag_appends_colon_separated_prefixes() {
        let args = [
            "/sys".to_string(),
            "--prefix".to_string(),
            "/a:/b/c".to_string(),
            "--prefix".to_string(),
            "/d".to_string(),
        ];
        let parsed = parse_index_args(&args);
        assert_eq!(
            parsed.prefixes,
            vec![
                PathBuf::from("/sys"),
                PathBuf::from("/a"),
                PathBuf::from("/b/c"),
                PathBuf::from("/d"),
            ]
        );
    }
}
