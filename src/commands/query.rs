// SPDX-License-Identifier: EUPL-1.2
//! `inshellah query`.

use std::{
    io,
    path::PathBuf,
};

use inshellah::store::lookup_raw;

pub fn run(cmd: &str, dirs: &[PathBuf]) -> io::Result<()> {
    let data = lookup_raw(dirs, cmd)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("not found: {cmd}")))?;
    print!("{data}");
    Ok(())
}
