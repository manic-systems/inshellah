// SPDX-License-Identifier: EUPL-1.2
//! `inshellah purge` and `inshellah refresh`.

use std::{
    io,
    path::{
        Path,
        PathBuf,
    },
};

use inshellah::{
    executable::find_in_path,
    indexer::{
        Resolved,
        resolve_command_path_and_cache,
    },
    store::{
        purge_command,
        purge_dir,
        sweep_stale_temp_files,
    },
};

use super::complete::mandirs_with_bin_prefix;

pub fn purge(user_dir: &Path, cmds: &[String]) -> io::Result<()> {
    if cmds.is_empty() {
        let removed = purge_dir(user_dir)?;
        println!(
            "purged {removed} cached entries from {}",
            user_dir.display()
        );
        return Ok(());
    }
    for cmd in cmds {
        let removed = purge_command(user_dir, cmd)?;
        println!("purged {removed} cached entries for {cmd}");
    }
    Ok(())
}

pub fn refresh(
    user_dir: &Path,
    mandirs: &[PathBuf],
    cmds: &[String],
    timeout_ms: u64,
) -> io::Result<()> {
    sweep_stale_temp_files(user_dir);
    for cmd in cmds {
        let bin = find_in_path(cmd).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("{cmd}: not in PATH"))
        })?;
        purge_command(user_dir, cmd)?;
        let all_mandirs = mandirs_with_bin_prefix(mandirs, Some(&bin));
        let Resolved { result, saved } =
            resolve_command_path_and_cache(user_dir, &all_mandirs, cmd, &[], &bin, timeout_ms);
        saved.map_err(|err| io::Error::new(err.kind(), format!("{cmd}: cache write: {err}")))?;
        if result.is_none() {
            return Err(io::Error::other(format!(
                "{cmd}: no usable help or manpage"
            )));
        }
        println!("refreshed {cmd} from {}", bin.display());
    }
    Ok(())
}
