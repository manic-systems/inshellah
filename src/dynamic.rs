// SPDX-License-Identifier: EUPL-1.2
//! per-command dynamic completion: git refs, kubectl resources, systemctl
//! units, etc. fires from `cmd_complete` when the static cache yields
//! nothing. the nushell shim is now just JSON glue.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::config::Config;

mod providers;
mod shared;

use shared::{DynCtx, filter_candidates};

/// candidates as JSON `{"value":..,"description":..}`, or None to hand
/// off to nu's file completer.
pub fn dynamic_complete(spans: &[String], cfg: &Config) -> Option<Vec<String>> {
    dynamic_complete_with_path(spans, None, cfg)
}

/// Like `dynamic_complete`, but subprocess-backed providers invoke
/// `explicit_cmd_path` for the completed command instead of resolving
/// `spans[0]` through PATH.
pub fn dynamic_complete_with_path(
    spans: &[String],
    explicit_cmd_path: Option<&Path>,
    cfg: &Config,
) -> Option<Vec<String>> {
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
    };
    let raw = providers::dispatch(spans, &ctx)?;
    let last = spans.last().map(String::as_str).unwrap_or("");
    let filtered = filter_candidates(raw, last)?;
    if filtered.is_empty() {
        None
    } else {
        Some(filtered.into_iter().map(|c| c.into_json()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::providers::jj::{is_revset_symbol_prefix, split_revset_trailing_name};
    use super::providers::kubectl::kubectl_scope;
    use super::providers::nix::looks_like_flake_pkg;
    use super::shared::{Candidate, filter_candidates};
    use super::*;

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
        assert!(filter_candidates(items, "show").is_none());
    }

    #[test]
    fn fuzzy_filter_keeps_exact_when_description_does_not_mark_subcommand() {
        let items = vec![Candidate::new("show", "shows things")];
        let filtered = filter_candidates(items, "show").expect("non-empty");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].value, "show");
    }

    #[test]
    fn fuzzy_filter_empty_prefix_keeps_all() {
        let items = vec![Candidate::new("first", "a"), Candidate::new("second", "b")];
        let filtered = filter_candidates(items, "").expect("non-empty");
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
            "".into(),
        ];
        let scope = kubectl_scope(&spans);
        assert!(!scope.all);
        assert_eq!(scope.args, vec!["-n".to_string(), "prod".to_string()]);
    }

    #[test]
    fn kubectl_scope_detects_all_namespaces() {
        let spans = vec![
            "kubectl".into(),
            "get".into(),
            "pods".into(),
            "-A".into(),
            "".into(),
        ];
        let scope = kubectl_scope(&spans);
        assert!(scope.all);
    }

    #[test]
    fn revset_split_plain_symbol_has_no_prefix() {
        assert_eq!(split_revset_trailing_name("main"), ("", "main"));
        assert_eq!(split_revset_trailing_name(""), ("", ""));
    }

    #[test]
    fn revset_split_compound_keeps_prefix() {
        assert_eq!(split_revset_trailing_name("main & dev"), ("main & ", "dev"));
        assert_eq!(split_revset_trailing_name("ancestors("), ("ancestors(", ""));
        assert_eq!(split_revset_trailing_name("a..b"), ("a..", "b"));
        assert_eq!(split_revset_trailing_name("x|y~z"), ("x|y~", "z"));
    }

    #[test]
    fn revset_symbol_prefix_accepts_remote_and_rejects_leading_sep() {
        assert!(is_revset_symbol_prefix(""));
        assert!(is_revset_symbol_prefix("main"));
        assert!(is_revset_symbol_prefix("main@"));
        assert!(is_revset_symbol_prefix("main@origin"));
        assert!(is_revset_symbol_prefix("feature/x"));
        assert!(!is_revset_symbol_prefix("@"));
        assert!(!is_revset_symbol_prefix("a@@b"));
        assert!(!is_revset_symbol_prefix("a b"));
    }

    #[test]
    fn flake_pkg_pattern_matches_flake_pkg() {
        assert!(looks_like_flake_pkg("nixpkgs#hello"));
        assert!(!looks_like_flake_pkg("hello"));
        assert!(!looks_like_flake_pkg("#hello"));
        assert!(!looks_like_flake_pkg("nixpkgs#"));
    }
}
