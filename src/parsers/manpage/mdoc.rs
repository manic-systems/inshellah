//! BSD mdoc format support. semantic macros rather than presentation:
//!   .Fl v    -> flag: -v
//!   .Ar file -> argument: file
//!   .Op ...  -> optional: [...]
//!   .Bl/.It/.El -> list begin/item/end
//!   .Sh      -> section header (lowercase 'h', vs groff's .SH)

use crate::{
    parsers::manpage::{
        ManpageEntry,
        ManpageResult,
        OwnedParam,
        OwnedSwitch,
        ParamChoice,
        ParamKind,
        groff::{
            GroffLine,
            strip_groff_escapes,
        },
    },
    types::Positional,
};

/// mdoc is detected by any .Sh macro.
pub fn is_mdoc(lines: &[GroffLine]) -> bool {
    lines
        .iter()
        .any(|line| matches!(line, GroffLine::Macro { name, .. } if name == "Sh"))
}

/// renderable text from an mdoc line, skipping structural macros.
fn mdoc_text_of(line: &GroffLine) -> Option<String> {
    match line {
        GroffLine::Text(text) => Some(strip_groff_escapes(text)),
        GroffLine::Macro { name, args } => {
            match name.as_str() {
                "Pp" | "Bl" | "El" | "Sh" | "Ss" | "Os" | "Dd" | "Dt" | "Oo" | "Oc" | "Op" => None,
                _ => {
                    let rendered = strip_groff_escapes(args);
                    let text = rendered.trim();
                    if text.is_empty() {
                        None
                    } else {
                        Some(text.to_owned())
                    }
                },
            }
        },
        GroffLine::Blank | GroffLine::Comment => None,
    }
}

/// the values of a `Cm a | b | c` run right behind a flag, e.g. ssh-keygen
/// `.It Fl t Cm rsa | ed25519`.
fn cm_choices_param(words: &[&str]) -> Option<OwnedParam> {
    let run: Vec<&str> = words
        .iter()
        .copied()
        .take_while(|word| !matches!(*word, "Ar" | "Op" | "Fl" | "Oo" | "Oc"))
        .collect();
    // `Cm os Ns = Ns Ar name` is a keyword glued to a value, not a choice list.
    if run.iter().any(|word| matches!(*word, "=" | "Ns")) {
        return None;
    }
    let values: Vec<&str> = run
        .into_iter()
        .filter(|word| !matches!(*word, "|" | "Cm" | "..." | ","))
        .collect();
    if values.is_empty() {
        return None;
    }
    let mut param = OwnedParam::new(ParamKind::Mandatory, values.join("|"));
    param.choices = values
        .into_iter()
        .map(|value| {
            ParamChoice {
                value: value.to_owned(),
                desc:  String::new(),
            }
        })
        .collect();
    Some(param)
}

/// parse an .It (list item) flag definition, e.g. ".It Fl v Ar file"
/// (Fl = flag, Ar = argument).
fn parse_mdoc_it(args: &str) -> Option<ManpageEntry> {
    let words: Vec<&str> = args.split(' ').filter(|w| !w.is_empty()).collect();
    let ["Fl", flag, tail @ ..] = words.as_slice() else {
        return None;
    };
    let param = match tail.strip_prefix(&["Ns"]).unwrap_or(tail) {
        ["Ar", name, ..] => Some(OwnedParam::new(ParamKind::Mandatory, (*name).to_owned())),
        ["Cm", choices @ ..] => {
            cm_choices_param(choices).or_else(|| {
                let ar = choices.iter().position(|word| *word == "Ar")?;
                let name = choices.get(ar + 1)?;
                Some(OwnedParam::new(ParamKind::Mandatory, (*name).to_owned()))
            })
        },
        _ => None,
    };
    let switch = if let [short] = flag.as_bytes()
        && short.is_ascii_alphanumeric()
    {
        OwnedSwitch::Short(char::from(*short))
    } else if let Some(long) = flag.strip_prefix('-')
        && !long.is_empty()
    {
        OwnedSwitch::Long(long.to_owned())
    } else {
        return None;
    };
    Some(ManpageEntry {
        switch,
        param,
        desc: String::new(),
    })
}

/// positional argument from an mdoc line (.Ar or .Op Ar).
fn positional_of_mdoc_line(args: &str) -> Option<(String, bool)> {
    let words: Vec<&str> = args.split(' ').filter(|w| !w.is_empty()).collect();
    let variadic = words.contains(&"...");
    match words.first() {
        Some(name) if name.len() >= 2 => Some((name.to_ascii_lowercase(), variadic)),
        _ => None,
    }
}

fn desc_of(lines: &[GroffLine], start: usize) -> (String, usize) {
    let mut acc: Vec<String> = Vec::new();
    let mut i = start;
    let mut nested = 0_usize;
    while i < lines.len() {
        if let GroffLine::Macro { name, .. } = &lines[i] {
            match name.as_str() {
                "Bl" => nested += 1,
                "El" if nested > 0 => nested -= 1,
                "It" if nested > 0 => (),
                "It" | "El" | "Sh" | "Ss" => break,
                _ => (),
            }
        }
        if let Some(text) = mdoc_text_of(&lines[i]) {
            acc.push(text);
        }
        i += 1;
    }
    (acc.join(" ").trim().to_owned(), i)
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

/// an item opened by .Xo spans lines up to .Xc. a page that never closes it
/// ends the block at the next item or list end, so later options survive.
/// returns the end of the block's content and the index after it.
fn xo_block_end(lines: &[GroffLine], start: usize) -> (usize, usize) {
    for (offset, line) in lines[start..].iter().enumerate() {
        if let GroffLine::Macro { name, .. } = line {
            match name.as_str() {
                "Xc" => return (start + offset, start + offset + 1),
                "It" | "El" | "Sh" | "Ss" => return (start + offset, start + offset),
                _ => (),
            }
        }
    }
    (lines.len(), lines.len())
}

/// the first argument name inside an .Xo block and the index after it.
fn parse_xo_block(lines: &[GroffLine], start: usize) -> (Option<OwnedParam>, usize) {
    let (end, after) = xo_block_end(lines, start);
    let param = lines[start..end].iter().find_map(|line| {
        let GroffLine::Macro { name, args } = line else {
            return None;
        };
        let mut words = args.split(' ').filter(|w| !w.is_empty());
        let found = match name.as_str() {
            "Ar" => words.next(),
            _ => words.skip_while(|w| *w != "Ar").nth(1),
        };
        found.map(|arg| OwnedParam::new(ParamKind::Mandatory, arg.to_owned()))
    });
    (param, after)
}

/// the item arguments of a bare `.It Xo`, whose macros sit on the lines up to
/// `.Xc` and not on the `.It` line. returns them with the index after the
/// block.
fn bare_xo_item(lines: &[GroffLine], start: usize) -> (String, usize) {
    let (end, after) = xo_block_end(lines, start);
    let item_args = lines[start..end]
        .iter()
        .filter_map(|line| {
            match line {
                GroffLine::Macro { name, args } => Some(format!("{name} {args}")),
                GroffLine::Text(_) | GroffLine::Blank | GroffLine::Comment => None,
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    (item_args, after)
}

fn parse_it(
    args: &str,
    lines: &[GroffLine],
    start: usize,
    entries: &mut Vec<ManpageEntry>,
) -> usize {
    let (item_args, desc_start, xo_param) = if args.trim() == "Xo" {
        let (item_args, after_xc) = bare_xo_item(lines, start);
        (item_args, after_xc, None)
    } else {
        args.strip_suffix(" Xo").map_or_else(
            || (args.to_owned(), start, None),
            |item_args| {
                let (param, after_xc) = parse_xo_block(lines, start);
                (item_args.to_owned(), after_xc, param)
            },
        )
    };
    let (desc, new_start) = desc_of(lines, desc_start);
    if let Some(mut entry) = parse_mdoc_it(&item_args) {
        entry.param = entry.param.or(xo_param);
        entry.desc = desc;
        push_entry(entries, entry);
    }
    new_start
}

/// `.It Fl M Cm generate` and `.It Fl M Cm screen` document one flag with a
/// closed value set across two items.
fn push_entry(entries: &mut Vec<ManpageEntry>, entry: ManpageEntry) {
    let earlier = entries.iter_mut().find(|earlier| {
        earlier.switch == entry.switch
            && earlier
                .param
                .as_ref()
                .is_some_and(|param| !param.choices.is_empty())
    });
    match (earlier, &entry.param) {
        (
            Some(ManpageEntry {
                param: Some(param), ..
            }),
            Some(incoming),
        ) if !incoming.choices.is_empty() => {
            for choice in &incoming.choices {
                if param
                    .choices
                    .iter()
                    .all(|known| known.value != choice.value)
                {
                    param.choices.push(choice.clone());
                }
            }
            param.name = param
                .choices
                .iter()
                .map(|choice| choice.value.as_str())
                .collect::<Vec<_>>()
                .join("|");
        },
        _ => entries.push(entry),
    }
}

fn parse_option_list(entries: &mut Vec<ManpageEntry>, lines: &[GroffLine], start: usize) -> usize {
    let mut i = start;
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Macro { name, .. } if name == "El" => return i + 1,
            GroffLine::Macro { name, args } if name == "It" => {
                i = parse_it(args, lines, i + 1, entries);
            },
            GroffLine::Macro { .. }
            | GroffLine::Text(_)
            | GroffLine::Blank
            | GroffLine::Comment => i += 1,
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
                if let Some((pos_name, variadic)) = positional_of_mdoc_line(args) {
                    positionals.push((pos_name, false, variadic));
                }
                i += 1;
            },
            GroffLine::Macro { name, args } if name == "Op" => {
                let words: Vec<&str> = args.split(' ').filter(|w| !w.is_empty()).collect();
                if matches!(words.first(), Some(&"Ar")) {
                    let rest = args.get(3..).unwrap_or("");
                    if let Some((pos_name, variadic)) = positional_of_mdoc_line(rest) {
                        positionals.push((pos_name, true, variadic));
                    }
                }
                i += 1;
            },
            GroffLine::Macro { .. }
            | GroffLine::Text(_)
            | GroffLine::Blank
            | GroffLine::Comment => i += 1,
        }
    }
    i
}

/// parse a full mdoc manpage: .Bl/.It/.El list blocks for flag definitions,
/// .Sh SYNOPSIS for positional arguments (.Ar, .Op Ar).
pub fn parse_mdoc_lines(lines: &[GroffLine]) -> ManpageResult {
    let mut entries: Vec<ManpageEntry> = Vec::new();
    let mut positionals: Vec<(String, bool, bool)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        // peek at the first .It to decide if this .Bl is a flag list
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
                let bare_xo_flag = matches!(words.as_slice(), ["Xo"])
                    && matches!(lines.get(j + 1), Some(GroffLine::Macro { name, .. }) if name == "Fl");
                if matches!(words.first(), Some(&"Fl")) || bare_xo_flag {
                    let next = parse_it(it_args, lines, j + 1, &mut entries);
                    i = parse_option_list(&mut entries, lines, next);
                    continue;
                }
                i = skip_to_el(lines, j + 1);
                continue;
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

    // dedup by name, keeping first-seen order
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
