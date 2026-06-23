// SPDX-License-Identifier: EUPL-1.2
//! `inshellah complete`.

use std::collections::HashSet;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use inshellah::complete::{Candidate, generate_candidates};
use inshellah::config::Config;
use inshellah::dynamic::{
    dynamic_complete_candidates_with_path, dynamic_complete_with_path, dynamic_value_completions,
};
use inshellah::indexer::{is_executable, resolve_and_cache, resolve_command_path_and_cache};
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
    let mut lookup_tokens = lookup_path_tokens(&dirs, &cmd_name, complete_rest);
    if last_token.is_empty()
        && !rest.is_empty()
        && !lookup_tokens.last().is_some_and(|t| t.is_empty())
    {
        lookup_tokens.push(String::new());
    }

    // try longest-prefix match: "git stash apply" -> "git stash" -> "git"
    let find_result = |toks: &[String]| -> Option<(String, ManpageResult, usize)> {
        let n = toks.len();
        for drop in 0..n {
            let prefix = &toks[..n - drop];
            if prefix.is_empty() {
                continue;
            }
            let name = prefix.join(" ");
            if let Some(r) = lookup(&dirs, &name) {
                return Some((name, r, prefix.len()));
            }
        }
        None
    };

    let mut found = find_result(&lookup_tokens);

    // nothing matched or only a parent matched, so try --help.
    let resolve_tokens: Vec<String> = lookup_tokens
        .iter()
        .filter(|t| !t.is_empty())
        .cloned()
        .collect();
    let resolve_depth = resolve_tokens.len();
    // a stale hit re-resolves through the same path: the resolve block
    // overwrites the cache and falls back to the stale value if rescrape fails.
    let need_resolve = cache_is_stale(user_dir, found.as_ref(), cfg.cache_ttl_secs)
        || match &found {
            Some((_, _, depth)) => *depth < resolve_depth,
            None => resolve_depth > 0,
        };
    if need_resolve
        && let Some(path) = explicit_cmd_path
            .as_ref()
            .cloned()
            .or_else(|| find_in_path(&cmd_name))
    {
        // also search the binary's own prefix for manpages.
        let mut all_mandirs = mandirs.to_vec();
        if let Some(parent) = path.parent()
            && let Some(prefix) = parent.parent()
        {
            let share_man = prefix.join("share/man");
            if share_man.is_dir() {
                all_mandirs.push(share_man);
            }
        }
        let sub_args = if resolve_tokens.len() > 1 {
            resolve_tokens[1..].to_vec()
        } else {
            Vec::new()
        };
        let resolved = if sub_args.is_empty() {
            resolve_and_cache(user_dir, &all_mandirs, &cmd_name, &path, timeout_ms)
        } else {
            resolve_command_path_and_cache(
                user_dir,
                &all_mandirs,
                &cmd_name,
                &sub_args,
                &path,
                timeout_ms,
            )
        };
        if resolved.is_some() {
            found = find_result(&lookup_tokens);
        }
    }

    let typing_flag = cfg.triggers_flags(&last_token);
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
        None => Vec::new(),
        Some((_, r, depth)) => generate_candidates(
            r,
            *depth,
            resolve_depth,
            &last_token,
            &fallback_subcommands,
            typing_flag,
            cfg,
        ),
    };
    // hand off at non-flag leaf positions so file and dynamic completers can
    // answer argument prefixes. a leading "-" keeps flags.
    let want_files = !typing_flag && !has_subs && (last_token.is_empty() || candidates.is_empty());
    if !typing_flag
        && let Some(dyn_candidates) =
            dynamic_complete_candidates_with_path(&spans, explicit_cmd_path.as_deref(), cfg)
    {
        let combined = if candidates.is_empty() || want_files {
            dyn_candidates
        } else {
            merge_completion_candidates(dyn_candidates, candidates, cfg.max_completions)
        };
        print_completion_candidate_values(combined);
    } else if want_files || candidates.is_empty() {
        // spans are post-elevation, so `sudo nix ...` reaches this as
        // `[nix, ...]` and hits the nix branch.
        if let Some(dyn_candidates) =
            dynamic_complete_with_path(&spans, explicit_cmd_path.as_deref(), cfg)
        {
            print_completion_candidates(&dyn_candidates);
        } else {
            println!("null");
        }
    } else {
        print_completion_candidate_values(candidates);
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
    let path = PathBuf::from(span);
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

fn lookup_path_tokens(dirs: &[PathBuf], cmd_name: &str, rest: &[String]) -> Vec<String> {
    let mut tokens = vec![cmd_name.to_string()];
    let mut current = lookup(dirs, cmd_name);
    let mut skip_next_value = false;

    for token in rest {
        if token.is_empty() {
            tokens.push(token.clone());
            continue;
        }
        if skip_next_value {
            skip_next_value = false;
            continue;
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

        // map an alias to its canonical child name (cargo `b` -> build) so the
        // path resolves to the real node and descends into its flags/subs.
        let resolved = current
            .as_ref()
            .and_then(|r| canonical_for_alias(r, token))
            .unwrap_or_else(|| token.clone());
        tokens.push(resolved);
        let name = tokens
            .iter()
            .filter(|t| !t.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        current = lookup(dirs, &name).or(current);
    }

    tokens
}

/// canonical child name for a token that matches one of `result`'s subcommand
/// aliases (not its name), else None.
fn canonical_for_alias(result: &ManpageResult, token: &str) -> Option<String> {
    result.subcommands.iter().find_map(|sc| {
        sc.aliases
            .iter()
            .any(|a| a.eq_ignore_ascii_case(token))
            .then(|| sc.name.clone())
    })
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
