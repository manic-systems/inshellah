// SPDX-License-Identifier: EUPL-1.2
//! Single source-priority resolver for one command node.
//!
//! Historically the same "native completions, else manpage (+help supplement),
//! else --help" pipeline was written three times — the batch indexer
//! (`process_pool_job`), the runtime on-the-fly resolver
//! (`resolve_command_path_and_cache`), and the help-only recursion
//! (`help_resolve`/`recurse_subcommand`) — and had drifted: the runtime path
//! ran native completions even for subcommands (which have no payload of their
//! own), returning a parent's completion payload for a child.
//!
//! This module owns that pipeline once. The side-effecting probes (subprocess
//! `--help`, ELF classification, manpage lookup, the help-supplement merge) are
//! injected through [`Probe`], so the priority logic here is pure and
//! unit-testable; the binary supplies a filesystem/subprocess-backed impl and
//! the two drivers (the worker pool for indexing, the sequential walk for
//! runtime) keep only their executor and cache-write concerns.

use crate::parsers::manpage::{ManpageResult, ManpageSubcommand};

/// What an injected probe thinks a binary is, mirroring the indexer's ELF
/// classification. Only `Skip` is acted on by a driver (the index skips such
/// binaries entirely); the resolver core uses `HasNativeCompletions` to decide
/// whether the native-payload probe is worth attempting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeClass {
    TryHelp,
    HasNativeCompletions,
    Skip,
}

/// The side effects a resolve needs, abstracted so the pipeline is pure. A
/// `Probe` is bound to one binary (it already knows its path, base command,
/// man directories and timeout); methods take only the per-node `sub_args`.
pub trait Probe {
    /// ELF/script classification of the bound binary.
    fn classify(&self) -> NodeClass;

    /// The native nushell completion payload, if the binary ships one. Only
    /// ever called at top level (a subcommand has no payload of its own).
    fn native_completions(&self) -> Option<String>;

    /// Raw manpage contents for the hyphenated command name (`git-remote`),
    /// or `None` if no page exists.
    fn manpage(&self, hyphenated: &str) -> Option<String>;

    /// `--help`/`-h` text for the node (`sub_args` past the base command),
    /// ansi-stripped, or `None` on failure/timeout/empty output.
    fn help_text(&self, sub_args: &[String]) -> Option<String>;

    /// Merge complementary `--help` data into a manpage-derived result
    /// (descriptions, flag aliases, missing flags/subs/positionals). Returns
    /// `true` if anything was added. This is the legacy supplement subsystem,
    /// behavior-preserved and now called from exactly one place.
    fn supplement_from_help(&self, result: &mut ManpageResult, sub_args: &[String]) -> bool;

    /// Recover children for a group command whose manpage enumerated none
    /// (sibling `cmd-sub.N` pages and/or `--help`). `Some(children)` replaces
    /// the empty list; `None` leaves it (e.g. when sibling pages were indexed
    /// out-of-band and will be found by a later lookup).
    fn group_children(&self, hyphenated: &str, sub_args: &[String]) -> Option<Vec<ManpageSubcommand>>;
}

/// The outcome of resolving one node. The driver decides how to persist it
/// (the core performs no I/O) and whether to recurse into `children`.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// A native completion payload. The driver persists `nu` as the native
    /// blob and (for callers that want candidates) parses it via the store's
    /// `parse_nu_completions`. Native payloads describe the whole tool, so
    /// there are no `children` to recurse and no JSON result to write.
    Native { nu: String },
    /// Structured content from a manpage or `--help`. `source` is the cache
    /// tag; `children` are the filtered subcommand tokens to recurse into.
    Content {
        result: ManpageResult,
        source: &'static str,
        children: Vec<String>,
    },
    /// Nothing usable: an empty parse, or a sub-probe that just echoed the
    /// parent (the leaf token appeared in its own subcommand list). Nothing to
    /// cache.
    Empty,
}

/// `base` and `sub_args` joined with spaces — the canonical command name
/// (`git`, `git stash apply`).
pub fn full_cmd(base: &str, sub_args: &[String]) -> String {
    if sub_args.is_empty() {
        base.to_string()
    } else {
        format!("{base} {}", sub_args.join(" "))
    }
}

/// `base` and `sub_args` joined with hyphens — the manpage lookup name
/// (`git`, `git-stash-apply`).
pub fn hyphenated_cmd(base: &str, sub_args: &[String]) -> String {
    if sub_args.is_empty() {
        base.to_string()
    } else {
        format!("{base}-{}", sub_args.join("-"))
    }
}

/// Subcommand tokens worth recursing into: at least two chars, not a flag, not
/// the ubiquitous `help` pseudo-command.
pub fn child_tokens(subcommands: &[ManpageSubcommand]) -> Vec<String> {
    subcommands
        .iter()
        .map(|sc| sc.name.clone())
        .filter(|n| n.len() >= 2 && !n.starts_with('-') && n != "help")
        .collect()
}

/// True when a sub-probe just echoed its parent: the leaf token appears in the
/// result's own subcommand list, so the binary didn't recognize it.
fn self_listed(result: &ManpageResult, sub_args: &[String]) -> bool {
    sub_args.last().is_some_and(|leaf| {
        result
            .subcommands
            .iter()
            .any(|sc| sc.name.eq_ignore_ascii_case(leaf))
    })
}

fn parse_is_empty(r: &ManpageResult) -> bool {
    r.entries.is_empty() && r.subcommands.is_empty() && r.positionals.is_empty()
}

/// Resolve a single command node, in source-priority order:
/// 1. native completions (top level only — the drift fix),
/// 2. manpage as primary content, supplemented by `--help` and with group
///    children recovered when the page enumerated none,
/// 3. `--help` text as a fallback.
///
/// `parse_help` and `parse_manpage` are passed in (they live in the parser
/// layer / binary) so this module stays free of the parser wiring; in practice
/// they are `parse_help_text` and `parse_manpage_string`.
pub fn resolve_node(
    probe: &dyn Probe,
    base: &str,
    sub_args: &[String],
    parse_manpage: &dyn Fn(&str) -> ManpageResult,
    parse_help: &dyn Fn(&str) -> ManpageResult,
    strip_subcmd_prefix: &dyn Fn(&mut ManpageResult, &str),
    looks_like_unenumerated_group: &dyn Fn(&ManpageResult) -> bool,
) -> Outcome {
    // 1. native completions — top level only. Parsing the blob
    // (`parse_nu_completions`) needs the full command name and lives in the
    // store layer, so the driver does it; here we just surface the raw blob.
    if sub_args.is_empty()
        && probe.classify() == NodeClass::HasNativeCompletions
        && let Some(nu) = probe.native_completions()
    {
        return Outcome::Native { nu };
    }

    let hyphenated = hyphenated_cmd(base, sub_args);

    // 2. manpage as primary content.
    if let Some(contents) = probe.manpage(&hyphenated) {
        let mut result = parse_manpage(&contents);
        if !result.entries.is_empty() || !result.subcommands.is_empty() {
            strip_subcmd_prefix(&mut result, &hyphenated);
            let mut source = "manpage";
            if probe.supplement_from_help(&mut result, sub_args) {
                source = "manpage+help";
            }
            if looks_like_unenumerated_group(&result)
                && let Some(children) = probe.group_children(&hyphenated, sub_args)
            {
                result.subcommands = children;
                source = "manpage+help";
            }
            let children = child_tokens(&result.subcommands);
            return Outcome::Content {
                result,
                source,
                children,
            };
        }
    }

    // 3. --help fallback.
    let Some(text) = probe.help_text(sub_args) else {
        return Outcome::Empty;
    };
    let result = parse_help(&text);
    if parse_is_empty(&result) || self_listed(&result, sub_args) {
        return Outcome::Empty;
    }
    let children = child_tokens(&result.subcommands);
    Outcome::Content {
        result,
        source: "help",
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::manpage::OwnedSwitch;
    use std::cell::Cell;

    fn sub(name: &str) -> ManpageSubcommand {
        ManpageSubcommand {
            name: name.to_string(),
            desc: String::new(),
        }
    }

    fn with_subs(names: &[&str]) -> ManpageResult {
        ManpageResult {
            entries: Vec::new(),
            subcommands: names.iter().map(|n| sub(n)).collect(),
            positionals: Vec::new(),
            description: String::new(),
        }
    }

    fn with_flag() -> ManpageResult {
        ManpageResult {
            entries: vec![crate::parsers::manpage::ManpageEntry {
                switch: OwnedSwitch::Long("verbose".into()),
                param: None,
                desc: String::new(),
            }],
            subcommands: Vec::new(),
            positionals: Vec::new(),
            description: String::new(),
        }
    }

    /// Configurable fake probe; counts native-completion probes so we can
    /// prove they don't fire for subcommands.
    struct FakeProbe {
        class: NodeClass,
        native: Option<String>,
        native_calls: Cell<u32>,
        manpage: Option<String>,
        help: Option<String>,
    }

    impl Default for FakeProbe {
        fn default() -> Self {
            FakeProbe {
                class: NodeClass::TryHelp,
                native: None,
                native_calls: Cell::new(0),
                manpage: None,
                help: None,
            }
        }
    }

    impl Probe for FakeProbe {
        fn classify(&self) -> NodeClass {
            self.class
        }
        fn native_completions(&self) -> Option<String> {
            self.native_calls.set(self.native_calls.get() + 1);
            self.native.clone()
        }
        fn manpage(&self, _hyphenated: &str) -> Option<String> {
            self.manpage.clone()
        }
        fn help_text(&self, _sub_args: &[String]) -> Option<String> {
            self.help.clone()
        }
        fn supplement_from_help(&self, _r: &mut ManpageResult, _s: &[String]) -> bool {
            false
        }
        fn group_children(&self, _h: &str, _s: &[String]) -> Option<Vec<ManpageSubcommand>> {
            None
        }
    }

    // parser stand-ins keyed off a marker in the text.
    fn parse_manpage_stub(text: &str) -> ManpageResult {
        if text == "MAN_SUBS" {
            with_subs(&["build", "check"])
        } else if text == "MAN_FLAG" {
            with_flag()
        } else {
            ManpageResult::default()
        }
    }
    fn parse_help_stub(text: &str) -> ManpageResult {
        match text {
            "HELP_SUBS" => with_subs(&["alpha", "beta"]),
            "HELP_FLAG" => with_flag(),
            "HELP_SELFLISTED" => with_subs(&["alpha"]),
            _ => ManpageResult::default(),
        }
    }
    fn noop_strip(_r: &mut ManpageResult, _b: &str) {}
    fn never_group(_r: &ManpageResult) -> bool {
        false
    }

    fn resolve(probe: &dyn Probe, base: &str, sub_args: &[&str]) -> Outcome {
        let sub: Vec<String> = sub_args.iter().map(|s| s.to_string()).collect();
        resolve_node(
            probe,
            base,
            &sub,
            &parse_manpage_stub,
            &parse_help_stub,
            &noop_strip,
            &never_group,
        )
    }

    #[test]
    fn native_completions_fire_at_top_level() {
        let probe = FakeProbe {
            class: NodeClass::HasNativeCompletions,
            native: Some("export extern foo []".into()),
            ..Default::default()
        };
        match resolve(&probe, "foo", &[]) {
            Outcome::Native { nu } => assert_eq!(nu, "export extern foo []"),
            other => panic!("expected Native, got {other:?}"),
        }
        assert_eq!(probe.native_calls.get(), 1);
    }

    #[test]
    fn native_completions_never_fire_for_subcommands() {
        // the drift fix: a subcommand has no native payload of its own, so the
        // probe must not be consulted — otherwise `foo bar` would resolve to
        // foo's whole-tool completion payload.
        let probe = FakeProbe {
            class: NodeClass::HasNativeCompletions,
            native: Some("export extern foo []".into()),
            help: Some("HELP_FLAG".into()),
            ..Default::default()
        };
        match resolve(&probe, "foo", &["bar"]) {
            Outcome::Content { source, .. } => assert_eq!(source, "help"),
            other => panic!("expected help Content, got {other:?}"),
        }
        assert_eq!(
            probe.native_calls.get(),
            0,
            "native payload must not be probed for a subcommand"
        );
    }

    #[test]
    fn manpage_is_primary_over_help() {
        let probe = FakeProbe {
            manpage: Some("MAN_SUBS".into()),
            help: Some("HELP_SUBS".into()),
            ..Default::default()
        };
        match resolve(&probe, "foo", &[]) {
            Outcome::Content {
                source, children, ..
            } => {
                assert_eq!(source, "manpage");
                assert_eq!(children, vec!["build".to_string(), "check".to_string()]);
            }
            other => panic!("expected manpage Content, got {other:?}"),
        }
    }

    #[test]
    fn empty_manpage_falls_through_to_help() {
        let probe = FakeProbe {
            manpage: Some("MAN_EMPTY".into()),
            help: Some("HELP_FLAG".into()),
            ..Default::default()
        };
        match resolve(&probe, "foo", &[]) {
            Outcome::Content { source, .. } => assert_eq!(source, "help"),
            other => panic!("expected help Content, got {other:?}"),
        }
    }

    #[test]
    fn self_listed_subprobe_is_empty() {
        // `foo alpha --help` that echoes alpha in its own sub list = not a real
        // subcommand; discard.
        let probe = FakeProbe {
            help: Some("HELP_SELFLISTED".into()),
            ..Default::default()
        };
        assert!(matches!(resolve(&probe, "foo", &["alpha"]), Outcome::Empty));
    }

    #[test]
    fn nothing_anywhere_is_empty() {
        let probe = FakeProbe::default();
        assert!(matches!(resolve(&probe, "foo", &[]), Outcome::Empty));
    }

    #[test]
    fn help_pseudocommand_is_not_a_child() {
        let probe = FakeProbe {
            help: Some("HELP_SUBS".into()),
            ..Default::default()
        };
        // HELP_SUBS yields alpha/beta; add a manpage path that yields help+sub
        match resolve(&probe, "foo", &[]) {
            Outcome::Content { children, .. } => {
                assert!(!children.contains(&"help".to_string()));
            }
            other => panic!("expected Content, got {other:?}"),
        }
    }
}
