// SPDX-License-Identifier: EUPL-1.2
//! `inshellah complete`.

use std::borrow::Cow;
use std::collections::HashSet;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use inshellah::complete::{Candidate, generate_candidates};
use inshellah::config::Config;
use inshellah::dynamic::{dynamic_complete_candidates_with_path, dynamic_value_completions};
use inshellah::indexer::{is_executable, resolve_command_path_and_cache};
use inshellah::parsers::manpage::{ManpageResult, OwnedSwitch};
use inshellah::store::{self, default_store_path, lookup, subcommands_of};

use super::common::find_in_path;

const ELEVATION_COMMANDS: &[&str] = &["sudo", "doas", "pkexec", "su", "run0"];

pub fn run(
    spans: &[String],
    user_dir: &Path,
    system_dirs: &[PathBuf],
    mandirs: &[PathBuf],
    timeout_ms: u64,
    cfg: &Config,
) {
    let mut dirs: Vec<PathBuf> = Vec::with_capacity(system_dirs.len() + 1);
    dirs.push(user_dir.to_path_buf());
    dirs.extend(system_dirs.iter().cloned());

    // skip past elevation wrappers (sudo, doas) to the real command.
    let mut explicit_cmd_path: Option<PathBuf> = None;
    let mut spans: Vec<String> = match spans.first() {
        Some(first) if ELEVATION_COMMANDS.contains(&first.as_str()) => {
            let rest = &spans[1..];
            let mut real_spans = None;
            for (idx, s) in rest.iter().enumerate() {
                if let Some(path) = executable_span_path(s)
                    && let Some(name) = command_name_for_path(&path)
                {
                    let mut target = rest[idx..].to_vec();
                    target[0] = name;
                    explicit_cmd_path = Some(path);
                    real_spans = Some(target);
                    break;
                }
                if !s.is_empty()
                    && !s.starts_with('-')
                    && (lookup(&dirs, s).is_some() || find_in_path(s).is_some())
                {
                    real_spans = Some(rest[idx..].to_vec());
                    break;
                }
            }
            real_spans.unwrap_or_else(|| spans.to_vec())
        }
        _ => spans.to_vec(),
    };
    if explicit_cmd_path.is_none()
        && let Some(first) = spans.first()
        && let Some(path) = executable_span_path(first)
        && let Some(name) = command_name_for_path(&path)
    {
        spans[0] = name;
        explicit_cmd_path = Some(path);
    }

    if spans.is_empty() || (explicit_cmd_path.is_none() && find_in_path(&spans[0]).is_none()) {
        println!("null");
        return;
    }

    let cmd_name = spans[0].clone();
    let rest: Vec<String> = spans[1..].to_vec();

    if let Some(candidates) =
        dynamic_value_completions(&cmd_name, &rest, explicit_cmd_path.as_deref(), timeout_ms)
    {
        print_completion_candidates(&candidates);
        return;
    }

    let last_token = rest.last().cloned().unwrap_or_default();
    let complete_rest: &[String] = if last_token.is_empty() || rest.is_empty() {
        &rest
    } else {
        &rest[..rest.len() - 1]
    };
    let mut cmd_path = command_path(&dirs, None, &cmd_name, complete_rest);

    // try longest-prefix match: "git stash apply" -> "git stash" -> "git"
    let find_result = |path: &[String]| -> Option<(String, ManpageResult, usize)> {
        (1..=path.len()).rev().find_map(|len| {
            let name = path[..len].join(" ");
            lookup(&dirs, &name).map(|r| (name, r, len))
        })
    };

    let mut found = find_result(&cmd_path.path);

    let bin_path = explicit_cmd_path
        .as_ref()
        .cloned()
        .or_else(|| find_in_path(&cmd_name));
    // also search the binary's own prefix for manpages.
    let mut all_mandirs = mandirs.to_vec();
    if let Some(prefix) = bin_path
        .as_deref()
        .and_then(Path::parent)
        .and_then(Path::parent)
    {
        let share_man = prefix.join("share/man");
        if share_man.is_dir() {
            all_mandirs.push(share_man);
        }
    }

    while let Some(bin) = &bin_path
        && (cache_is_stale(user_dir, found.as_ref(), cfg.cache_ttl_secs)
            || found
                .as_ref()
                .is_none_or(|(_, _, depth)| *depth < cmd_path.path.len()))
    {
        let Some(result) = resolve_command_path_and_cache(
            user_dir,
            &all_mandirs,
            &cmd_name,
            &cmd_path.path[1..],
            bin,
            timeout_ms,
        ) else {
            break;
        };
        let resolved_len = cmd_path.path.len();
        let probed = cmd_path.probe;
        found = Some((cmd_path.path.join(" "), result, resolved_len));
        let fresh = found.as_ref().map(|(name, r, _)| (name.as_str(), r));
        cmd_path = command_path(&dirs, fresh, &cmd_name, complete_rest);
        // a probe's --help can echo the parent's, which caches it as a group
        // and would license another probe one word deeper.
        if probed || cmd_path.path.len() <= resolved_len {
            break;
        }
        if let Some(hit) = find_result(&cmd_path.path)
            && hit.2 > resolved_len
        {
            found = Some(hit);
        }
    }

    let slot = cmd_path.slot;
    let resolve_depth = cmd_path.depth;
    let typing_flag = slot == ArgSlot::Free && cfg.triggers_flags(&last_token);
    let fallback_subcommands = match &found {
        Some((matched_name, r, _)) if r.subcommands.is_empty() => {
            subcommands_of(&dirs, matched_name)
        }
        _ => Vec::new(),
    };
    // positional value choices (getent databases) fill the same slot as
    // subcommands, so they suppress the file/dynamic handoff too.
    let has_subs = match &found {
        Some((_, r, _)) => {
            !r.subcommands.is_empty()
                || !r.positional_choices.is_empty()
                || !fallback_subcommands.is_empty()
        }
        None => false,
    };
    let candidates: Vec<Candidate> = match &found {
        Some((_, r, depth)) if slot == ArgSlot::Free => generate_candidates(
            r,
            *depth,
            resolve_depth,
            &last_token,
            &fallback_subcommands,
            typing_flag,
            cfg,
        ),
        _ => Vec::new(),
    };
    // hand off at non-flag leaf positions so file and dynamic completers can
    // answer argument prefixes. a leading "-" keeps flags.
    let want_files = !typing_flag
        && (slot != ArgSlot::Free
            || (!has_subs && (last_token.is_empty() || candidates.is_empty())));
    // spans are post-elevation, so `sudo nix ...` reaches this as
    // `[nix, ...]` and hits the nix branch.
    let dyn_candidates = (!typing_flag || candidates.is_empty())
        .then(|| dynamic_complete_candidates_with_path(&spans, explicit_cmd_path.as_deref(), cfg))
        .flatten();
    match dyn_candidates {
        Some(dyn_candidates) if candidates.is_empty() || want_files => {
            print_completion_candidate_values(dyn_candidates);
        }
        Some(dyn_candidates) => print_completion_candidate_values(merge_completion_candidates(
            dyn_candidates,
            candidates,
            cfg.max_completions,
        )),
        None if want_files => println!("null"),
        None => print_completion_candidate_values(candidates),
    }
}

pub fn mandirs_for_system_dirs(system_dirs: &[PathBuf]) -> Vec<PathBuf> {
    system_dirs
        .iter()
        .filter_map(|d| mandir_for_completion_dir(d))
        .filter(|p| p.is_dir())
        .collect()
}

pub fn default_user_dir_and_system_dirs(dirs: Vec<PathBuf>) -> (PathBuf, Vec<PathBuf>) {
    match dirs.split_first() {
        Some((first, rest)) => (first.clone(), rest.to_vec()),
        None => (default_store_path(), Vec::new()),
    }
}

fn executable_span_path(span: &str) -> Option<PathBuf> {
    if !span.contains('/') {
        return None;
    }
    // the scrape runner's cwd is /tmp, so a relative path would resolve there.
    let path = std::path::absolute(span).ok()?;
    is_executable(&path).then_some(path)
}

fn command_name_for_path(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn merge_completion_candidates(
    first: Vec<Candidate>,
    second: Vec<Candidate>,
    max_completions: usize,
) -> Vec<Candidate> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(first.len() + second.len());
    for candidate in first.into_iter().chain(second) {
        if seen.insert(candidate.value.clone()) {
            out.push(candidate);
            if max_completions > 0 && out.len() >= max_completions {
                break;
            }
        }
    }
    out
}

fn print_completion_candidate_values(candidates: Vec<Candidate>) {
    let candidates: Vec<String> = candidates.into_iter().map(Candidate::into_json).collect();
    print_completion_candidates(&candidates);
}

// `null` is nushell's no-match form.
fn print_completion_candidates(candidates: &[String]) {
    if candidates.is_empty() {
        println!("null");
    } else {
        let mut out = io::stdout().lock();
        out.write_all(b"[").expect("write completion output");
        for (idx, candidate) in candidates.iter().enumerate() {
            if idx > 0 {
                out.write_all(b",").expect("write completion output");
            }
            out.write_all(candidate.as_bytes())
                .expect("write completion output");
        }
        out.write_all(b"]\n").expect("write completion output");
    }
}

fn switch_takes_value(result: &ManpageResult, token: &str) -> bool {
    if token.contains('=') {
        return false;
    }
    result.entries.iter().any(|entry| {
        if entry.param.is_none() {
            return false;
        }
        match &entry.switch {
            OwnedSwitch::Long(long) => token == format!("--{long}"),
            OwnedSwitch::Short(short) => {
                let mut chars = token.chars();
                matches!(
                    (chars.next(), chars.next(), chars.next()),
                    (Some('-'), Some(c), None) if c == *short
                )
            }
            OwnedSwitch::Both(short, long) => {
                token == format!("--{long}") || {
                    let mut chars = token.chars();
                    matches!(
                        (chars.next(), chars.next(), chars.next()),
                        (Some('-'), Some(c), None) if c == *short
                    )
                }
            }
        }
    })
}

/// the slot the token under the cursor fills.
#[derive(Clone, Copy, PartialEq)]
enum ArgSlot {
    Free,
    FlagValue,
    AfterDoubleDash,
}

struct CommandPath {
    path: Vec<String>,
    depth: usize,
    slot: ArgSlot,
    probe: bool,
}

/// `fresh` preserves flag metadata when caching fails.
fn command_path(
    dirs: &[PathBuf],
    fresh: Option<(&str, &ManpageResult)>,
    cmd_name: &str,
    rest: &[String],
) -> CommandPath {
    let node = |name: &str| match fresh {
        Some((fresh_name, result)) if fresh_name == name => Some(Cow::Borrowed(result)),
        _ => lookup(dirs, name).map(Cow::Owned),
    };
    let mut path = vec![cmd_name.to_string()];
    let mut depth = 1;
    let mut current = node(cmd_name);
    let mut current_is_exact = current.is_some();
    let mut on_path = true;
    let mut probe = false;
    let mut skip_next_value = false;

    for token in rest {
        if token.is_empty() {
            continue;
        }
        if skip_next_value {
            skip_next_value = false;
            continue;
        }
        if token == "--" {
            return CommandPath {
                path,
                depth,
                slot: ArgSlot::AfterDoubleDash,
                probe,
            };
        }
        if token.starts_with('-') {
            if current
                .as_ref()
                .is_some_and(|result| switch_takes_value(result, token))
            {
                skip_next_value = true;
            }
            continue;
        }

        depth += 1;
        if !on_path {
            continue;
        }
        // map an alias to its canonical child name (cargo `b` -> build) so the
        // path resolves to the real node and descends into its flags/subs.
        let group = current
            .as_ref()
            .filter(|r| current_is_exact && !r.subcommands.is_empty());
        let in_group = group.is_some();
        let listed = group.and_then(|r| listed_subcommand(r, token));
        let is_listed = listed.is_some();
        let name = listed.unwrap_or_else(|| token.clone());
        let child = node(&format!("{} {name}", path.join(" ")));
        if !is_listed && child.is_none() {
            on_path = false;
            // Plugins like `cargo nextest` may be missing from help output.
            if !in_group {
                continue;
            }
            probe = true;
        }
        path.push(name);
        current_is_exact = child.is_some();
        current = child.or(current);
    }

    let slot = if skip_next_value {
        ArgSlot::FlagValue
    } else {
        ArgSlot::Free
    };
    CommandPath {
        path,
        depth,
        slot,
        probe,
    }
}

/// canonical child name for a token that names one of `result`'s subcommands
/// or their aliases, else None.
fn listed_subcommand(result: &ManpageResult, token: &str) -> Option<String> {
    result
        .subcommands
        .iter()
        .find(|sc| sc.name == token || sc.aliases.iter().any(|a| a.eq_ignore_ascii_case(token)))
        .map(|sc| sc.name.clone())
}

// a user-cache set is stale when its newest file is older than the ttl. ttl 0
// disables; system-dir hits aren't in the user cache so they never go stale.
fn cache_is_stale(
    user_dir: &Path,
    found: Option<&(String, ManpageResult, usize)>,
    ttl_secs: u64,
) -> bool {
    ttl_secs > 0
        && found.is_some_and(|(name, _, _)| {
            store::user_cache_age(user_dir, name).is_some_and(|age| age.as_secs() > ttl_secs)
        })
}

fn man_dir_of_prefix(prefix: &Path) -> PathBuf {
    prefix.join("share/man")
}

// completer is pointed at `<prefix>/share/inshellah`, so manpages sit two
// levels up at `<prefix>/share/man`, the bin/share-man colocation `index`
// assumes.
fn mandir_for_completion_dir(dir: &Path) -> Option<PathBuf> {
    dir.parent().and_then(Path::parent).map(man_dir_of_prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_dir_mandir_resolves_to_prefix_share_man() {
        assert_eq!(
            mandir_for_completion_dir(Path::new("/run/current-system/sw/share/inshellah")),
            Some(PathBuf::from("/run/current-system/sw/share/man"))
        );
        assert_eq!(
            mandir_for_completion_dir(Path::new("/etc/profiles/per-user/alice/share/inshellah")),
            Some(PathBuf::from("/etc/profiles/per-user/alice/share/man"))
        );
    }
}
