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
#![cfg(test)]

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use inshellah::parsers::help::help_parser;
use inshellah::parsers::manpage::{ManpageResult, OwnedSwitch, parse_manpage_string};

mod common;

fn render_switch(switch: &OwnedSwitch) -> String {
    match switch {
        OwnedSwitch::Short(short) => format!("-{short}"),
        OwnedSwitch::Long(long) => format!("--{long}"),
        OwnedSwitch::Both(short, long) => format!("-{short}|--{long}"),
    }
}

// both parsers now produce the same owned ManpageResult, so one renderer.
fn render(result: &ManpageResult) -> String {
    let mut out = String::new();
    out.push_str("subcommands:\n");
    for sc in &result.subcommands {
        writeln!(out, "  {}", sc.name).expect("write to String");
    }
    out.push_str("positional_choices:\n");
    for sc in &result.positional_choices {
        writeln!(out, "  {}", sc.name).expect("write to String");
    }
    out.push_str("flags:\n");
    for entry in &result.entries {
        writeln!(out, "  {}", render_switch(&entry.switch)).expect("write to String");
    }
    out.push_str("positionals:\n");
    for (name, _) in &result.positionals {
        writeln!(out, "  {name}").expect("write to String");
    }
    out
}

fn render_help(txt: &str) -> String {
    let (_, result) = help_parser(txt).expect("help parse");
    render(&result)
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
            .unwrap_or_else(|err| panic!("read fixture {file}: {err}"));
        let rendered = if Path::new(file).extension().is_some_and(|ext| ext == "txt") {
            render_help(&txt)
        } else {
            render_manpage(&txt)
        };
        let name = file.rsplit_once('.').map_or(*file, |(stem, _)| stem);
        common::check_golden("parser", name, "snap", &rendered);
    }
}
