// SPDX-License-Identifier: EUPL-1.2
//! `inshellah complete`.

use std::{
    borrow::Cow,
    collections::HashSet,
    fmt::{
        self,
        Display,
        Formatter,
    },
    io::{
        self,
        Write as _,
    },
    path::{
        Path,
        PathBuf,
        absolute,
    },
};

use inshellah::{
    complete::{
        Candidate,
        Response,
        choice_candidates,
        entry_for_flag,
        generate_candidates,
    },
    config::Config,
    dynamic::{
        Files,
        Slot,
        dynamic_complete_candidates_with_path,
        dynamic_value_completions,
    },
    indexer::{
        probe::is_executable,
        resolve_command_path_and_cache,
    },
    parsers::{
        manpage::{
            ManpageEntry,
            ManpageResult,
            OwnedSwitch,
        },
        nushell::nushell_type_of_param,
    },
    store::{
        self,
        ExeIdentity,
        default_store_path,
        lookup,
        purge_command,
        read_exe,
        recent_miss,
        subcommands_of,
    },
};

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
    let decision = decide(raw_spans, user_dir, system_dirs, mandirs, timeout_ms, cfg);
    writeln!(io::stdout().lock(), "{}", decision.response)
}

/// what `complete` answers, why, and what it looked at on the way.
pub struct Decision {
    pub response: Response,
    pub reason:   Reason,
    pub trace:    Trace,
}

#[derive(Default)]
pub struct Trace {
    pub spans:   Vec<String>,
    pub bin:     Option<PathBuf>,
    pub dropped: Vec<(PathBuf, ExeIdentity)>,
    pub path:    Vec<String>,
    pub node:    Option<String>,
    pub slot:    Option<String>,
    pub dynamic: Option<usize>,
}

pub enum Reason {
    NotOnPath,
    ValueProvider,
    FlagChoices,
    ProviderWithFiles,
    Provider,
    ProviderOverFiles,
    Merged,
    Files,
    Static,
    Nothing,
}

impl Display for Reason {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotOnPath => "the command is not on PATH",
            Self::ValueProvider => "a live provider answers this command before the cache",
            Self::FlagChoices => "the flag takes one of a known set of values",
            Self::ProviderWithFiles => "provider candidates sit beside nushell's file completion",
            Self::Provider => "the cache had nothing here, a live provider answered",
            Self::ProviderOverFiles => "a live provider answered a file-like position",
            Self::Merged => "cache and provider candidates merged",
            Self::Files => "file-like position with no candidates, nushell completes files",
            Self::Static => "candidates from the cache",
            Self::Nothing => "nothing matched, nushell decides",
        })
    }
}

pub fn decide(
    raw_spans: &[String],
    user_dir: &Path,
    system_dirs: &[PathBuf],
    mandirs: &[PathBuf],
    timeout_ms: u64,
    cfg: &Config,
) -> Decision {
    let mut all_dirs: Vec<PathBuf> = Vec::with_capacity(system_dirs.len() + 1);
    all_dirs.push(user_dir.to_path_buf());
    all_dirs.extend(system_dirs.iter().cloned());

    let (spans, explicit_cmd_path) = strip_elevation(raw_spans, &all_dirs);
    let mut trace = Trace {
        spans: spans.clone(),
        ..Trace::default()
    };
    if spans.is_empty() || (explicit_cmd_path.is_none() && find_in_path(&spans[0]).is_none()) {
        return Decision {
            response: Response::Defer,
            reason: Reason::NotOnPath,
            trace,
        };
    }

    let cmd_name = spans[0].clone();
    let rest: Vec<String> = spans[1..].to_vec();
    trace.bin = explicit_cmd_path
        .clone()
        .or_else(|| find_in_path(&cmd_name));
    let (dirs, dropped) = dirs_for_binary(all_dirs, user_dir, &cmd_name, trace.bin.as_deref());
    trace.dropped = dropped;

    if let Some(candidates) =
        dynamic_value_completions(&cmd_name, &rest, explicit_cmd_path.as_deref(), timeout_ms)
    {
        trace.dynamic = Some(candidates.len());
        return Decision {
            response: Response::or_defer(candidates),
            reason: Reason::ValueProvider,
            trace,
        };
    }

    let last_token = rest.last().cloned().unwrap_or_default();
    let complete_rest: &[String] = if last_token.is_empty() || rest.is_empty() {
        &rest
    } else {
        &rest[..rest.len() - 1]
    };
    let all_mandirs = mandirs_with_bin_prefix(mandirs, trace.bin.as_deref());
    let (cmd_path, found) = resolve_node(&Resolve {
        dirs: &dirs,
        user_dir,
        mandirs: &all_mandirs,
        cmd_name: &cmd_name,
        rest: complete_rest,
        bin: trace.bin.as_deref(),
        timeout_ms,
        ttl_secs: cfg.cache_ttl_secs,
    });
    trace.path.clone_from(&cmd_path.path);
    trace.node = found.as_ref().map(|(name, ..)| name.clone());

    let slot = cmd_path.slot;
    trace.slot = Some(match slot {
        ArgSlot::Free => "free".to_owned(),
        ArgSlot::FlagValue(flag) => format!("value of {flag}"),
        ArgSlot::AfterDoubleDash => "after --".to_owned(),
    });
    if let Some(candidates) = flag_value_choices(found.as_ref(), slot, &last_token, cfg) {
        return Decision {
            response: Response::Candidates(candidates),
            reason: Reason::FlagChoices,
            trace,
        };
    }

    let resolve_depth = cmd_path.depth;
    let typing_flag = slot == ArgSlot::Free && cfg.triggers_flags(&last_token);
    let fallback_subcommands = match &found {
        Some((matched_name, result, _)) if result.subcommands.is_empty() => {
            subcommands_of(&dirs, matched_name)
        },
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
        Some((_, result, depth)) if slot == ArgSlot::Free => {
            generate_candidates(
                result,
                *depth,
                resolve_depth,
                &last_token,
                &fallback_subcommands,
                typing_flag,
                cfg,
            )
        },
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
                &value_flags(&dirs, &cmd_name, found.as_ref()),
                cfg,
            )
        })
        .flatten();
    trace.dynamic = dyn_candidates.as_ref().map(|(dynamic, _)| dynamic.len());
    let (response, reason) = combine(dyn_candidates, candidates, want_files, cfg.max_completions);
    Decision {
        response,
        reason,
        trace,
    }
}

struct Resolve<'ctx> {
    dirs:       &'ctx [PathBuf],
    user_dir:   &'ctx Path,
    mandirs:    &'ctx [PathBuf],
    cmd_name:   &'ctx str,
    rest:       &'ctx [String],
    bin:        Option<&'ctx Path>,
    timeout_ms: u64,
    ttl_secs:   u64,
}

/// the deepest cached node for the typed path, resolving missing or stale ones.
fn resolve_node<'ctx>(
    ctx: &Resolve<'ctx>,
) -> (CommandPath<'ctx>, Option<(String, ManpageResult, usize)>) {
    let mut cmd_path = command_path(ctx.dirs, None, ctx.cmd_name, ctx.rest);

    // try longest-prefix match: "git stash apply" -> "git stash" -> "git"
    let find_result = |path: &[String]| -> Option<(String, ManpageResult, usize)> {
        (1..=path.len()).rev().find_map(|len| {
            let name = path[..len].join(" ");
            lookup(ctx.dirs, &name).map(|result| (name, result, len))
        })
    };

    let mut found = find_result(&cmd_path.path);
    let exe = ctx.bin.and_then(ExeIdentity::of);
    while let Some(bin) = ctx.bin
        && (cache_is_stale(ctx.user_dir, found.as_ref(), ctx.ttl_secs)
            || found
                .as_ref()
                .is_none_or(|(_, _, depth)| *depth < cmd_path.path.len()))
        && !recent_miss(ctx.user_dir, &cmd_path.path.join(" "), exe.as_ref())
    {
        let Some(result) = resolve_command_path_and_cache(
            ctx.user_dir,
            ctx.mandirs,
            ctx.cmd_name,
            &cmd_path.path[1..],
            bin,
            ctx.timeout_ms,
        )
        .result
        else {
            break;
        };
        let resolved_len = cmd_path.path.len();
        let probed = cmd_path.probe;
        found = Some((cmd_path.path.join(" "), result, resolved_len));
        let fresh = found
            .as_ref()
            .map(|(name, cached, _)| (name.as_str(), cached));
        cmd_path = command_path(ctx.dirs, fresh, ctx.cmd_name, ctx.rest);
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
    (cmd_path, found)
}

/// dirs whose `cmd_name` entries match `bin`, purging stale user-cache ones.
fn dirs_for_binary(
    dirs: Vec<PathBuf>,
    user_dir: &Path,
    cmd_name: &str,
    bin: Option<&Path>,
) -> (Vec<PathBuf>, Vec<(PathBuf, ExeIdentity)>) {
    let Some(identity) = bin.and_then(ExeIdentity::of) else {
        return (dirs, Vec::new());
    };
    let mut dropped = Vec::new();
    let kept = dirs
        .into_iter()
        .filter(|dir| {
            let Some(stored) = read_exe(dir, cmd_name).filter(|stored| *stored != identity) else {
                return true;
            };
            dropped.push((dir.clone(), stored));
            dir == user_dir && purge_command(dir, cmd_name).is_ok()
        })
        .collect();
    (kept, dropped)
}

/// also search the binary's own prefix for manpages.
pub fn mandirs_with_bin_prefix(mandirs: &[PathBuf], bin_path: Option<&Path>) -> Vec<PathBuf> {
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
        },
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

fn combine(
    answer: Option<(Vec<Candidate>, Files)>,
    candidates: Vec<Candidate>,
    want_files: bool,
    max_completions: usize,
) -> (Response, Reason) {
    match answer {
        Some((dynamic, Files::Alongside)) if want_files => {
            (Response::WithFiles(dynamic), Reason::ProviderWithFiles)
        },
        Some((dynamic, _)) if want_files => {
            (Response::Candidates(dynamic), Reason::ProviderOverFiles)
        },
        Some((dynamic, _)) if candidates.is_empty() => {
            (Response::Candidates(dynamic), Reason::Provider)
        },
        Some((dynamic, _)) => {
            let merged = merge_completion_candidates(dynamic, candidates, max_completions);
            (Response::Candidates(merged), Reason::Merged)
        },
        None if want_files => (Response::Defer, Reason::Files),
        None if candidates.is_empty() => (Response::Defer, Reason::Nothing),
        None => (Response::Candidates(candidates), Reason::Static),
    }
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

/// value choices for `--flag val` and `--flag=val`.
fn flag_value_choices(
    found: Option<&(String, ManpageResult, usize)>,
    slot: ArgSlot<'_>,
    last_token: &str,
    cfg: &Config,
) -> Option<Vec<Candidate>> {
    let (flag, typed, prefix) = match slot {
        ArgSlot::FlagValue(flag) => (flag, last_token, String::new()),
        ArgSlot::Free if last_token.starts_with("--") => {
            let (flag, typed) = last_token.split_once('=')?;
            (flag, typed, format!("{flag}="))
        },
        ArgSlot::Free | ArgSlot::AfterDoubleDash => return None,
    };
    let (_, result, _) = found?;
    let param = entry_for_flag(result, flag)?.param.as_ref()?;
    if param.choices.is_empty() {
        return None;
    }
    let candidates = choice_candidates(&param.choices, typed, &prefix, cfg);
    let takes_path = nushell_type_of_param(&param.name) == "path";
    (!candidates.is_empty() || !takes_path).then_some(candidates)
}

/// value-taking flags of the root command and the resolved subcommand.
fn value_flags(
    dirs: &[PathBuf],
    cmd_name: &str,
    found: Option<&(String, ManpageResult, usize)>,
) -> Vec<String> {
    let root = lookup(dirs, cmd_name);
    let nodes = root.iter().chain(found.map(|(_, result, _)| result));
    nodes
        .flat_map(|result| &result.entries)
        .filter(|entry| entry.takes_next_word())
        .flat_map(|entry| {
            match &entry.switch {
                OwnedSwitch::Short(short) => vec![format!("-{short}")],
                OwnedSwitch::Long(long) => vec![format!("--{long}")],
                OwnedSwitch::Both(short, long) => vec![format!("-{short}"), format!("--{long}")],
            }
        })
        .collect()
}

fn switch_takes_value(result: &ManpageResult, token: &str) -> bool {
    !token.contains('=') && entry_for_flag(result, token).is_some_and(ManpageEntry::takes_next_word)
}

/// the slot the token under the cursor fills.
#[derive(Clone, Copy, PartialEq)]
enum ArgSlot<'rest> {
    Free,
    FlagValue(&'rest str),
    AfterDoubleDash,
}

struct CommandPath<'rest> {
    path:  Vec<String>,
    depth: usize,
    slot:  ArgSlot<'rest>,
    probe: bool,
}

/// `fresh` preserves flag metadata when caching fails.
fn command_path<'rest>(
    dirs: &[PathBuf],
    fresh: Option<(&str, &ManpageResult)>,
    cmd_name: &str,
    rest: &'rest [String],
) -> CommandPath<'rest> {
    let node = |name: &str| {
        match fresh {
            Some((fresh_name, result)) if fresh_name == name => Some(Cow::Borrowed(result)),
            _ => lookup(dirs, name).map(Cow::Owned),
        }
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
        && found.is_some_and(|(name, ..)| {
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
