use crate::parsers::help::helpers::rest_of_line;
use crate::types::Positional;
use crate::{make_parser, make_predicate};
use nom::branch::alt;
use nom::bytes::complete::{tag, tag_no_case, take_till, take_till1, take_while, take_while1};
use nom::character::complete::{char, line_ending, satisfy, space0, space1};
use nom::combinator::{map, not, opt, peek, recognize, value, verify};
use nom::multi::many0;
use nom::sequence::{delimited, preceded, terminated};
use nom::{AsChar, IResult, Parser};

#[derive(Clone)]
enum PositionalParse<'a> {
    Curly,
    Flag,
    Skip,
    Mandatory(&'a str),
    Optional(&'a str),
    ManVariadic(&'a str),
    OptVariadic(&'a str),
}

make_predicate!(is_word_char, |c| c.is_alphanumeric()
    || matches!(c, '-' | '_' | '/' | '.'));

make_predicate!(is_pos_char, |c| c.is_ascii_uppercase()
    || c.is_numeric()
    || matches!(c, '_' | '-'));

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

make_parser!(braces -> PositionalParse<'a>,
    value(PositionalParse::Curly, delimited(char('{'), take_till1(|c| c == '}'), char('}')))
);

// FIXME should this be a take_while is_option_char?
// why tf do we have a ']' condition
make_parser!(flag -> PositionalParse<'a>,
    value(PositionalParse::Flag, preceded(char('-'), take_till1(|c: char| c.is_space() || c == ']')))
);

fn check_positional(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    // reject names starting with '-' — these are flag tokens accidentally
    // captured by the bracket parser, e.g. "[--at-operation]" in jj's
    // synopsis. without this guard every `[--flag]` token would be
    // recorded as a positional named "--flag".
    if s.starts_with('-') {
        return false;
    }
    if section_label.parse(s).is_ok() {
        return false;
    }
    let upper = s.to_ascii_uppercase();
    if matches!(upper.as_str(), "OPTIONS" | "OPTION" | "FLAGS" | "FLAG") {
        return false;
    }
    s.chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
}

// recognize a balanced `[...]` block, tolerating ONE level of nested
// brackets inside. expressed entirely via nom combinators:
//
//   `[` + many0(alt((nested_bracket_block, non_bracket_char))) + `]`
//
// nested_bracket_block is `[ chars_until_] ]`, which means we accept a
// single inner `[...]` correctly but not arbitrarily-deep nesting —
// manpages don't go deeper than two levels.
// returns the inner content (everything between the outer brackets).
make_parser!(balanced_bracket_inner -> &'a str,
    recognize(delimited(
        char('['),
        many0(alt((
            recognize((char('['), take_till(|c: char| c == ']'), char(']'))),
            recognize(satisfy(|c: char| c != ']' && c != '[')),
        ))),
        char(']'),
    ))
    => |whole: &'a str| &whole[1..whole.len() - 1]
);

/// extract a positional name from already-trimmed bracket-inner content.
/// returns the name slice and a flag indicating whether the bracket inner
/// carried a trailing `...` (in-bracket variadic marker).
fn parse_bracket_inner_name(inner: &str) -> Option<(&str, bool)> {
    let inner = inner.trim();
    // strip trailing "..." for in-bracket variadic.
    let (rest, has_dots) = if let Some(stripped) = inner.strip_suffix("...") {
        (stripped.trim_end(), true)
    } else if let Some(stripped) = inner.strip_suffix('\u{2026}') {
        (stripped.trim_end(), true)
    } else {
        (inner, false)
    };
    if rest.starts_with('[') {
        let mut found = None;
        let mut remaining = rest;
        while let Some(start) = remaining.find('[') {
            let after_start = &remaining[start + 1..];
            let Some(end) = after_start.find(']') else {
                break;
            };
            let nested = &after_start[..end];
            if let Some((nested_name, nested_dots)) = parse_bracket_inner_name(nested)
                && check_positional(nested_name)
            {
                found = Some((nested_name, has_dots || nested_dots));
            }
            remaining = &after_start[end + 1..];
        }
        return found;
    }
    let name = if let Some(after_lt) = rest.strip_prefix('<') {
        // angle-bracket name: take everything up to the matching '>'
        let end = after_lt.find('>')?;
        let inner = after_lt[..end].trim();
        let (inner, inner_dots) = if let Some(stripped) = inner.strip_suffix("...") {
            (stripped.trim_end(), true)
        } else if let Some(stripped) = inner.strip_suffix('\u{2026}') {
            (stripped.trim_end(), true)
        } else {
            (inner, false)
        };
        return Some((inner, has_dots || inner_dots));
    } else {
        // bare name: take leading word
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '[' || c == ']')
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        &rest[..end]
    };
    Some((name, has_dots))
}

// extract a balanced `[...]` block and decompose its inner content into
// (name, has-inner-`...` flag). `map_opt` turns a `None` from
// `parse_bracket_inner_name` into a nom parse error.
make_parser!(opt_bracket_name -> (&'a str, bool),
    nom::combinator::map_opt(balanced_bracket_inner, parse_bracket_inner_name)
);

make_parser!(
    opt_positional -> PositionalParse<'a>,
    verify(
        // tuple parser: (name + in-bracket variadic, post-bracket ellipsis).
        // matches "[name]", "[name...]", "[name ...]", "[name] ...",
        // "[<name>]", and one-level nests like "[<program> [<arg>...]]".
        (opt_bracket_name, opt(ellipses)),
        |((name, _), _): &((&'a str, bool), Option<()>)| check_positional(name)
    ) => |((name, has_inner_dots), post_dots): ((&'a str, bool), Option<()>)| {
        if has_inner_dots || post_dots.is_some() {
            PositionalParse::OptVariadic(name)
        } else {
            PositionalParse::Optional(name)
        }
    }
);

make_parser!(man_positional -> PositionalParse<'a>,
    verify(
        (
            delimited(
                char('<'),
                (
                    take_till1(|c| c == '.' || c == '\u{2026}' || c == '>'),
                    opt(ellipses)
                ),
                char('>')
            ),
            opt(ellipses)
        ),
        |((ss, _), _)| check_positional(ss)
    ) => |((p, v), v1): ((&'a str, Option<()>), Option<()>)|
        if v.is_some() || v1.is_some() { PositionalParse::ManVariadic(p) }
        else { PositionalParse::Mandatory(p) }
);

make_parser!(allcaps_positional -> PositionalParse<'a>,
    verify(
        (
            preceded(
                peek(
                    satisfy(|c: char| c.is_ascii_uppercase())
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
        |(ss, _): &(&str, _)| check_positional(ss)
    ) => |(p, v): (&'a str, Option<&'a str>)|
        if v.is_some() { PositionalParse::ManVariadic(p) } else { PositionalParse::Mandatory(p) }
);

fn caseless_push<'a>(k: &'a str, v: Positional, acc: &mut Vec<(&'a str, Positional)>) {
    let dupe = acc.iter().any(|(ik, _)| ik.eq_ignore_ascii_case(k));
    if !dupe {
        acc.push((k, v));
    }
}

// parse_usage_args runs on a single logical usage line. SKIP refuses to
// cross a newline boundary so many0 stops at end-of-line — without this
// the parser would happily wander into the OPTIONS section and treat
// every `--flag <name>` angle-bracket parameter as a positional.
//
// the inner positional terminator uses peek(line_ending) instead of
// consuming the newline, so the trailing `opt(line_ending)` in the
// outer delimited eats it cleanly and we never advance past the usage
// line.
make_parser!(pub parse_usage_args -> Vec<(&'a str, Positional)>,
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
                                value("", peek(nom::combinator::eof)),
                            ))
                        ),
                        // catch "[section] ..." patterns where the ellipsis is
                        // on the *next* token, separated by whitespace.
                        opt(terminated(
                            alt((tag("..."), tag("\u{2026}"))),
                            alt((
                                space1,
                                value("", peek(line_ending)),
                                value("", peek(nom::combinator::eof)),
                            ))
                        ))
                    ),
                    |(positional, trailing): (PositionalParse<'a>, Option<_>)| {
                        if trailing.is_none() { positional }
                        else {
                            match positional {
                                PositionalParse::Optional(n) => PositionalParse::OptVariadic(n),
                                PositionalParse::Mandatory(n) => PositionalParse::ManVariadic(n),
                                other => other,
                            }
                        }
                    }
                ),
                // SKIP must NOT consume a newline. without this, many0 keeps
                // iterating past the usage line into OPTIONS-section flag
                // syntax and over-extracts positionals.
                value(PositionalParse::Skip, satisfy(|c: char| c != '\n' && c != '\r')),
            ))
        ),
        opt((space0, line_ending))
    )) => |p: Vec<PositionalParse<'a>>|
            p.into_iter().fold(Vec::new(), |mut acc, parse|
            {
                match parse {
                    PositionalParse::Curly => (),
                    PositionalParse::Flag => (),
                    PositionalParse::Skip => (),
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
                    |ss: &str| ss.chars().any(|c: char| c.is_ascii_lowercase())
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
            // accept any of:
            //   "Usage:"              — inline form with colon
            //   "Usage args"          — inline form, space follows the word
            //   "USAGE\n  cmd args"   — clap-style header on its own line
            alt(
                (
                    value((), char(':')),
                    value((), peek(line_ending)),
                    value((), peek(satisfy(|c: char| c == ' ' || c == '\t'))),
                )
            )
        )
    ))
);

make_parser!(pub extract_usage_positionals -> Vec<(&'a str, Positional)>,
    preceded(
        many0(preceded(not(find_usage_line), (rest_of_line, line_ending))),
        preceded(
            (find_usage_line, space0, opt(line_ending), space0, skip_command_name),
            parse_usage_args
        )
    )
);

make_predicate!(is_cli11_name_char, |c| c.is_alphanumeric()
    || matches!(c, '_' | '-'));

make_parser!(cli11_section_header -> (),
    value((),
        delimited(
            space0,
            alt((tag("POSITIONALS:"), tag("Positionals:"))),
            (rest_of_line, opt(line_ending))
        )
    )
);

make_parser!(cli11_pos_line -> (&'a str, bool),
    preceded(
        verify(space0, |ss: &str| !ss.is_empty()),
        terminated(
            (
                verify(take_while1(is_cli11_name_char), |s: &str| s.len() >= 2),
                preceded(
                    (space0, take_while(|c: char| c.is_ascii_uppercase()), space0),
                    opt(tag("..."))
                )
            ),
            (rest_of_line, opt(line_ending))
        )
    ) => |(name, variadic): (&'a str, Option<_>)| (name, variadic.is_some())
);

make_parser!(parse_cli11_body -> Vec<(&'a str, Positional)>,
    many0(cli11_pos_line) => |entries: Vec<(&'a str, bool)>|
        entries.into_iter().fold(Vec::new(), |mut acc, (name, variadic)| {
            caseless_push(name, Positional { optional: false, variadic }, &mut acc);
            acc
        })
);

make_parser!(pub extract_cli11_positionals -> Vec<(&'a str, Positional)>,
    preceded(
        many0(preceded(not(cli11_section_header), (rest_of_line, line_ending))),
        preceded(cli11_section_header, parse_cli11_body)
    )
);

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<(String, bool, bool)> {
        let (_, v) = parse_usage_args(line).expect("parse_usage_args");
        v.into_iter()
            .map(|(n, p)| (n.to_string(), p.optional, p.variadic))
            .collect()
    }

    #[test]
    fn mandatory_optional_and_allcaps() {
        // `<src>` mandatory, `[dst]` optional, `FILE` bare all-caps mandatory.
        assert_eq!(
            args("<src> [dst] FILE"),
            vec![
                ("src".into(), false, false),
                ("dst".into(), true, false),
                ("FILE".into(), false, false),
            ]
        );
    }

    #[test]
    fn variadic_markers() {
        // in-bracket `<files...>` and post-token `[paths] ...` both set variadic.
        assert_eq!(
            args("<files...>"),
            vec![("files".into(), false, true)]
        );
        assert_eq!(
            args("[paths] ..."),
            vec![("paths".into(), true, true)]
        );
    }

    #[test]
    fn flags_and_braces_are_skipped() {
        // `--flag` and `{a|b}` choice braces are not positionals; only the
        // real positional `<name>` survives.
        assert_eq!(
            args("--flag {a|b} <name>"),
            vec![("name".into(), false, false)]
        );
    }

    #[test]
    fn options_placeholder_is_not_a_positional() {
        // a bare `OPTIONS` / `[OPTIONS]` token is a section marker, not an arg.
        assert!(args("[OPTIONS] <cmd>")
            .iter()
            .all(|(n, _, _)| n != "OPTIONS"));
        assert_eq!(
            args("[OPTIONS] <cmd>"),
            vec![("cmd".into(), false, false)]
        );
    }

    #[test]
    fn parsing_stops_at_newline() {
        // many0 must not wander past the usage line into a following OPTIONS
        // block; the `--out <name>` on the next line must not be mined.
        assert_eq!(
            args("<input>\n  --out <name>\n"),
            vec![("input".into(), false, false)]
        );
    }

    #[test]
    fn duplicate_names_collapse_case_insensitively() {
        // caseless_push drops the second `FILE`/`file`.
        assert_eq!(
            args("<file> FILE"),
            vec![("file".into(), false, false)]
        );
    }
}
