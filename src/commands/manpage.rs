// SPDX-License-Identifier: EUPL-1.2
//! `inshellah manpage`.

use std::path::Path;

use inshellah::indexer::process_manpage;
use inshellah::parsers::nushell::generate_extern;

pub fn run(file: &Path) -> std::io::Result<()> {
    if let Some((name, result, sub_sections)) = process_manpage(file) {
        print!("{}", generate_extern(&name, &result));
        for (sub_cmd, sub_result) in sub_sections {
            print!("{}", generate_extern(&sub_cmd, &sub_result));
        }
    }
    Ok(())
}
