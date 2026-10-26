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
};

mod providers;
mod shared;

use shared::{
    DynCtx,
    filter_candidates,
};

/// whether nushell's file completion joins a provider's candidates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Files {
    Replace,
    Alongside,
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
    /// guessed from the previous word alone, for callers without a cache.
    pub fn from_spans(spans: &'span [String]) -> Self {
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
        value_flags: &[],
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
        &[],
        cfg,
    )
    .map(|(candidates, _)| candidates.into_iter().map(Candidate::into_json).collect())
}

pub fn dynamic_complete_candidates_with_path(
    spans: &[String],
    explicit_cmd_path: Option<&Path>,
    slot: Slot<'_>,
    value_flags: &[String],
    cfg: &Config,
) -> Option<(Vec<Candidate>, Files)> {
    if spans.is_empty() {
        return None;
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
        value_flags,
    };
    let (raw, files, matching) = providers::dispatch(spans, &ctx)?;
    let last = spans.last().map_or("", String::as_str);
    let filtered = filter_candidates(raw, last, matching)?;
    (!filtered.is_empty()).then_some((filtered, files))
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
        let scope = kubectl_scope(&Words::parse(&spans, |flag| flag == "-n"));
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
        let scope = kubectl_scope(&Words::parse(&spans, |_| false));
        assert!(scope.all);
    }

    #[test]
    fn flake_pkg_pattern_matches_flake_pkg() {
        assert!(looks_like_flake_pkg("nixpkgs#hello"));
        assert!(!looks_like_flake_pkg("hello"));
        assert!(!looks_like_flake_pkg("#hello"));
        assert!(!looks_like_flake_pkg("nixpkgs#"));
    }
}
