//! parse unix manpages (groff/mdoc format) into a structured result.
//!
//! manpages are written in roff/groff markup — a decades-old typesetting language
//! used by man(1). this module strips the formatting and extracts structured data
//! (flags, subcommands, positionals) from the raw groff source.
//!
//! there are two major manpage macro packages:
//!   - man (groff) — used by gnu/linux tools. uses macros like .SH, .TP, .IP, .PP
//!   - mdoc (bsd) — used by bsd tools. uses .Sh, .Fl, .Ar, .Op, .It, .Bl/.El
//!
//! this module handles both, auto-detecting the format by checking for .Sh macros.
//!
//! for groff manpages, flag extraction uses multiple "strategies" that target
//! different common formatting patterns:
//!   - strategy_tp: .TP tagged paragraphs (gnu coreutils, help2man)
//!   - strategy_ip: .IP indented paragraphs (curl, hand-written)
//!   - strategy_pp_rs: .PP + .RS/.RE blocks (git, docbook)
//!   - strategy_nix: nix3-style bullet .IP with .UR/.UE hyperlinks
//!   - strategy_deroff: fallback — strip all groff, feed to help text parser
//!
//! the module tries all applicable strategies and picks the one that extracts
//! the most flag entries, on the theory that more results = better match.

mod commands;
mod groff;
mod mdoc;
mod sections;
mod strategies;

use std::io::{self, Read};
use std::path::Path;

use crate::types::{HelpResult, OptionEntry, Param, Positional, Subcommand, Switch};

pub use self::groff::{GroffLine, classify_line, strip_groff_escapes};
pub use self::sections::{extract_subcommand_sections, extract_synopsis_command};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedSwitch {
    Short(char),
    Long(String),
    Both(char, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedParam {
    Mandatory(String),
    Optional(String),
}

#[derive(Debug, Clone)]
pub struct ManpageEntry {
    pub switch: OwnedSwitch,
    pub param: Option<OwnedParam>,
    pub desc: String,
}

#[derive(Debug, Clone)]
pub struct ManpageSubcommand {
    pub name: String,
    pub desc: String,
}

#[derive(Debug, Clone, Default)]
pub struct ManpageResult {
    pub entries: Vec<ManpageEntry>,
    pub subcommands: Vec<ManpageSubcommand>,
    pub positionals: Vec<(String, Positional)>,
    pub description: String,
}

impl From<&Switch<'_>> for OwnedSwitch {
    fn from(s: &Switch<'_>) -> Self {
        match s {
            Switch::Short(c) => OwnedSwitch::Short(*c),
            Switch::Long(l) => OwnedSwitch::Long((*l).to_string()),
            Switch::Both(c, l) => OwnedSwitch::Both(*c, (*l).to_string()),
        }
    }
}

impl From<&Param<'_>> for OwnedParam {
    fn from(p: &Param<'_>) -> Self {
        match p {
            Param::Mandatory(s) => OwnedParam::Mandatory((*s).to_string()),
            Param::Optional(s) => OwnedParam::Optional((*s).to_string()),
        }
    }
}

impl From<&OptionEntry<'_>> for ManpageEntry {
    fn from(e: &OptionEntry<'_>) -> Self {
        let desc: String = e
            .desc
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        ManpageEntry {
            switch: (&e.switch).into(),
            param: e.param.as_ref().map(Into::into),
            desc,
        }
    }
}

impl From<&Subcommand<'_>> for ManpageSubcommand {
    fn from(sc: &Subcommand<'_>) -> Self {
        // lowercase the subcommand name here so (a) file naming is
        // consistent (meat_yum.json vs meat_YUM.json) and (b) recursive
        // --help probes use the lowercase form, which is what most real
        // CLIs accept — even tools like meat that DISPLAY uppercase
        // names in their help text dispatch on the lowercased argument.
        ManpageSubcommand {
            name: sc.name.to_ascii_lowercase(),
            desc: sc.desc.to_string(),
        }
    }
}

impl From<&HelpResult<'_>> for ManpageResult {
    fn from(r: &HelpResult<'_>) -> Self {
        ManpageResult {
            entries: merge_short_long_pairs(r.entries.iter().map(Into::into).collect()),
            subcommands: r.subcommands.iter().map(Into::into).collect(),
            // positional names are stored lowercased so output is
            // stable across the various places we extract them from
            // (synopsis, usage, cli11 sections).
            positionals: r
                .positionals
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
                .collect(),
            description: r.desc.to_string(),
        }
    }
}

/// merge non-adjacent Short/Long entries that share an identical, non-empty
/// description into a single `Both` entry. some manpage styles emit `-h` and
/// `--help` as independent .TP / .IP blocks rather than as the comma-joined
/// `-h, --help` form that `combine_short_long_alternates` already handles.
/// without this pass, two separate completions reach the runtime — and the
/// completer can't offer the "(aka --help) show help" / "(aka -h) show help"
/// cross-references that `Both` triggers.
///
/// pairing rule (deliberately conservative): a Short and a Long pair up only
/// when their `desc` fields are byte-equal and non-empty. matching on
/// description avoids false positives like merging `-n` (number) with
/// `--no-color`, where the short letter happens to match the long's first
/// letter but the flags are unrelated. each Short is consumed at most once,
/// each Long takes at most one Short — repeated descriptions don't cascade.
pub fn merge_short_long_pairs(entries: Vec<ManpageEntry>) -> Vec<ManpageEntry> {
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::collections::hash_map::Entry;

    // index `Both` pairs already present so we never synthesize a duplicate.
    // some manpages re-state a flag in multiple sections (an OPTIONS body
    // line `-h, --help` plus a SYNOPSIS-only stub) and the entry list ends
    // up with all three forms — Both, standalone Short, standalone Long —
    // for the same flag. without this check, pairing the standalone pair
    // would emit a second Both with the same (c, l).
    let mut existing_both: HashSet<(char, &str)> = HashSet::new();
    for e in entries.iter() {
        if let OwnedSwitch::Both(c, l) = &e.switch {
            existing_both.insert((*c, l.as_str()));
        }
    }

    let mut short_for_desc: HashMap<&str, (usize, char, Option<OwnedParam>)> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if let OwnedSwitch::Short(c) = &e.switch
            && !e.desc.is_empty()
            && let Entry::Vacant(slot) = short_for_desc.entry(e.desc.as_str())
        {
            slot.insert((i, *c, e.param.clone()));
        }
    }
    if short_for_desc.is_empty() {
        return entries;
    }

    let mut to_drop: HashSet<usize> = HashSet::new();
    let mut out: Vec<ManpageEntry> = Vec::with_capacity(entries.len());
    for (i, e) in entries.iter().enumerate() {
        if let OwnedSwitch::Long(l) = &e.switch
            && !e.desc.is_empty()
            && let Some((s_idx, c, s_param)) = short_for_desc.get(e.desc.as_str())
            && *s_idx != i
            && !to_drop.contains(s_idx)
        {
            if existing_both.contains(&(*c, l.as_str())) {
                // a Both(c, l) with the same chars already exists, so the
                // standalone Short+Long pair is redundant — drop both rather
                // than emit a duplicate Both.
                to_drop.insert(*s_idx);
                to_drop.insert(i);
                out.push(e.clone());
            } else {
                to_drop.insert(*s_idx);
                out.push(ManpageEntry {
                    switch: OwnedSwitch::Both(*c, l.clone()),
                    param: e.param.clone().or_else(|| s_param.clone()),
                    desc: e.desc.clone(),
                });
            }
        } else {
            out.push(e.clone());
        }
    }
    if to_drop.is_empty() {
        return out;
    }
    out.into_iter()
        .enumerate()
        .filter_map(|(i, e)| (!to_drop.contains(&i)).then_some(e))
        .collect()
}

/// parse a manpage from its classified lines.
/// auto-detects mdoc vs groff format. for groff, runs the multi-strategy
/// extraction pipeline.
pub fn parse_manpage_lines(lines: &[GroffLine]) -> ManpageResult {
    let mut result = parse_manpage_lines_raw(lines);
    result.entries = merge_short_long_pairs(result.entries);
    result
}

fn parse_manpage_lines_raw(lines: &[GroffLine]) -> ManpageResult {
    if mdoc::is_mdoc(lines) {
        mdoc::parse_mdoc_lines(lines)
    } else {
        let options_section = sections::extract_options_section(lines);
        let mut entries = strategies::extract_entries(&options_section);
        // merge SYNOPSIS-only flags (nix-env's `[{--profile | -p} path]`
        // pattern, where the flag is declared in the synopsis but never
        // listed as an entry in the OPTIONS body). body entries take
        // precedence on duplicate names — they carry the descriptions.
        let synopsis_flags = sections::extract_synopsis_flags(lines);
        if !synopsis_flags.is_empty() {
            let have_long: std::collections::HashSet<String> = entries
                .iter()
                .filter_map(|e| match &e.switch {
                    OwnedSwitch::Long(l) | OwnedSwitch::Both(_, l) => Some(l.to_ascii_lowercase()),
                    _ => None,
                })
                .collect();
            let have_short: std::collections::HashSet<char> = entries
                .iter()
                .filter_map(|e| match &e.switch {
                    OwnedSwitch::Short(c) | OwnedSwitch::Both(c, _) => Some(*c),
                    _ => None,
                })
                .collect();
            for e in synopsis_flags {
                let dup = match &e.switch {
                    OwnedSwitch::Long(l) => have_long.contains(&l.to_ascii_lowercase()),
                    OwnedSwitch::Short(c) => have_short.contains(c),
                    OwnedSwitch::Both(c, l) => {
                        have_short.contains(c) || have_long.contains(&l.to_ascii_lowercase())
                    }
                };
                if !dup {
                    entries.push(e);
                }
            }
        }
        let positionals = sections::extract_synopsis_positionals(lines);
        let commands_section = sections::extract_commands_section(lines);
        let mut subcommands = commands::extract_subcommands_from_commands(&commands_section);
        for positional in sections::extract_description_positionals(lines) {
            if !subcommands
                .iter()
                .any(|sc| sc.name.eq_ignore_ascii_case(&positional.name))
            {
                subcommands.push(positional);
            }
        }
        ManpageResult {
            entries,
            subcommands,
            positionals,
            description: String::new(),
        }
    }
}

/// parse a manpage from its raw string contents.
/// splits into lines, parses, then extracts the NAME section description.
pub fn parse_manpage_string(contents: &str) -> ManpageResult {
    let lines: Vec<GroffLine> = contents.split('\n').map(classify_line).collect();
    let mut result = parse_manpage_lines(&lines);
    if let Some(desc) = sections::extract_name_description(&lines) {
        result.description = desc;
    }
    result
}

/// parse a manpage and also pull out clap-style `.SH SUBCOMMAND` sections
/// as separate per-subcommand results. each subcommand section in a
/// clap-generated manpage is its own command with its own flags; the
/// parent's subcommand list is populated from their names.
///
/// returns (main_result, sub_results) where each sub_result has
/// name=full_command ("nh os"), desc, and its own ManpageResult.
pub fn parse_manpage_with_subs(contents: &str) -> (ManpageResult, Vec<(String, ManpageResult)>) {
    let lines: Vec<GroffLine> = contents.split('\n').map(classify_line).collect();
    let mut result = parse_manpage_lines(&lines);
    if let Some(desc) = sections::extract_name_description(&lines) {
        result.description = desc;
    }
    let sub_sections = sections::extract_subcommand_sections(&lines);
    if !sub_sections.is_empty() {
        // overwrite subcommands with the SUBCOMMAND-section names —
        // these are the authoritative list for clap-generated manpages.
        result.subcommands = sub_sections
            .iter()
            .map(|(name, desc, _)| ManpageSubcommand {
                name: name.to_ascii_lowercase(),
                desc: desc.clone(),
            })
            .collect();
    }
    // each SUBCOMMAND section body is parsed via the same strategy-picker
    // as the top-level OPTIONS section — clap puts flag definitions
    // directly under the .SH SUBCOMMAND header with no inner .SH wrapping,
    // so parse_manpage_lines (which looks for a child OPTIONS section)
    // would come back empty.
    let subs: Vec<(String, ManpageResult)> = sub_sections
        .into_iter()
        .map(|(name, desc, lines)| {
            let entries = merge_short_long_pairs(strategies::extract_entries(&lines));
            let sub_result = ManpageResult {
                entries,
                subcommands: Vec::new(),
                positionals: Default::default(),
                description: desc,
            };
            (name, sub_result)
        })
        .collect();
    (result, subs)
}

/// read a manpage file from disk. handles .gz compressed files (the common
/// case — most installed manpages are gzipped). plain text files are read directly.
pub fn read_manpage_file<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)?;
    if path.extension().and_then(|e| e.to_str()) == Some("gz") {
        let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
        let mut out = String::new();
        decoder.read_to_string(&mut out)?;
        Ok(out)
    } else {
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// read + parse a manpage file in one step.
pub fn parse_manpage_file<P: AsRef<Path>>(path: P) -> io::Result<ManpageResult> {
    let contents = read_manpage_file(path)?;
    Ok(parse_manpage_string(&contents))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TP_MANPAGE: &str = r#".TH FOO 1 "2024" "1.0" "User Commands"
.SH NAME
foo \- a synthetic test command
.SH SYNOPSIS
.B foo
[\fIOPTIONS\fR] <input> [output]
.SH OPTIONS
.TP
\fB\-v\fR, \fB\-\-verbose\fR
increase output verbosity
.TP
\fB\-o\fR \fIFILE\fR, \fB\-\-output\fR=\fIFILE\fR
write to FILE
.TP
\fB\-h\fR, \fB\-\-help\fR
show this help and exit
"#;

    #[test]
    fn tp_strategy_extracts_flags() {
        let r = parse_manpage_string(TP_MANPAGE);
        assert_eq!(
            r.entries.len(),
            3,
            "expected 3 entries, got {:?}",
            r.entries
        );
        assert_eq!(r.description, "a synthetic test command");
        assert!(matches!(
            r.entries[0].switch,
            OwnedSwitch::Both('v', ref l) if l == "verbose"
        ));
        assert!(matches!(
            r.entries[2].switch,
            OwnedSwitch::Both('h', ref l) if l == "help"
        ));
        assert!(r.entries[0].desc.contains("verbosity"));
    }

    #[test]
    fn mdoc_format_detected() {
        let src = ".Sh NAME\n.Nm test\n.Nd a test\n.Sh DESCRIPTION\nstuff\n";
        let lines: Vec<GroffLine> = src.split('\n').map(classify_line).collect();
        assert!(mdoc::is_mdoc(&lines));
    }

    #[test]
    fn groff_escapes_stripped() {
        let stripped = groff::strip_groff_escapes("\\fB\\-v\\fR \\fIfile\\fR");
        assert_eq!(stripped.trim(), "-v file");
    }

    fn entry(switch: OwnedSwitch, desc: &str) -> ManpageEntry {
        ManpageEntry {
            switch,
            param: None,
            desc: desc.to_string(),
        }
    }

    #[test]
    fn merges_non_adjacent_short_long_with_identical_desc() {
        let entries = vec![
            entry(OwnedSwitch::Short('h'), "show help"),
            entry(OwnedSwitch::Long("verbose".to_string()), "be verbose"),
            entry(OwnedSwitch::Long("help".to_string()), "show help"),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(merged.len(), 2, "expected the short to fold into the long");
        assert!(matches!(
            merged[0].switch,
            OwnedSwitch::Long(ref l) if l == "verbose"
        ));
        assert!(matches!(
            merged[1].switch,
            OwnedSwitch::Both('h', ref l) if l == "help"
        ));
    }

    #[test]
    fn merge_skips_empty_or_mismatched_descriptions() {
        let entries = vec![
            entry(OwnedSwitch::Short('q'), ""),
            entry(OwnedSwitch::Long("quiet".to_string()), ""),
            entry(OwnedSwitch::Short('n'), "number of results"),
            entry(OwnedSwitch::Long("no-color".to_string()), "disable colors"),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(merged.len(), 4, "no pair should merge");
        assert!(matches!(merged[0].switch, OwnedSwitch::Short('q')));
        assert!(matches!(merged[1].switch, OwnedSwitch::Long(ref l) if l == "quiet"));
        assert!(matches!(merged[2].switch, OwnedSwitch::Short('n')));
        assert!(matches!(merged[3].switch, OwnedSwitch::Long(ref l) if l == "no-color"));
    }

    #[test]
    fn merge_pairs_each_short_at_most_once() {
        // two longs share a description with a single short — the first long
        // wins, the second stays as-is.
        let entries = vec![
            entry(OwnedSwitch::Short('h'), "show help"),
            entry(OwnedSwitch::Long("help".to_string()), "show help"),
            entry(OwnedSwitch::Long("usage".to_string()), "show help"),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(merged.len(), 2);
        assert!(matches!(
            merged[0].switch,
            OwnedSwitch::Both('h', ref l) if l == "help"
        ));
        assert!(matches!(
            merged[1].switch,
            OwnedSwitch::Long(ref l) if l == "usage"
        ));
    }

    #[test]
    fn merge_drops_redundant_pair_when_both_already_present() {
        let entries = vec![
            ManpageEntry {
                switch: OwnedSwitch::Both('h', "help".to_string()),
                param: None,
                desc: "show help".to_string(),
            },
            entry(OwnedSwitch::Short('h'), "show help"),
            entry(OwnedSwitch::Long("help".to_string()), "show help"),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(
            merged.len(),
            1,
            "standalone short+long should drop when a Both with matching (c, l) already exists"
        );
        assert!(matches!(
            merged[0].switch,
            OwnedSwitch::Both('h', ref l) if l == "help"
        ));
    }

    #[test]
    fn merge_still_pairs_when_existing_both_has_different_long() {
        // a Both('v', "verbose") is in scope, but the standalone pair is
        // ('v', "version") — different long, so the pair should merge into
        // Both('v', "version") rather than being dropped as redundant.
        let entries = vec![
            ManpageEntry {
                switch: OwnedSwitch::Both('v', "verbose".to_string()),
                param: None,
                desc: "be verbose".to_string(),
            },
            entry(OwnedSwitch::Short('v'), "print version"),
            entry(OwnedSwitch::Long("version".to_string()), "print version"),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(merged.len(), 2);
        assert!(matches!(
            merged[0].switch,
            OwnedSwitch::Both('v', ref l) if l == "verbose"
        ));
        assert!(matches!(
            merged[1].switch,
            OwnedSwitch::Both('v', ref l) if l == "version"
        ));
    }

    #[test]
    fn merge_carries_short_param_when_long_has_none() {
        let entries = vec![
            ManpageEntry {
                switch: OwnedSwitch::Short('o'),
                param: Some(OwnedParam::Mandatory("FILE".to_string())),
                desc: "write output".to_string(),
            },
            entry(OwnedSwitch::Long("output".to_string()), "write output"),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(merged.len(), 1);
        match &merged[0].param {
            Some(OwnedParam::Mandatory(p)) => assert_eq!(p, "FILE"),
            other => panic!("expected param carried over, got {other:?}"),
        }
    }
}
