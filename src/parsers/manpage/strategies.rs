//! one strategy per groff formatting pattern, since authors use different macro
//! combinations for the same purpose.

use nom::{Parser, combinator::opt};

use crate::make_macro_walker;
use crate::parsers::help::{help_parser, param_parser, switch_parser};
use crate::parsers::manpage::desc;
use crate::parsers::manpage::groff::{
    GroffLine, strip_groff_escapes, strip_inline_macro_args, strip_space_macro_args,
};
use crate::parsers::manpage::{ManpageEntry, OwnedParam, OwnedSwitch};

fn collect_text_lines(lines: &[GroffLine]) -> (String, &[GroffLine]) {
    let mut acc: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Text(t) => acc.push(t),
            _ => break,
        }
        i += 1;
    }
    (acc.join(" "), &lines[i..])
}

fn collect_description_lines(lines: &[GroffLine], start: usize) -> (String, usize) {
    // stop_on_blank caps clap-style "summary\n\nbody" entries at the summary
    desc::collect(
        lines,
        start,
        desc::DescOpts {
            boundaries: &["TP", "TQ", "IP", "PP", "SH", "SS"],
            skip_rs: false,
            stop_on_blank: true,
            tags: desc::TagMacros::Wide,
        },
    )
}

/// parse a tag string (e.g. "-v, --verbose FILE") into an entry, None if it
/// isn't a flag definition.
pub fn parse_tag_to_entry(tag: &str, desc: String) -> Option<ManpageEntry> {
    let tag = strip_groff_escapes(tag);
    let tag = tag.trim();
    let result: nom::IResult<&str, (OwnedSwitch, Option<OwnedParam>)> =
        (switch_parser, opt(param_parser)).parse(tag);
    match result {
        Ok((_, (switch, param))) => Some(ManpageEntry { switch, param, desc }),
        Err(_) => None,
    }
}

/// .B and .I preserve spaces (single argument); .BI, .BR, .IR alternate fonts
/// and concatenate arguments.
pub fn tag_of_macro(name: &str, args: &str) -> String {
    match name {
        "B" | "I" => strip_space_macro_args(args),
        _ => strip_groff_escapes(&strip_inline_macro_args(args))
            .trim()
            .to_string(),
    }
}

// .TP style (gnu coreutils, help2man). line after .TP is the tag (plain or
// wrapped in .B/.BI/...), then description text lines.
pub fn strategy_tp(lines: &[GroffLine]) -> Vec<ManpageEntry> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let GroffLine::Macro { name, .. } = &lines[i] else {
            i += 1;
            continue;
        };
        if name != "TP" {
            i += 1;
            continue;
        }

        let (tags, body_start) = collect_tp_tags(lines, i + 1);
        if tags.is_empty() {
            i += 1;
            continue;
        }
        let (desc, new_i) = collect_description_lines(lines, body_start);
        out.extend(entries_from_tag_alternates(&tags, desc));
        i = new_i;
    }
    out
}

fn collect_tp_tags(lines: &[GroffLine], start: usize) -> (Vec<String>, usize) {
    let mut tags = Vec::new();
    let mut i = start;
    loop {
        if i >= lines.len() {
            break;
        }
        let Some(tag) = tag_from_line(&lines[i]) else {
            break;
        };
        tags.push(tag);
        i += 1;
        if i < lines.len() && matches!(&lines[i], GroffLine::Macro { name, .. } if name == "TQ") {
            i += 1;
            continue;
        }
        break;
    }
    (tags, i)
}

fn tag_from_line(line: &GroffLine) -> Option<String> {
    match line {
        GroffLine::Text(tag) => Some(tag.clone()),
        GroffLine::Macro { name, args }
            if matches!(
                name.as_str(),
                "B" | "I" | "BI" | "BR" | "IR" | "IB" | "RB" | "RI"
            ) =>
        {
            Some(tag_of_macro(name, args))
        }
        _ => None,
    }
}

fn entries_from_tag_alternates(tags: &[String], desc: String) -> Vec<ManpageEntry> {
    let entries: Vec<ManpageEntry> = tags
        .iter()
        .filter_map(|tag| parse_tag_to_entry(tag, desc.clone()))
        .collect();
    if entries.len() == 2
        && let Some(combined) = combine_short_long_alternates(&entries[0], &entries[1])
    {
        return vec![combined];
    }
    entries
}

fn combine_short_long_alternates(
    left: &ManpageEntry,
    right: &ManpageEntry,
) -> Option<ManpageEntry> {
    match (&left.switch, &right.switch) {
        (OwnedSwitch::Long(l), OwnedSwitch::Short(c)) => Some(ManpageEntry {
            switch: OwnedSwitch::Both(*c, l.clone()),
            param: left.param.clone().or_else(|| right.param.clone()),
            desc: left.desc.clone(),
        }),
        (OwnedSwitch::Short(c), OwnedSwitch::Long(l)) => Some(ManpageEntry {
            switch: OwnedSwitch::Both(*c, l.clone()),
            param: right.param.clone().or_else(|| left.param.clone()),
            desc: left.desc.clone(),
        }),
        _ => None,
    }
}

// .IP style (curl, hand-written manpages). inline tag arg `.IP "-v, --verbose"`.
// nested .IP inside .RS lists example values that look like flags, so only
// outer-scope .IP counts.
pub fn strategy_ip(lines: &[GroffLine]) -> Vec<ManpageEntry> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut rs_depth: u32 = 0;
    while i < lines.len() {
        if let GroffLine::Macro { name, args } = &lines[i] {
            match name.as_str() {
                "RS" => {
                    rs_depth += 1;
                    i += 1;
                    continue;
                }
                "RE" => {
                    rs_depth = rs_depth.saturating_sub(1);
                    i += 1;
                    continue;
                }
                "IP" if rs_depth == 0 => {
                    let tag = strip_groff_escapes(args);
                    let (desc, rest) = collect_text_lines(&lines[i + 1..]);
                    let new_i = lines.len() - rest.len();
                    if let Some(entry) = parse_tag_to_entry(&tag, desc) {
                        out.push(entry);
                        i = new_i;
                        continue;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    out
}

// .HP hanging-indent (bat, help2man). next text line is the tag, then an empty
// `.IP` opens the description body. `.RS/.RE` listings skipped.
//
//   .HP
//   \fB\-A\fR, \fB\-\-show\-all\fR
//   .IP
//   Show non-printable characters ...
pub fn strategy_hp(lines: &[GroffLine]) -> Vec<ManpageEntry> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let GroffLine::Macro { name, .. } = &lines[i] else {
            i += 1;
            continue;
        };
        if name != "HP" {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < lines.len() && matches!(&lines[j], GroffLine::Blank | GroffLine::Comment) {
            j += 1;
        }
        let Some(tag) = lines.get(j).and_then(tag_from_line) else {
            i += 1;
            continue;
        };
        let mut body_start = j + 1;
        if let Some(GroffLine::Macro { name, .. }) = lines.get(body_start)
            && name == "IP"
        {
            body_start += 1;
        }
        let (desc, new_i) = collect_hp_description(lines, body_start);
        if let Some(entry) = parse_tag_to_entry(&tag, desc) {
            out.push(entry);
        }
        i = if new_i > i { new_i } else { i + 1 };
    }
    out
}

/// `.HP` description collector. `.RS/.RE` blocks are sub-value listings, not
/// the flag's own description.
fn collect_hp_description(lines: &[GroffLine], start: usize) -> (String, usize) {
    desc::collect(
        lines,
        start,
        desc::DescOpts {
            boundaries: &["HP", "TP", "TQ", "PP", "SH", "SS"],
            skip_rs: true,
            stop_on_blank: true,
            tags: desc::TagMacros::Wide,
        },
    )
}

// bare Text tag immediately followed by `.RS/.RE` (ripgrep, some help2man
// variants). like `.PP+.RS` but with no `.PP` anchor; flags sit directly under
// `.SS` headers separated only by `.sp`:
//
//   .SS INPUT OPTIONS
//   \fB\-e\fP \fIPATTERN\fP, \fB\-\-regexp\fP=\fIPATTERN\fP
//   .RS 4
//   A pattern to search for ...
//   .RE
//
// a top-level Text line is a tag only when an `.RS` immediately follows
// (skipping blanks/comments) and it starts with `-`. depth tracking skips
// nested Text so a description paragraph starting with a flag isn't taken as a
// tag.
pub fn strategy_text_rs(lines: &[GroffLine]) -> Vec<ManpageEntry> {
    let mut out = Vec::new();
    let mut rs_depth: u32 = 0;
    let mut i = 0;
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Macro { name, .. } if name == "RS" => {
                rs_depth += 1;
                i += 1;
            }
            GroffLine::Macro { name, .. } if name == "RE" => {
                rs_depth = rs_depth.saturating_sub(1);
                i += 1;
            }
            GroffLine::Text(tag) if rs_depth == 0 && tag.trim_start().starts_with('-') => {
                let mut j = i + 1;
                while j < lines.len() && matches!(&lines[j], GroffLine::Blank | GroffLine::Comment)
                {
                    j += 1;
                }
                if let Some(GroffLine::Macro { name, .. }) = lines.get(j)
                    && name == "RS"
                {
                    let (desc, new_i) = collect_pp_rs_desc(lines, j);
                    if let Some(entry) = parse_tag_to_entry(tag, desc) {
                        out.push(entry);
                        i = if new_i > i { new_i } else { i + 1 };
                        continue;
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

// .PP + plain-text flag name + .RS description block + .RE (git,
// docbook-generated manpages).
make_macro_walker!(pub strategy_pp_rs -> Vec<ManpageEntry>, on macro "PP" =>
    |lines, i, _args| {
        if i + 1 >= lines.len() { return None; }
        if let GroffLine::Text(tag) = &lines[i + 1] {
            let (desc, new_i) = collect_pp_rs_desc(lines, i + 2);
            parse_tag_to_entry(tag, desc).map(|e| (e, new_i))
        } else {
            None
        }
    }
);

fn collect_pp_rs_desc(lines: &[GroffLine], start: usize) -> (String, usize) {
    let mut acc: Vec<String> = Vec::new();
    let mut i = start;
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Macro { name, .. } if name == "RS" => {
                // depth-tracked so a nested sub-value `.RE` doesn't end the
                // description early.
                let mut depth: u32 = 1;
                i += 1;
                while i < lines.len() && depth > 0 {
                    match &lines[i] {
                        GroffLine::Macro { name, .. } if name == "RS" => {
                            depth += 1;
                            i += 1;
                        }
                        GroffLine::Macro { name, .. } if name == "RE" => {
                            depth -= 1;
                            i += 1;
                        }
                        GroffLine::Text(t) => {
                            // deeper .RS Text is a sub-value listing, not desc.
                            if depth == 1 {
                                acc.push(t.clone());
                            }
                            i += 1;
                        }
                        GroffLine::Macro { name, .. }
                            if name == "PP" || name == "SH" || name == "SS" =>
                        {
                            // abort even on an unclosed .RS so this can't run to EOF.
                            return (acc.join(" "), i);
                        }
                        _ => i += 1,
                    }
                }
                return (acc.join(" "), i);
            }
            GroffLine::Text(t) => {
                acc.push(t.clone());
                i += 1;
            }
            _ => return (acc.join(" "), i),
        }
    }
    (acc.join(" "), i)
}

/// deroff fallback. strip all groff markup, feed plain text to the help parser.
pub fn strategy_deroff(lines: &[GroffLine]) -> Vec<ManpageEntry> {
    let mut buffer = String::with_capacity(256);
    for line in lines {
        match line {
            GroffLine::Text(text) => {
                buffer.push_str(text);
                buffer.push('\n');
            }
            GroffLine::Macro { name, args }
                if matches!(name.as_str(), "BI" | "BR" | "IR" | "B" | "I") =>
            {
                let text = strip_groff_escapes(&strip_inline_macro_args(args));
                buffer.push_str(&text);
                buffer.push('\n');
            }
            GroffLine::Blank => buffer.push('\n'),
            _ => (),
        }
    }
    match help_parser(&buffer) {
        Ok((_, result)) => result.entries,
        Err(_) => Vec::new(),
    }
}

fn is_bullet_ip(args: &str) -> bool {
    !args.trim().is_empty()
}

// nix3-style bullet .IP with .UR/.UE hyperlinks. tag is the text after a bullet
// .IP (interleaved with .UR/.UE); description follows a non-bullet .IP marker.
make_macro_walker!(pub strategy_nix -> Vec<ManpageEntry>, on macro "IP" =>
    |lines, i, args| {
        if !is_bullet_ip(args) { return None; }
        let mut tag_idx = i + 1;
        let mut tag_parts: Vec<String> = Vec::new();
        while tag_idx < lines.len() {
            match &lines[tag_idx] {
                GroffLine::Macro { name, .. } if name == "UR" || name == "UE" => {
                    tag_idx += 1;
                }
                GroffLine::Text(t) => {
                    tag_parts.push(t.clone());
                    tag_idx += 1;
                }
                _ => break,
            }
        }
        let tag = tag_parts.join(" ");
        let (desc, new_i) = collect_nix_desc(lines, tag_idx);
        parse_tag_to_entry(&tag, desc).map(|e| (e, new_i))
    }
);

fn collect_nix_desc(lines: &[GroffLine], start: usize) -> (String, usize) {
    if start >= lines.len() {
        return (String::new(), start);
    }
    let mut i = start;
    // a non-bullet .IP marker opens the description body
    if let GroffLine::Macro { name, args } = &lines[i]
        && name == "IP"
        && args.trim().is_empty()
    {
        i += 1;
    } else {
        return (String::new(), start);
    }
    let mut parts: Vec<String> = Vec::new();
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Text(t) => {
                parts.push(t.clone());
                i += 1;
            }
            GroffLine::Macro { name, args } if name == "IP" => {
                if !args.trim().is_empty() {
                    // bullet = next entry; non-bullet = continuation paragraph
                    return (parts.join(" "), i);
                }
                i += 1;
            }
            GroffLine::Macro { name, .. } if name == "SS" || name == "SH" => {
                return (parts.join(" "), i);
            }
            GroffLine::Macro { name, .. } if name == "RS" => {
                i = skip_rs(lines, i + 1, 1);
            }
            GroffLine::Macro { .. } => {
                i += 1;
            }
            GroffLine::Blank | GroffLine::Comment => {
                i += 1;
            }
        }
    }
    (parts.join(" "), i)
}

fn skip_rs(lines: &[GroffLine], start: usize, mut depth: usize) -> usize {
    let mut i = start;
    while i < lines.len() {
        if let GroffLine::Macro { name, .. } = &lines[i] {
            if name == "RE" {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            } else if name == "RS" {
                depth += 1;
            }
        }
        i += 1;
    }
    i
}

fn count_macro(name: &str, lines: &[GroffLine]) -> usize {
    lines
        .iter()
        .filter(|line| matches!(line, GroffLine::Macro { name: n, .. } if n == name))
        .count()
}

fn specialized_candidates(lines: &[GroffLine]) -> Vec<(&'static str, Vec<ManpageEntry>)> {
    let tp = count_macro("TP", lines);
    let ip = count_macro("IP", lines);
    let pp = count_macro("PP", lines);
    let rs = count_macro("RS", lines);
    let ur = count_macro("UR", lines);
    let hp = count_macro("HP", lines);

    let mut specialized: Vec<(&str, Vec<ManpageEntry>)> = Vec::new();
    if tp > 0 {
        specialized.push(("TP", strategy_tp(lines)));
    }
    if ip > 0 {
        specialized.push(("IP", strategy_ip(lines)));
    }
    if pp > 0 && rs > 0 {
        specialized.push(("PP+RS", strategy_pp_rs(lines)));
    }
    if ur > 0 && ip > 0 {
        specialized.push(("nix", strategy_nix(lines)));
    }
    if hp > 0 {
        specialized.push(("HP", strategy_hp(lines)));
    }
    if rs > 0 {
        specialized.push(("Text+RS", strategy_text_rs(lines)));
    }
    specialized
        .into_iter()
        .filter(|(_, e)| !e.is_empty())
        .collect()
}

/// tie-break priority, higher wins on equal entry counts. explicit so
/// reordering the candidate list can't silently change the winner.
fn strategy_priority(tag: &str) -> u8 {
    match tag {
        "TP" => 0,
        "IP" => 1,
        "PP+RS" => 2,
        "nix" => 3,
        "HP" => 4,
        "Text+RS" => 5,
        _ => 0,
    }
}

/// most entries first, then the `strategy_priority` tie-break.
fn best_entries(candidates: Vec<(&'static str, Vec<ManpageEntry>)>) -> Option<Vec<ManpageEntry>> {
    candidates
        .into_iter()
        .filter(|(_, e)| !e.is_empty())
        .max_by(|(a_tag, a), (b_tag, b)| {
            a.len()
                .cmp(&b.len())
                .then_with(|| strategy_priority(a_tag).cmp(&strategy_priority(b_tag)))
        })
        .map(|(_, entries)| entries)
}

fn entry_sections(lines: &[GroffLine]) -> Vec<&[GroffLine]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, line) in lines.iter().enumerate() {
        if matches!(line, GroffLine::Macro { name, .. } if matches!(name.as_str(), "SH" | "SS")) {
            if start < i {
                out.push(&lines[start..i]);
            }
            start = i + 1;
        }
    }
    if start < lines.len() {
        out.push(&lines[start..]);
    }
    out
}

/// auto-detect and try strategies.
///
/// manpages can mix option layouts by subsection (a `.TP` global-options
/// section then a ripgrep-style Text+RS one). picking the single largest global
/// result loses the smaller subsection, so once any structured strategy works,
/// split at `.SH`/`.SS` and keep the best per section. the deroff fallback runs
/// only when nothing structured works anywhere, so it can't mine prose for
/// false positives.
pub fn extract_entries(lines: &[GroffLine]) -> Vec<ManpageEntry> {
    let whole_candidates = specialized_candidates(lines);
    if whole_candidates.is_empty() {
        return strategy_deroff(lines);
    }

    let sections = entry_sections(lines);
    if sections.len() <= 1 {
        return best_entries(whole_candidates).unwrap_or_default();
    }

    let mut out = Vec::new();
    for section in sections {
        if let Some(entries) = best_entries(specialized_candidates(section)) {
            out.extend(entries);
        }
    }
    if out.is_empty() {
        best_entries(whole_candidates).unwrap_or_default()
    } else {
        out
    }
}
