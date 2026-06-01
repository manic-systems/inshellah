//! strategy-based entry extraction.
//!
//! rather than a single monolithic parser, we use multiple "strategies" that
//! each target a specific groff formatting pattern. this is necessary because
//! manpage authors use very different macro combinations for the same purpose.

use nom::{Parser, combinator::opt};

use crate::make_macro_walker;
use crate::parsers::help::{help_parser, param_parser, switch_parser};
use crate::parsers::manpage::desc;
use crate::parsers::manpage::groff::{
    GroffLine, strip_groff_escapes, strip_inline_macro_args, strip_space_macro_args,
};
use crate::parsers::manpage::{ManpageEntry, OwnedParam, OwnedSwitch};

/// collect consecutive text lines, joining them with spaces.
/// returns (collected, remaining).
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
    // a blank line ends the description, but only after some text was
    // collected — leading blanks between the tag and the first body line
    // are skipped. this caps clap-style "summary\n\nexpanded body" entries
    // (jj, etc.) at the summary, which is what completion tooltips want.
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

/// attempt to parse a tag string (e.g. "-v, --verbose FILE") into an entry.
/// uses the nom switch_parser + param_parser from the help module.
/// returns None if the tag doesn't look like a flag definition.
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

/// extract tag text from a macro line.
/// .B and .I preserve spaces (single argument); .BI, .BR, .IR alternate
/// fonts and concatenate arguments.
pub fn tag_of_macro(name: &str, args: &str) -> String {
    match name {
        "B" | "I" => strip_space_macro_args(args),
        _ => strip_groff_escapes(&strip_inline_macro_args(args))
            .trim()
            .to_string(),
    }
}

// strategy a: .TP style (most common — gnu coreutils, help2man).
// .TP introduces a tagged paragraph: the next line is the "tag" (flag name)
// and subsequent text lines are the description. the tag can be plain text
// or wrapped in a formatting macro (.B, .BI, etc.).
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

// strategy b: .IP style (curl, hand-written manpages).
// .IP takes an inline tag argument: .IP "-v, --verbose"
// the description follows as text lines.
//
// .RS/.RE depth-aware: man pages frequently nest .IP inside .RS blocks to
// list example values (e.g. bat's `.IP "caret"` under `--nonprintable-notation`).
// those nested tags look like flag definitions and confuse the parser, so we
// only treat `.IP` at outer scope as a flag entry.
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

// strategy b': .HP style (bat, help2man with hanging paragraphs).
// .HP introduces a hanging-indent paragraph: the next text line is the tag,
// followed by an empty `.IP` macro that starts the description body. example
// value listings are wrapped in `.RS/.RE` and skipped during description
// collection.
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

/// description collector for `.HP` entries. stops at the next flag-boundary
/// macro (`.HP`, `.TP`, `.PP`, `.SH`, `.SS`) and skips entire `.RS/.RE`
/// example blocks — those are sub-value listings, not part of the flag's
/// own description text.
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

// strategy b'': bare Text tag immediately followed by `.RS/.RE` (ripgrep,
// some help2man variants). like the `.PP+.RS` shape, but with no `.PP`
// anchor between flag entries — flags sit directly under `.SS` headers
// separated only by `.sp`:
//
//   .SS INPUT OPTIONS
//   \fB\-e\fP \fIPATTERN\fP, \fB\-\-regexp\fP=\fIPATTERN\fP
//   .RS 4
//   A pattern to search for ...
//   .RE
//   .sp
//   \fB\-f\fP \fIPATTERNFILE\fP, \fB\-\-file\fP=\fIPATTERNFILE\fP
//   .RS 4
//   ...
//
// we only treat a top-level Text line as a tag when an `.RS` immediately
// follows (skipping blanks/comments) and the text starts with `-`. nested
// Text lines inside an existing `.RS` block are skipped via depth tracking
// so description paragraphs that happen to begin with a flag reference
// don't get mis-recognized.
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

// strategy c: .PP + .RS/.RE style (git, docbook-generated manpages).
// flag entries are introduced by .PP (paragraph), with the flag name as
// plain text, followed by a .RS (indent) block containing the description,
// closed by .RE (de-indent).
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
    // outer: look for .RS marker or text
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Macro { name, .. } if name == "RS" => {
                // depth-tracked .RS walk. some manpages nest a sub-value
                // .RS/.RE inside the flag's main .RS block — without
                // tracking depth here, the inner `.RE` would end the
                // description early and leave the outer block half-parsed.
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
                            // skip Text inside nested .RS blocks (sub-value
                            // listings, not part of the flag's own desc).
                            if depth == 1 {
                                acc.push(t.clone());
                            }
                            i += 1;
                        }
                        GroffLine::Macro { name, .. }
                            if name == "PP" || name == "SH" || name == "SS" =>
                        {
                            // section/paragraph boundary — abort even with
                            // an unclosed .RS (malformed manpage) so we
                            // don't run off to EOF.
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

/// strategy d: deroff fallback — strip all groff markup, then feed the
/// resulting plain text through the help parser.
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
        // help_parser already emits owned ManpageEntry with a joined desc.
        Ok((_, result)) => result.entries,
        Err(_) => Vec::new(),
    }
}

fn is_bullet_ip(args: &str) -> bool {
    !args.trim().is_empty()
}

// strategy e: nix3-style bullet .IP with .UR/.UE hyperlinks.
// nix's manpages use .IP with bullet markers for flag entries, interleaved
// with .UR/.UE hyperlink macros. the flag tag is in text lines after the
// bullet .IP, and the description follows a non-bullet .IP marker.
make_macro_walker!(pub strategy_nix -> Vec<ManpageEntry>, on macro "IP" =>
    |lines, i, args| {
        if !is_bullet_ip(args) { return None; }
        // collect tag: skip .UR/.UE macros, gather Text lines
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
    // require non-bullet .IP marker for description
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
                    // next bullet entry — stop
                    return (parts.join(" "), i);
                }
                // non-bullet .IP = continuation paragraph
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

/// count occurrences of a specific macro in the section.
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

/// tie-break priority for a strategy, higher wins. when two strategies
/// extract the same number of entries, this explicit ranking decides — so
/// editing or reordering the candidate list can no longer silently change
/// which strategy wins (the old code resolved ties by `>=` push order, an
/// invisible coupling). the values preserve that historical order: the
/// later a strategy was pushed, the higher it ranked on a tie.
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

/// pick the strongest candidate: most entries first, then the deterministic
/// `strategy_priority` tie-break.
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
/// Manpages can mix option layouts by subsection (for example, a `.TP`
/// "global options" section followed by a ripgrep-style Text+RS section).
/// Running the strategies once globally and picking the largest result loses
/// the smaller subsection. Instead, once any structured strategy works for
/// the full input, split at `.SH`/`.SS` boundaries and keep the best
/// structured result per local section. The broad deroff fallback is still
/// used only when no structured strategy works anywhere, which keeps it from
/// mining unrelated prose subsections for false positives.
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
