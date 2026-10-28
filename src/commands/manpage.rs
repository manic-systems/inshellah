// SPDX-License-Identifier: EUPL-1.2
//! `inshellah manpage`.

use std::{
    fs,
    io,
    path::Path,
};

use inshellah::{
    indexer::process_manpage,
    parsers::nushell::generate_extern,
};

/// # Errors
/// fails when `file` is missing, unreadable or not a regular file.
pub fn run(file: &Path) -> io::Result<()> {
    let metadata = fs::metadata(file).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("cannot read manpage {}, {err}", file.display()),
        )
    })?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("manpage {} is not a file", file.display()),
        ));
    }
    if let Some((name, result, sub_sections)) = process_manpage(file) {
        print!("{}", generate_extern(&name, &result));
        for (sub_cmd, sub_result) in sub_sections {
            print!("{}", generate_extern(&sub_cmd, &sub_result));
        }
    }
    Ok(())
}
