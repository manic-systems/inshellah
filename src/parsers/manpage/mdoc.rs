//! BSD mdoc format support.
//!
//! mdoc is the bsd manpage macro package. it uses semantic macros rather than
//! presentation macros:
//!   .Fl v    -> flag: -v
//!   .Ar file -> argument: file
//!   .Op ...  -> optional: [...]
//!   .Bl/.It/.El -> list begin/item/end
//!   .Sh      -> section header (note lowercase 'h', vs groff's .SH)

use crate::parsers::manpage::groff::{GroffLine, strip_groff_escapes};
use crate::parsers::manpage::{ManpageEntry, ManpageResult, OwnedParam, OwnedSwitch};
use crate::types::Positional;

/// detect mdoc format by looking for any .Sh macro.
pub fn is_mdoc(lines: &[GroffLine]) -> bool {
    lines
        .iter()
        .any(|l| matches!(l, GroffLine::Macro { name, .. } if name == "Sh"))
}

/// extract renderable text from an mdoc line, skipping structural macros.
fn mdoc_text_of(line: &GroffLine) -> Option<String> {
    match line {
        GroffLine::Text(t) => Some(strip_groff_escapes(t)),
        GroffLine::Macro { name, args } => match name.as_str() {
            "Pp" | "Bl" | "El" | "Sh" | "Ss" | "Os" | "Dd" | "Dt" | "Oo" | "Oc" | "Op" => None,
            _ => {
                let text = strip_groff_escapes(args);
                let text = text.trim();
                if text.is_empty() {
                    None
                } else {
                    Some(text.to_string())
                }
            }
        },
        _ => None,
    }
}

/// parse an mdoc .It (list item) line that contains flag definitions.
/// mdoc .It lines look like: ".It Fl v Ar file"
/// where Fl = flag, Ar = argument.
fn parse_mdoc_it(args: &str) -> Option<ManpageEntry> {
    let words: Vec<&str> = args
        .split(' ')
        .filter(|w| !w.is_empty() && *w != "Ns")
        .collect();
    let param = match words.as_slice() {
        [_, _, "Ar", name, ..] => Some(OwnedParam::Mandatory(name.to_string())),
        _ => None,
    };
    match words.as_slice() {
        ["Fl", ch, ..] if ch.len() == 1 && ch.chars().next().unwrap().is_ascii_alphanumeric() => {
            Some(ManpageEntry {
                switch: OwnedSwitch::Short(ch.chars().next().unwrap()),
                param,
                desc: String::new(),
            })
        }
        ["Fl", name, ..] if name.len() > 1 && name.starts_with('-') => Some(ManpageEntry {
            switch: OwnedSwitch::Long(name[1..].to_string()),
            param,
            desc: String::new(),
        }),
        _ => None,
    }
}

/// extract a positional argument from an mdoc line (.Ar or .Op Ar).
fn positional_of_mdoc_line(args: &str) -> Option<(String, bool)> {
    let words: Vec<&str> = args.split(' ').filter(|w| !w.is_empty()).collect();
    let variadic = words.contains(&"...");
    match words.first() {
        Some(name) if name.len() >= 2 => Some((name.to_ascii_lowercase(), variadic)),
        _ => None,
    }
}

/// parse an entire mdoc-format manpage.
/// walks through all classified lines looking for:
///   1. .Bl/.It/.El list blocks containing flag definitions
///   2. .Sh SYNOPSIS sections containing positional arguments (.Ar, .Op Ar)
pub fn parse_mdoc_lines(lines: &[GroffLine]) -> ManpageResult {
    // collect description for an entry — until next structural macro
    fn desc_of(lines: &[GroffLine], start: usize) -> (String, usize) {
        let mut acc: Vec<String> = Vec::new();
        let mut i = start;
        while i < lines.len() {
            if let GroffLine::Macro { name, .. } = &lines[i]
                && matches!(name.as_str(), "It" | "El" | "Sh" | "Ss")
            {
                break;
            }
            if let Some(t) = mdoc_text_of(&lines[i]) {
                acc.push(t);
            }
            i += 1;
        }
        (acc.join(" ").trim().to_string(), i)
    }

    fn skip_to_el(lines: &[GroffLine], start: usize) -> usize {
        let mut i = start;
        while i < lines.len() {
            if let GroffLine::Macro { name, .. } = &lines[i]
                && name == "El"
            {
                return i + 1;
            }
            i += 1;
        }
        i
    }

    /// parse a single .It entry: extract flag, collect description.
    fn parse_it(
        args: &str,
        lines: &[GroffLine],
        start: usize,
        entries: &mut Vec<ManpageEntry>,
    ) -> usize {
        let (desc, new_start) = desc_of(lines, start);
        if let Some(mut entry) = parse_mdoc_it(args) {
            entry.desc = desc;
            entries.push(entry);
        }
        new_start
    }

    /// parse all .It entries within a .Bl/.El option list.
    fn parse_option_list(
        entries: &mut Vec<ManpageEntry>,
        lines: &[GroffLine],
        start: usize,
    ) -> usize {
        let mut i = start;
        while i < lines.len() {
            match &lines[i] {
                GroffLine::Macro { name, .. } if name == "El" => return i + 1,
                GroffLine::Macro { name, args } if name == "It" => {
                    i = parse_it(args, lines, i + 1, entries);
                }
                _ => i += 1,
            }
        }
        i
    }

    fn parse_synopsis(
        positionals: &mut Vec<(String, bool, bool)>,
        lines: &[GroffLine],
        start: usize,
    ) -> usize {
        let mut i = start;
        while i < lines.len() {
            match &lines[i] {
                GroffLine::Macro { name, .. } if name == "Sh" => return i,
                GroffLine::Macro { name, args } if name == "Ar" => {
                    if let Some((n, v)) = positional_of_mdoc_line(args) {
                        positionals.push((n, false, v));
                    }
                    i += 1;
                }
                GroffLine::Macro { name, args } if name == "Op" => {
                    let words: Vec<&str> = args.split(' ').filter(|w| !w.is_empty()).collect();
                    if matches!(words.first(), Some(&"Ar")) {
                        let rest = if args.len() > 3 { &args[3..] } else { "" };
                        if let Some((n, v)) = positional_of_mdoc_line(rest) {
                            positionals.push((n, true, v));
                        }
                    }
                    i += 1;
                }
                _ => i += 1,
            }
        }
        i
    }

    let mut entries: Vec<ManpageEntry> = Vec::new();
    let mut positionals: Vec<(String, bool, bool)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        // .Bl + .It header sequence — peek at first .It to decide if this is a flag list
        if let GroffLine::Macro { name: n1, .. } = &lines[i]
            && n1 == "Bl"
        {
            let j = i + 1;
            if j < lines.len()
                && let GroffLine::Macro {
                    name: n2,
                    args: it_args,
                } = &lines[j]
                && n2 == "It"
            {
                let words: Vec<&str> = it_args.split(' ').filter(|w| !w.is_empty()).collect();
                if matches!(words.first(), Some(&"Fl")) {
                    let k = parse_it(it_args, lines, j + 1, &mut entries);
                    i = parse_option_list(&mut entries, lines, k);
                    continue;
                } else {
                    i = skip_to_el(lines, j + 1);
                    continue;
                }
            }
            i = skip_to_el(lines, j);
            continue;
        }
        if let GroffLine::Macro { name, args } = &lines[i]
            && name == "Sh"
            && args.trim().eq_ignore_ascii_case("SYNOPSIS")
        {
            i = parse_synopsis(&mut positionals, lines, i + 1);
            continue;
        }
        i += 1;
    }

    // deduplicate positionals by name, preserving first-seen order
    let mut seen: Vec<String> = Vec::new();
    let mut deduped: Vec<(String, Positional)> = Vec::new();
    for (name, optional, variadic) in positionals {
        if !seen.contains(&name) {
            seen.push(name.clone());
            deduped.push((name, Positional { optional, variadic }));
        }
    }

    ManpageResult {
        entries,
        subcommands: Vec::new(),
        positional_choices: Vec::new(),
        positionals: deduped,
        description: String::new(),
    }
}
