// SPDX-License-Identifier: EUPL-1.2
//! `inshellah merge` picks the package that owns each command and refuses
//! inputs it cannot read rather than silently dropping them.
#![cfg(test)]

use std::{
    env,
    fs,
    os::unix::fs::{
        PermissionsExt as _,
        symlink,
    },
    path::{
        Path,
        PathBuf,
    },
    process::{
        self,
        Command,
        Output,
    },
    sync::atomic::{
        AtomicU64,
        Ordering,
    },
};

use inshellah::{
    parsers::manpage::ManpageResult,
    store::{
        filename_of_command,
        write_result,
    },
};

fn unique_temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = env::temp_dir().join(format!(
        "{name}-{}-{}",
        process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    drop(fs::remove_dir_all(&dir));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    fs::create_dir_all(dir).expect("script dir");
    let path = dir.join(name);
    fs::write(&path, body).expect("write script");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod script");
    path.canonicalize().expect("canonical script")
}

fn described(description: &str) -> ManpageResult {
    ManpageResult {
        description: description.to_owned(),
        ..ManpageResult::default()
    }
}

struct Fixture {
    root:    PathBuf,
    profile: PathBuf,
    cache:   PathBuf,
    tools:   PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = unique_temp_dir(name);
        let tools = root.join("tools");
        write_script(&tools, "nu", "#!/bin/sh\nprintf '[\"nu\"]\\n'\n");
        let profile = root.join("profile");
        fs::create_dir_all(profile.join("bin")).expect("profile bin");
        Self {
            cache: root.join("cache"),
            profile,
            tools,
            root,
        }
    }

    fn package(&self, name: &str) -> PathBuf {
        let package = self.root.join(name);
        write_script(&package.join("bin"), "demo", "#!/bin/sh\nexit 0\n");
        package
    }

    fn index(&self, name: &str, description: &str) -> PathBuf {
        let index = self.root.join(name);
        write_result(&index, "demo", "manpage", &described(description)).expect("source index");
        index
    }

    fn merge(&self, profile: &Path, sources: &str) -> Output {
        let sources_file = self.root.join("sources");
        fs::write(&sources_file, sources).expect("sources");
        Command::new(env!("CARGO_BIN_EXE_inshellah"))
            .arg("merge")
            .arg(profile)
            .arg("--dir")
            .arg(&self.cache)
            .arg("--sources")
            .arg(&sources_file)
            .current_dir(&self.root)
            .env("PATH", &self.tools)
            .output()
            .expect("run merge")
    }

    fn merged(&self) -> Option<String> {
        let name = filename_of_command("demo");
        fs::read_to_string(self.cache.join(format!("{name}.json"))).ok()
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn merge_resolves_profile_and_package_paths_before_selecting_an_owner() {
    let fixture = Fixture::new("inshellah-merge-resolved-owner");
    let package_a = fixture.package("package-a");
    let package_b = fixture.package("package-b");
    fixture.index("index-a", "owned by package A");
    fixture.index("index-b", "owned by package B");
    fs::create_dir_all(fixture.root.join("links")).expect("links");
    symlink("../package-b/bin/demo", fixture.root.join("links/demo")).expect("intermediate link");
    symlink("../../links/demo", fixture.profile.join("bin/demo")).expect("profile link");
    assert_eq!(
        fs::canonicalize(fixture.profile.join("bin/demo")).expect("resolved binary"),
        package_b
            .join("bin/demo")
            .canonicalize()
            .expect("package binary")
    );
    assert!(package_a.is_dir());

    let output = fixture.merge(&fixture.profile, "package-a index-a\npackage-b index-b\n");
    assert_success(&output);
    let merged = fixture.merged().expect("merged cache");
    assert!(merged.contains("owned by package B"), "cache = {merged}");
    assert!(!merged.contains("owned by package A"), "cache = {merged}");
}

#[test]
fn merge_falls_back_to_a_claiming_index_when_the_owner_has_no_files_for_the_root() {
    let fixture = Fixture::new("inshellah-merge-owner-fallback");
    let package = fixture.package("package");
    symlink(package.join("bin/demo"), fixture.profile.join("bin/demo")).expect("profile link");
    fixture.package("other-package");
    let empty = fixture.root.join("empty-index");
    fs::create_dir_all(&empty).expect("empty index");
    let index = fixture.index("source-index", "from the claiming index");

    let sources = format!(
        "{} {}\n{} {}\n",
        package.display(),
        empty.display(),
        fixture.root.join("other-package").display(),
        index.display()
    );
    assert_success(&fixture.merge(&fixture.profile, &sources));
    let merged = fixture.merged().expect("merged cache");
    assert!(
        merged.contains("from the claiming index"),
        "cache = {merged}"
    );
}

#[test]
fn merge_with_unreadable_inputs_fails_and_keeps_the_live_index() {
    let fixture = Fixture::new("inshellah-merge-bad-inputs");
    let package = fixture.package("package");
    symlink(package.join("bin/demo"), fixture.profile.join("bin/demo")).expect("profile link");
    let index = fixture.index("source-index", "live");
    let good = format!("{} {}\n", package.display(), index.display());
    assert_success(&fixture.merge(&fixture.profile, &good));
    assert!(fixture.merged().is_some());

    let gone = fixture.root.join("gone");
    let cases = [
        (gone.clone(), good, "is not a directory"),
        (
            fixture.profile.clone(),
            format!("{} {}\n", gone.display(), index.display()),
            "cannot resolve source package",
        ),
        (
            fixture.profile.clone(),
            format!("{} {}\n", package.display(), gone.display()),
            "cannot read source index",
        ),
    ];
    for (profile, sources, message) in cases {
        let output = fixture.merge(&profile, &sources);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "stderr = {stderr}");
        assert!(stderr.contains(message), "stderr = {stderr}");
        assert!(fixture.merged().is_some());
    }
}

#[test]
fn merge_accepts_an_empty_source_index() {
    let fixture = Fixture::new("inshellah-merge-empty-index");
    let package = fixture.package("package");
    symlink(package.join("bin/demo"), fixture.profile.join("bin/demo")).expect("profile link");
    let empty = fixture.root.join("empty-index");
    fs::create_dir_all(&empty).expect("empty index");
    assert_success(&fixture.merge(
        &fixture.profile,
        &format!("{} {}\n", package.display(), empty.display()),
    ));
}
