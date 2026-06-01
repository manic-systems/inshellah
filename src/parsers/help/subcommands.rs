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

/// chars allowed inside a bare (unbracketed) placeholder token, e.g.
/// "FILE", "PATTERN...", "A|B". excludes lowercase letters so mixed-case
/// description words like "NixOS" or "Home-manager" don't get swallowed
/// as placeholders.
fn is_bare_placeholder_char(c: char) -> bool {
    matches!(c, 'A'..='Z' | '0'..='9' | '_' | '-' | '.' | '|' | ',')
}

make_parser!(
    skip_arg_placeholders -> (),
    value(
        (),
        many0(preceded(
            // peek ahead one char (don't consume) so the per-branch parser can
            // see the full token. needed because the bare ALL_CAPS branch must
            // verify the *entire* token before deciding to consume.
            char(' '),
            alt((
                // <...> bracketed placeholder
                delimited(char('<'), take_while1(is_placeholder), char('>')),
                // [...] optional bracketed placeholder
                delimited(char('['), take_while1(is_placeholder), char(']')),
                // bare ALL_CAPS placeholder — first char must be uppercase or
                // a digit (allows e.g. "N", "M2"), and the whole token must
                // be uppercase-friendly. rejects "NixOS"-style mixed-case so
                // descriptions don't get swallowed.
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

// some help formats list a subcommand with comma-separated aliases before
// the description gap, e.g. cargo's `build, b` / `check, c`. consume and
// discard the aliases so the entry parses and the canonical (first) name is
// kept; without this the comma fails the two-space check and the whole line
// — every aliased subcommand — is dropped.
make_parser!(
    skip_subcommand_aliases -> (),
    value(
        (),
        many0(preceded(tag(", "), take_while1(is_option_char))),
    )
);

// parse a subcommand entry: leading whitespace, then a name (2+ option
// chars, not starting with '-'), optional comma-separated aliases, optional
// argument placeholders, exactly two spaces, optional padding, then the
// description text and eol.
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
        tag("  "),
        space0,
        terminated(take_till(|c: char| c.is_newline()), eol),
    ) => |(name, _, _, _, _, desc): (&'a str, _, _, _, _, &'a str)| {
        // some help formats prefix desc with "- " (manpage-style); strip it.
        let d = desc.trim_start();
        let desc = d.strip_prefix("- ").map(|s| s.trim_start()).unwrap_or(d);
        // name kept as-parsed here; build_help_result lowercases at assembly
        // (matching the former From<&HelpResult> behavior).
        ManpageSubcommand { name: name.to_string(), desc: desc.to_string() }
    }
);
