use nom::{
    character::complete::space0,
    combinator::verify,
    multi::many0,
    sequence::{preceded, terminated},
    IResult, Parser,
};

use crate::make_parser;
use crate::parsers::help::helpers::{at_least_indent, eol, rest_of_line};

// continuation line: an indented (≥8 visual cols), non-flag-shaped line
// belonging to the previous flag's description. blank-but-indented lines
// are accepted (content = ""), filtered out by the caller's join.
make_parser!(continuation_line -> &'a str,
    verify(
        preceded(
            // assert ≥8 visual cols of leading horizontal whitespace
            // without consuming — space0 inside `rest_of_line`'s preceded
            // will eat them next.
            at_least_indent(8),
            terminated(preceded(space0, rest_of_line), eol)
        ),
        // reject lines whose first non-space char is '-' — that's a new
        // flag entry, not a continuation of the previous one.
        |content: &&str| !content.starts_with('-')
    )
);

// description: the line of text after the switch+param, plus any
// continuation lines. always succeeds — first line may be empty (when
// the switch is followed immediately by a newline, "clap long" style).
make_parser!(pub description -> (&'a str, Vec<&'a str>),
(
    terminated(preceded(space0, rest_of_line), eol),
    many0(continuation_line),
));
