// SPDX-License-Identifier: EUPL-1.2
//! `inshellah manpage-dir`.

use std::path::Path;

use inshellah::indexer::{list_manpages, process_manpage};
use inshellah::parsers::nushell::generate_extern;

pub fn run(dir: &Path) -> std::io::Result<()> {
    for path in list_manpages(&[dir.to_path_buf()]) {
        if let Some((name, result, sub_sections)) = process_manpage(&path) {
            print!("{}", generate_extern(&name, &result));
            for (sub_cmd, sub_result) in sub_sections {
                print!("{}", generate_extern(&sub_cmd, &sub_result));
            }
        }
    }
    Ok(())
}
