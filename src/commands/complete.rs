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
        attached_directory_candidates,
        choice_candidates,
        entry_for_flag,
        generate_candidates,
        path_candidates,
    },
    config::Config,
    dynamic::{
        Answer,
        CachedFlags,
        Files,
        Reply,
        Slot,
        adapters::{
            Detection,
            Protocol,
        },
        builtin_completions,
        dynamic_complete_candidates_with_path,
        dynamic_value_completions,
        provider_kind,
        provider_signal_word,
        provider_takes_value,
    },
    executable::{
        find_in_path,
        is_executable,
    },
    indexer::resolve_command_path_and_cache,
    parsers::{
        manpage::{
            ManpageEntry,
            ManpageResult,
            OwnedSwitch,
        },
        nushell::nushell_type_of_param,
    },
    shell::{
        BASH_WORDBREAKS,
        BashLine,
        Shell,
    },
    store::{
        self,
        ExeIdentity,
        default_store_path,
        lookup,
        purge_command,
        read_exe,
        read_fact,
        recent_miss,
        subcommands_of,
        write_fact,
    },
};
use pound::Parse;

use super::{
    common::Dirs,
    wrappers::wrapped_command,
};

const ELEVATION_COMMANDS: &[&str] = &["sudo", "doas", "pkexec", "su", "run0"];

#[derive(Parse, Debug)]
pub struct Line {
    /// a bash command line up to the cursor, split here instead of SPANs
    #[pound(long, value_name = "LINE")]
    line:       Option<String>,
    /// bash's `COMP_WORDBREAKS` for --line
    #[pound(long, value_name = "CHARS")]
    wordbreaks: Option<String>,
}

impl Line {
    /// the spans to complete, and the line they were split from.
    ///
    /// # Errors
    /// fails when both --line and SPANs are given.
    pub fn spans(self, raw: Vec<String>) -> io::Result<(Vec<String>, Option<BashLine>)> {
        let Some(text) = self.line else {
            return Ok((raw, None));
        };
        if !raw.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--line and SPANs exclude each other",
            ));
        }
        let parsed = BashLine::parse(&text, self.wordbreaks.as_deref().unwrap_or(BASH_WORDBREAKS));
        Ok((parsed.words.clone(), Some(parsed)))
    }
}

/// # Errors
/// fails when both --line and SPANs are given, or stdout cannot be written.
pub fn run(
    dirs: Dirs,
    timeout_override: Option<u64>,
    shell: Shell,
    line: Line,
    raw: Vec<String>,
) -> io::Result<()> {
    let (spans, parsed) = line.spans(raw)?;
    let cfg = Config::from_env();
    let timeout_ms = timeout_override.unwrap_or(cfg.timeout_ms);
    let (user_dir, system_dirs) = default_user_dir_and_system_dirs(dirs.resolve_lenient());
    let mandirs = mandirs_for_system_dirs(&system_dirs);
    let response = if parsed.as_ref().is_some_and(|bash| bash.redirect) {
        Response::Defer
    } else {
        decide(
            &spans,
            &user_dir,
            &system_dirs,
            &mandirs,
            timeout_ms,
            shell,
            &cfg,
        )
        .response
    };
    let mut out = io::stdout().lock();
    match shell {
        Shell::Nushell => {
            serde_json::to_writer(&mut out, &response)?;
            writeln!(out)
        },
        Shell::Bash => {
            let bash = parsed.unwrap_or_default();
            let inserted = for_bash(response, &bash);
            let command = bash.words.first().map_or("", String::as_str);
            writeln!(out, "{}", inserted.lines_for(command))
        },
        Shell::Zsh | Shell::Fish => writeln!(out, "{}", response.lines()),
    }
}

/// candidates as readline inserts them, dropping ones it can't. readline
/// replaces the word with the candidates' common prefix, so a fuzzy match
/// that doesn't start with the typed word would delete what was typed.
fn for_bash(response: Response, line: &BashLine) -> Response {
    let typed = line.words.last().map_or("", String::as_str);
    let insert = |candidates: Vec<Candidate>| -> Vec<Candidate> {
        candidates
            .into_iter()
            .filter(|candidate| candidate.value.starts_with(typed))
            .filter_map(|candidate| {
                let value = line.insertion(&candidate.value)?;
                Some(Candidate::new(value, candidate.description))
            })
            .collect()
    };
    match response {
        Response::Defer => Response::Defer,
        Response::Candidates(candidates) if candidates.is_empty() => {
            Response::Candidates(candidates)
        },
        Response::WithFiles(candidates) if line.lead.is_empty() => {
            Response::WithFiles(insert(candidates))
        },
        Response::Candidates(candidates) | Response::WithFiles(candidates) => {
            Response::or_defer(insert(candidates))
        },
    }
}

/// what `complete` answers, why, and what it looked at on the way.
pub struct Decision {
    pub response: Response,
    pub reason:   Reason,
    pub trace:    Trace,
}

#[derive(Default)]
pub struct Trace {
    pub spans:    Vec<String>,
    pub bin:      Option<PathBuf>,
    pub dropped:  Vec<Stale>,
    pub dirs:     Vec<PathBuf>,
    pub path:     Vec<String>,
    pub node:     Option<String>,
    pub slot:     Option<String>,
    pub dynamic:  Option<usize>,
    pub protocol: Option<Protocol>,
}

pub enum StaleReason {
    BinaryChanged,
    OtherBuild,
}

/// a cache dir whose entries came from another binary or inshellah build.
pub struct Stale {
    pub dir:    PathBuf,
    pub stored: ExeIdentity,
    pub reason: StaleReason,
}

/// the deepest cached command for the typed path.
pub struct Node {
    name:   String,
    result: ManpageResult,
    depth:  usize,
}

pub enum Reason {
    NotOnPath,
    ValueProvider,
    FlagChoices,
    FlagPaths,
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
            Self::FlagPaths => {
                "the flag takes a path attached with =, listed here for shells that do not split \
                 there"
            },
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
    shell: Shell,
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
    let dirs = dirs_for_binary(all_dirs, user_dir, &cmd_name, &mut trace);
    trace.dirs.clone_from(&dirs);

    let builtin = builtin_provider(raw_spans, shell, timeout_ms, cfg, &mut trace);
    let provided = builtin.or_else(|| {
        let path = explicit_cmd_path.as_deref();
        value_provider(&spans, path, timeout_ms, cfg, &mut trace)
    });
    if let Some(response) = provided {
        return Decision {
            response,
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
    trace.node = found.as_ref().map(|node| node.name.clone());

    let slot = cmd_path.slot;
    trace.slot = Some(slot.to_string());
    if let Some((candidates, reason)) = flag_value_choices(found.as_ref(), slot, &last_token, cfg) {
        return Decision {
            response: Response::Candidates(candidates),
            reason,
            trace,
        };
    }

    let resolve_depth = cmd_path.depth;
    let typing_flag = slot == ArgSlot::Free && cfg.triggers_flags(&last_token);
    let fallback_subcommands = match &found {
        Some(node) if node.result.subcommands.is_empty() => subcommands_of(&dirs, &node.name),
        _ => Vec::new(),
    };
    // positional value choices (getent databases) fill the same slot as
    // subcommands, so they suppress the file/dynamic handoff too.
    let has_subs = found.as_ref().is_some_and(|node| {
        !node.result.subcommands.is_empty()
            || !node.result.positional_choices.is_empty()
            || !fallback_subcommands.is_empty()
    });
    let candidates: Vec<Candidate> = match &found {
        Some(node) if slot == ArgSlot::Free => {
            generate_candidates(
                &node.result,
                node.depth,
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
    trace.protocol = protocol_of(user_dir, &trace, timeout_ms);
    // spans are post-elevation, so `sudo nix ...` reaches this as
    // `[nix, ...]` and hits the nix branch.
    let reply = if typing_flag && !candidates.is_empty() && !provider_signal_word(&spans, shell) {
        Reply::Silent
    } else {
        dynamic_complete_candidates_with_path(
            &spans,
            explicit_cmd_path.as_deref(),
            match slot {
                ArgSlot::FlagValue(flag) => Slot::FlagValue(flag),
                ArgSlot::AfterDoubleDash => Slot::Positional,
                ArgSlot::Free => Slot::from_spans(&spans),
            },
            FlagLists::of(lookup(&dirs, &cmd_name).as_ref(), found.as_ref()).cached(),
            trace.protocol,
            shell,
            cfg,
        )
    };
    forget_foreign_protocol(user_dir, &reply, &mut trace);
    let dyn_candidates = reply.into_answer();
    trace.dynamic = dyn_candidates
        .as_ref()
        .map(|answer| answer.candidates.len());
    let (response, reason) = combine(dyn_candidates, candidates, want_files, cfg.max_completions);
    Decision {
        response,
        reason,
        trace,
    }
}

/// where the shell's own `kill` starts. `builtin` always reaches it and bash's
/// `command` does too, while zsh's `command` and every `exec` run the binary.
fn builtin_kill_at(raw_spans: &[String], shell: Shell) -> Option<usize> {
    let mut at = 0;
    while let Some(word) = raw_spans.get(at) {
        match word.as_str() {
            "kill" => return (at == 0 || at + 1 < raw_spans.len()).then_some(at),
            "builtin" => at += 1,
            "command" if shell == Shell::Bash => {
                at += 1;
                while raw_spans.get(at).is_some_and(|flag| flag == "-p") {
                    at += 1;
                }
            },
            _ => return None,
        }
    }
    None
}

/// `kill` typed as the first word is the shell's own builtin.
fn builtin_provider(
    raw_spans: &[String],
    shell: Shell,
    timeout_ms: u64,
    cfg: &Config,
    trace: &mut Trace,
) -> Option<Response> {
    let kill = builtin_kill_at(raw_spans, shell)?;
    let candidates = builtin_completions(&raw_spans[kill..], shell, timeout_ms, cfg)?;
    trace.dynamic = Some(candidates.len());
    Some(Response::or_defer(candidates))
}

fn value_provider(
    spans: &[String],
    explicit_cmd_path: Option<&Path>,
    timeout_ms: u64,
    cfg: &Config,
    trace: &mut Trace,
) -> Option<Response> {
    let (cmd_name, rest) = spans.split_first()?;
    let candidates = dynamic_value_completions(cmd_name, rest, explicit_cmd_path, timeout_ms, cfg)?;
    trace.dynamic = Some(candidates.len());
    Some(Response::or_defer(candidates))
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
fn resolve_node<'ctx>(ctx: &Resolve<'ctx>) -> (CommandPath<'ctx>, Option<Node>) {
    let mut cmd_path = command_path(ctx.dirs, None, ctx.cmd_name, ctx.rest);

    // try longest-prefix match: "git stash apply" -> "git stash" -> "git"
    let find_result = |path: &[String]| -> Option<Node> {
        (1..=path.len()).rev().find_map(|len| {
            let name = path[..len].join(" ");
            lookup(ctx.dirs, &name).map(|result| {
                Node {
                    name,
                    result,
                    depth: len,
                }
            })
        })
    };

    let mut found = find_result(&cmd_path.path);
    let exe = ctx.bin.and_then(ExeIdentity::of);
    while let Some(bin) = ctx.bin
        && (cache_is_stale(ctx.user_dir, found.as_ref(), ctx.ttl_secs)
            || found
                .as_ref()
                .is_none_or(|node| node.depth < cmd_path.path.len()))
        && !recent_miss(ctx.user_dir, &cmd_path.path.join(" "), exe.as_ref())
    {
        let ancestor = found
            .as_ref()
            .filter(|node| node.depth < cmd_path.path.len())
            .map(|node| &node.result);
        let Some(result) = resolve_command_path_and_cache(
            ctx.user_dir,
            ctx.mandirs,
            ctx.cmd_name,
            &cmd_path.path[1..],
            bin,
            ctx.timeout_ms,
            ancestor,
        )
        .result
        else {
            break;
        };
        let resolved_len = cmd_path.path.len();
        let probed = cmd_path.probe;
        found = Some(Node {
            name: cmd_path.path.join(" "),
            result,
            depth: resolved_len,
        });
        let fresh = found
            .as_ref()
            .map(|node| (node.name.as_str(), &node.result));
        cmd_path = command_path(ctx.dirs, fresh, ctx.cmd_name, ctx.rest);
        // a probe's --help can echo the parent's, which caches it as a group
        // and would license another probe one word deeper.
        if probed || cmd_path.path.len() <= resolved_len {
            break;
        }
        if let Some(hit) = find_result(&cmd_path.path)
            && hit.depth > resolved_len
        {
            found = Some(hit);
        }
    }
    (cmd_path, found)
}

/// dirs matching the traced binary, purging stale user-cache entries. system
/// dirs are rebuilt by nix, so only the user cache cares which inshellah wrote
/// an entry.
fn dirs_for_binary(
    dirs: Vec<PathBuf>,
    user_dir: &Path,
    cmd_name: &str,
    trace: &mut Trace,
) -> Vec<PathBuf> {
    let Some(identity) = trace.bin.as_deref().and_then(ExeIdentity::of) else {
        return dirs;
    };
    dirs.into_iter()
        .filter(|dir| {
            let writable = dir == user_dir;
            let why_stale = |stored: &ExeIdentity| {
                if !stored.same_binary(&identity) {
                    Some(StaleReason::BinaryChanged)
                } else if writable && *stored != identity {
                    Some(StaleReason::OtherBuild)
                } else {
                    None
                }
            };
            let Some((stored, reason)) = read_exe(dir, cmd_name)
                .and_then(|stored| why_stale(&stored).map(|why| (stored, why)))
            else {
                return true;
            };
            trace.dropped.push(Stale {
                dir: dir.clone(),
                stored,
                reason,
            });
            writable && purge_command(dir, cmd_name).is_ok()
        })
        .collect()
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

/// skip past wrappers (sudo, doas, env, nice, ...) to the real command.
fn strip_elevation(spans: &[String], dirs: &[PathBuf]) -> (Vec<String>, Option<PathBuf>) {
    let mut real_spans = spans.to_vec();
    while let Some(at) = wrapped_command_index(&real_spans, dirs) {
        real_spans.drain(..at);
    }
    let explicit = real_spans.first().and_then(|first| {
        let path = executable_span_path(first)?;
        Some((command_name_for_path(&path)?, path))
    });
    match explicit {
        Some((name, path)) => {
            real_spans[0] = name;
            (real_spans, Some(path))
        },
        None => (real_spans, None),
    }
}

fn wrapped_command_index(spans: &[String], dirs: &[PathBuf]) -> Option<usize> {
    let (first, rest) = spans.split_first()?;
    if !ELEVATION_COMMANDS.contains(&first.as_str()) {
        return wrapped_command(spans);
    }
    rest.iter()
        .position(|span| {
            (executable_span_path(span).is_some_and(|path| command_name_for_path(&path).is_some()))
                || (!span.is_empty()
                    && !span.starts_with('-')
                    && (lookup(dirs, span).is_some() || find_in_path(span).is_some()))
        })
        .map(|idx| idx + 1)
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
    provided: Option<Answer>,
    candidates: Vec<Candidate>,
    want_files: bool,
    max_completions: usize,
) -> (Response, Reason) {
    let Some(answer) = provided else {
        return if want_files {
            (Response::Defer, Reason::Files)
        } else if candidates.is_empty() {
            (Response::Defer, Reason::Nothing)
        } else {
            (Response::Candidates(candidates), Reason::Static)
        };
    };

    if want_files {
        return match answer.files {
            Files::Alongside => {
                (
                    Response::WithFiles(answer.candidates),
                    Reason::ProviderWithFiles,
                )
            },
            Files::Replace | Files::Never => {
                (
                    Response::Candidates(answer.candidates),
                    Reason::ProviderOverFiles,
                )
            },
        };
    }
    if candidates.is_empty() {
        return (Response::Candidates(answer.candidates), Reason::Provider);
    }
    let merged = merge_completion_candidates(answer.candidates, candidates, max_completions);
    (Response::Candidates(merged), Reason::Merged)
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

/// value choices for `--flag val` and `--flag=val`, and the paths of a path
/// flag typed as `--flag=val`.
fn flag_value_choices(
    found: Option<&Node>,
    slot: ArgSlot<'_>,
    last_token: &str,
    cfg: &Config,
) -> Option<(Vec<Candidate>, Reason)> {
    let (flag, typed, prefix) = match slot {
        ArgSlot::FlagValue(flag) => (flag, last_token, String::new()),
        ArgSlot::Free if last_token.starts_with("--") => {
            let (flag, typed) = last_token.split_once('=')?;
            (flag, typed, format!("{flag}="))
        },
        ArgSlot::Free | ArgSlot::AfterDoubleDash => return None,
    };
    let param = entry_for_flag(&found?.result, flag)?.param.as_ref()?;
    let takes_path = nushell_type_of_param(&param.name) == "path";
    if param.choices.is_empty() {
        if !takes_path || prefix.is_empty() || typed.starts_with(['~', '$']) {
            return None;
        }
        let paths = if matches!(param.name.as_str(), "DIR" | "dir" | "DIRECTORY") {
            attached_directory_candidates(typed, &prefix, cfg)
        } else {
            path_candidates(typed, &prefix, cfg)
        };
        return (!paths.is_empty()).then_some((paths, Reason::FlagPaths));
    }
    let candidates = choice_candidates(&param.choices, typed, &prefix, cfg);
    (!candidates.is_empty() || !takes_path).then_some((candidates, Reason::FlagChoices))
}

/// the protocol a provider-less traced binary answers, scanned once and cached.
fn protocol_of(user_dir: &Path, trace: &Trace, timeout_ms: u64) -> Option<Protocol> {
    let cmd_name = trace.spans.first()?;
    if provider_kind(cmd_name).is_some() {
        return None;
    }
    let bin = trace.bin.as_deref()?;
    let exe = ExeIdentity::of(bin)?;
    if let Some(Detection(cached)) = read_fact(user_dir, cmd_name, &exe) {
        return cached;
    }
    let detection = Detection::scan(bin, timeout_ms)?;
    // an uncached scan would reread a large binary on every Tab.
    write_fact(user_dir, cmd_name, &exe, &detection)
        .ok()
        .and(detection.0)
}

/// stop asking a binary that linked the library but ignored its protocol.
fn forget_foreign_protocol(user_dir: &Path, reply: &Reply, trace: &mut Trace) {
    let (Reply::Foreign, Some(cmd_name), Some(exe)) = (
        reply,
        trace.spans.first(),
        trace.bin.as_deref().and_then(ExeIdentity::of),
    ) else {
        return;
    };
    if write_fact(user_dir, cmd_name, &exe, &Detection(None)).is_ok() {
        trace.protocol = None;
    }
}

/// value-taking flags of the root command or the resolved subcommand.
fn value_flags(result: Option<&ManpageResult>) -> Vec<String> {
    result
        .into_iter()
        .flat_map(|found| &found.entries)
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

struct FlagLists {
    global_value: Vec<String>,
    value:        Vec<String>,
    global_short: Vec<String>,
    short:        Vec<String>,
}

impl FlagLists {
    fn of(root: Option<&ManpageResult>, found: Option<&Node>) -> Self {
        let sub = found.filter(|node| node.depth > 1).map(|node| &node.result);
        Self {
            global_value: value_flags(root),
            value:        value_flags(sub),
            global_short: short_flags(root),
            short:        short_flags(sub),
        }
    }

    fn cached(&self) -> CachedFlags<'_> {
        CachedFlags {
            global_value: &self.global_value,
            value:        &self.value,
            global_short: &self.global_short,
            short:        &self.short,
        }
    }
}

/// every short switch of the command, so a cluster of them can be split.
fn short_flags(result: Option<&ManpageResult>) -> Vec<String> {
    result
        .into_iter()
        .flat_map(|found| &found.entries)
        .filter_map(|entry| {
            match &entry.switch {
                OwnedSwitch::Short(short) | OwnedSwitch::Both(short, _) => {
                    Some(format!("-{short}"))
                },
                OwnedSwitch::Long(_) => None,
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

impl Display for ArgSlot<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Free => f.write_str("free"),
            Self::FlagValue(flag) => write!(f, "value of {flag}"),
            Self::AfterDoubleDash => f.write_str("after --"),
        }
    }
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
                || (!token.contains('=') && provider_takes_value(cmd_name, token, depth == 1))
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
fn cache_is_stale(user_dir: &Path, found: Option<&Node>, ttl_secs: u64) -> bool {
    ttl_secs > 0
        && found.is_some_and(|node| {
            store::user_cache_age(user_dir, &node.name).is_some_and(|age| age.as_secs() > ttl_secs)
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
