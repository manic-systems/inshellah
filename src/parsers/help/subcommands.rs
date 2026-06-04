use nom::{
    branch::alt,
    bytes::complete::{tag, take_till, take_while1},
    character::complete::{char, space0},
    combinator::{not, value, verify},
    multi::many0,
    sequence::{delimited, preceded, terminated},
    AsChar, IResult, Parser,
};

use crate::make_parser;
use crate::parsers::help::helpers::{eol, is_option_char};
use crate::parsers::manpage::ManpageSubcommand;

fn is_placeholder(c: char) -> bool {
    match c {
        _ if c.is_alphanumeric() => true,
        '_' | '-' | '.' | '|' | ',' => true,
        _ => false,
    }
}

/// chars in a bare placeholder token. excludes lowercase so mixed-case words
/// like "NixOS" aren't swallowed.
fn is_bare_placeholder_char(c: char) -> bool {
    matches!(c, 'A'..='Z' | '0'..='9' | '_' | '-' | '.' | '|' | ',')
}

make_parser!(
    skip_arg_placeholders -> (),
    value(
        (),
        many0(preceded(
            char(' '),
            alt((
                delimited(char('<'), take_while1(is_placeholder), char('>')),
                delimited(char('['), take_while1(is_placeholder), char(']')),
                // first char uppercase or digit rejects "NixOS"-style mixed-case.
                verify(
                    take_while1(is_bare_placeholder_char),
                    |s: &str| {
                        let first = s.chars().next().unwrap();
                        first.is_ascii_uppercase() || first.is_ascii_digit()
                    }
                ),
            )),
        )),
    )
);

// comma-separated aliases before the desc gap, e.g. cargo's `build, b`.
// discard to keep the canonical first name, else the comma fails the
// two-space check and the whole line is dropped.
make_parser!(
    skip_subcommand_aliases -> (),
    value(
        (),
        many0(preceded(alt((tag(", "), tag(" | "))), take_while1(is_option_char))),
    )
);

make_parser!(pub subcommand_entry -> ManpageSubcommand,
    (
        preceded(
            space0,
            verify(
                preceded(not(char('-')), take_while1(is_option_char)),
                |n: &str| n.len() >= 2,
            ),
        ),
        skip_subcommand_aliases,
        skip_arg_placeholders,
        // column gap is 2+ spaces or a tab; long names (get-permissions) abut a
        // bare tab with no padding spaces.
        alt((tag("  "), tag("\t"))),
        space0,
        terminated(take_till(|c: char| c.is_newline()), eol),
    ) => |(name, _, _, _, _, desc): (&'a str, _, _, _, _, &'a str)| {
        let d = desc.trim_start();
        let desc = d.strip_prefix("- ").map(|s| s.trim_start()).unwrap_or(d);
        // name kept as-parsed, build_help_result lowercases at assembly.
        ManpageSubcommand { name: name.to_string(), desc: desc.to_string() }
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> (String, String) {
        let (_, sc) = subcommand_entry(line).expect("subcommand_entry");
        (sc.name, sc.desc)
    }

    #[test]
    fn plain_entry() {
        let (name, desc) = parse("    build    Compile the package\n");
        assert_eq!(name, "build");
        assert_eq!(desc, "Compile the package");
    }

    #[test]
    fn comma_alias_keeps_canonical_name() {
        // cargo's `build, b`: alias discarded, canonical name survives,
        // two-space gap still parses.
        let (name, desc) = parse("    build, b    Compile the current package\n");
        assert_eq!(name, "build");
        assert_eq!(desc, "Compile the current package");
        let (name, _) = parse("    check, c    Analyze the package\n");
        assert_eq!(name, "check");
    }

    #[test]
    fn pipe_alias_keeps_canonical_name() {
        // pw-cli's `help | h`: pipe-separated alias discarded, canonical survives.
        let (name, desc) = parse("    help | h    Show this help\n");
        assert_eq!(name, "help");
        assert_eq!(desc, "Show this help");
    }

    #[test]
    fn tab_column_gap_separates_desc() {
        // long names abut a bare tab with no padding spaces (pw-cli get-permissions).
        let (name, desc) = parse("    get-permissions | gp\tGet permissions of a client\n");
        assert_eq!(name, "get-permissions");
        assert_eq!(desc, "Get permissions of a client");
    }

    #[test]
    fn dash_prefixed_description_is_stripped() {
        let (name, desc) = parse("  clone  - Clone a repository\n");
        assert_eq!(name, "clone");
        assert_eq!(desc, "Clone a repository");
    }

    #[test]
    fn arg_placeholders_are_skipped() {
        // `<url>` and `[DIR]` are placeholders, not name or desc.
        let (name, desc) = parse("    add <url> [DIR]    Add a dependency\n");
        assert_eq!(name, "add");
        assert_eq!(desc, "Add a dependency");
    }

    #[test]
    fn flag_line_is_rejected() {
        assert!(subcommand_entry("    --verbose    be loud\n").is_err());
    }

    #[test]
    fn single_char_name_is_rejected() {
        assert!(subcommand_entry("    x    too short\n").is_err());
    }
}
