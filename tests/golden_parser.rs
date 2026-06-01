// SPDX-License-Identifier: EUPL-1.2
//! Golden characterization of the parser layer over a small corpus of real
//! tool-output shapes. Guards the Phase 4 (model) and Phase 5 (parser dedup /
//! strategy selection) refactors: a behavior-preserving change leaves every
//! golden untouched; an intentional change (e.g. moving getent-style choices
//! out of `subcommands` into a positional-choices channel) is a deliberate
//! re-bless.
//!
//! `.txt` fixtures are parsed as `--help` text; `.1` fixtures as groff
//! manpages. The snapshot is a compact, stable projection (subcommands and
//! flags in parse order) — enough to pin the behaviors the refactor touches
//! without coupling to debug formatting.
//!
//! Re-bless: `INSHELLAH_BLESS=1 cargo test --test golden_parser`.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use inshellah::parsers::help::help_parser;
use inshellah::parsers::manpage::{ManpageResult, OwnedSwitch, parse_manpage_string};

mod common;

fn render_switch(s: &OwnedSwitch) -> String {
    match s {
        OwnedSwitch::Short(c) => format!("-{c}"),
        OwnedSwitch::Long(l) => format!("--{l}"),
        OwnedSwitch::Both(c, l) => format!("-{c}|--{l}"),
    }
}

// both parsers now produce the same owned ManpageResult, so one renderer.
fn render(r: &ManpageResult) -> String {
    let mut out = String::new();
    out.push_str("subcommands:\n");
    for sc in &r.subcommands {
        let _ = writeln!(out, "  {}", sc.name);
    }
    out.push_str("positional_choices:\n");
    for sc in &r.positional_choices {
        let _ = writeln!(out, "  {}", sc.name);
    }
    out.push_str("flags:\n");
    for e in &r.entries {
        let _ = writeln!(out, "  {}", render_switch(&e.switch));
    }
    out.push_str("positionals:\n");
    for (name, _) in &r.positionals {
        let _ = writeln!(out, "  {name}");
    }
    out
}

fn render_help(txt: &str) -> String {
    let (_, r) = help_parser(txt).expect("help parse");
    render(&r)
}

fn render_manpage(txt: &str) -> String {
    render(&parse_manpage_string(txt))
}

// jj is clap-generated with a separate `Global Options:` section, short+long
// flag pairs that fold into `Both`, and revset/bookmark value flags. these are
// real `jj <sub> --help` captures: a regression in jj flag coverage (the hard
// case to keep fully covered) shows up as a golden diff here.
const CASES: &[&str] = &[
    "cargo_help.txt",
    "getent.1",
    "widget.1",
    "jj-squash.txt",
    "jj-rebase.txt",
    "jj-log.txt",
    "jj-git-push.txt",
    "jj-bookmark-set.txt",
];

#[test]
fn golden_parser_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parser");
    for file in CASES {
        let txt = fs::read_to_string(dir.join(file))
            .unwrap_or_else(|e| panic!("read fixture {file}: {e}"));
        let rendered = if file.ends_with(".txt") {
            render_help(&txt)
        } else {
            render_manpage(&txt)
        };
        let name = file.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(file);
        common::check_golden("parser", name, "snap", &rendered);
    }
}
