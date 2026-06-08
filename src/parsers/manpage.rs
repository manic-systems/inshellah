// SPDX-License-Identifier: EUPL-1.2
//! parse unix manpages (groff/mdoc) into a structured result.

mod commands;
mod desc;
mod groff;
mod mdoc;
mod sections;
mod strategies;

use std::io::{self, Read};
use std::path::Path;

use crate::types::Positional;

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
    /// alternate invocations (cargo `b` for build, pw-cli `lm` for load-module);
    /// rendered as `(aka ...)` and accepted during descent, like switch shorts.
    pub aliases: Vec<String>,
}

impl ManpageSubcommand {
    pub fn new(name: String, desc: String) -> Self {
        Self {
            name,
            desc,
            aliases: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ManpageResult {
    pub entries: Vec<ManpageEntry>,
    pub subcommands: Vec<ManpageSubcommand>,
    /// prose-mined positional-slot values, kept out of `subcommands` so they never
    /// flow into real-child paths (recursion, supplement, prefix stripping).
    pub positional_choices: Vec<ManpageSubcommand>,
    pub positionals: Vec<(String, Positional)>,
    pub description: String,
}

impl ManpageResult {
    /// canonicalise so the cache holds one shape. runs once at the end of every
    /// parse path.
    pub fn normalize(&mut self) {
        let entries = std::mem::take(&mut self.entries);
        self.entries = dedup_entries(merge_short_long_pairs(entries));
        for e in &mut self.entries {
            clamp_description(&mut e.desc);
        }
        for sc in self
            .subcommands
            .iter_mut()
            .chain(&mut self.positional_choices)
        {
            clamp_description(&mut sc.desc);
        }
        clamp_description(&mut self.description);
    }
}

/// soft cap on a tooltip description; nushell truncates the menu anyway.
const MAX_DESC_LEN: usize = 256;

/// break on the first space at or after the cap so a word is never split; an
/// unbroken token is hard-cut.
pub(crate) fn clamp_description(desc: &mut String) {
    let Some((cut, _)) = desc.char_indices().nth(MAX_DESC_LEN) else {
        return;
    };
    let end = desc[cut..]
        .find(char::is_whitespace)
        .map(|off| cut + off)
        .unwrap_or(cut);
    desc.truncate(end);
    let trimmed = desc.trim_end().len();
    desc.truncate(trimmed);
    desc.push('…');
}

fn entry_key(e: &ManpageEntry) -> String {
    match &e.switch {
        OwnedSwitch::Short(c) => format!("-{c}"),
        OwnedSwitch::Long(l) | OwnedSwitch::Both(_, l) => format!("--{l}"),
    }
}

fn entry_score(e: &ManpageEntry) -> i32 {
    let switch_bonus = if matches!(e.switch, OwnedSwitch::Both(_, _)) {
        10
    } else {
        0
    };
    let param_bonus = if e.param.is_some() { 5 } else { 0 };
    let desc_bonus = (e.desc.len() / 10).min(5) as i32;
    switch_bonus + param_bonus + desc_bonus
}

type ShortAliasCandidate = (usize, char, Option<OwnedParam>);
type LongAliasCandidate<'a> = (usize, &'a str);

/// collapse duplicate entries for the same flag. manpages emit dups: clap lists
/// inherited globals, btrfs documents a flag as both a deprecated alias and a
/// global option, example blocks restate flags.
///
/// per key keep the highest-scoring entry, then strip standalone Shorts whose
/// char a surviving `Both` already covers. survivor sits at its key's first
/// occurrence so order is stable.
pub fn dedup_entries(entries: Vec<ManpageEntry>) -> Vec<ManpageEntry> {
    use std::collections::HashMap;
    use std::collections::HashSet;

    if entries.len() < 2 {
        return entries;
    }

    let mut best: HashMap<String, usize> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        let key = entry_key(e);
        match best.get(&key) {
            Some(&prev) if entry_score(&entries[prev]) >= entry_score(e) => {}
            _ => {
                best.insert(key, i);
            }
        }
    }

    let mut covered: HashSet<char> = HashSet::new();
    for &idx in best.values() {
        if let OwnedSwitch::Both(c, _) = &entries[idx].switch {
            covered.insert(*c);
        }
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<ManpageEntry> = Vec::with_capacity(entries.len());
    for e in entries.iter() {
        let key = entry_key(e);
        if seen.contains(&key) {
            continue;
        }
        if let OwnedSwitch::Short(c) = &e.switch
            && covered.contains(c)
        {
            continue;
        }
        seen.insert(key.clone());
        let best_idx = *best.get(&key).unwrap();
        out.push(entries[best_idx].clone());
    }
    out
}

/// merge non-adjacent Short/Long entries into a single `Both`. some styles emit
/// `-h` and `--help` as independent .TP/.IP blocks, not the comma-joined form
/// `combine_short_long_alternates` handles.
///
/// conservative since a wrong alias is worse than a missing one: the two must be
/// the only entries sharing that exact non-empty description, the short must
/// abbreviate the long, and tiny generic descriptions pair only for obvious
/// aliases.
pub fn merge_short_long_pairs(entries: Vec<ManpageEntry>) -> Vec<ManpageEntry> {
    use std::collections::HashMap;
    use std::collections::HashSet;

    // a flag restated across sections can yield all three forms (Both, standalone
    // Short, standalone Long); index existing Both so pairing doesn't emit a
    // second Both with the same (c, l).
    let mut existing_both: HashSet<(char, &str)> = HashSet::new();
    for e in entries.iter() {
        if let OwnedSwitch::Both(c, l) = &e.switch {
            existing_both.insert((*c, l.as_str()));
        }
    }

    let mut shorts_by_desc: HashMap<&str, Vec<ShortAliasCandidate>> = HashMap::new();
    let mut longs_by_desc: HashMap<&str, Vec<LongAliasCandidate<'_>>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if let OwnedSwitch::Short(c) = &e.switch
            && !e.desc.is_empty()
        {
            shorts_by_desc
                .entry(e.desc.as_str())
                .or_default()
                .push((i, *c, e.param.clone()));
        }
        if let OwnedSwitch::Long(l) = &e.switch
            && !e.desc.is_empty()
        {
            longs_by_desc
                .entry(e.desc.as_str())
                .or_default()
                .push((i, l.as_str()));
        }
    }
    if shorts_by_desc.is_empty() || longs_by_desc.is_empty() {
        return entries;
    }

    let mut to_drop: HashSet<usize> = HashSet::new();
    let mut out: Vec<ManpageEntry> = Vec::with_capacity(entries.len());
    for (i, e) in entries.iter().enumerate() {
        if let OwnedSwitch::Long(l) = &e.switch
            && !e.desc.is_empty()
            && let Some(shorts) = shorts_by_desc.get(e.desc.as_str())
            && let Some(longs) = longs_by_desc.get(e.desc.as_str())
            && shorts.len() == 1
            && longs.len() == 1
        {
            let (s_idx, c, s_param) = &shorts[0];
            if existing_both.contains(&(*c, l.as_str())) {
                // matching Both already present; drop the redundant standalone pair.
                to_drop.insert(*s_idx);
                to_drop.insert(i);
                out.push(e.clone());
            } else if *s_idx != i
                && !to_drop.contains(s_idx)
                && plausible_description_alias(*c, l, &e.desc)
            {
                to_drop.insert(*s_idx);
                out.push(ManpageEntry {
                    switch: OwnedSwitch::Both(*c, l.clone()),
                    param: e.param.clone().or_else(|| s_param.clone()),
                    desc: e.desc.clone(),
                });
            } else {
                out.push(e.clone());
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

fn plausible_description_alias(short: char, long: &str, desc: &str) -> bool {
    let short = short.to_ascii_lowercase();
    let first = long
        .chars()
        .find(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase());
    if first != Some(short) {
        return false;
    }
    if is_common_obvious_alias(short, long) {
        return true;
    }
    desc.split_whitespace().count() >= 4
}

fn is_common_obvious_alias(short: char, long: &str) -> bool {
    matches!(
        (short, long.to_ascii_lowercase().as_str()),
        ('h', "help") | ('v', "verbose") | ('v', "version")
    )
}

pub fn parse_manpage_lines(lines: &[GroffLine]) -> ManpageResult {
    let mut result = parse_manpage_lines_raw(lines);
    result.normalize();
    result
}

fn parse_manpage_lines_raw(lines: &[GroffLine]) -> ManpageResult {
    if mdoc::is_mdoc(lines) {
        mdoc::parse_mdoc_lines(lines)
    } else {
        let options_section = sections::extract_options_section(lines);
        let mut entries = strategies::extract_entries(&options_section);
        // flags declared only in the synopsis, never in OPTIONS. body wins on dup
        // names since it carries the description.
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
        let mut positionals = sections::extract_synopsis_positionals(lines);
        let commands_section = sections::extract_commands_section(lines);
        let mut subcommands = commands::extract_subcommands_from_commands(&commands_section);
        if subcommands.is_empty() {
            // jj/clap group pages list children as `.SH SUBCOMMANDS` xref entries
            // instead of an inline COMMANDS layout.
            let xref_section = sections::extract_subcommand_list_section(lines);
            if !xref_section.is_empty() {
                subcommands = commands::extract_subcommand_xrefs(&xref_section);
            }
        }
        if !subcommands.is_empty() {
            // drop the synopsis `<subcommands>` placeholder so it can't fall
            // through to file completion when a prefix matches no child.
            positionals.retain(|(name, _)| {
                !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "subcommand" | "subcommands" | "command" | "commands"
                )
            });
        }
        // prose-mined choices go to their own channel; skip collisions with a real
        // subcommand name.
        let positional_choices = sections::extract_description_positionals(lines)
            .into_iter()
            .filter(|choice| {
                !subcommands
                    .iter()
                    .any(|sc| sc.name.eq_ignore_ascii_case(&choice.name))
            })
            .collect();
        ManpageResult {
            entries,
            subcommands,
            positional_choices,
            positionals,
            description: String::new(),
        }
    }
}

pub fn parse_manpage_string(contents: &str) -> ManpageResult {
    let lines: Vec<GroffLine> = contents.split('\n').map(classify_line).collect();
    let mut result = parse_manpage_lines(&lines);
    if let Some(desc) = sections::extract_name_description(&lines) {
        result.description = desc;
    }
    result
}

/// also pull clap-style `.SH SUBCOMMAND` sections out as separate per-subcommand
/// results; the parent's subcommand list comes from their names. each sub keyed
/// by full command ("nh os").
pub fn parse_manpage_with_subs(contents: &str) -> (ManpageResult, Vec<(String, ManpageResult)>) {
    let lines: Vec<GroffLine> = contents.split('\n').map(classify_line).collect();
    let mut result = parse_manpage_lines(&lines);
    if let Some(desc) = sections::extract_name_description(&lines) {
        result.description = desc;
    }
    let sub_sections = sections::extract_subcommand_sections(&lines);
    if !sub_sections.is_empty() {
        // SUBCOMMAND-section names are authoritative for clap manpages.
        result.subcommands = sub_sections
            .iter()
            .map(|(name, desc, _)| {
                let mut desc = desc.clone();
                clamp_description(&mut desc);
                ManpageSubcommand::new(name.to_ascii_lowercase(), desc)
            })
            .collect();
    }
    // clap puts flags directly under the .SH SUBCOMMAND header with no inner .SH,
    // so parse_manpage_lines (which wants a child OPTIONS) comes back empty; parse
    // each body with the same strategy-picker as top-level OPTIONS.
    let subs: Vec<(String, ManpageResult)> = sub_sections
        .into_iter()
        .map(|(name, desc, lines)| {
            let mut positionals = sections::extract_synopsis_positionals(&lines);
            if positionals.is_empty() {
                positionals = sections::extract_usage_positionals_from_lines(&lines);
            }
            let mut sub_result = ManpageResult {
                entries: strategies::extract_entries(&lines),
                subcommands: Vec::new(),
                positional_choices: Vec::new(),
                positionals,
                description: desc,
            };
            sub_result.normalize();
            (name, sub_result)
        })
        .collect();
    (result, subs)
}

/// decompresses .gz (most installed manpages).
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

pub fn parse_manpage_file<P: AsRef<Path>>(path: P) -> io::Result<ManpageResult> {
    let contents = read_manpage_file(path)?;
    Ok(parse_manpage_string(&contents))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_leaves_short_descriptions_untouched() {
        let mut d = "a short description".to_string();
        clamp_description(&mut d);
        assert_eq!(d, "a short description");
        assert!(!d.ends_with('…'));
    }

    #[test]
    fn clamp_breaks_long_descriptions_on_a_word_boundary() {
        let word = "lorem ipsum ";
        let mut d = word.repeat(40); // ~480 chars
        clamp_description(&mut d);
        assert!(d.ends_with('…'));
        assert!(d.chars().count() <= MAX_DESC_LEN + 16);
        let body = d.trim_end_matches('…');
        assert!(!body.ends_with(' '));
        assert!(
            body.split(' ')
                .all(|w| w.is_empty() || w == "lorem" || w == "ipsum")
        );
    }

    #[test]
    fn clamp_hard_cuts_an_unbroken_token_with_no_trailing_space() {
        let mut d = "x".repeat(400);
        clamp_description(&mut d);
        assert_eq!(d.chars().count(), MAX_DESC_LEN + 1);
        assert!(d.ends_with('…'));
    }

    #[test]
    fn clamp_respects_utf8_char_boundaries() {
        // multibyte chars straddling the cap must not split a code point.
        let mut d = "é".repeat(400);
        clamp_description(&mut d);
        assert!(d.ends_with('…'));
        assert!(d.chars().all(|c| c == 'é' || c == '…'));
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
    fn merge_rejects_ambiguous_repeated_descriptions() {
        // two longs share a description with one short, so description equality
        // alone is not enough to synthesize either alias.
        let entries = vec![
            entry(OwnedSwitch::Short('h'), "show help"),
            entry(OwnedSwitch::Long("help".to_string()), "show help"),
            entry(OwnedSwitch::Long("usage".to_string()), "show help"),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(merged.len(), 3);
        assert!(matches!(merged[0].switch, OwnedSwitch::Short('h')));
        assert!(matches!(
            merged[1].switch,
            OwnedSwitch::Long(ref l) if l == "help"
        ));
        assert!(matches!(
            merged[2].switch,
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
        // ('v', "version"), a different long, so it should merge into
        // Both('v', "version") rather than drop as redundant.
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
                desc: "write command output to file".to_string(),
            },
            entry(
                OwnedSwitch::Long("output".to_string()),
                "write command output to file",
            ),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(merged.len(), 1);
        match &merged[0].param {
            Some(OwnedParam::Mandatory(p)) => assert_eq!(p, "FILE"),
            other => panic!("expected param carried over, got {other:?}"),
        }
    }
}
