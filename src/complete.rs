// SPDX-License-Identifier: EUPL-1.2
//! Completion candidates: the shared scorer, the typed `Candidate`, and JSON
//! rendering.
//!
//! `fuzzy_score` and the JSON escaper previously existed verbatim in both
//! `main.rs` (the static completer) and `dynamic/shared.rs` (the live
//! providers), with a comment warning that the two copies had to be kept in
//! sync by hand. They live here once now; both consumers import them.

use std::fmt::Write as _;

/// A single completion candidate. Rendered to JSON only at the output
/// boundary (`into_json` / `completion_json`), so the rest of the pipeline
/// works with typed values rather than pre-serialized strings.
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

/// `{"value":"…","description":"…"}` with both fields JSON-escaped.
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

/// Rank `needle` against `haystack`: exact match 1000, case-insensitive prefix
/// 900+length-bonus, otherwise a subsequence score rewarding word boundaries
/// and runs. 0 means no match; empty needle scores 1 (keep, unranked).
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
}
