// SPDX-License-Identifier: EUPL-1.2
use std::{
    borrow::Cow,
    env,
    fs,
    path::{
        Path,
        PathBuf,
    },
    process::Command,
    time::Instant,
};

// the typed candidate and scorer are shared with the static completer.
use crate::complete::{
    Candidate,
    fuzzy_score,
};
use crate::{
    dynamic::{
        CachedFlags,
        Slot,
    },
    subprocess::{
        self,
        Run,
    },
};

#[derive(Clone, Copy)]
pub enum Matching {
    Value,
    ValueOrDescription,
}

#[derive(Clone, Copy)]
pub struct DynCtx<'span> {
    pub deadline:          Option<Instant>,
    pub limit:             usize,
    pub cmd_name:          &'span str,
    pub explicit_cmd_path: Option<&'span Path>,
    pub slot:              Slot<'span>,
    pub flags:             CachedFlags<'span>,
}

/// the words before the cursor, split into positionals and flags.
pub struct Words<'span> {
    spans:       &'span [String],
    positionals: Vec<&'span str>,
    flags:       Vec<(Cow<'span, str>, Option<&'span str>)>,
    /// how many of `flags` came before the first positional.
    leading:     usize,
    /// the last word before the cursor when it is a flag still owed a value.
    pending:     Option<Cow<'span, str>>,
    past_dashes: bool,
}

impl<'span> Words<'span> {
    /// `takes_value(flag, leading)` says whether `flag` consumes the next word,
    /// where `leading` is true before the first positional. `is_switch` says
    /// whether a short flag is known at all, which lets a cluster be split.
    pub fn parse<TakesValue, IsSwitch>(
        spans: &'span [String],
        takes_value: TakesValue,
        is_switch: IsSwitch,
    ) -> Self
    where
        TakesValue: Fn(&str, bool) -> bool,
        IsSwitch: Fn(&str, bool) -> bool,
    {
        let before = spans.split_last().map_or(&[][..], |(_, before)| before);
        let mut words = before.iter().skip(1).map(String::as_str);
        let mut positionals = Vec::new();
        let mut flags = Vec::new();
        let mut after_dashes = false;
        let mut leading = None;
        let mut pending = None;

        while let Some(word) = words.next() {
            if word.is_empty() {
                continue;
            }
            if after_dashes || word == "-" || !word.starts_with('-') {
                leading.get_or_insert(flags.len());
                positionals.push(word);
            } else if word == "--" {
                after_dashes = true;
            } else if let Some((flag, value)) = word.split_once('=')
                && (flag.starts_with("--") || flag.len() == 2)
            {
                flags.push((Cow::Borrowed(flag), Some(value)));
            } else if takes_value(word, leading.is_none()) {
                let value = words.next();
                pending = value.is_none().then_some(Cow::Borrowed(word));
                flags.push((Cow::Borrowed(word), value));
            } else if let Some(cluster) =
                split_cluster(word, leading.is_none(), &takes_value, &is_switch)
            {
                for letter in cluster.switches.chars() {
                    flags.push((Cow::Owned(format!("-{letter}")), None));
                }
                if let Some(taker) = cluster.taker {
                    let flag: Cow<str> = Cow::Owned(format!("-{}", taker.letter));
                    let value = taker.attached.or_else(|| words.next());
                    pending = value.is_none().then(|| flag.clone());
                    flags.push((flag, value));
                }
            } else {
                flags.push((Cow::Borrowed(word), None));
            }
        }
        Self {
            spans,
            positionals,
            leading: leading.unwrap_or(flags.len()),
            flags,
            pending,
            past_dashes: after_dashes,
        }
    }

    pub const fn spans(&self) -> &'span [String] {
        self.spans
    }

    pub fn command(&self) -> &'span str {
        self.spans.first().map_or("", String::as_str)
    }

    pub fn current(&self) -> &'span str {
        self.spans.last().map_or("", String::as_str)
    }

    /// positional `idx`, counting the first word after the command as 0.
    pub fn arg(&self, idx: usize) -> &'span str {
        self.positionals.get(idx).copied().unwrap_or("")
    }

    /// positionals typed before the cursor word.
    pub const fn arg_count(&self) -> usize {
        self.positionals.len()
    }

    /// whether the cursor word is a command operand rather than a flag's value.
    pub const fn at_operand(&self, ctx: &DynCtx<'_>) -> bool {
        self.pending.is_none() && ctx.slot.positional()
    }

    /// the flag whose value the cursor word is, from the cache or the provider.
    pub fn value_slot<'words>(&'words self, ctx: &'words DynCtx<'_>) -> Option<&'words str> {
        self.pending.as_deref().or(match ctx.slot {
            Slot::FlagValue(flag) => Some(flag),
            Slot::Positional | Slot::MaybeFlagValue(_) => None,
        })
    }

    /// whether a bare `--` came before the cursor.
    pub const fn past_dashes(&self) -> bool {
        self.past_dashes
    }

    pub fn has(&self, names: &[&str]) -> bool {
        self.flags.iter().any(|(flag, _)| names.contains(&&**flag))
    }

    pub fn value(&self, names: &[&str]) -> Option<&'span str> {
        flag_in(&self.flags, names)
    }

    /// value of a flag typed before the subcommand, so still the global flag.
    pub fn global_value(&self, names: &[&str]) -> Option<&'span str> {
        flag_in(&self.flags[..self.leading], names)
    }

    /// every value of a flag typed before the subcommand, in order.
    pub fn global_values(&self, names: &[&str]) -> Vec<&'span str> {
        self.flags[..self.leading]
            .iter()
            .filter(|(flag, _)| names.contains(&&**flag))
            .filter_map(|(_, value)| value.filter(|text| !text.is_empty()))
            .collect()
    }

    pub fn global_has(&self, names: &[&str]) -> bool {
        self.flags[..self.leading]
            .iter()
            .any(|(flag, _)| names.contains(&&**flag))
    }
}

/// a short cluster like `-fNL` is a run of known switches, optionally ending
/// in a value-taking letter that gets the rest of the word, or the next word
/// when it ends the cluster.
struct Cluster<'span> {
    switches: &'span str,
    taker:    Option<ClusterTaker<'span>>,
}

struct ClusterTaker<'span> {
    letter:   char,
    attached: Option<&'span str>,
}

/// a word is only a cluster when every letter up to its value letter is a
/// known short switch, so `-Dstrip` stays one opaque flag.
fn split_cluster<'span, TakesValue, IsSwitch>(
    word: &'span str,
    leading: bool,
    takes_value: &TakesValue,
    is_switch: &IsSwitch,
) -> Option<Cluster<'span>>
where
    TakesValue: Fn(&str, bool) -> bool,
    IsSwitch: Fn(&str, bool) -> bool,
{
    let cluster = word
        .strip_prefix('-')
        .filter(|cluster| cluster.len() > 1 && !cluster.starts_with('-'))?;
    for (at, letter) in cluster.char_indices() {
        let flag = format!("-{letter}");
        if takes_value(&flag, leading) {
            let (switches, from_letter) = cluster.split_at(at);
            let rest = from_letter.strip_prefix(letter)?;
            return Some(Cluster {
                switches,
                taker: Some(ClusterTaker {
                    letter,
                    attached: (!rest.is_empty()).then_some(rest),
                }),
            });
        }
        if !is_switch(&flag, leading) {
            return None;
        }
    }
    Some(Cluster {
        switches: cluster,
        taker:    None,
    })
}

fn flag_in<'span>(
    flags: &[(Cow<'span, str>, Option<&'span str>)],
    names: &[&str],
) -> Option<&'span str> {
    flags
        .iter()
        .rev()
        .find(|(flag, _)| names.contains(&&**flag))
        .and_then(|(_, value)| *value)
        .filter(|value| !value.is_empty())
}

impl DynCtx<'_> {
    pub(crate) fn ms_left(&self) -> u64 {
        self.deadline.map_or(u64::MAX, |deadline| {
            let left = deadline
                .saturating_duration_since(Instant::now())
                .as_millis();
            u64::try_from(left).unwrap_or(u64::MAX)
        })
    }

    pub(crate) fn budget_exhausted(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// a backend row limit, dropped once a prefix is typed because the backend
    /// cuts its list before `filter_candidates` sees the prefix.
    pub(crate) fn limit_args(&self, flag: &str, prefix: &str) -> Vec<String> {
        if self.limit == 0 || !prefix.is_empty() {
            Vec::new()
        } else {
            vec![flag.to_owned(), self.limit.to_string()]
        }
    }

    pub(crate) fn bin(&self, name: &str) -> String {
        if self.cmd_name == name
            && let Some(path) = self.explicit_cmd_path
        {
            return path.to_string_lossy().into_owned();
        }
        name.to_owned()
    }

    pub(crate) fn command_spans(&self, spans: &[String]) -> Vec<String> {
        let mut args = spans.to_vec();
        if let Some(path) = self.explicit_cmd_path
            && !args.is_empty()
        {
            args[0] = path.to_string_lossy().into_owned();
        }
        args
    }
}

pub fn run(args: &[String], ctx: &DynCtx) -> Option<String> {
    subprocess::run_quiet(args, ctx.ms_left())
}

/// scrape runner (cwd /tmp) for providers reading tool output, not repo state.
/// adb's `devices`/`pm list` must not inherit the user's cwd.
pub fn run_scrape(args: &[String], ctx: &DynCtx) -> Option<String> {
    subprocess::run_cmd(args, ctx.ms_left())
}

/// resolve a bare name through PATH (adb, when no explicit path was given).
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var("PATH").ok()?;
    for dir in path_var.split(':') {
        let candidate = Path::new(dir).join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && (meta.permissions().mode() & 0o111) != 0)
}

pub fn run_with<F>(args: &[String], ctx: &DynCtx, customize: F) -> Option<String>
where
    F: FnOnce(&mut Command),
{
    subprocess::run_quiet_with(args, ctx.ms_left(), customize)
}

pub fn run_ended<F>(args: &[String], ctx: &DynCtx, customize: F) -> Run
where
    F: FnOnce(&mut Command),
{
    subprocess::run_quiet_ended(args, ctx.ms_left(), customize)
}

/// drop exact-match subcommand/external candidates so a typed-out word isn't
/// echoed back, masking downstream completers.
pub fn filter_candidates(
    items: Vec<Candidate>,
    prefix: &str,
    matching: Matching,
) -> Option<Vec<Candidate>> {
    if items.is_empty() {
        return None;
    }
    if prefix.is_empty() {
        return Some(items);
    }
    let mut scored: Vec<(i32, usize, Candidate)> = Vec::with_capacity(items.len());
    for (idx, item) in items.into_iter().enumerate() {
        let score = match matching {
            Matching::Value => fuzzy_score(prefix, &item.value),
            Matching::ValueOrDescription => {
                fuzzy_score(prefix, &item.value).max(fuzzy_score(prefix, &item.description))
            },
        };
        if score <= 0 {
            continue;
        }
        let desc_lc = item.description.to_ascii_lowercase();
        let exact_command = item.value == prefix
            && (desc_lc.contains("subcommand") || desc_lc == "external command");
        if exact_command {
            continue;
        }
        scored.push((score, idx, item));
    }
    if scored.is_empty() {
        return None;
    }
    scored.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    Some(
        scored
            .into_iter()
            .map(|(_, _, candidate)| candidate)
            .collect(),
    )
}

/// shared tab-split helper for command output with tabular templates.
pub fn parse_tabular<F>(out: &str, make: F) -> Vec<Candidate>
where
    F: Fn(&[&str]) -> Option<Candidate>,
{
    let mut candidates = Vec::new();
    for line in out.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if let Some(candidate) = make(&parts) {
            candidates.push(candidate);
        }
    }
    candidates
}
