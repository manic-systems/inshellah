// SPDX-License-Identifier: EUPL-1.2
//! `inshellah manpage-dir`.

use std::{
    fs,
    io,
    path::Path,
};

use inshellah::{
    indexer::{
        list_manpages,
        process_manpage,
    },
    parsers::nushell::generate_extern,
};

/// # Errors
/// fails when `dir` is missing, unreadable or not a directory.
pub fn run(dir: &Path) -> io::Result<()> {
    let unreadable = |err: io::Error| {
        io::Error::new(
            err.kind(),
            format!("cannot read manpage directory {}, {err}", dir.display()),
        )
    };
    if !fs::metadata(dir).map_err(unreadable)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("manpage directory {} is not a directory", dir.display()),
        ));
    }
    fs::read_dir(dir).map_err(unreadable)?;
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
