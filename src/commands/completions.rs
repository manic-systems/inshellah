// SPDX-License-Identifier: EUPL-1.2
//! `inshellah completions`.

pub fn run() {
    // inshellah's own surface is small enough that explicit externs beat the
    // parser-driven generator aimed at arbitrary cmds.
    print!(
        r#"module inshellah-completions {{
export extern "inshellah" [
    --help(-h)                      # show help
]

export extern "inshellah index" [
    ...prefix: path
    --dir: path                     # completion output directory
    --ignore: path                  # file of commands to skip
    --help-only: path               # file of commands to scrape with --help only
    --prefix: string                # extra colon-separated scrape prefixes
    --timeout-ms: int               # per-subprocess timeout in milliseconds
    --workers: int                  # parallel scrape workers
]

export extern "inshellah complete" [
    cmd: string
    ...args: string
    --dir: string                   # writable cache plus read-only dirs
    --timeout-ms: int               # on-the-fly scrape timeout in milliseconds
]

export extern "inshellah query" [
    cmd: string
    ...subcommand: string
    --dir: string                   # completion directories to read
]

export extern "inshellah dump" [
    --dir: string                   # completion directories to read
]

export extern "inshellah diff" [
    cmd?: string
    ...subcommand: string
    --dir: path                     # extra man directory to inspect
    --timeout-ms: int               # help scrape timeout in milliseconds
    --scan: path                    # scan a prefix for source divergence
]

export extern "inshellah purge" [
    --dir: string                   # writable cache plus read-only dirs
]

export extern "inshellah manpage" [
    file: path
]

export extern "inshellah manpage-dir" [
    dir: path
]

export extern "inshellah completions" []
}}

use inshellah-completions *
"#
    );
}
