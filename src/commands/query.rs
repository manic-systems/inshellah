// SPDX-License-Identifier: EUPL-1.2
//! `inshellah query`.

use std::path::PathBuf;

use inshellah::store::lookup_raw;

pub fn run(cmd: &str, dirs: &[PathBuf]) -> std::io::Result<()> {
    match lookup_raw(dirs, cmd) {
        Some(data) => {
            print!("{data}");
            Ok(())
        }
        None => {
            eprintln!("not found: {cmd}");
            std::process::exit(1);
        }
    }
}
