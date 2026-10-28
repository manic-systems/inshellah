// SPDX-License-Identifier: EUPL-1.2
use std::{
    collections::HashSet,
    env,
    ffi::{
        CStr,
        CString,
        OsStr,
    },
    fs::{
        self,
        File,
    },
    io::Read as _,
    mem,
    os::unix::ffi::OsStrExt as _,
    path::{
        Path,
        PathBuf,
    },
    slice,
};

use crate::{
    complete::Candidate,
    dynamic::shared::{
        DynCtx,
        Words,
    },
};

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CONFIG_FILES: usize = 64;
const MAX_INCLUDE_DEPTH: usize = 8;

struct HostSet {
    user_prefix: String,
    suffix:      &'static str,
    seen:        HashSet<String>,
    out:         Vec<Candidate>,
}

impl HostSet {
    fn add(&mut self, host: &str) {
        let value = if self.suffix == ":" && host.contains(':') {
            format!("{}[{host}]:", self.user_prefix)
        } else {
            format!("{}{host}{}", self.user_prefix, self.suffix)
        };
        if self.seen.insert(value.clone()) {
            self.out.push(Candidate::new(value, ""));
        }
    }
}

struct ConfigWalk<'home> {
    home:    Option<&'home Path>,
    visited: HashSet<PathBuf>,
}

pub(super) fn complete(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let jump = match words.value_slot(ctx) {
        Some("-J") => true,
        Some(_) => return None,
        None => false,
    };
    let command = words.command();
    let active = words.current();
    if active.starts_with('-') {
        return None;
    }
    let host_operand = match command {
        "scp" => {
            !active.is_empty()
                && !active.contains(':')
                && !active.contains('/')
                && active != "."
                && active != ".."
        },
        _ => words.arg_count() == 0 && !active.contains(':'),
    };
    if !jump && !host_operand {
        return None;
    }
    let home = env::var_os("HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    candidates(
        user_prefix(active)?,
        if command == "scp" && !jump { ":" } else { "" },
        home.as_deref(),
        Path::new("/etc/ssh"),
    )
}

fn candidates(
    user_prefix: &str,
    suffix: &'static str,
    home: Option<&Path>,
    system_dir: &Path,
) -> Option<Vec<Candidate>> {
    let mut hosts = HostSet {
        user_prefix: user_prefix.to_owned(),
        suffix,
        seen: HashSet::new(),
        out: Vec::new(),
    };
    let mut walk = ConfigWalk {
        home,
        visited: HashSet::new(),
    };
    let user_dir = home.map(|dir| dir.join(".ssh"));
    if let Some(dir) = &user_dir {
        walk.read_config(&dir.join("config"), dir, 0, &mut hosts);
    }
    walk.read_config(&system_dir.join("ssh_config"), system_dir, 0, &mut hosts);
    if let Some(dir) = &user_dir {
        read_known_hosts(&dir.join("known_hosts"), &mut hosts);
        read_known_hosts(&dir.join("known_hosts2"), &mut hosts);
    }
    read_known_hosts(&system_dir.join("ssh_known_hosts"), &mut hosts);
    (!hosts.out.is_empty()).then_some(hosts.out)
}

fn read_capped(path: &Path) -> Option<String> {
    if !fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_FILE_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

impl ConfigWalk<'_> {
    fn read_config(&mut self, path: &Path, base: &Path, depth: usize, hosts: &mut HostSet) {
        if depth > MAX_INCLUDE_DEPTH || self.visited.len() >= MAX_CONFIG_FILES {
            return;
        }
        let Ok(canonical) = path.canonicalize() else {
            return;
        };
        if !self.visited.insert(canonical) {
            return;
        }
        let Some(contents) = read_capped(path) else {
            return;
        };
        for line in contents.lines() {
            if let Some(patterns) = config_value(line, "host") {
                for host in patterns.split_whitespace().filter(|host| is_literal(host)) {
                    hosts.add(host);
                }
            } else if let Some(patterns) = config_value(line, "include") {
                for pattern in split_quoted(patterns) {
                    for included in self.expand_include(&pattern, base) {
                        self.read_config(&included, base, depth + 1, hosts);
                    }
                }
            }
        }
    }

    fn expand_include(&self, pattern: &str, base: &Path) -> Vec<PathBuf> {
        let path = if let Some(rest) = pattern.strip_prefix("~/") {
            let Some(home) = self.home else {
                return Vec::new();
            };
            home.join(rest)
        } else {
            base.join(pattern)
        };
        glob_paths(&path)
    }
}

fn split_quoted(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    for ch in text.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                started = true;
            },
            space if space.is_whitespace() && !quoted => {
                if started {
                    words.push(mem::take(&mut current));
                    started = false;
                }
            },
            other => {
                current.push(other);
                started = true;
            },
        }
    }
    if started {
        words.push(current);
    }
    words
}

fn glob_paths(path: &Path) -> Vec<PathBuf> {
    let Ok(pattern) = CString::new(path.as_os_str().as_bytes()) else {
        return Vec::new();
    };
    let mut matches = Vec::new();
    // SAFETY: glob_t is plain data and all-zero is the state glob expects.
    let mut found: libc::glob_t = unsafe { mem::zeroed() };
    // SAFETY: pattern is NUL-terminated and found is a valid glob_t.
    if unsafe { libc::glob(pattern.as_ptr(), 0, None, &raw mut found) } == 0 {
        // SAFETY: glob filled gl_pathv with gl_pathc entries.
        let entries = unsafe { slice::from_raw_parts(found.gl_pathv, found.gl_pathc) };
        for &entry in entries.iter().take(MAX_CONFIG_FILES) {
            // SAFETY: every entry glob returned is NUL-terminated.
            let bytes = unsafe { CStr::from_ptr(entry) }.to_bytes();
            matches.push(PathBuf::from(OsStr::from_bytes(bytes)));
        }
    }
    // SAFETY: found came from glob above and is freed exactly once.
    unsafe { libc::globfree(&raw mut found) };
    matches
}

fn read_known_hosts(path: &Path, hosts: &mut HostSet) {
    let Some(contents) = read_capped(path) else {
        return;
    };
    for line in contents.lines() {
        let mut fields = line.split_whitespace();
        let Some(mut names) = fields.next() else {
            continue;
        };
        if names.starts_with('@') {
            if names.eq_ignore_ascii_case("@revoked") {
                continue;
            }
            let Some(marked) = fields.next() else {
                continue;
            };
            names = marked;
        }
        for host in names.split(',').filter_map(known_host_name) {
            hosts.add(host);
        }
    }
}

fn user_prefix(active: &str) -> Option<&str> {
    if active.contains(':') || active.contains('/') {
        return None;
    }
    match active.rsplit_once('@') {
        Some((user, host)) if !user.is_empty() => active.strip_suffix(host),
        Some(_) => None,
        None => Some(""),
    }
}

fn config_value<'line>(line: &'line str, wanted: &str) -> Option<&'line str> {
    let code = line
        .split_once('#')
        .map_or(line, |(before, _)| before)
        .trim();
    let separator = code.find(|ch: char| ch.is_whitespace() || ch == '=')?;
    let (keyword, after) = code.split_at(separator);
    if !keyword.eq_ignore_ascii_case(wanted) {
        return None;
    }
    let rest = after.trim_start();
    let value = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    (!value.is_empty()).then_some(value)
}

fn is_literal(host: &str) -> bool {
    !host.is_empty() && !host.starts_with('!') && !host.contains('*') && !host.contains('?')
}

fn known_host_name(entry: &str) -> Option<&str> {
    if entry.starts_with('|') || !is_literal(entry) {
        return None;
    }
    if let Some(bracketed) = entry.strip_prefix('[') {
        let (host, _) = bracketed.split_once("]:")?;
        return (!host.is_empty()).then_some(host);
    }
    Some(entry)
}

#[cfg(test)]
mod tests {
    use std::{
        env::temp_dir,
        fs,
        path::{
            Path,
            PathBuf,
        },
        process,
    };

    use super::candidates;

    fn scratch(name: &str) -> PathBuf {
        let dir = temp_dir().join(format!("inshellah-ssh-{name}-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn values(
        user: &str,
        suffix: &'static str,
        home: Option<&Path>,
        system: &Path,
    ) -> Option<Vec<String>> {
        candidates(user, suffix, home, system)
            .map(|found| found.into_iter().map(|cand| cand.value).collect())
    }

    #[test]
    fn system_config_and_known_hosts_are_read_after_the_user_files() {
        let root = scratch("system");
        let system = root.join("etc");
        let home = root.join("home");
        fs::create_dir_all(&system).unwrap();
        fs::create_dir_all(home.join(".ssh")).unwrap();
        fs::write(home.join(".ssh/config"), "Host user-conf\n").unwrap();
        fs::write(system.join("ssh_config"), "Host system-conf\n").unwrap();
        fs::write(
            system.join("ssh_known_hosts"),
            "global.example ssh-ed25519 AAAA\n",
        )
        .unwrap();

        assert_eq!(
            values("", "", Some(&home), &system),
            Some(vec![
                "user-conf".into(),
                "system-conf".into(),
                "global.example".into()
            ])
        );
    }

    #[test]
    fn system_inventories_work_without_home() {
        let root = scratch("no-home");
        fs::write(root.join("ssh_config"), "Host system-conf\n").unwrap();
        assert_eq!(
            values("", "", None, &root),
            Some(vec!["system-conf".into()])
        );
    }

    #[test]
    fn scp_brackets_ipv6_hosts() {
        let root = scratch("ipv6");
        fs::write(
            root.join("ssh_known_hosts"),
            "[fe80::1]:2222 ssh-ed25519 AAAA\nfe80::2 ssh-ed25519 AAAA\n",
        )
        .unwrap();

        assert_eq!(
            values("bob@", ":", None, &root),
            Some(vec!["bob@[fe80::1]:".into(), "bob@[fe80::2]:".into()])
        );
    }

    #[test]
    fn missing_system_dir_yields_no_hosts() {
        let root = scratch("missing");
        assert_eq!(values("", "", None, &root.join("absent")), None);
    }
}
