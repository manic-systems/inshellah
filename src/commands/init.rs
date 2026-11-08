// SPDX-License-Identifier: EUPL-1.2
//! `inshellah init`.

use std::io::{
    self,
    Write as _,
};

use inshellah::shell::Shell;

/// # Errors
/// fails when stdout cannot be written.
pub fn run(shell: Shell) -> io::Result<()> {
    let script = match shell {
        Shell::Nushell => include_str!("../../shells/inshellah.nu"),
        Shell::Bash => include_str!("../../shells/inshellah.bash"),
        Shell::Zsh => include_str!("../../shells/inshellah.zsh"),
        Shell::Fish => {
            return Err(io::Error::other(format!("no {shell} hook yet")));
        },
    };
    io::stdout().lock().write_all(script.as_bytes())
}
