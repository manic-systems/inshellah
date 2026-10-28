// SPDX-License-Identifier: EUPL-1.2
//! A temp directory removed on drop, also when the test panics. Pulled in with
//! `#[path = "common/temp_dir.rs"] mod temp_dir;` by the tests that need it.

use std::{
    env,
    fs,
    path::{
        Path,
        PathBuf,
    },
    process,
    sync::atomic::{
        AtomicU64,
        Ordering,
    },
};

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = env::temp_dir().join(format!(
            "{name}-{}-{}",
            process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        drop(fs::remove_dir_all(&self.0));
    }
}
