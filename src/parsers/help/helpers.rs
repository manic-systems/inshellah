use nom::{
    AsChar as _,
    IResult,
    Parser,
    branch::alt,
    bytes::complete::{
        take_till,
        take_while,
    },
    character::complete::line_ending,
    combinator::{
        eof,
        peek,
        verify,
    },
    error::Error,
};

#[macro_export]
macro_rules! make_parser {
    ($vis:vis $name:ident<$src:lifetime> -> $out:ty, $parser:expr $(=> $wrap:expr)?) => {
        /// # Errors
        ///
        /// nom error when `input` does not match.
        $vis fn $name<$src>(input: &$src str) -> IResult<&$src str, $out> {
            let (rem, val) = $parser.parse(input)?;
            Ok((rem, $($wrap)?(val)))
        }
    };
    ($vis:vis $name:ident -> $out:ty, $parser:expr $(=> $wrap:expr)?) => {
        /// # Errors
        ///
        /// nom error when `input` does not match.
        $vis fn $name(input: &str) -> IResult<&str, $out> {
            let (rem, val) = $parser.parse(input)?;
            Ok((rem, $($wrap)?(val)))
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

make_predicate!(pub is_option_char, |ch| ch.is_alphanumeric() || matches!(ch, '-' | '_'));

make_parser!(pub rest_of_line<'src> -> &'src str,
    take_till(|ch: char| ch.is_newline())
);

make_parser!(pub eol<'src> -> &'src str, alt((line_ending, eof)));

/// visual indent of a leading whitespace run. spaces count 1, tabs count 8.
pub fn visual_indent(whitespace: &str) -> u8 {
    whitespace.chars().fold(0_u8, |acc, ch| {
        acc.saturating_add(match ch {
            ' ' => 1,
            '\t' => 8,
            _ => 0,
        })
    })
}

/// non-consuming check that input begins with ≥`min` visual cols of
/// horizontal whitespace. pair with `space0`/`take_while` to eat it.
pub fn at_least_indent<'src>(
    min: u8,
) -> impl Parser<&'src str, Output = &'src str, Error = Error<&'src str>> {
    verify(
        peek(take_while(|ch: char| ch == ' ' || ch == '\t')),
        move |run: &str| visual_indent(run) >= min,
    )
}

/// (byte index of first non-space, visual indent), for callers that need the
/// byte index.
pub fn get_indent(text: &str) -> (usize, u8) {
    let mut traversed = 0;
    let mut indent = 0;
    for (idx, ch) in text.char_indices() {
        let incr = match ch {
            ' ' => 1,
            '\t' => 8,
            _ => 0,
        };
        if incr == 0 {
            traversed = idx;
            break;
        }
        indent += incr;
    }
    (traversed, indent)
}
