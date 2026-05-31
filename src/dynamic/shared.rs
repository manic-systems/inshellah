// SPDX-License-Identifier: EUPL-1.2
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use crate::subprocess;

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

#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub(crate) value: String,
    pub(crate) description: String,
}

impl Candidate {
    pub(crate) fn new(value: impl Into<String>, description: impl Into<String>) -> Self {
        Candidate {
            value: value.into(),
            description: description.into(),
        }
    }

    pub(crate) fn into_json(self) -> String {
        let mut out = String::with_capacity(self.value.len() + self.description.len() + 30);
        out.push_str(r#"{"value":""#);
        push_json_escaped(&mut out, &self.value);
        out.push_str(r#"","description":""#);
        push_json_escaped(&mut out, &self.description);
        out.push_str(r#""}"#);
        out
    }
}

fn push_json_escaped(out: &mut String, s: &str) {
    use std::fmt::Write as _;
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
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

/// duplicate of `main.rs::fuzzy_score`. binary symbols aren't reachable
/// from the lib crate, so changes here must mirror there.
fn fuzzy_score(needle: &str, haystack: &str) -> i32 {
    let needle_len = needle.len();
    let haystack_len = haystack.len();
    if needle_len == 0 {
        return 1;
    }
    if needle_len > haystack_len {
        return 0;
    }
    if needle == haystack {
        return 1000;
    }

    let needle = needle.as_bytes();
    let haystack = haystack.as_bytes();
    if starts_with_ignore_ascii_case(haystack, needle) {
        return 900 + (needle_len as i32 * 100 / haystack_len as i32);
    }

    let mut needle_idx = 0usize;
    let mut score = 0i32;
    let mut prev_match: Option<usize> = None;

    for (hay_idx, &c) in haystack.iter().enumerate() {
        if needle_idx >= needle_len {
            break;
        }
        if c.eq_ignore_ascii_case(&needle[needle_idx]) {
            let boundary = hay_idx == 0
                || haystack[hay_idx - 1] == b'-'
                || haystack[hay_idx - 1] == b'_'
                || (haystack[hay_idx - 1].is_ascii_lowercase()
                    && haystack[hay_idx].is_ascii_uppercase());
            let consecutive = prev_match == Some(hay_idx.saturating_sub(1));
            score += if boundary { 50 } else { 10 };
            if consecutive {
                score += 20;
            }
            needle_idx += 1;
            prev_match = Some(hay_idx);
            continue;
        }
    }

    if needle_idx == needle_len { score } else { 0 }
}

fn starts_with_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len()
        && haystack
            .iter()
            .zip(needle)
            .all(|(&hay, &needle)| hay.eq_ignore_ascii_case(&needle))
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
