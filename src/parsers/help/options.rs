use crate::make_parser;
use crate::parsers::help::helpers::is_option_char;
use crate::parsers::manpage::{OwnedParam, OwnedSwitch};

// zero-copy intermediates, live only between nom combinators here. public
// parsers emit the owned model.
enum Switch<'src> {
    Short(char),
    Long(&'src str),
    Both(char, &'src str),
}

enum Param<'src> {
    Mandatory(&'src str),
    Optional(&'src str),
}

impl From<Switch<'_>> for OwnedSwitch {
    fn from(switch: Switch<'_>) -> Self {
        match switch {
            Switch::Short(short) => Self::Short(short),
            Switch::Long(long) => Self::Long(long.to_owned()),
            Switch::Both(short, long) => Self::Both(short, long.to_owned()),
        }
    }
}

impl From<Param<'_>> for OwnedParam {
    fn from(param: Param<'_>) -> Self {
        match param {
            Param::Mandatory(name) => Self::Mandatory(name.to_owned()),
            Param::Optional(name) => Self::Optional(name.to_owned()),
        }
    }
}

use nom::bytes::complete::{take_till, take_till1};
use nom::character::complete::{space0, space1};
use nom::combinator::{map, opt};
use nom::multi::many0;
use nom::sequence::separated_pair;
use nom::{
    IResult, Parser as _,
    branch::alt,
    bytes::complete::{tag, take_while1},
    character::complete::{char, satisfy},
    combinator::{value, verify},
    sequence::{delimited, preceded},
};

make_parser!(short_switch -> char,
    preceded(char('-'), satisfy(char::is_alphanumeric)));

// first char must be alphanumeric to reject `----------` separator lines in
// `less --help` (and zstdless/bzless wrappers), else the dash run becomes a
// flag named `------…`.
make_parser!(long_switch<'src> -> &'src str,
preceded(tag("--"), verify(take_while1(is_option_char), |name: &str| {
    name.chars().next().is_some_and(|ch| ch.is_ascii_alphanumeric())
})));

make_parser!(negatable_long_switch<'src> -> &'src str,
    preceded(tag("--[no-]"), take_while1(is_option_char)));

make_parser!(comma -> (),
    value((), preceded(char(','), space0)));

make_parser!(eq_optional_param<'src> -> Param<'src>,
    delimited(tag("[="), take_while1(is_option_char), char(']')) => Param::Optional);

make_parser!(eq_optional_angle_param<'src> -> Param<'src>,
    delimited(tag("[=<"), take_till1(|ch| ch == '>'), tag(">]")) => Param::Optional);

make_parser!(eq_mandatory_param<'src> -> Param<'src>,
    preceded(char('='), take_while1(is_option_char)) => Param::Mandatory);

// widen to a full token then verify all-caps. taking only uppercase chars
// would match just "N" of " Needs:..." and leave "eeds:..." as desc.
make_parser!(spaced_uppercase_param<'src> -> Param<'src>,
    preceded(
        char(' '),
        verify(
            take_while1(|ch: char|
                ch.is_ascii_alphabetic() || ch.is_ascii_digit() || ch == '_' || ch == '-'
            ),
            |token: &str| {
                let Some(first) = token.chars().next() else { return false };
                if !(first.is_ascii_uppercase() || first == '_') { return false; }
                token.chars().all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
            }
        )
    ) => Param::Mandatory);

make_parser!(spaced_angle_param<'src> -> Param<'src>,
    preceded(char(' '), delimited(char('<'), take_till1(|ch| ch == '>'), char('>'))) => Param::Mandatory);

make_parser!(spaced_opt_angle_param<'src> -> Param<'src>,
    preceded(char(' '), delimited(char('<'),
        delimited(char('['), take_while1(|ch| ch != ']'), char(']')),
        char('>'))) => Param::Optional);

make_parser!(spaced_angle_param_after_space<'src> -> Param<'src>,
    preceded(space1, delimited(char('<'), take_till1(|ch| ch == '>'), char('>'))) => Param::Mandatory);

// full lowercase token then verify <=10 chars. a 10-char-capped
// take_while_m_n leaves a partial match: "--foo nanoseconds" yields
// "nanosecond" and "s" as desc. a word >10 chars is almost certainly desc.
make_parser!(spaced_type_param<'src> -> Param<'src>,
    preceded(
        char(' '),
        verify(
            take_while1(|ch: char| !ch.is_whitespace()),
            |word: &str| word.len() <= 10 && word.chars().all(|ch| ch.is_ascii_lowercase())
        )
    ) => Param::Mandatory
);

make_parser!(param_parser_borrowed<'src> -> Param<'src>, alt((
    eq_optional_angle_param,
    eq_optional_param,
    eq_mandatory_param,
    spaced_opt_angle_param,
    spaced_angle_param_after_space,
    spaced_angle_param,
    spaced_uppercase_param,
    spaced_type_param,
)));

make_parser!(pub param_parser -> OwnedParam, param_parser_borrowed => OwnedParam::from);

macro_rules! switch_pair {
    ($name:ident, $left:expr, $sep:expr, $right:expr => |$a:ident, $b:ident| $body:expr) => {
        fn $name(input: &str) -> IResult<&str, Switch<'_>> {
            use nom::sequence::separated_pair;
            let (rem, ($a, $b)) = separated_pair($left, $sep, $right).parse(input)?;
            Ok((rem, $body))
        }
    };
}

switch_pair!(short_comma_long,
    short_switch, comma, long_switch => |short, long| Switch::Both(short, long));

switch_pair!(short_comma_negatable_long,
    short_switch, comma, negatable_long_switch => |short, long| Switch::Both(short, long));

switch_pair!(short_space_long,
    short_switch, char(' '), long_switch => |short, long| Switch::Both(short, long));

switch_pair!(short_space_negatable_long,
    short_switch, char(' '), negatable_long_switch => |short, long| Switch::Both(short, long));

make_parser!(slash_sep -> (),
    value((), delimited(space0, char('/'), space0)));

switch_pair!(long_slash_short,
    long_switch, slash_sep, short_switch => |long, short| Switch::Both(short, long));

// `-s PARAM, --long` form. ripgrep and other clap-via-roff manpages emit the
// placeholder between short and long forms (`\-e PATTERN, \-\-regexp=PATTERN`).
// matched conservatively (uppercase ident or `<angle>`) to avoid eating desc
// words. trailing `=PATTERN` / ` PATTERN` left for opt(param_parser).
make_parser!(placeholder_token<'src> -> &'src str,
    alt((
        delimited(char('<'), take_till1(|ch: char| ch == '>'), char('>')),
        verify(
            take_while1(|ch: char|
                ch.is_ascii_alphabetic() || ch.is_ascii_digit() || ch == '_' || ch == '-'
            ),
            |token: &str| {
                let Some(first) = token.chars().next() else { return false };
                if !(first.is_ascii_uppercase() || first == '_') { return false; }
                token.chars().all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
            }
        ),
    ))
);

fn short_param_comma_long(input: &str) -> IResult<&str, Switch<'_>> {
    let (rem, (short, _, _ph, (), long)) =
        (short_switch, space1, placeholder_token, comma, long_switch).parse(input)?;
    Ok((rem, Switch::Both(short, long)))
}

make_parser!(short_as_switch<'src> -> Switch<'src>, short_switch => Switch::Short);
make_parser!(negatable_long_as_switch<'src> -> Switch<'src>, negatable_long_switch => Switch::Long);
make_parser!(long_as_switch<'src> -> Switch<'src>, long_switch => Switch::Long);

make_parser!(switch_parser_borrowed<'src> -> Switch<'src>,
    alt((
        short_comma_negatable_long,
        short_space_negatable_long,
        short_comma_long,
        short_space_long,
        short_param_comma_long,
        long_slash_short,
        short_as_switch,
        negatable_long_as_switch,
        long_as_switch,
    ))
);

make_parser!(pub switch_parser -> OwnedSwitch, switch_parser_borrowed => OwnedSwitch::from);

// `{--long | -s}` SYNOPSIS-line switch pair, e.g. nix-env's
// `[{--file | -f} path]`.
make_parser!(brace_pipe_long_short<'src> -> Switch<'src>,
    separated_pair(long_switch, (space0, char('|'), space0), short_switch)
    => |(long, short): (&'src str, char)| Switch::Both(short, long)
);

make_parser!(brace_pipe_short_long<'src> -> Switch<'src>,
    separated_pair(short_switch, (space0, char('|'), space0), long_switch)
    => |(short, long): (char, &'src str)| Switch::Both(short, long)
);

make_parser!(brace_pipe_switch<'src> -> Switch<'src>,
    delimited(
        (char('{'), space0),
        alt((brace_pipe_long_short, brace_pipe_short_long)),
        (space0, char('}'))
    )
);

make_parser!(usage_switch_parser<'src> -> Switch<'src>,
    alt((brace_pipe_switch, switch_parser_borrowed))
);

// trailing tokens inside a flag bracket. `[--option name value]` keeps
// switch=Long("option") param=Mandatory("name"), discards ` value`.
make_parser!(take_till_bracket<'src> -> &'src str, take_till(|ch: char| ch == ']'));

make_parser!(flag_in_bracket<'src> -> (Switch<'src>, Option<Param<'src>>),
    delimited(
        (char('['), space0),
        (usage_switch_parser, opt(param_parser_borrowed)),
        (take_till_bracket, char(']'))
    )
);

// collect every flag-bracketed switch and its first param. non-flag tokens
// (positional brackets, command name, ellipses) skipped one char at a time.
make_parser!(pub parse_usage_flags<'src> -> Vec<(OwnedSwitch, Option<OwnedParam>)>,
    many0(alt((
        map(flag_in_bracket, Some),
        // `value(None, ...)` forces Clone on Switch/Param, `map(..., |_| None)`
        // doesn't.
        map(satisfy(|ch| ch != '\n' && ch != '\r'), |_| None),
    )))
    => |flags: Vec<Option<(Switch<'src>, Option<Param<'src>>)>>|
        flags.into_iter()
            .flatten()
            .map(|(switch, param)| (OwnedSwitch::from(switch), param.map(OwnedParam::from)))
            .collect()
);
