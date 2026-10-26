use nom::{
    IResult,
    Parser as _,
    character::complete::{
        line_ending,
        space0,
    },
    combinator::verify,
    multi::many1,
    sequence::{
        preceded,
        terminated,
    },
};

use crate::{
    make_parser,
    parsers::help::helpers::{
        eol,
        rest_of_line,
        visual_indent,
    },
};

/// dash-led lines at or past the desc column stay desc.
fn continuation_line(input: &str, body_indent: Option<u8>) -> IResult<&str, &str> {
    verify(terminated(rest_of_line, eol), |line: &str| {
        let (indent, content) = indent_and_content(line);
        let flag_shaped = content.starts_with('-') && !content.starts_with("- ");
        indent >= 8 && !(flag_shaped && body_indent.is_none_or(|body| indent < body))
    })
    .parse(input)
}

fn indent_and_content(line: &str) -> (u8, &str) {
    let content = line.trim_start();
    (
        visual_indent(line.strip_suffix(content).unwrap_or("")),
        content,
    )
}

make_parser!(blank_line<'src> -> &'src str, terminated(space0, line_ending));

/// always succeeds, first line may be empty (switch immediately followed by a
/// newline, "clap long" style).
///
/// # Errors
///
/// never errors past the first line.
pub fn description(input: &str) -> IResult<&str, (&str, Vec<&str>)> {
    let (mut rem, first) = terminated(preceded(space0, rest_of_line), eol).parse(input)?;
    let mut lines = Vec::new();
    let mut body_indent = None;

    loop {
        if let Ok((next, line)) = continuation_line(rem, body_indent) {
            let (indent, content) = indent_and_content(line);
            if !content.is_empty() {
                body_indent.get_or_insert(indent);
            }
            lines.push(content);
            rem = next;
        } else if let Ok((after_blanks, _)) = many1(blank_line).parse(rem)
            && continuation_line(after_blanks, body_indent).is_ok()
        {
            lines.push("");
            rem = after_blanks;
        } else {
            break;
        }
    }
    Ok((rem, (first, lines)))
}
