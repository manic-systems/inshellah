use nom::{
    AsChar as _,
    IResult,
    Parser as _,
    branch::alt,
    bytes::complete::{
        tag,
        take_till,
        take_while1,
    },
    character::complete::{
        char,
        space0,
    },
    combinator::{
        not,
        value,
        verify,
    },
    multi::many0,
    sequence::{
        delimited,
        preceded,
        terminated,
    },
};

use crate::{
    make_parser,
    parsers::{
        help::helpers::{
            eol,
            is_option_char,
        },
        manpage::ManpageSubcommand,
    },
};

fn is_placeholder(ch: char) -> bool {
    match ch {
        _ if ch.is_alphanumeric() => true,
        '_' | '-' | '.' | '|' | ',' => true,
        _ => false,
    }
}

/// chars in a bare placeholder token. excludes lowercase so mixed-case words
/// like "NixOS" aren't swallowed.
const fn is_bare_placeholder_char(ch: char) -> bool {
    matches!(ch, 'A'..='Z' | '0'..='9' | '_' | '-' | '.' | '|' | ',')
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
                    |token: &str| {
                        token
                            .chars()
                            .next()
                            .is_some_and(|first| first.is_ascii_uppercase() || first.is_ascii_digit())
                    }
                ),
            )),
        )),
    )
);

// aliases before the desc gap, comma- (cargo `build, b`) or pipe-separated
// (pw-cli `help | h`). captured as alternates; the canonical first name leads.
make_parser!(
    collect_subcommand_aliases<'src> -> Vec<&'src str>,
    many0(preceded(alt((tag(", "), tag(" | "))), take_while1(is_option_char)))
);

make_parser!(pub subcommand_entry<'src> -> ManpageSubcommand,
    (
        preceded(
            space0,
            verify(
                preceded(not(char('-')), take_while1(is_option_char)),
                |name: &str| name.len() >= 2,
            ),
        ),
        collect_subcommand_aliases,
        skip_arg_placeholders,
        // column gap is 2+ spaces or a tab; long names (get-permissions) abut a
        // bare tab with no padding spaces.
        alt((tag("  "), tag("\t"))),
        space0,
        terminated(take_till(|ch: char| ch.is_newline()), eol),
    ) => |(name, aliases, (), _, _, desc): (&'src str, Vec<&'src str>, _, _, _, &'src str)| {
        let trimmed = desc.trim_start();
        let clean_desc = trimmed.strip_prefix("- ").map_or(trimmed, str::trim_start);
        // name/aliases kept as-parsed, build_help_result lowercases at assembly.
        ManpageSubcommand {
            name: name.to_owned(),
            desc: clean_desc.to_owned(),
            aliases: aliases.into_iter().map(str::to_owned).collect(),
        }
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
        let (check_name, _) = parse("    check, c    Analyze the package\n");
        assert_eq!(check_name, "check");
    }

    #[test]
    fn pipe_alias_keeps_canonical_name() {
        // pw-cli's `help | h`: pipe-separated alias discarded, canonical
        // survives.
        let (name, desc) = parse("    help | h    Show this help\n");
        assert_eq!(name, "help");
        assert_eq!(desc, "Show this help");
    }

    #[test]
    fn tab_column_gap_separates_desc() {
        // long names abut a bare tab with no padding spaces (pw-cli
        // get-permissions).
        let (name, desc) = parse("    get-permissions | gp\tGet permissions of a client\n");
        assert_eq!(name, "get-permissions");
        assert_eq!(desc, "Get permissions of a client");
    }

    #[test]
    fn aliases_are_captured_pipe_and_comma() {
        let (_, pipe) = subcommand_entry("    help | h    Show this help\n").expect("pipe");
        assert_eq!(pipe.aliases, vec!["h"]);
        let (_, comma) = subcommand_entry("    build, b    Compile\n").expect("comma");
        assert_eq!(comma.aliases, vec!["b"]);
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
        subcommand_entry("    --verbose    be loud\n").unwrap_err();
    }

    #[test]
    fn single_char_name_is_rejected() {
        subcommand_entry("    x    too short\n").unwrap_err();
    }
}
