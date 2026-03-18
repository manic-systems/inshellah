use crate::make_parser;
use crate::parsers::help::helpers::is_option_char;
use crate::types::*;

use nom::bytes::complete::{take_till, take_till1};
use nom::character::complete::{space0, space1};
use nom::combinator::{map, opt};
use nom::multi::many0;
use nom::sequence::separated_pair;
use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::{tag, take_while1},
    character::complete::{char, satisfy},
    combinator::{value, verify},
    sequence::{delimited, preceded},
};

make_parser!(short_switch -> char,
    preceded(char('-'), satisfy(|c| c.is_alphanumeric())));

make_parser!(long_switch -> &'a str,
    preceded(tag("--"), take_while1(is_option_char)));

make_parser!(negatable_long_switch -> &'a str,
    preceded(tag("--[no-]"), take_while1(is_option_char)));

make_parser!(comma -> (),
    value((), preceded(char(','), space0)));

make_parser!(eq_optional_param -> Param<'a>,
    delimited(tag("[="), take_while1(is_option_char), char(']')) => Param::Optional);

make_parser!(eq_optional_angle_param -> Param<'a>,
    delimited(tag("[=<"), take_till1(|c| c == '>'), tag(">]")) => Param::Optional);

make_parser!(eq_mandatory_param -> Param<'a>,
    preceded(char('='), take_while1(is_option_char)) => Param::Mandatory);

// take a wide alphanumeric/_/- token then verify the WHOLE thing looks
// like an ALL_CAPS-style param name. taking only uppercase chars would
// match just "N" of " Needs: ..." and leave "eeds:..." as desc, so we
// widen, then reject anything that doesn't pass the all-caps check.
make_parser!(spaced_uppercase_param -> Param<'a>,
    preceded(
        char(' '),
        verify(
            take_while1(|c: char|
                c.is_ascii_alphabetic() || c.is_ascii_digit() || c == '_' || c == '-'
            ),
            |s: &str| {
                let first = match s.chars().next() { Some(c) => c, None => return false };
                if !(first.is_ascii_uppercase() || first == '_') { return false; }
                s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            }
        )
    ) => Param::Mandatory);

make_parser!(spaced_angle_param -> Param<'a>,
    preceded(char(' '), delimited(char('<'), take_till1(|c| c == '>'), char('>'))) => Param::Mandatory);

make_parser!(spaced_opt_angle_param -> Param<'a>,
    preceded(char(' '), delimited(char('<'),
        delimited(char('['), take_while1(|c| c != ']'), char(']')),
        char('>'))) => Param::Optional);

make_parser!(spaced_angle_param_after_space -> Param<'a>,
    preceded(space1, delimited(char('<'), take_till1(|c| c == '>'), char('>'))) => Param::Mandatory);

// take the full lowercase token then verify it's <=10 chars. a
// take_while_m_n with a 10-char cap would leave a partial match — e.g.
// "--foo nanoseconds" would extract param "nanosecond" and leave "s" as
// the description. a word longer than 10 chars is almost certainly the
// start of the description, not a type annotation.
make_parser!(spaced_type_param -> Param<'a>,
    preceded(
        char(' '),
        verify(
            take_while1(|c: char| !c.is_whitespace()),
            |s: &str| s.len() <= 10 && s.chars().all(|c| c.is_ascii_lowercase())
        )
    ) => Param::Mandatory
);

make_parser!(pub param_parser -> Param<'a>, alt((
    eq_optional_angle_param,
    eq_optional_param,
    eq_mandatory_param,
    spaced_opt_angle_param,
    spaced_angle_param_after_space,
    spaced_angle_param,
    spaced_uppercase_param,
    spaced_type_param,
)));

macro_rules! switch_pair {
    ($name:ident, $left:expr, $sep:expr, $right:expr => |$a:ident, $b:ident| $body:expr) => {
        fn $name<'a>(s: &'a str) -> IResult<&'a str, Switch<'a>> {
            use nom::sequence::separated_pair;
            let (rem, ($a, $b)) = separated_pair($left, $sep, $right).parse(s)?;
            Ok((rem, $body))
        }
    };
}

switch_pair!(short_comma_long,
    short_switch, comma, long_switch => |s, l| Switch::Both(s, l));

switch_pair!(short_comma_negatable_long,
    short_switch, comma, negatable_long_switch => |s, l| Switch::Both(s, l));

switch_pair!(short_space_long,
    short_switch, char(' '), long_switch => |s, l| Switch::Both(s, l));

switch_pair!(short_space_negatable_long,
    short_switch, char(' '), negatable_long_switch => |s, l| Switch::Both(s, l));

make_parser!(slash_sep -> (),
    value((), delimited(space0, char('/'), space0)));

switch_pair!(long_slash_short,
    long_switch, slash_sep, short_switch => |l, s| Switch::Both(s, l));

make_parser!(short_as_switch -> Switch<'a>, short_switch => Switch::Short);
make_parser!(negatable_long_as_switch -> Switch<'a>, negatable_long_switch => Switch::Long);
make_parser!(long_as_switch -> Switch<'a>, long_switch => Switch::Long);

make_parser!(pub switch_parser -> Switch<'a>,
    alt((
        short_comma_negatable_long,
        short_space_negatable_long,
        short_comma_long,
        short_space_long,
        long_slash_short,
        short_as_switch,
        negatable_long_as_switch,
        long_as_switch,
    ))
);

// `{--long | -s}` — manpage SYNOPSIS-line switch pair. nix-env's
// synopsis is the canonical case: `[{--file | -f} path] [{--profile |
// -p} path]`. emits Switch::Both with the long name.
make_parser!(brace_pipe_long_short -> Switch<'a>,
    separated_pair(long_switch, (space0, char('|'), space0), short_switch)
    => |(l, s): (&'a str, char)| Switch::Both(s, l)
);

make_parser!(brace_pipe_short_long -> Switch<'a>,
    separated_pair(short_switch, (space0, char('|'), space0), long_switch)
    => |(s, l): (char, &'a str)| Switch::Both(s, l)
);

make_parser!(brace_pipe_switch -> Switch<'a>,
    delimited(
        (char('{'), space0),
        alt((brace_pipe_long_short, brace_pipe_short_long)),
        (space0, char('}'))
    )
);

make_parser!(usage_switch_parser -> Switch<'a>,
    alt((brace_pipe_switch, switch_parser))
);

// consume any chars except `]`. used to swallow trailing tokens inside a
// flag bracket — e.g. `[--option name value]` keeps switch=Long("option")
// and param=Mandatory("name"), discarding ` value` before the closing `]`.
make_parser!(take_till_bracket -> &'a str, take_till(|c: char| c == ']'));

// `[<switch> [param] <junk>]` inside the SYNOPSIS line.
make_parser!(flag_in_bracket -> (Switch<'a>, Option<Param<'a>>),
    delimited(
        (char('['), space0),
        (usage_switch_parser, opt(param_parser)),
        (take_till_bracket, char(']'))
    )
);

// walk the joined SYNOPSIS-line text, collecting every flag-bracketed
// switch + its first param. non-flag tokens (positional brackets,
// command name, ellipses) are skipped one char at a time.
make_parser!(pub parse_usage_flags -> Vec<(Switch<'a>, Option<Param<'a>>)>,
    many0(alt((
        map(flag_in_bracket, Some),
        // `value(None, ...)` requires `None: Clone` which forces Clone
        // on Switch/Param; `map(..., |_| None)` doesn't.
        map(satisfy(|c| c != '\n' && c != '\r'), |_| None),
    )))
    => |v: Vec<Option<(Switch<'a>, Option<Param<'a>>)>>|
        v.into_iter().flatten().collect()
);
