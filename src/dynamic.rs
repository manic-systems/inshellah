// SPDX-License-Identifier: EUPL-1.2
//! per-command dynamic completion (git refs, systemctl units, ...). fires from
//! `cmd_complete` when the static cache is empty.

use std::{
    path::Path,
    time::{
        Duration,
        Instant,
    },
};

use crate::{
    complete::Candidate,
    config::Config,
    dynamic::adapters::Protocol,
};

pub mod adapters;
mod providers;
mod shared;

use shared::{
    DynCtx,
    filter_candidates,
};

/// what the static cache knows about the flags of the root command and of the
/// resolved subcommand.
#[derive(Clone, Copy, Default)]
pub struct CachedFlags<'span> {
    pub global_value: &'span [String],
    pub value:        &'span [String],
    /// every short switch of the root command, whether it takes a value or not.
    pub global_short: &'span [String],
    /// every short switch of the resolved subcommand.
    pub short:        &'span [String],
}

/// whether nushell's file completion joins a provider's candidates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Files {
    Replace,
    Alongside,
    /// no files even when the candidates run out, so an empty answer is final.
    Never,
}

/// what a provider or a program's own completer offers.
pub struct Answer {
    pub candidates: Vec<Candidate>,
    pub files:      Files,
}

/// how a provider or a program's own completer responded.
pub enum Reply {
    Answered(Answer),
    Silent,
    /// the program replied outside its protocol, so it does not speak it.
    Foreign,
}

impl Reply {
    pub fn into_answer(self) -> Option<Answer> {
        match self {
            Self::Answered(answer) => Some(answer),
            Self::Silent | Self::Foreign => None,
        }
    }
}

/// what the word under the cursor fills.
#[derive(Clone, Copy)]
pub enum Slot<'span> {
    Positional,
    /// the static cache says this flag takes a value.
    FlagValue(&'span str),
    /// the previous word is a flag of unknown arity.
    MaybeFlagValue(&'span str),
}

impl<'span> Slot<'span> {
    /// guessed from the words alone, for callers without a cache.
    pub fn from_spans(spans: &'span [String]) -> Self {
        let attached = spans
            .last()
            .and_then(|word| word.split_once('='))
            .filter(|(flag, _)| flag.starts_with("--"));
        if let Some((flag, _)) = attached {
            return Self::FlagValue(flag);
        }
        match spans.len().checked_sub(2).map(|at| spans[at].as_str()) {
            Some(prev) if prev.starts_with('-') && prev != "--" => Self::MaybeFlagValue(prev),
            _ => Self::Positional,
        }
    }

    pub(crate) const fn value_flag(self) -> Option<&'span str> {
        match self {
            Self::FlagValue(flag) | Self::MaybeFlagValue(flag) => Some(flag),
            Self::Positional => None,
        }
    }

    pub(crate) const fn positional(self) -> bool {
        !matches!(self, Self::FlagValue(_))
    }
}

pub fn provider_kind(cmd: &str) -> Option<&'static str> {
    providers::kind_of(cmd)
}

/// candidates as JSON, or None to hand off to nu's file completer.
pub fn dynamic_complete(spans: &[String], cfg: &Config) -> Option<Vec<String>> {
    dynamic_complete_with_path(spans, None, cfg)
}

/// preempt path: value completions (adb serials/packages) that override static
/// flags. `Some` (even empty) suppresses static. already scored, so they skip
/// `filter_candidates`.
pub fn dynamic_value_completions(
    cmd_name: &str,
    rest: &[String],
    explicit_cmd_path: Option<&Path>,
    timeout_ms: u64,
) -> Option<Vec<Candidate>> {
    let mut spans = Vec::with_capacity(rest.len() + 1);
    spans.push(cmd_name.to_owned());
    spans.extend_from_slice(rest);
    let deadline = if timeout_ms == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(timeout_ms))
    };
    let ctx = DynCtx {
        deadline,
        limit: 0,
        cmd_name,
        explicit_cmd_path,
        slot: Slot::from_spans(&spans),
        flags: CachedFlags::default(),
    };
    providers::value_completions(&spans, &ctx)
}

/// like `dynamic_complete` but runs `explicit_cmd_path` instead of resolving
/// `spans[0]` through PATH.
pub fn dynamic_complete_with_path(
    spans: &[String],
    explicit_cmd_path: Option<&Path>,
    cfg: &Config,
) -> Option<Vec<String>> {
    dynamic_complete_candidates_with_path(
        spans,
        explicit_cmd_path,
        Slot::from_spans(spans),
        CachedFlags::default(),
        None,
        cfg,
    )
    .into_answer()
    .map(|answer| {
        answer
            .candidates
            .into_iter()
            .map(Candidate::into_json)
            .collect()
    })
}

pub fn dynamic_complete_candidates_with_path(
    spans: &[String],
    explicit_cmd_path: Option<&Path>,
    slot: Slot<'_>,
    flags: CachedFlags<'_>,
    protocol: Option<Protocol>,
    cfg: &Config,
) -> Reply {
    if spans.is_empty() {
        return Reply::Silent;
    }
    let deadline = if cfg.dynamic_timeout_ms == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(cfg.dynamic_timeout_ms))
    };
    let ctx = DynCtx {
        deadline,
        limit: cfg.dynamic_limit,
        cmd_name: &spans[0],
        explicit_cmd_path,
        slot,
        flags,
    };
    let reply = match (providers::dispatch(spans, &ctx), protocol) {
        (Some(answer), _) => Reply::Answered(answer),
        (None, Some(asked)) => asked.complete(spans, &ctx),
        (None, None) => Reply::Silent,
    };
    let Reply::Answered(Answer {
        candidates: mut raw,
        files,
    }) = reply
    else {
        return reply;
    };

    let last = spans.last().map_or("", String::as_str);
    let (inline, needle) = match (ctx.slot, last.split_once('=')) {
        (Slot::FlagValue(flag), Some((typed, value))) if typed == flag => {
            (Some(format!("{flag}=")), value)
        },
        _ => (None, last),
    };
    if let Some(prefix) = &inline {
        for candidate in &mut raw {
            if let Some(bare) = candidate.value.strip_prefix(prefix.as_str()) {
                candidate.value = bare.to_owned();
            }
        }
    }

    let matching = providers::matching_of(&spans[0]);
    let mut filtered = filter_candidates(raw, needle, matching).unwrap_or_default();
    if let Some(prefix) = &inline {
        for candidate in &mut filtered {
            candidate.value.insert_str(0, prefix);
        }
    }
    if files != Files::Never && filtered.is_empty() {
        return Reply::Silent;
    }
    Reply::Answered(Answer {
        candidates: filtered,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        providers::{
            kubectl::kubectl_scope,
            nix::looks_like_flake_pkg,
        },
        shared::{
            Matching,
            Words,
            filter_candidates,
        },
        *,
    };

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn unknown_command_returns_none() {
        let cfg = cfg();
        assert!(dynamic_complete(&["zzznotacommand".into()], &cfg).is_none());
    }

    #[test]
    fn empty_spans_returns_none() {
        let cfg = cfg();
        assert!(dynamic_complete(&[], &cfg).is_none());
    }

    #[test]
    fn nix_too_few_spans_returns_none() {
        let cfg = cfg();
        assert!(dynamic_complete(&["nix".into()], &cfg).is_none());
    }

    #[test]
    fn fuzzy_filter_drops_exact_subcommand_matches() {
        let items = vec![Candidate::new("show", "remote subcommand")];
        assert!(filter_candidates(items, "show", Matching::Value).is_none());
    }

    #[test]
    fn fuzzy_filter_keeps_exact_when_description_does_not_mark_subcommand() {
        let items = vec![Candidate::new("show", "shows things")];
        let filtered = filter_candidates(items, "show", Matching::Value).expect("non-empty");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].value, "show");
    }

    #[test]
    fn fuzzy_filter_empty_prefix_keeps_all() {
        let items = vec![Candidate::new("first", "a"), Candidate::new("second", "b")];
        let filtered = filter_candidates(items, "", Matching::Value).expect("non-empty");
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn kubectl_scope_parses_namespace_flag() {
        let spans = vec![
            "kubectl".into(),
            "get".into(),
            "pods".into(),
            "-n".into(),
            "prod".into(),
            String::new(),
        ];
        let scope = kubectl_scope(&Words::parse(&spans, |flag, _| flag == "-n", |_, _| false));
        assert!(!scope.all);
        assert_eq!(scope.args, vec!["-n".to_owned(), "prod".to_owned()]);
    }

    #[test]
    fn kubectl_scope_detects_all_namespaces() {
        let spans = vec![
            "kubectl".into(),
            "get".into(),
            "pods".into(),
            "-A".into(),
            String::new(),
        ];
        let scope = kubectl_scope(&Words::parse(&spans, |_, _| false, |_, _| false));
        assert!(scope.all);
    }

    fn spans_of(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn leading_only_flags_take_no_value_after_the_subcommand() {
        let spans = spans_of(&["git", "-c", "a=b", "log", "-c", "main", ""]);
        let words = Words::parse(
            &spans,
            |flag, leading| flag == "-c" && leading,
            |_, _| false,
        );
        assert_eq!(words.arg_count(), 2);
        assert_eq!(words.arg(1), "main");
    }

    #[test]
    fn a_cluster_takes_a_value_only_when_its_value_letter_is_last() {
        let takes_l = |flag: &str, _| flag == "-L";
        let switches = |flag: &str, _| flag == "-f" || flag == "-N";
        let trailing = spans_of(&["ssh", "-fNL", "8080:db:80", "host", ""]);
        assert_eq!(Words::parse(&trailing, takes_l, switches).arg(0), "host");
        let attached = spans_of(&["ssh", "-L8080:db:80", "host", ""]);
        assert_eq!(Words::parse(&attached, takes_l, switches).arg(0), "host");
    }

    #[test]
    fn flake_pkg_pattern_matches_flake_pkg() {
        assert!(looks_like_flake_pkg("nixpkgs#hello"));
        assert!(!looks_like_flake_pkg("hello"));
        assert!(!looks_like_flake_pkg("#hello"));
        assert!(!looks_like_flake_pkg("nixpkgs#"));
    }
}
