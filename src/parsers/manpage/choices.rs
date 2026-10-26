//! closed value sets for a flag param.

use std::collections::HashSet;

use crate::parsers::manpage::ParamChoice;

const MAX_VALUE_LEN: usize = 40;

const LIST_HEADERS: &[&str] = &[
    "possible values",
    "one of the following",
    "following values",
    "valid values",
    "supported values",
    "accepted values",
    "can be one of",
];

const VALUE_PHRASES: &[&str] = &[
    "accepted values are ",
    "possible values are ",
    "valid values are ",
    "supported values are ",
    "valid options are ",
];

const LINKING_VERBS: &[&str] = &[
    " can be one of ",
    " is one of ",
    " must be one of ",
    " can be ",
    " must be ",
    " may be ",
    " should be ",
    " is either ",
    " is ",
];

type Miner = fn(&str, &str) -> Vec<ParamChoice>;

/// `desc` keeps its source line breaks.
pub fn mine(param_name: &str, desc: &str) -> Vec<ParamChoice> {
    let miners: [Miner; 5] = [
        placeholder_list,
        bracketed_possible_values,
        listed_after_header,
        parenthesized_list,
        inline_enumeration,
    ];
    let mut found = miners
        .iter()
        .map(|miner| miner(param_name, desc))
        .find(|choices| choices.len() >= 2)
        .unwrap_or_default();
    let mut seen = HashSet::new();
    found.retain(|choice| seen.insert(choice.value.clone()));
    found
}

fn choice(value: &str, desc: &str) -> ParamChoice {
    ParamChoice {
        value: value.to_owned(),
        desc:  desc.to_owned(),
    }
}

const fn is_value_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '+' | '/')
}

fn is_value(token: &str) -> bool {
    token.len() <= MAX_VALUE_LEN
        && token.starts_with(|ch: char| ch.is_ascii_alphanumeric())
        && token.chars().all(is_value_char)
}

fn values_of<'src, Parts>(parts: Parts) -> Vec<ParamChoice>
where
    Parts: Iterator<Item = &'src str>,
{
    let values: Vec<&str> = parts.map(str::trim).collect();
    if values.iter().all(|value| is_value(value)) {
        values.into_iter().map(|value| choice(value, "")).collect()
    } else {
        Vec::new()
    }
}

fn flatten(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn placeholder_list(param_name: &str, _desc: &str) -> Vec<ParamChoice> {
    let braced = param_name
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'));
    match braced {
        Some(inner) => values_of(inner.split(',')),
        None if param_name.matches('|').count() >= 2 => values_of(param_name.split('|')),
        None => Vec::new(),
    }
}

fn bracketed_possible_values(_param_name: &str, desc: &str) -> Vec<ParamChoice> {
    let marker = "[possible values:";
    desc.to_ascii_lowercase()
        .find(marker)
        .and_then(|start| desc.get(start + marker.len()..))
        .and_then(|tail| tail.split_once(']'))
        .map(|(inner, _)| values_of(inner.split(',')))
        .unwrap_or_default()
}

fn listed_after_header(param_name: &str, desc: &str) -> Vec<ParamChoice> {
    let lines: Vec<&str> = desc.lines().map(str::trim).collect();
    let param_lower = param_name.to_ascii_lowercase();
    let by_header = list_after(&lines, |paragraph| {
        LIST_HEADERS.iter().any(|header| paragraph.contains(header))
    });
    if by_header.len() >= 2 || param_lower.len() < 2 {
        return by_header;
    }
    list_after(&lines, |paragraph| contains_word(paragraph, &param_lower))
}

fn list_after<Announces>(lines: &[&str], announces: Announces) -> Vec<ParamChoice>
where
    Announces: Fn(&str) -> bool,
{
    let mut paragraph = String::new();

    for (idx, line) in lines.iter().enumerate() {
        if line.is_empty() {
            paragraph.clear();
            continue;
        }

        paragraph.push(' ');
        paragraph.push_str(&line.to_ascii_lowercase());
        if !line.ends_with(':') {
            continue;
        }

        if !announces(&paragraph) {
            continue;
        }

        let rest = &lines[idx + 1..];
        let labelled = labelled_items(rest);
        if labelled.len() >= 2 {
            return labelled;
        }
        let commas = comma_items(rest);
        if commas.len() >= 2 {
            return commas;
        }
    }
    Vec::new()
}

fn contains_word(haystack: &str, word: &str) -> bool {
    haystack.match_indices(word).any(|(at, _)| {
        let Some((before, rest)) = haystack.split_at_checked(at) else {
            return false;
        };
        let after = rest.strip_prefix(word).and_then(|tail| tail.chars().next());
        !before
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
            && !after.is_some_and(|ch| ch.is_ascii_alphanumeric())
    })
}

fn labelled_items(lines: &[&str]) -> Vec<ParamChoice> {
    let mut items: Vec<ParamChoice> = Vec::new();
    let mut paragraph_start = true;

    for line in lines {
        if line.is_empty() {
            paragraph_start = true;
            continue;
        }

        let lowercase_list = items
            .first()
            .is_some_and(|first| first.value.starts_with(|ch: char| ch.is_ascii_lowercase()));
        let labelled = labelled_item(line).filter(|candidate| {
            !(lowercase_list
                && candidate
                    .value
                    .starts_with(|ch: char| ch.is_ascii_uppercase()))
        });
        match (labelled, items.last_mut()) {
            (Some(item), _) => items.push(item),
            (None, Some(last)) if !paragraph_start => {
                if !last.desc.is_empty() {
                    last.desc.push(' ');
                }
                last.desc.push_str(line);
            },
            (None, _) => break,
        }
        paragraph_start = false;
    }
    items
}

fn labelled_item(line: &str) -> Option<ParamChoice> {
    let (bulleted, body) = ["- ", "* ", "\u{2022} "]
        .iter()
        .find_map(|bullet| line.strip_prefix(bullet))
        .map_or((false, line), |rest| (true, rest.trim_start()));
    let end = body
        .find(|ch: char| !is_value_char(ch))
        .unwrap_or(body.len());
    let (value, tail) = body.split_at(end);
    if !is_value(value) {
        return None;
    }
    if let Some(desc) = tail.strip_prefix(':') {
        let spaced = desc.is_empty() || desc.starts_with(char::is_whitespace);
        return spaced.then(|| choice(value, desc.trim()));
    }
    (bulleted && tail.is_empty()).then(|| choice(value, ""))
}

fn comma_items(lines: &[&str]) -> Vec<ParamChoice> {
    let joined = lines
        .iter()
        .take_while(|line| !line.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let list = joined.trim_end_matches('.');
    values_of(list.split(',').map(|raw| {
        let part = raw.trim();
        part.strip_prefix("or ")
            .or_else(|| part.strip_prefix("and "))
            .unwrap_or(part)
    }))
}

fn parenthesized_list(_param_name: &str, desc: &str) -> Vec<ParamChoice> {
    let flat = flatten(desc);
    flat.split('(')
        .skip(1)
        .filter_map(|tail| tail.split_once(')'))
        .map(|(inner, _)| values_of(inner.split(',')))
        .find(|choices| {
            choices.len() >= 3
                && choices.iter().all(|choice| {
                    choice
                        .value
                        .starts_with(|ch: char| ch.is_ascii_alphabetic())
                })
        })
        .unwrap_or_default()
}

fn inline_enumeration(param_name: &str, desc: &str) -> Vec<ParamChoice> {
    let flat = flatten(desc);
    let lower = flat.to_ascii_lowercase();
    let after_phrase = VALUE_PHRASES.iter().flat_map(|phrase| {
        lower
            .match_indices(phrase)
            .filter_map(|(at, _)| flat.get(at + phrase.len()..))
    });
    let after_param = flat
        .match_indices(param_name)
        .filter(|_| param_name.len() >= 2)
        .filter_map(|(at, _)| flat.split_at_checked(at))
        .filter(|(before, _)| {
            !before
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_ascii_alphanumeric())
        })
        .filter_map(|(_, rest)| {
            let after = rest.strip_prefix(param_name)?.trim_start_matches('>');
            LINKING_VERBS
                .iter()
                .find_map(|verb| after.strip_prefix(verb))
        });
    after_phrase
        .chain(after_param)
        .map(enumeration)
        .find(|choices| choices.len() >= 2)
        .unwrap_or_default()
}

fn enumeration(text: &str) -> Vec<ParamChoice> {
    let mut items = Vec::new();
    let mut list_quoted = None;
    let mut conjunction = false;
    let mut rest = text;

    while let Some((value, quoted, after)) = enumerated_item(rest) {
        if *list_quoted.get_or_insert(quoted) != quoted {
            break;
        }
        items.push(choice(value, ""));
        let separator = [
            (", or ", true),
            (", and ", true),
            (" or ", true),
            (" and ", true),
            (", ", false),
        ]
        .iter()
        .find_map(|(sep, conj)| after.strip_prefix(sep).map(|tail| (tail, *conj)));
        let Some((tail, conj)) = separator else {
            break;
        };
        conjunction |= conj;
        rest = tail;
    }

    if list_quoted == Some(false) && !conjunction {
        return Vec::new();
    }
    items
}

fn enumerated_item(text: &str) -> Option<(&str, bool, &str)> {
    const OPENERS: &[char] = &['`', '\'', '"', '\u{2018}', '\u{201c}'];
    const CLOSERS: &[char] = &['\'', '`', '"', '\u{2019}', '\u{201d}'];

    if let Some(body) = text.strip_prefix(OPENERS) {
        let (value, after) = body.split_once(CLOSERS)?;
        return is_value(value).then_some((value, true, after));
    }

    let end = text
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')))
        .unwrap_or(text.len());
    let (value, after) = text.split_at_checked(end)?;
    is_value(value).then_some((value, false, after))
}
