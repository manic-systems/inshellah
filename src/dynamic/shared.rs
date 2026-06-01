// SPDX-License-Identifier: EUPL-1.2
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use crate::subprocess;

// the typed candidate and scorer are shared with the static completer.
pub(crate) use crate::complete::{Candidate, fuzzy_score};

#[derive(Clone, Copy)]
pub(crate) struct DynCtx<'a> {
    pub(crate) deadline: Option<Instant>,
    pub(crate) limit: usize,
    pub(crate) cmd_name: &'a str,
    pub(crate) explicit_cmd_path: Option<&'a Path>,
}

impl DynCtx<'_> {
    pub(crate) fn ms_left(&self) -> u64 {
        match self.deadline {
            None => u64::MAX,
            Some(d) => d
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u64::MAX as u128) as u64,
        }
    }

    pub(crate) fn budget_exhausted(&self) -> bool {
        match self.deadline {
            None => false,
            Some(d) => Instant::now() >= d,
        }
    }

    pub(crate) fn limit_args(&self, flag: &str) -> Vec<String> {
        if self.limit == 0 {
            Vec::new()
        } else {
            vec![flag.to_string(), self.limit.to_string()]
        }
    }

    pub(crate) fn bin(&self, name: &str) -> String {
        if self.cmd_name == name
            && let Some(path) = self.explicit_cmd_path
        {
            return path.to_string_lossy().into_owned();
        }
        name.to_string()
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

pub(crate) fn run(args: &[String], ctx: &DynCtx) -> Option<String> {
    subprocess::run_quiet(args, ctx.ms_left())
}

pub(crate) fn run_with(
    args: &[String],
    ctx: &DynCtx,
    customize: impl FnOnce(&mut Command),
) -> Option<String> {
    subprocess::run_quiet_with(args, ctx.ms_left(), customize)
}

/// drops exact-match subcommand/external candidates so a typed-out word
/// doesn't get echoed back and mask downstream completers.
pub(crate) fn filter_candidates(items: Vec<Candidate>, prefix: &str) -> Option<Vec<Candidate>> {
    if items.is_empty() {
        return None;
    }
    if prefix.is_empty() {
        return Some(items);
    }
    let mut scored: Vec<(i32, usize, Candidate)> = Vec::with_capacity(items.len());
    for (idx, item) in items.into_iter().enumerate() {
        let score = fuzzy_score(prefix, &item.value);
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
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    Some(scored.into_iter().map(|(_, _, c)| c).collect())
}

/// shared tab-split helper for command output with tabular templates.
pub(crate) fn parse_tabular<F>(out: &str, min_parts: usize, mk: F) -> Vec<Candidate>
where
    F: Fn(&[&str]) -> Candidate,
{
    let mut candidates = Vec::new();
    for line in out.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < min_parts {
            continue;
        }
        candidates.push(mk(&parts));
    }
    candidates
}
