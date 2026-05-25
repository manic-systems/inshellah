use nom::{
    branch::alt, bytes::complete::take_till, character::complete::line_ending, combinator::eof,
    AsChar, IResult, Parser,
};
#[allow(unused_imports)]
use nom::{bytes::complete::take_while, combinator::peek, combinator::verify};

#[macro_export]
macro_rules! make_parser {
    (pub $name:ident -> $out:ty, $parser:expr => $wrap:expr) => {
        #[allow(clippy::needless_lifetimes)]
        #[allow(mismatched_lifetime_syntaxes)]
        pub fn $name<'a>(s: &'a str) -> IResult<&'a str, $out> {
            let (rem, val) = $parser.parse(s)?;
            Ok((rem, $wrap(val)))
        }
    };
    (pub $name:ident -> $out:ty, $parser:expr) => {
        #[allow(clippy::needless_lifetimes)]
        #[allow(mismatched_lifetime_syntaxes)]
        pub fn $name<'a>(s: &'a str) -> IResult<&'a str, $out> {
            $parser.parse(s)
        }
    };
    ($name:ident -> $out:ty, $parser:expr => $wrap:expr) => {
        #[allow(clippy::needless_lifetimes)]
        #[allow(mismatched_lifetime_syntaxes)]
        fn $name<'a>(s: &'a str) -> IResult<&'a str, $out> {
            let (rem, val) = $parser.parse(s)?;
            Ok((rem, $wrap(val)))
        }
    };
    ($name:ident -> $out:ty, $parser:expr) => {
        #[allow(clippy::needless_lifetimes)]
        #[allow(mismatched_lifetime_syntaxes)]
        fn $name<'a>(s: &'a str) -> IResult<&'a str, $out> {
            $parser.parse(s)
        }
    };
}

#[macro_export]
macro_rules! make_predicate {
    (pub $name:ident, |$c:ident| $($body:tt)*) => {
        pub fn $name($c: char) -> bool { $($body)* }
    };
    ($name:ident, |$c:ident| $($body:tt)*) => {
        fn $name($c: char) -> bool { $($body)* }
    };
}

make_predicate!(pub is_option_char, |c| c.is_alphanumeric() || matches!(c, '-' | '_'));

make_parser!(pub rest_of_line -> &'a str,
    take_till(|c: char| c.is_newline())
);

// end of line — matches either a newline or end of input.
// permissive version used in most line-consuming parsers.
make_parser!(pub eol -> &'a str, alt((line_ending, eof)));

/// compute the visual indent of a leading whitespace run.
/// spaces count 1, tabs count 8 (typical terminal default).
pub fn visual_indent(s: &str) -> u8 {
    s.chars().fold(0u8, |acc, c| {
        acc.saturating_add(match c {
            ' ' => 1,
            '\t' => 8,
            _ => 0,
        })
    })
}

/// nom-shaped check that the input begins with at least `min` visual
/// columns of horizontal whitespace (spaces or tabs). doesn't consume —
/// pair with `space0`/`take_while` to actually eat the indent.
pub fn at_least_indent<'a>(
    min: u8,
) -> impl Parser<&'a str, Output = &'a str, Error = nom::error::Error<&'a str>> {
    verify(
        peek(take_while(|c: char| c == ' ' || c == '\t')),
        move |s: &str| visual_indent(s) >= min,
    )
}

/// legacy helper: returns (byte index of first non-space, visual indent).
/// used by callers that still need the byte index.
pub fn get_indent(s: &str) -> (usize, u8) {
    let mut traversed = 0;
    let mut indent = 0;
    for (i, c) in s.char_indices() {
        let incr = match c {
            ' ' => 1,
            '\t' => 8,
            _ => 0,
        };
        if incr == 0 {
            traversed = i;
            break;
        } else {
            indent += incr;
        }
    }
    (traversed, indent)
}
