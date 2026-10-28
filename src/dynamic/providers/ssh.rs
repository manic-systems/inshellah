// SPDX-License-Identifier: EUPL-1.2
use std::{
    collections::HashSet,
    env,
    ffi::OsStr,
    fs::{
        self,
        File,
    },
    io::Read as _,
    mem,
    path::{
        Path,
        PathBuf,
    },
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
    let typed = words.current();
    if typed.starts_with('-') {
        return None;
    }
    let (hops, active) = if jump {
        typed.rsplit_once(',').map_or(("", typed), |(_, tail)| {
            (typed.strip_suffix(tail).unwrap_or_default(), tail)
        })
    } else {
        ("", typed)
    };
    let host_operand = match command {
        "scp" => !active.contains(':') && !active.contains('/') && active != "." && active != "..",
        _ => words.arg_count() == 0 && !active.contains(':'),
    };
    if !jump && !host_operand {
        return None;
    }
    let home = env::var_os("HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    let config = words
        .value(&["-F"])
        .map(|file| expand_home(file, home.as_deref()));
    candidates(
        &format!("{hops}{}", user_prefix(active)?),
        if command == "scp" && !jump { ":" } else { "" },
        home.as_deref(),
        Path::new("/etc/ssh"),
        config.as_deref(),
    )
}

fn expand_home(file: &str, home_dir: Option<&Path>) -> PathBuf {
    match (file.strip_prefix("~/"), home_dir) {
        (Some(rest), Some(dir)) => dir.join(rest),
        _ => PathBuf::from(file),
    }
}

fn candidates(
    user_prefix: &str,
    suffix: &'static str,
    home: Option<&Path>,
    system_dir: &Path,
    config: Option<&Path>,
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
    if let Some(file) = config {
        let base = user_dir
            .as_deref()
            .or_else(|| file.parent())
            .unwrap_or(system_dir);
        walk.read_config(file, base, 0, &mut hosts);
    } else {
        if let Some(dir) = &user_dir {
            walk.read_config(&dir.join("config"), dir, 0, &mut hosts);
        }
        walk.read_config(&system_dir.join("ssh_config"), system_dir, 0, &mut hosts);
    }
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

fn glob_paths(pattern: &Path) -> Vec<PathBuf> {
    let mut matches = vec![PathBuf::new()];
    for component in pattern.components() {
        matches = expand_component(&matches, component.as_os_str());
    }
    matches
}

fn expand_component(dirs: &[PathBuf], part: &OsStr) -> Vec<PathBuf> {
    let Some(wild_part) = part.to_str().filter(|text| text.contains(['*', '?'])) else {
        return dirs.iter().map(|dir| dir.join(part)).collect();
    };
    dirs.iter()
        .flat_map(|dir| matching_entries(dir, wild_part))
        .take(MAX_CONFIG_FILES)
        .collect()
}

fn matching_entries(dir: &Path, wild_part: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let pattern: Vec<char> = wild_part.chars().collect();
    let mut found: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.') || pattern.first() == Some(&'.'))
        .filter(|name| wildcard_match(&pattern, &name.chars().collect::<Vec<_>>()))
        .collect();
    found.sort();
    found.into_iter().map(|name| dir.join(name)).collect()
}

fn wildcard_match(pattern: &[char], name: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|skip| wildcard_match(rest, &name[skip..])),
        Some(('?', rest)) => {
            name.split_first()
                .is_some_and(|(_, tail)| wildcard_match(rest, tail))
        },
        Some((literal, rest)) => {
            name.split_first()
                .is_some_and(|(first, tail)| first == literal && wildcard_match(rest, tail))
        },
    }
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
        let (host, port) = bracketed.split_once("]:")?;
        return (!host.is_empty() && port == "22").then_some(host);
    }
    Some(entry)
}

#[cfg(test)]
mod tests {
    use std::{
        env::temp_dir,
        fs,
        ops::Deref,
        path::{
            Path,
            PathBuf,
        },
        process,
        sync::atomic::{
            AtomicU32,
            Ordering,
        },
    };

    use super::{
        candidates,
        wildcard_match,
    };

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = temp_dir().join(format!("inshellah-ssh-{name}-{}-{id}", process::id()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Deref for Scratch {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    fn values(
        user: &str,
        suffix: &'static str,
        home: Option<&Path>,
        system: &Path,
    ) -> Option<Vec<String>> {
        candidates(user, suffix, home, system, None)
            .map(|found| found.into_iter().map(|cand| cand.value).collect())
    }

    #[test]
    fn hosts_on_other_ports_are_not_offered_as_bare_names() {
        let root = Scratch::new("ports");
        fs::write(
            root.join("ssh_known_hosts"),
            "[box.example]:2222,[both.example]:22 ssh-ed25519 AAAA\n[other.example]:22 \
             ssh-ed25519 AAAA\n",
        )
        .unwrap();

        assert_eq!(
            values("", "", None, &root),
            Some(vec!["both.example".into(), "other.example".into()])
        );
    }

    #[test]
    fn wildcards_backtrack_over_stars() {
        let matches = |pattern: &str, name: &str| {
            wildcard_match(
                &pattern.chars().collect::<Vec<_>>(),
                &name.chars().collect::<Vec<_>>(),
            )
        };
        assert!(matches("*a*b", "xxaxxb"));
        assert!(!matches("*a*b", "xxaxx"));
        assert!(matches("a?c", "abc"));
        assert!(!matches("a?c", "ac"));
    }
}
