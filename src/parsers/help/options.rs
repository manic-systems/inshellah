use crate::{
    make_parser,
    parsers::{
        help::helpers::is_option_char,
        manpage::{
            OwnedParam,
            OwnedSwitch,
            ParamChoice,
            ParamKind,
            ParamOrigin,
        },
    },
};

const PLACEHOLDER_PARAM: &str = "value";

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
    OptionalWord(&'src str),
    Guessed(&'src str),
    Placeholder,
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

// `yes|no` spelled out in place of a value name.
fn word_choices(name: &str) -> Vec<ParamChoice> {
    let words: Vec<&str> = name.split('|').collect();
    let all_words = words.iter().all(|word| {
        !word.is_empty()
            && word
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    });
    if words.len() < 2 || !all_words {
        return Vec::new();
    }
    words
        .into_iter()
        .map(|word| {
            ParamChoice {
                value: word.to_owned(),
                desc:  String::new(),
            }
        })
        .collect()
}

impl From<Param<'_>> for OwnedParam {
    fn from(param: Param<'_>) -> Self {
        match param {
            Param::Mandatory(name) => Self::new(ParamKind::Mandatory, name.to_owned()),
            Param::Optional(name) => {
                let mut optional = Self::new(ParamKind::Optional, name.to_owned());
                optional.choices = word_choices(name);
                optional
            },
            Param::OptionalWord(name) => Self::new(ParamKind::OptionalWord, name.to_owned()),
            Param::Guessed(name) => {
                Self::new(ParamKind::Mandatory, name.to_owned())
                    .with_origin(ParamOrigin::ProseGuess)
            },
            Param::Placeholder => {
                Self::new(ParamKind::Mandatory, PLACEHOLDER_PARAM.to_owned())
                    .with_origin(ParamOrigin::Placeholder)
            },
        }
    }
}

use nom::{
    Err as NomErr,
    IResult,
    Parser as _,
    branch::alt,
    bytes::complete::{
        tag,
        take_till,
        take_till1,
        take_while1,
    },
    character::complete::{
        char,
        satisfy,
        space0,
        space1,
    },
    combinator::{
        eof,
        map,
        opt,
        peek,
        recognize,
        value,
        verify,
    },
    error::{
        Error as NomError,
        ErrorKind,
    },
    multi::many0,
    sequence::{
        delimited,
        preceded,
        separated_pair,
        terminated,
    },
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

// the text up to the bracket that closes an already opened one, nested pairs
// included.
fn balanced_brackets(input: &str) -> IResult<&str, &str> {
    let mut depth = 0_usize;
    for (idx, ch) in input.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' if depth == 0 => {
                return match input.split_at_checked(idx) {
                    Some((inner, rest)) if idx > 0 => Ok((rest, inner)),
                    _ => Err(NomErr::Error(NomError::new(input, ErrorKind::TakeTill1))),
                };
            },
            ']' => depth -= 1,
            '\n' | ' ' => break,
            _ => {},
        }
    }
    Err(NomErr::Error(NomError::new(input, ErrorKind::TakeTill1)))
}

make_parser!(comma -> (),
    value((), preceded(char(','), space0)));

make_parser!(eq_optional_param<'src> -> Param<'src>,
    delimited(tag("[="), take_while1(is_option_char), char(']')) => Param::Optional);

make_parser!(eq_optional_angle_param<'src> -> Param<'src>,
    delimited(tag("[=<"), take_till1(|ch| ch == '>'), tag(">]")) => Param::Optional);

// `--lines[=[+]INTEGER]` and `--diff[=yes|no]`, any text up to the matching
// bracket.
make_parser!(eq_optional_any_param<'src> -> Param<'src>,
    delimited(tag("[="), balanced_brackets, char(']')) => Param::Optional);

make_parser!(eq_mandatory_param<'src> -> Param<'src>,
    preceded(char('='), take_while1(is_option_char)) => Param::Mandatory);

// `--to=[USER@]HOST`.
make_parser!(eq_bracket_prefixed_param<'src> -> Param<'src>,
    preceded(char('='), recognize((
        delimited(char('['), balanced_brackets, char(']')),
        take_while1(is_option_char),
    ))) => Param::Mandatory);

make_parser!(eq_angle_param<'src> -> Param<'src>,
    preceded(char('='), delimited(char('<'), take_till1(|ch| ch == '>'), char('>')))
        => Param::Mandatory);

// `--require=...` and `--debugger-arguments=…` take a value without naming it.
make_parser!(eq_ellipsis_param<'src> -> Param<'src>,
    preceded(char('='), alt((tag("..."), tag("\u{2026}")))) => |_| Param::Placeholder);

// systemd manpages write `--type=` with the placeholder dropped.
make_parser!(eq_bare_param<'src> -> Param<'src>,
    terminated(char('='), eof) => |_| Param::Placeholder);

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
        char('>'))) => Param::OptionalWord);

make_parser!(spaced_opt_bracket_param<'src> -> Param<'src>,
    preceded(char(' '), delimited(tag("[<"), take_till1(|ch| ch == '>'), tag(">]")))
        => Param::OptionalWord);

make_parser!(brace_choice_param<'src> -> Param<'src>,
    preceded(
        alt((char(' '), char('='))),
        recognize(delimited(char('{'), take_till1(|ch| ch == '}' || ch == '\n'), char('}')))
    ) => Param::Mandatory);

make_parser!(spaced_angle_param_after_space<'src> -> Param<'src>,
    preceded(space1, delimited(char('<'), take_till1(|ch| ch == '>'), char('>'))) => Param::Mandatory);

// value type names pflag prints after a flag. `count` is left out since a
// counter flag takes no value.
const PFLAG_TYPES: &[&str] = &[
    "string",
    "stringArray",
    "stringSlice",
    "stringToString",
    "stringToInt",
    "stringToInt64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "intSlice",
    "int32Slice",
    "int64Slice",
    "uintSlice",
    "float",
    "float32",
    "float64",
    "float32Slice",
    "float64Slice",
    "duration",
    "durationSlice",
    "ip",
    "ipSlice",
    "ipMask",
    "ipNet",
    "bytesHex",
    "bytesBase64",
    "boolSlice",
];

// full lowercase token then verify <=10 chars. a 10-char-capped
// take_while_m_n leaves a partial match: "--foo nanoseconds" yields
// "nanosecond" and "s" as desc. a word >10 chars is almost certainly desc.
// only a lowercase word with prose right behind it on the same line may be
// description, `--ignore-command-error ignore exit codes`.
fn spaced_type_param(input: &str) -> IResult<&str, Param<'_>> {
    let (rem, word) = preceded(
        char(' '),
        verify(take_while1(|ch: char| !ch.is_whitespace()), |word: &str| {
            PFLAG_TYPES.contains(&word)
                || (word.len() <= 10 && word.chars().all(|ch| ch.is_ascii_lowercase()))
        }),
    )
    .parse(input)?;
    let prose_follows = rem
        .strip_prefix(' ')
        .and_then(|tail| tail.chars().next())
        .is_some_and(|ch| !ch.is_whitespace());
    let letter_name = word.len() == 1 && word != "a";
    if prose_follows && !PFLAG_TYPES.contains(&word) && !letter_name {
        Ok((rem, Param::Guessed(word)))
    } else {
        Ok((rem, Param::Mandatory(word)))
    }
}

enum Default<'src> {
    Switch,
    Value(Param<'src>),
}

// cobra prints `--flag=<default>:` and pflag manpages `--flag=<default>`. a
// boolean default is a switch, any other default means the flag takes a value.
// `None` is not a default form, and an all-caps word names the value.
fn default_value(line: &str) -> Option<Default<'_>> {
    let body = line.strip_prefix('=')?.trim_end();
    let default = body.strip_suffix(':').unwrap_or(body);
    let first = default.chars().next()?;
    let quoted = default.len() >= 2 && matches!(first, '\'' | '"') && default.ends_with(first);
    let bracketed = first == '[' && default.ends_with(']');
    let numeric = (first.is_ascii_digit() || first == '-')
        && default
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'));
    let word = first.is_alphabetic() && default.chars().all(is_option_char);
    match default {
        "false" | "true" => Some(Default::Switch),
        _ if word && !default.chars().any(|ch| ch.is_ascii_lowercase()) => {
            Some(Default::Value(Param::Mandatory(default)))
        },
        _ if quoted || bracketed || numeric || word => Some(Default::Value(Param::Placeholder)),
        _ => None,
    }
}

fn default_value_param(input: &str) -> IResult<&str, Option<Param<'_>>> {
    let line_end = input.find(['\n', '\r']).unwrap_or(input.len());
    let (line, rem) = input.split_at(line_end);
    default_value(line)
        .map(|default| {
            match default {
                Default::Switch => (rem, None),
                Default::Value(param) => (rem, Some(param)),
            }
        })
        .ok_or_else(|| NomErr::Error(NomError::new(input, ErrorKind::Verify)))
}

make_parser!(param_parser_borrowed<'src> -> Param<'src>, alt((
    brace_choice_param,
    eq_optional_angle_param,
    eq_optional_param,
    eq_optional_any_param,
    eq_mandatory_param,
    eq_bracket_prefixed_param,
    eq_angle_param,
    eq_ellipsis_param,
    eq_bare_param,
    spaced_opt_angle_param,
    spaced_opt_bracket_param,
    spaced_angle_param_after_space,
    spaced_angle_param,
    spaced_uppercase_param,
    spaced_type_param,
)));

make_parser!(pub param_parser -> OwnedParam, param_parser_borrowed => OwnedParam::from);

/// the value after a flag, `None` for a switch. never errors since a flag may
/// have no value at all.
///
/// # Errors
///
/// never errors, the `IResult` shape composes with the nom parsers.
pub fn flag_param(input: &str) -> IResult<&str, Option<OwnedParam>> {
    if let Ok((rem, param)) = default_value_param(input) {
        return Ok((rem, param.map(OwnedParam::from)));
    }
    opt(param_parser).parse(input)
}

macro_rules! switch_pair {
    ($name:ident, $left:expr, $sep:expr, $right:expr => | $a:ident, $b:ident | $body:expr) => {
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

switch_pair!(long_comma_short,
    long_switch, comma, short_switch => |long, short| Switch::Both(short, long));

// iptables `--table\t-t table` and `--set-counters -c PKTS BYTES`. gnu tar
// opens descriptions with a flag, `--null   -T reads null-terminated names`.
fn long_space_short(input: &str) -> IResult<&str, Switch<'_>> {
    let (rem, (long, _, short)) = (
        long_switch,
        take_while1(|ch| ch == ' ' || ch == '\t'),
        terminated(short_switch, peek(alt((space1, eof)))),
    )
        .parse(input)?;
    let column = rem
        .strip_prefix(' ')
        .and_then(|tail| tail.split(['\t', '\n']).next())
        .and_then(|cell| cell.split("  ").next())
        .unwrap_or_default();
    let sentence = column.starts_with(|ch: char| ch.is_ascii_lowercase())
        && column.split_whitespace().count() >= 3;
    if sentence {
        return Err(NomErr::Error(NomError::new(input, ErrorKind::Verify)));
    }
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
        long_comma_short,
        long_space_short,
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
