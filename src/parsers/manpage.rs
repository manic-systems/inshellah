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
            entries: r.entries.iter().map(Into::into).collect(),
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

/// parse a manpage from its classified lines.
/// auto-detects mdoc vs groff format. for groff, runs the multi-strategy
/// extraction pipeline.
pub fn parse_manpage_lines(lines: &[GroffLine]) -> ManpageResult {
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
            let entries = strategies::extract_entries(&lines);
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
}
