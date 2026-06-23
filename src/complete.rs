// SPDX-License-Identifier: EUPL-1.2
//! completion candidates: the shared scorer, the typed `Candidate`, and JSON
//! rendering.

use std::fmt::Write as _;

use crate::config::Config;
use crate::parsers::manpage::{
    ManpageEntry, ManpageResult, ManpageSubcommand, OwnedParam, OwnedSwitch,
};

#[derive(Clone, Debug)]
pub struct Candidate {
    pub value: String,
    pub description: String,
}

impl Candidate {
    pub fn new(value: impl Into<String>, description: impl Into<String>) -> Self {
        Candidate {
            value: value.into(),
            description: description.into(),
        }
    }

    pub fn into_json(self) -> String {
        completion_json(&self.value, &self.description)
    }
}

pub fn completion_json(value: &str, desc: &str) -> String {
    let mut out = String::with_capacity(value.len() + desc.len() + 30);
    out.push_str(r#"{"value":""#);
    push_json_escaped(&mut out, value);
    out.push_str(r#"","description":""#);
    push_json_escaped(&mut out, desc);
    out.push_str(r#""}"#);
    out
}

fn push_json_escaped(out: &mut String, s: &str) {
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

/// exact 1000, ci-prefix 900+len bonus, else a subsequence score rewarding word
/// boundaries and runs. 0 = no match; empty needle = 1 (keep, unranked).
pub fn fuzzy_score(needle: &str, haystack: &str) -> i32 {
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

pub fn starts_with_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len()
        && haystack
            .iter()
            .zip(needle)
            .all(|(&hay, &needle)| hay.eq_ignore_ascii_case(&needle))
}

fn long_flag_score(needle: &str, dashed: &str, bare_name: &str) -> i32 {
    if needle.is_empty() {
        return 1;
    }
    if needle == dashed {
        return 1000;
    }
    if needle == "-" {
        return 900;
    }
    if starts_with_ignore_ascii_case(dashed.as_bytes(), needle.as_bytes()) {
        return 950 + (needle.len() as i32 * 100 / dashed.len() as i32);
    }

    let bare_needle = needle.trim_start_matches('-');
    if bare_needle.is_empty() {
        return 0;
    }
    let words: Vec<&str> = bare_name
        .split('-')
        .filter(|word| !word.is_empty())
        .collect();
    if words
        .iter()
        .any(|word| starts_with_ignore_ascii_case(word.as_bytes(), bare_needle.as_bytes()))
    {
        return 850 + bare_needle.len() as i32;
    }
    if acronym_matches(bare_needle, &words) {
        return 700 + bare_needle.len() as i32;
    }
    0
}

fn acronym_matches(needle: &str, words: &[&str]) -> bool {
    let mut needle = needle.chars();
    let Some(mut want) = needle.next() else {
        return false;
    };

    for word in words {
        let Some(first) = word.chars().next() else {
            continue;
        };
        if want.eq_ignore_ascii_case(&first) {
            match needle.next() {
                Some(next) => want = next,
                None => return true,
            }
        }
    }
    false
}

/// param placeholder convention: `<FILE>` mandatory, `[FILE]` optional.
pub fn entry_completion_desc(e: &ManpageEntry) -> String {
    match &e.param {
        Some(OwnedParam::Mandatory(p)) => {
            if e.desc.is_empty() {
                format!("<{p}>")
            } else {
                format!("{} <{p}>", e.desc)
            }
        }
        Some(OwnedParam::Optional(p)) => {
            if e.desc.is_empty() {
                format!("[{p}]")
            } else {
                format!("{} [{p}]", e.desc)
            }
        }
        None => e.desc.clone(),
    }
}

/// subcommands emit only when `matched_depth >= resolve_depth` (match is a full
/// prefix of what was typed). typing past the deepest cached command stays silent
/// so nushell's dynamic completer can take over. a token equal to a candidate is
/// dropped so it isn't echoed back.
pub fn generate_candidates(
    result: &ManpageResult,
    matched_depth: usize,
    resolve_depth: usize,
    last_token: &str,
    fallback_subcommands: &[ManpageSubcommand],
    typing_flag: bool,
    cfg: &Config,
) -> Vec<Candidate> {
    let subs: &[ManpageSubcommand] = if !result.subcommands.is_empty() {
        &result.subcommands
    } else {
        fallback_subcommands
    };

    let mut scored: Vec<(i32, Candidate)> = Vec::with_capacity(
        (if matched_depth >= resolve_depth {
            subs.len() + result.positional_choices.len()
        } else {
            0
        }) + if typing_flag { result.entries.len() } else { 0 },
    );

    if matched_depth >= resolve_depth {
        // subcommands and positional-arg choices (getent db names) share one slot.
        let choices = subs.iter().chain(result.positional_choices.iter());
        for sc in choices {
            // exact match on the name or any alias hands off to a dynamic
            // completer, mirroring the switch path.
            if !last_token.is_empty()
                && (last_token == sc.name || sc.aliases.iter().any(|a| a == last_token))
            {
                continue;
            }
            // emit the best-scoring form (name or alias) as the value; the rest
            // become `(aka ...)`, like a switch's short/long pair.
            let mut best: Option<(i32, &str)> = None;
            let mut others: Vec<&str> = Vec::new();
            for form in
                std::iter::once(sc.name.as_str()).chain(sc.aliases.iter().map(String::as_str))
            {
                let fs = fuzzy_score(last_token, form);
                match best {
                    Some((bs, bf)) if fs > bs => {
                        others.push(bf);
                        best = Some((fs, form));
                    }
                    Some(_) => others.push(form),
                    None => best = Some((fs, form)),
                }
            }
            if let Some((score, value)) = best
                && score > 0
            {
                let desc = if others.is_empty() {
                    sc.desc.clone()
                } else {
                    format!("(aka {}) {}", others.join(", "), sc.desc)
                };
                scored.push((score, Candidate::new(value.to_string(), desc)));
            }
        }
    }

    // bare-vs-dashed scoring depends on the trigger typed (Config::flag_needle),
    // default "-" keeps the dashed form.
    if typing_flag {
        let fneedle = cfg.flag_needle(last_token);
        let score_against = |dashed: &str, bare_name: &str| -> i32 {
            if fneedle.bare {
                fuzzy_score(fneedle.needle, bare_name)
            } else if dashed.starts_with("--") {
                long_flag_score(fneedle.needle, dashed, bare_name)
            } else {
                fuzzy_score(fneedle.needle, dashed)
            }
        };
        for e in &result.entries {
            let (flag, aka, score) = match &e.switch {
                OwnedSwitch::Long(l) => {
                    let flag = format!("--{l}");
                    let score = score_against(&flag, l);
                    (flag, None, score)
                }
                OwnedSwitch::Short(c) => {
                    let flag = format!("-{c}");
                    let short_bare = c.to_string();
                    let score = score_against(&flag, &short_bare);
                    (flag, None, score)
                }
                OwnedSwitch::Both(c, l) => {
                    let long_flag = format!("--{l}");
                    let short_flag = format!("-{c}");
                    let short_bare = c.to_string();
                    let ls = score_against(&long_flag, l);
                    let ss = score_against(&short_flag, &short_bare);
                    if ss >= ls {
                        (short_flag, Some(long_flag), ss)
                    } else {
                        (long_flag, Some(short_flag), ls)
                    }
                }
            };
            if score > 0 {
                let base_desc = entry_completion_desc(e);
                let desc = match aka {
                    Some(aka) => format!("(aka {aka}) {base_desc}"),
                    None => base_desc,
                };
                scored.push((score, Candidate::new(flag, desc)));
            }
        }
    }

    scored.sort_by(|a, b| b.0.cmp(&a.0));
    if cfg.max_completions > 0 {
        scored.truncate(cfg.max_completions);
    }
    scored.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_score_ranking_shape() {
        assert_eq!(fuzzy_score("", "build"), 1);
        assert_eq!(fuzzy_score("build", "build"), 1000);
        assert_eq!(fuzzy_score("BUILD", "build"), 1000);
        assert_eq!(fuzzy_score("bl", "build"), 60);
        assert_eq!(fuzzy_score("bl", "bundle"), 60);
        assert_eq!(fuzzy_score("bl", "branch-list"), 100);
        assert_eq!(fuzzy_score("bl", "blacklist"), 922);
        assert_eq!(fuzzy_score("bl", "table"), 40);
    }

    #[test]
    fn json_escapes_without_changing_shape() {
        assert_eq!(
            completion_json("a\"b", "line\nnext"),
            r#"{"value":"a\"b","description":"line\nnext"}"#
        );
    }

    fn result_with_subs(names: &[&str]) -> ManpageResult {
        ManpageResult {
            entries: Vec::new(),
            subcommands: names
                .iter()
                .map(|n| ManpageSubcommand::new(n.to_string(), String::new()))
                .collect(),
            positional_choices: Vec::new(),
            positionals: Vec::new(),
            description: String::new(),
        }
    }

    #[test]
    fn depth_guard_suppresses_subs_on_shallow_match() {
        let r = result_with_subs(&["start", "stop", "status"]);
        let cfg = Config::default();
        let shallow = generate_candidates(&r, 1, 2, "stat", &[], false, &cfg);
        assert!(shallow.is_empty(), "shallow match must not emit subs");

        let full = generate_candidates(&r, 1, 1, "st", &[], false, &cfg);
        let values: Vec<&str> = full.iter().map(|c| c.value.as_str()).collect();
        assert!(values.contains(&"start"));
        assert!(values.contains(&"status"));
    }

    #[test]
    fn exact_token_is_dropped_to_unmask_downstream() {
        let r = result_with_subs(&["status"]);
        let cfg = Config::default();
        let out = generate_candidates(&r, 1, 1, "status", &[], false, &cfg);
        assert!(out.is_empty());
    }

    fn result_with_flags(entries: Vec<ManpageEntry>) -> ManpageResult {
        ManpageResult {
            entries,
            subcommands: Vec::new(),
            positional_choices: Vec::new(),
            positionals: Vec::new(),
            description: String::new(),
        }
    }

    #[test]
    fn exact_short_flag_is_kept_for_preview() {
        let r = result_with_flags(vec![ManpageEntry {
            switch: OwnedSwitch::Both('x', "catalog".to_string()),
            param: None,
            desc: "explain log messages".to_string(),
        }]);
        let cfg = Config::default();
        let out = generate_candidates(&r, 1, 1, "-x", &[], true, &cfg);
        let values: Vec<&str> = out.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(values, vec!["-x"]);
    }

    #[test]
    fn exact_long_flag_is_kept_alongside_longer_matches() {
        let r = result_with_flags(vec![
            ManpageEntry {
                switch: OwnedSwitch::Long("image".to_string()),
                param: None,
                desc: "use an image".to_string(),
            },
            ManpageEntry {
                switch: OwnedSwitch::Long("image-policy".to_string()),
                param: None,
                desc: "set image policy".to_string(),
            },
        ]);
        let cfg = Config::default();
        let out = generate_candidates(&r, 1, 1, "--image", &[], true, &cfg);
        let values: Vec<&str> = out.iter().map(|c| c.value.as_str()).collect();
        assert!(values.contains(&"--image"), "values = {values:?}");
        assert!(values.contains(&"--image-policy"), "values = {values:?}");
    }

    #[test]
    fn long_flags_match_word_prefixes_and_acronyms_not_random_subsequences() {
        let r = result_with_flags(vec![
            ManpageEntry {
                switch: OwnedSwitch::Long("no-write-lock-file".to_string()),
                param: None,
                desc: "do not update the lock file".to_string(),
            },
            ManpageEntry {
                switch: OwnedSwitch::Long("command".to_string()),
                param: None,
                desc: "run a command".to_string(),
            },
        ]);
        let cfg = Config::default();

        let values_for = |needle: &str| {
            generate_candidates(&r, 1, 1, needle, &[], true, &cfg)
                .into_iter()
                .map(|c| c.value)
                .collect::<Vec<_>>()
        };

        assert_eq!(values_for("--c"), vec!["--command"]);
        assert_eq!(values_for("--lock"), vec!["--no-write-lock-file"]);
        assert_eq!(values_for("--wlf"), vec!["--no-write-lock-file"]);
    }
}
