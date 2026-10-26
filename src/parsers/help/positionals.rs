use nom::{
    AsChar as _,
    IResult,
    Parser as _,
    branch::alt,
    bytes::complete::{
        tag,
        tag_no_case,
        take_till,
        take_till1,
        take_while,
        take_while1,
    },
    character::complete::{
        char,
        line_ending,
        satisfy,
        space0,
        space1,
    },
    combinator::{
        eof,
        map,
        map_opt,
        not,
        opt,
        peek,
        recognize,
        value,
        verify,
    },
    multi::many0,
    sequence::{
        delimited,
        preceded,
        terminated,
    },
};

use crate::{
    make_parser,
    make_predicate,
    parsers::help::helpers::rest_of_line,
    types::Positional,
};

#[derive(Clone)]
enum PositionalParse<'src> {
    Curly,
    Flag,
    Skip,
    Mandatory(&'src str),
    Optional(&'src str),
    ManVariadic(&'src str),
    OptVariadic(&'src str),
}

make_predicate!(is_word_char, |ch| {
    ch.is_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.')
});

make_predicate!(is_pos_char, |ch| {
    ch.is_ascii_uppercase() || ch.is_numeric() || matches!(ch, '_' | '-')
});

make_parser!(section_label -> (),
    value((), alt((
        tag_no_case("options"),
        tag_no_case("option"),
        tag_no_case("flags"),
        tag_no_case("flag")
    )))
);

make_parser!(ellipses -> (),
    value((),
        alt((tag("..."), tag("\u{2026}")))
    )
);

make_parser!(braces<'src> -> PositionalParse<'src>,
    value(PositionalParse::Curly, delimited(char('{'), take_till1(|ch| ch == '}'), char('}')))
);

make_parser!(flag<'src> -> PositionalParse<'src>,
    value(PositionalParse::Flag, preceded(char('-'), take_till1(|ch: char| ch.is_space() || ch == ']')))
);

fn check_positional(raw: &str) -> bool {
    let name = raw.trim();
    if name.is_empty() {
        return false;
    }
    // reject '-' leading names, else jj's "[--at-operation]" becomes a
    // positional named "--flag".
    if name.starts_with('-') {
        return false;
    }
    if section_label.parse(name).is_ok() {
        return false;
    }
    let upper = name.to_ascii_uppercase();
    if matches!(upper.as_str(), "OPTIONS" | "OPTION" | "FLAGS" | "FLAG") {
        return false;
    }
    name.chars()
        .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.'))
}

// balanced `[...]` block, one level of nesting. returns the inner content.
make_parser!(balanced_bracket_inner<'src> -> &'src str,
    delimited(
        char('['),
        recognize(many0(alt((
            recognize((char('['), take_till(|ch: char| ch == ']'), char(']'))),
            recognize(satisfy(|ch: char| ch != ']' && ch != '[')),
        )))),
        char(']'),
    )
);

fn strip_ellipsis(text: &str) -> (&str, bool) {
    text.strip_suffix("...")
        .or_else(|| text.strip_suffix('\u{2026}'))
        .map_or((text, false), |stripped| (stripped.trim_end(), true))
}

/// positional name from trimmed bracket-inner content. bool flags a trailing
/// `...` variadic marker.
fn parse_bracket_inner_name(raw: &str) -> Option<(&str, bool)> {
    let (rest, has_dots) = strip_ellipsis(raw.trim());
    if rest.starts_with('[') {
        let mut found = None;
        let mut remaining = rest;
        while let Some((_, after_start)) = remaining.split_once('[') {
            let Some((nested, after_end)) = after_start.split_once(']') else {
                break;
            };
            if let Some((nested_name, nested_dots)) = parse_bracket_inner_name(nested)
                && check_positional(nested_name)
            {
                found = Some((nested_name, has_dots || nested_dots));
            }
            remaining = after_end;
        }
        return found;
    }
    if let Some(after_lt) = rest.strip_prefix('<') {
        let (raw_name, _) = after_lt.split_once('>')?;
        let (name, inner_dots) = strip_ellipsis(raw_name.trim());
        return Some((name, has_dots || inner_dots));
    }
    let name = rest
        .split(|ch: char| ch.is_whitespace() || ch == '[' || ch == ']')
        .next()?;
    if name.is_empty() {
        return None;
    }
    Some((name, has_dots))
}

// `map_opt` turns a `None` from parse_bracket_inner_name into a parse error.
make_parser!(opt_bracket_name<'src> -> (&'src str, bool),
    map_opt(balanced_bracket_inner, parse_bracket_inner_name)
);

make_parser!(
    opt_positional<'src> -> PositionalParse<'src>,
    verify(
        // matches "[name]", "[name...]", "[name ...]", "[name] ...",
        // "[<name>]", and one-level nests like "[<program> [<arg>...]]".
        (opt_bracket_name, opt(ellipses)),
        |((name, _), _): &((&'src str, bool), Option<()>)| check_positional(name)
    ) => |((name, has_inner_dots), post_dots): ((&'src str, bool), Option<()>)| {
        if has_inner_dots || post_dots.is_some() {
            PositionalParse::OptVariadic(name)
        } else {
            PositionalParse::Optional(name)
        }
    }
);

make_parser!(man_positional<'src> -> PositionalParse<'src>,
    verify(
        (
            delimited(
                char('<'),
                (
                    take_till1(|ch| ch == '.' || ch == '\u{2026}' || ch == '>'),
                    opt(ellipses)
                ),
                char('>')
            ),
            opt(ellipses)
        ),
        |((name, _), _)| check_positional(name)
    ) => |((name, inner_dots), post_dots): ((&'src str, Option<()>), Option<()>)|
        if inner_dots.is_some() || post_dots.is_some() { PositionalParse::ManVariadic(name) }
        else { PositionalParse::Mandatory(name) }
);

make_parser!(allcaps_positional<'src> -> PositionalParse<'src>,
    verify(
        (
            preceded(
                peek(
                    satisfy(|ch: char| ch.is_ascii_uppercase())
                ),
                take_while1(is_pos_char)
            ),
            opt(
                alt((
                    tag("..."),
                    tag("\u{2026}"))
                )
            )
        ),
        |(name, _): &(&str, _)| check_positional(name)
    ) => |(name, dots): (&'src str, Option<&'src str>)|
        if dots.is_some() { PositionalParse::ManVariadic(name) } else { PositionalParse::Mandatory(name) }
);

fn caseless_push<'src>(
    name: &'src str,
    positional: Positional,
    acc: &mut Vec<(&'src str, Positional)>,
) {
    let dupe = acc
        .iter()
        .any(|(existing, _)| existing.eq_ignore_ascii_case(name));
    if !dupe {
        acc.push((name, positional));
    }
}

// single logical usage line. SKIP refuses to cross a newline so many0 stops
// at end-of-line, else it wanders into OPTIONS and treats every `--flag
// <name>` param as a positional. the inner terminator peeks line_ending
// rather than consuming it, so the outer `opt(line_ending)` eats it and the
// scan never advances past the usage line.
make_parser!(pub parse_usage_args<'src> -> Vec<(&'src str, Positional)>,
    (delimited(
        space0,
        many0(
            alt((
                map(
                    (
                        terminated(
                            alt((
                                braces,
                                opt_positional,
                                value(PositionalParse::Skip, balanced_bracket_inner),
                                man_positional,
                                flag,
                                allcaps_positional,
                            )),
                            alt((
                                space1,
                                value("", peek(line_ending)),
                                value("", peek(eof)),
                            ))
                        ),
                        // "[section] ..." where the ellipsis is the next
                        // whitespace-separated token.
                        opt(terminated(
                            alt((tag("..."), tag("\u{2026}"))),
                            alt((
                                space1,
                                value("", peek(line_ending)),
                                value("", peek(eof)),
                            ))
                        ))
                    ),
                    |(positional, trailing): (PositionalParse<'src>, Option<_>)| {
                        if trailing.is_none() { positional }
                        else {
                            match positional {
                                PositionalParse::Optional(name) => PositionalParse::OptVariadic(name),
                                PositionalParse::Mandatory(name) => PositionalParse::ManVariadic(name),
                                other @ (PositionalParse::Curly
                                | PositionalParse::Flag
                                | PositionalParse::Skip
                                | PositionalParse::ManVariadic(_)
                                | PositionalParse::OptVariadic(_)) => other,
                            }
                        }
                    }
                ),
                value(PositionalParse::Skip, satisfy(|ch: char| ch != '\n' && ch != '\r')),
            ))
        ),
        opt((space0, line_ending))
    )) => |parsed: Vec<PositionalParse<'src>>|
            parsed.into_iter().fold(Vec::new(), |mut acc, parse|
            {
                match parse {
                    PositionalParse::Curly | PositionalParse::Flag | PositionalParse::Skip => (),
                    PositionalParse::OptVariadic(arg) => caseless_push(arg, Positional {
                        optional: true,
                        variadic: true
                    }, &mut acc),
                    PositionalParse::ManVariadic(arg) => caseless_push(arg, Positional {
                        optional: false,
                        variadic: true
                    }, &mut acc),
                    PositionalParse::Optional(arg) => caseless_push(arg, Positional {
                        optional: true,
                        variadic: false,
                    }, &mut acc),
                    PositionalParse::Mandatory(arg) => caseless_push(arg, Positional {
                        optional: false,
                        variadic: false
                    }, &mut acc),
                }
                acc
            })
);

make_parser!(pub skip_command_name -> (),
    value((), preceded(space0,
        many0(
            (
                verify(
                    preceded(not(char('-')), take_while1(is_word_char)),
                    |ss: &str| ss.chars().any(|ch: char| ch.is_ascii_lowercase())
                ),
                space1
            )
        )
    ))
);

make_parser!(find_usage_line -> (),
    value((), preceded(
        space0,
        terminated(
            tag_no_case("usage"),
            // "Usage:" (inline+colon), "Usage args" (inline+space),
            // "USAGE\n  cmd args" (clap header on its own line).
            alt(
                (
                    value((), char(':')),
                    value((), peek(line_ending)),
                    value((), peek(satisfy(|ch: char| ch == ' ' || ch == '\t'))),
                )
            )
        )
    ))
);

make_parser!(pub extract_usage_positionals<'src> -> Vec<(&'src str, Positional)>,
    preceded(
        many0(preceded(not(find_usage_line), (rest_of_line, line_ending))),
        preceded(
            (find_usage_line, space0, opt(line_ending), space0, skip_command_name),
            parse_usage_args
        )
    )
);

make_predicate!(is_cli11_name_char, |ch| {
    ch.is_alphanumeric() || matches!(ch, '_' | '-')
});

make_parser!(cli11_section_header -> (),
    value((),
        delimited(
            space0,
            alt((tag("POSITIONALS:"), tag("Positionals:"))),
            (rest_of_line, opt(line_ending))
        )
    )
);

make_parser!(cli11_pos_line<'src> -> (&'src str, bool),
    preceded(
        verify(space0, |ss: &str| !ss.is_empty()),
        terminated(
            (
                verify(take_while1(is_cli11_name_char), |name: &str| name.len() >= 2),
                preceded(
                    (space0, take_while(|ch: char| ch.is_ascii_uppercase()), space0),
                    opt(tag("..."))
                )
            ),
            (rest_of_line, opt(line_ending))
        )
    ) => |(name, variadic): (&'src str, Option<_>)| (name, variadic.is_some())
);

make_parser!(parse_cli11_body<'src> -> Vec<(&'src str, Positional)>,
    many0(cli11_pos_line) => |entries: Vec<(&'src str, bool)>|
        entries.into_iter().fold(Vec::new(), |mut acc, (name, variadic)| {
            caseless_push(name, Positional { optional: false, variadic }, &mut acc);
            acc
        })
);

make_parser!(pub extract_cli11_positionals<'src> -> Vec<(&'src str, Positional)>,
    preceded(
        many0(preceded(not(cli11_section_header), (rest_of_line, line_ending))),
        preceded(cli11_section_header, parse_cli11_body)
    )
);

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<(String, bool, bool)> {
        let (_, parsed) = parse_usage_args(line).expect("parse_usage_args");
        parsed
            .into_iter()
            .map(|(name, positional)| (name.to_owned(), positional.optional, positional.variadic))
            .collect()
    }

    #[test]
    fn mandatory_optional_and_allcaps() {
        assert_eq!(args("<src> [dst] FILE"), vec![
            ("src".into(), false, false),
            ("dst".into(), true, false),
            ("FILE".into(), false, false),
        ]);
    }

    #[test]
    fn variadic_markers() {
        // in-bracket `<files...>` and post-token `[paths] ...` both set
        // variadic
        assert_eq!(args("<files...>"), vec![("files".into(), false, true)]);
        assert_eq!(args("[paths] ..."), vec![("paths".into(), true, true)]);
    }

    #[test]
    fn flags_and_braces_are_skipped() {
        // `--flag` and `{a|b}` are not positionals, only `<name>` survives
        assert_eq!(args("--flag {a|b} <name>"), vec![(
            "name".into(),
            false,
            false
        )]);
    }

    #[test]
    fn options_placeholder_is_not_a_positional() {
        // `OPTIONS` / `[OPTIONS]` is a section marker, not an arg
        assert!(
            args("[OPTIONS] <cmd>")
                .iter()
                .all(|(name, ..)| name != "OPTIONS")
        );
        assert_eq!(args("[OPTIONS] <cmd>"), vec![("cmd".into(), false, false)]);
    }

    #[test]
    fn parsing_stops_at_newline() {
        // many0 must not wander into a following OPTIONS block, the next-line
        // `--out <name>` must not be mined.
        assert_eq!(args("<input>\n  --out <name>\n"), vec![(
            "input".into(),
            false,
            false
        )]);
    }

    #[test]
    fn duplicate_names_collapse_case_insensitively() {
        // caseless_push drops the second `FILE`/`file`
        assert_eq!(args("<file> FILE"), vec![("file".into(), false, false)]);
    }
}
