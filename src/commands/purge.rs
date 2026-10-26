// SPDX-License-Identifier: EUPL-1.2
//! `inshellah purge`.

use std::io;
use std::path::Path;

use inshellah::store::purge_dir;

pub fn run(user_dir: &Path) -> io::Result<()> {
    let removed = purge_dir(user_dir)?;
    println!(
        "purged {removed} cached entries from {}",
        user_dir.display()
    );
    Ok(())
}
