// SPDX-License-Identifier: EUPL-1.2
//! `inshellah complete`.

use std::borrow::Cow;
use std::collections::HashSet;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf, absolute};

use inshellah::complete::{Candidate, generate_candidates};
use inshellah::config::Config;
use inshellah::dynamic::{Slot, dynamic_complete_candidates_with_path, dynamic_value_completions};
use inshellah::indexer::probe::is_executable;
use inshellah::indexer::resolve_command_path_and_cache;
use inshellah::parsers::manpage::{ManpageResult, OwnedSwitch};
use inshellah::store::{self, default_store_path, lookup, subcommands_of};

use super::common::find_in_path;

const ELEVATION_COMMANDS: &[&str] = &["sudo", "doas", "pkexec", "su", "run0"];

pub fn run(
    raw_spans: &[String],
    user_dir: &Path,
    system_dirs: &[PathBuf],
    mandirs: &[PathBuf],
    timeout_ms: u64,
    cfg: &Config,
) -> io::Result<()> {
    let mut dirs: Vec<PathBuf> = Vec::with_capacity(system_dirs.len() + 1);
    dirs.push(user_dir.to_path_buf());
    dirs.extend(system_dirs.iter().cloned());

    let (spans, explicit_cmd_path) = strip_elevation(raw_spans, &dirs);
    if spans.is_empty() || (explicit_cmd_path.is_none() && find_in_path(&spans[0]).is_none()) {
        println!("null");
        return Ok(());
    }

    let cmd_name = spans[0].clone();
    let rest: Vec<String> = spans[1..].to_vec();

    if let Some(candidates) =
        dynamic_value_completions(&cmd_name, &rest, explicit_cmd_path.as_deref(), timeout_ms)
    {
        return print_completion_candidates(&candidates);
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
            lookup(&dirs, &name).map(|result| (name, result, len))
        })
    };

    let mut found = find_result(&cmd_path.path);

    let bin_path = explicit_cmd_path
        .clone()
        .or_else(|| find_in_path(&cmd_name));
    let all_mandirs = mandirs_with_bin_prefix(mandirs, bin_path.as_deref());

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
        let fresh = found
            .as_ref()
            .map(|(name, cached, _)| (name.as_str(), cached));
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
        Some((matched_name, result, _)) if result.subcommands.is_empty() => {
            subcommands_of(&dirs, matched_name)
        }
        _ => Vec::new(),
    };
    // positional value choices (getent databases) fill the same slot as
    // subcommands, so they suppress the file/dynamic handoff too.
    let has_subs = found.as_ref().is_some_and(|(_, result, _)| {
        !result.subcommands.is_empty()
            || !result.positional_choices.is_empty()
            || !fallback_subcommands.is_empty()
    });
    let candidates: Vec<Candidate> = match &found {
        Some((_, result, depth)) if slot == ArgSlot::Free => generate_candidates(
            result,
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
        .then(|| {
            dynamic_complete_candidates_with_path(
                &spans,
                explicit_cmd_path.as_deref(),
                match slot {
                    ArgSlot::FlagValue(flag) => Slot::FlagValue(flag),
                    ArgSlot::AfterDoubleDash => Slot::Positional,
                    ArgSlot::Free => Slot::from_spans(&spans),
                },
                cfg,
            )
        })
        .flatten();
    let output = match dyn_candidates {
        Some(dynamic) if candidates.is_empty() || want_files => dynamic,
        Some(dynamic) => merge_completion_candidates(dynamic, candidates, cfg.max_completions),
        None if want_files => Vec::new(),
        None => candidates,
    };
    print_completion_candidate_values(output)
}

/// also search the binary's own prefix for manpages.
fn mandirs_with_bin_prefix(mandirs: &[PathBuf], bin_path: Option<&Path>) -> Vec<PathBuf> {
    let mut all_mandirs = mandirs.to_vec();
    if let Some(prefix) = bin_path.and_then(Path::parent).and_then(Path::parent) {
        let share_man = prefix.join("share/man");
        if share_man.is_dir() {
            all_mandirs.push(share_man);
        }
    }
    all_mandirs
}

/// skip past elevation wrappers (sudo, doas) to the real command.
fn strip_elevation(spans: &[String], dirs: &[PathBuf]) -> (Vec<String>, Option<PathBuf>) {
    let mut explicit_cmd_path: Option<PathBuf> = None;
    let mut real_spans: Vec<String> = match spans.split_first() {
        Some((first, rest)) if ELEVATION_COMMANDS.contains(&first.as_str()) => {
            let mut target_spans = None;
            for (idx, span) in rest.iter().enumerate() {
                if let Some(path) = executable_span_path(span)
                    && let Some(name) = command_name_for_path(&path)
                {
                    let mut target = rest[idx..].to_vec();
                    target[0] = name;
                    explicit_cmd_path = Some(path);
                    target_spans = Some(target);
                    break;
                }
                if !span.is_empty()
                    && !span.starts_with('-')
                    && (lookup(dirs, span).is_some() || find_in_path(span).is_some())
                {
                    target_spans = Some(rest[idx..].to_vec());
                    break;
                }
            }
            target_spans.unwrap_or_else(|| spans.to_vec())
        }
        _ => spans.to_vec(),
    };
    if explicit_cmd_path.is_none()
        && let Some(first) = real_spans.first()
        && let Some(path) = executable_span_path(first)
        && let Some(name) = command_name_for_path(&path)
    {
        real_spans[0] = name;
        explicit_cmd_path = Some(path);
    }
    (real_spans, explicit_cmd_path)
}

pub fn mandirs_for_system_dirs(system_dirs: &[PathBuf]) -> Vec<PathBuf> {
    system_dirs
        .iter()
        .filter_map(|dir| mandir_for_completion_dir(dir))
        .filter(|mandir| mandir.is_dir())
        .collect()
}

pub fn default_user_dir_and_system_dirs(dirs: Vec<PathBuf>) -> (PathBuf, Vec<PathBuf>) {
    let mut iter = dirs.into_iter();
    iter.next().map_or_else(
        || (default_store_path(), Vec::new()),
        |first| (first, iter.collect()),
    )
}

fn executable_span_path(span: &str) -> Option<PathBuf> {
    if !span.contains('/') {
        return None;
    }
    // the scrape runner's cwd is /tmp, so a relative path would resolve there.
    let path = absolute(span).ok()?;
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

fn print_completion_candidate_values(candidates: Vec<Candidate>) -> io::Result<()> {
    let json: Vec<String> = candidates.into_iter().map(Candidate::into_json).collect();
    print_completion_candidates(&json)
}

// `null` is nushell's no-match form.
fn print_completion_candidates(candidates: &[String]) -> io::Result<()> {
    if candidates.is_empty() {
        println!("null");
        return Ok(());
    }
    let mut out = io::stdout().lock();
    out.write_all(b"[")?;
    for (idx, candidate) in candidates.iter().enumerate() {
        if idx > 0 {
            out.write_all(b",")?;
        }
        out.write_all(candidate.as_bytes())?;
    }
    out.write_all(b"]\n")
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
                    (Some('-'), Some(flag_char), None) if flag_char == *short
                )
            }
            OwnedSwitch::Both(short, long) => {
                token == format!("--{long}") || {
                    let mut chars = token.chars();
                    matches!(
                        (chars.next(), chars.next(), chars.next()),
                        (Some('-'), Some(flag_char), None) if flag_char == *short
                    )
                }
            }
        }
    })
}

/// the slot the token under the cursor fills.
#[derive(Clone, Copy, PartialEq)]
enum ArgSlot<'rest> {
    Free,
    FlagValue(&'rest str),
    AfterDoubleDash,
}

struct CommandPath<'rest> {
    path: Vec<String>,
    depth: usize,
    slot: ArgSlot<'rest>,
    probe: bool,
}

/// `fresh` preserves flag metadata when caching fails.
fn command_path<'rest>(
    dirs: &[PathBuf],
    fresh: Option<(&str, &ManpageResult)>,
    cmd_name: &str,
    rest: &'rest [String],
) -> CommandPath<'rest> {
    let node = |name: &str| match fresh {
        Some((fresh_name, result)) if fresh_name == name => Some(Cow::Borrowed(result)),
        _ => lookup(dirs, name).map(Cow::Owned),
    };
    let mut path = vec![cmd_name.to_owned()];
    let mut depth = 1;
    let mut current = node(cmd_name);
    let mut current_is_exact = current.is_some();
    let mut on_path = true;
    let mut probe = false;
    let mut value_of = None;

    for token in rest {
        if token.is_empty() {
            continue;
        }
        if value_of.take().is_some() {
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
                value_of = Some(token.as_str());
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
            .filter(|result| current_is_exact && !result.subcommands.is_empty());
        let in_group = group.is_some();
        let listed = group.and_then(|result| listed_subcommand(result, token));
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

    CommandPath {
        path,
        depth,
        slot: value_of.map_or(ArgSlot::Free, ArgSlot::FlagValue),
        probe,
    }
}

/// canonical child name for a token that names one of `result`'s subcommands
/// or their aliases, else None.
fn listed_subcommand(result: &ManpageResult, token: &str) -> Option<String> {
    result
        .subcommands
        .iter()
        .find(|sc| {
            sc.name == token
                || sc
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(token))
        })
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
