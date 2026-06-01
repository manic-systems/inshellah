use nom::{
    character::complete::space0,
    combinator::verify,
    multi::many0,
    sequence::{preceded, terminated},
    IResult, Parser,
};

use crate::make_parser;
use crate::parsers::help::helpers::{at_least_indent, eol, rest_of_line};

// indented (≥8 visual cols), non-flag-shaped line belonging to the previous
// flag's desc. blank-but-indented lines accepted, filtered by the caller's join.
make_parser!(continuation_line -> &'a str,
    verify(
        preceded(
            // assert without consuming, space0 inside `rest_of_line`'s preceded
            // eats the indent next.
            at_least_indent(8),
            terminated(preceded(space0, rest_of_line), eol)
        ),
        // leading '-' is a new flag entry, not a continuation.
        |content: &&str| !content.starts_with('-')
    )
);

// always succeeds, first line may be empty (switch immediately followed by a
// newline, "clap long" style).
make_parser!(pub description -> (&'a str, Vec<&'a str>),
(
    terminated(preceded(space0, rest_of_line), eol),
    many0(continuation_line),
));
