// SPDX-License-Identifier: EUPL-1.2
//! one source-priority resolver for a command node: native completions, else
//! manpage (+help supplement), else --help. side effects go through [`Probe`]
//! so the priority logic stays pure and testable; the index pool and the
//! runtime walk supply their own executor and cache-write.

use crate::parsers::manpage::{ManpageResult, ManpageSubcommand};

/// what a probe thinks a binary is. only `Skip` drives behaviour (the index
/// skips it); the core uses `HasNativeCompletions` to decide whether to probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeClass {
    TryHelp,
    HasNativeCompletions,
    Skip,
}

/// side effects a resolve needs, abstracted so the pipeline stays pure. bound
/// to one binary; methods take only the per-node `sub_args`.
pub trait Probe {
    fn classify(&self) -> NodeClass;

    /// native nushell payload if the binary ships one. top level only.
    fn native_completions(&self) -> Option<String>;

    /// raw manpage for the hyphenated name (`git-remote`), or `None`.
    fn manpage(&self, hyphenated: &str) -> Option<String>;

    /// ansi-stripped `--help`/`-h` text, or `None` on failure/timeout/empty.
    fn help_text(&self, sub_args: &[String]) -> Option<String>;

    /// merge complementary `--help` data (descs, aliases, missing flags/subs/
    /// positionals) into a manpage result. `true` if anything was added.
    fn supplement_from_help(&self, result: &mut ManpageResult, sub_args: &[String]) -> bool;

    /// recover children for a group command whose manpage enumerated none
    /// (sibling `cmd-sub.N` pages and/or `--help`). `None` leaves the list.
    fn group_children(&self, hyphenated: &str, sub_args: &[String]) -> Option<Vec<ManpageSubcommand>>;
}

/// outcome of resolving one node; the driver persists it and decides recursion.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// native payload; describes the whole tool, so no children, no json.
    Native { nu: String },
    /// structured manpage/`--help` content. `source` is the cache tag;
    /// `children` are subcommand tokens to recurse.
    Content {
        result: ManpageResult,
        source: &'static str,
        children: Vec<String>,
    },
    /// nothing usable: empty parse, or a sub-probe that echoed its parent.
    Empty,
}

/// `base` + `sub_args` joined with spaces (`git stash apply`).
pub fn full_cmd(base: &str, sub_args: &[String]) -> String {
    if sub_args.is_empty() {
        base.to_string()
    } else {
        format!("{base} {}", sub_args.join(" "))
    }
}

/// `base` + `sub_args` joined with hyphens, the manpage name (`git-stash-apply`).
pub fn hyphenated_cmd(base: &str, sub_args: &[String]) -> String {
    if sub_args.is_empty() {
        base.to_string()
    } else {
        format!("{base}-{}", sub_args.join("-"))
    }
}

/// subcommand tokens worth recursing: >=2 chars, not a flag, not `help`.
pub fn child_tokens(subcommands: &[ManpageSubcommand]) -> Vec<String> {
    subcommands
        .iter()
        .map(|sc| sc.name.clone())
        .filter(|n| n.len() >= 2 && !n.starts_with('-') && n != "help")
        .collect()
}

/// sub-probe echoed its parent: the leaf appears in its own sub list, so the
/// binary didn't recognise it.
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

/// resolve one node in source priority: native (top level only), else manpage
/// as primary (+help supplement, +recovered group children), else `--help`.
/// parsers are injected so this stays free of parser wiring.
pub fn resolve_node(
    probe: &dyn Probe,
    base: &str,
    sub_args: &[String],
    parse_manpage: &dyn Fn(&str) -> ManpageResult,
    parse_help: &dyn Fn(&str) -> ManpageResult,
    strip_subcmd_prefix: &dyn Fn(&mut ManpageResult, &str),
    looks_like_unenumerated_group: &dyn Fn(&ManpageResult) -> bool,
) -> Outcome {
    // native, top level only. parsing the blob needs the full name + store
    // layer, so the driver does it; the raw blob is surfaced here.
    if sub_args.is_empty()
        && probe.classify() == NodeClass::HasNativeCompletions
        && let Some(nu) = probe.native_completions()
    {
        return Outcome::Native { nu };
    }

    let hyphenated = hyphenated_cmd(base, sub_args);

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
            positional_choices: Vec::new(),
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
            positional_choices: Vec::new(),
            positionals: Vec::new(),
            description: String::new(),
        }
    }

    /// counts native probes, to prove they don't fire for subcommands.
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
        // a subcommand has no native payload, so the probe must not fire, else
        // `foo bar` resolves to foo's whole-tool payload.
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
        match resolve(&probe, "foo", &[]) {
            Outcome::Content { children, .. } => {
                assert!(!children.contains(&"help".to_string()));
            }
            other => panic!("expected Content, got {other:?}"),
        }
    }
}
