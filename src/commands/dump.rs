// SPDX-License-Identifier: EUPL-1.2
//! `inshellah dump`.

use std::path::PathBuf;

use inshellah::store::{all_commands, file_type_of};

pub fn run(dirs: &[PathBuf]) {
    let cmds = all_commands(dirs);
    println!("{} commands", cmds.len());
    for cmd in &cmds {
        let src = file_type_of(dirs, cmd).unwrap_or_else(|| "?".to_string());
        println!("{src:>8}  {cmd}");
    }
}
