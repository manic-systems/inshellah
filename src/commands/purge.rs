// SPDX-License-Identifier: EUPL-1.2
//! `inshellah purge`.

use std::path::Path;

use inshellah::store::purge_dir;

pub fn run(user_dir: &Path) {
    match purge_dir(user_dir) {
        Ok(n) => println!("purged {n} cached entries from {}", user_dir.display()),
        Err(e) => {
            eprintln!("purge failed: {e}");
            std::process::exit(1);
        }
    }
}
