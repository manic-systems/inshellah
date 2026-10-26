use inshellah::parsers::manpage::{
    OwnedSwitch,
    ParamKind,
    parse_manpage_string,
    parse_manpage_with_subs,
};

const TP_MANPAGE: &str = r#".TH FOO 1 "2024" "1.0" "User Commands"
.SH NAME
foo \- a synthetic test command
.SH SYNOPSIS
.B foo
[\fIOPTIONS\fR] <input> [output]
.SH OPTIONS
.TP
\fB\-v\fR, \fB\-\-verbose\fR
increase output verbosity
.TP
\fB\-o\fR \fIFILE\fR, \fB\-\-output\fR=\fIFILE\fR
write to FILE
.TP
\fB\-h\fR, \fB\-\-help\fR
show this help and exit
"#;

const HP_MANPAGE: &str = r#".TH BAT "1"
.SH NAME
bat \- demo
.SH "OPTIONS"
.HP
\fB\-A\fR, \fB\-\-show\-all\fR
.IP
Show non-printable characters.
.HP
\fB\-\-nonprintable\-notation\fR <notation>
.IP
Specify how to display non-printable characters.

Possible values:
.RS
.IP "caret"
Use character sequences like ^G ...
.IP "unicode"
Use special Unicode code points ...
.RE
.HP
\fB\-l\fR, \fB\-\-language\fR <language>
.IP
Set the language.
"#;

const TEXT_RS_NESTED_MANPAGE: &str = r#".TH TOOL "1"
.SH NAME
tool \- demo
.SH "OPTIONS"
.SS INPUT
\fB\-x\fR, \fB\-\-foo\fR
.RS 4
First flag desc. Possible values:
.RS
some value
.RE
After the inner block.
.RE
.sp
\fB\-y\fR, \fB\-\-bar\fR
.RS 4
Second flag desc.
.RE
"#;

const TEXT_RS_MANPAGE: &str = r#".TH RG "1"
.SH NAME
rg \- demo
.SH "OPTIONS"
.SS INPUT OPTIONS
\fB\-e\fR \fIPATTERN\fR, \fB\-\-regexp\fR=\fIPATTERN\fR
.RS 4
A pattern to search for. This option can be provided multiple times.
.RE
.sp
\fB\-f\fR \fIPATTERNFILE\fR, \fB\-\-file\fR=\fIPATTERNFILE\fR
.RS 4
Search for patterns from the given file.
.RE
.sp
\fB\-x\fR, \fB\-\-line\-regexp\fR
.RS 4
Only show matches surrounded by line boundaries.
.RE
"#;

const TP_CLAP_DUAL_PARAGRAPH: &str = r#".TH JJ "1"
.SH NAME
jj \- demo
.SH OPTIONS
.TP
\fB\-\-at\-operation\fR <OP>
Operation to load the repo at

Operation to load the repo at. By default, Jujutsu loads the repo at the most recent operation, and lots of additional sentences that go on for paragraphs.
.TP
\fB\-h\fR, \fB\-\-help\fR
Print help
"#;

const JJ_XREF_MANPAGE: &str = r#".TH "JJ-BOOKMARK" "1"
.SH NAME
jj\-bookmark \- Manage bookmarks
.SH SYNOPSIS
\fBjj bookmark\fR [\fB\-h\fR|\fB\-\-help\fR] <\fIsubcommands\fR>
.SH SUBCOMMANDS
.TP
jj\-bookmark\-create(1)
Create a new bookmark
.TP
jj\-bookmark\-set\-url(1)
Update a bookmark's url
.TP
jj\-bookmark\-untrack(1)
Stop tracking given remote bookmarks
"#;

#[test]
fn hp_strategy_extracts_flags_and_skips_rs_example_values() {
    // bat uses .HP for flag tags and nests example values in .RS/.RE; the inner
    // .IP "caret"/"unicode" tags are not flags.
    let result = parse_manpage_string(HP_MANPAGE);
    let names: Vec<String> = result
        .entries
        .iter()
        .map(|entry| {
            match &entry.switch {
                OwnedSwitch::Long(long) | OwnedSwitch::Both(_, long) => long.clone(),
                OwnedSwitch::Short(short) => short.to_string(),
            }
        })
        .collect();
    assert_eq!(
        names,
        vec!["show-all", "nonprintable-notation", "language"],
        "expected 3 flags, got {names:?}"
    );
    assert!(
        !result.entries.iter().any(|entry| {
            matches!(
                &entry.switch,
                OwnedSwitch::Long(long) if long == "caret" || long == "unicode"
            )
        }),
        "inner .RS .IP example values must not be picked up as flags: {:?}",
        result.entries
    );
    assert!(matches!(
        result.entries[0].switch,
        OwnedSwitch::Both('A', ref long) if long == "show-all"
    ));
    assert!(matches!(
        result.entries[2].switch,
        OwnedSwitch::Both('l', ref long) if long == "language"
    ));
}

#[test]
fn text_rs_strategy_handles_nested_rs_in_description() {
    // a flag's `.RS` body nesting another `.RS/.RE` must not end early at the
    // inner `.RE`, else the next flag's tag is misread as top-level text or the
    // first desc is truncated.
    let result = parse_manpage_string(TEXT_RS_NESTED_MANPAGE);
    assert_eq!(
        result.entries.len(),
        2,
        "expected exactly 2 flags, got {}",
        result.entries.len()
    );
    assert!(matches!(
        result.entries[0].switch,
        OwnedSwitch::Both('x', ref long) if long == "foo"
    ));
    assert!(
        result.entries[0].desc.contains("First flag desc"),
        "outer .RS body should be captured, got: {:?}",
        result.entries[0].desc
    );
    assert!(
        result.entries[0].desc.contains("After the inner block"),
        "text after the nested .RE must still belong to the outer block, got: {:?}",
        result.entries[0].desc
    );
    assert!(
        !result.entries[0].desc.contains("some value"),
        "inner .RS sub-value text should be skipped, got: {:?}",
        result.entries[0].desc
    );
    assert!(matches!(
        result.entries[1].switch,
        OwnedSwitch::Both('y', ref long) if long == "bar"
    ));
    assert!(result.entries[1].desc.contains("Second flag desc"));
}

#[test]
fn text_rs_strategy_extracts_ripgrep_style_flags() {
    // rg's layout: bare Text tag immediately followed by `.RS/.RE`, separated
    // by `.sp`, no `.PP` to anchor on.
    let result = parse_manpage_string(TEXT_RS_MANPAGE);
    assert_eq!(
        result.entries.len(),
        3,
        "expected 3 entries, got {}",
        result.entries.len()
    );
    // PARAM between short and comma
    assert!(matches!(
        result.entries[0].switch,
        OwnedSwitch::Both('e', ref long) if long == "regexp"
    ));
    assert!(matches!(
        &result.entries[0].param,
        Some(param) if param.kind == ParamKind::Mandatory && param.name == "PATTERN"
    ));
    assert!(
        result.entries[0]
            .desc
            .starts_with("A pattern to search for")
    );
    assert!(matches!(
        result.entries[1].switch,
        OwnedSwitch::Both('f', ref long) if long == "file"
    ));
    // plain comma form, no PARAM
    assert!(matches!(
        result.entries[2].switch,
        OwnedSwitch::Both('x', ref long) if long == "line-regexp"
    ));
}

#[test]
fn tp_strategy_stops_description_at_blank_line() {
    // clap emits "summary\n\nexpanded body"; keep just the summary. leading
    // blanks (tag to first body line) skip; blanks only terminate once text is
    // collected.
    let result = parse_manpage_string(TP_CLAP_DUAL_PARAGRAPH);
    let at_op = result
        .entries
        .iter()
        .find(|entry| matches!(&entry.switch, OwnedSwitch::Long(long) if long == "at-operation"))
        .expect("--at-operation entry");
    assert_eq!(
        at_op.desc, "Operation to load the repo at",
        "expected only the summary line, got: {:?}",
        at_op.desc
    );
    // the second .TP block still parses (next entry not swallowed).
    assert!(result.entries.iter().any(|entry| {
        matches!(
            &entry.switch,
            OwnedSwitch::Both('h', long) if long == "help"
        )
    }));
}

#[test]
fn tp_strategy_extracts_flags() {
    let result = parse_manpage_string(TP_MANPAGE);
    assert_eq!(
        result.entries.len(),
        3,
        "expected 3 entries, got {:?}",
        result.entries
    );
    assert_eq!(result.description, "a synthetic test command");
    assert!(matches!(
        result.entries[0].switch,
        OwnedSwitch::Both('v', ref long) if long == "verbose"
    ));
    assert!(matches!(
        result.entries[2].switch,
        OwnedSwitch::Both('h', ref long) if long == "help"
    ));
    assert!(result.entries[0].desc.contains("verbosity"));
}

#[test]
fn subcommand_xrefs_populate_subcommands() {
    let result = parse_manpage_string(JJ_XREF_MANPAGE);
    let names: Vec<&str> = result
        .subcommands
        .iter()
        .map(|sub| sub.name.as_str())
        .collect();
    assert_eq!(names, ["create", "set-url", "untrack"], "got {names:?}");
    // shared "jj-bookmark-" prefix stripped, multi-word child intact.
    let set_url = result
        .subcommands
        .iter()
        .find(|sub| sub.name == "set-url")
        .expect("set-url child");
    assert_eq!(set_url.desc, "Update a bookmark's url");
}

#[test]
fn mixed_option_subsections_keep_local_strategy_winners() {
    let groff = r#".TH MIXED "1"
.SH NAME
mixed \- demo
.SH OPTIONS
.SS GENERAL OPTIONS
.TP
\fB\-a\fR, \fB\-\-all\fR
Show all entries.
.SS SEARCH OPTIONS
\fB\-e\fR \fIPATTERN\fR, \fB\-\-regexp\fR=\fIPATTERN\fR
.RS 4
Search for a pattern.
.RE
.sp
\fB\-f\fR \fIFILE\fR, \fB\-\-file\fR=\fIFILE\fR
.RS 4
Read patterns from a file.
.RE
"#;
    let result = parse_manpage_string(groff);
    assert_eq!(result.entries.len(), 3, "entries: {:?}", result.entries);
    assert!(result.entries.iter().any(|entry| {
        matches!(
            &entry.switch,
            OwnedSwitch::Both('a', long) if long == "all"
        )
    }));
    assert!(result.entries.iter().any(|entry| {
        matches!(
            &entry.switch,
            OwnedSwitch::Both('e', long) if long == "regexp"
        )
    }));
    assert!(result.entries.iter().any(|entry| {
        matches!(
            &entry.switch,
            OwnedSwitch::Both('f', long) if long == "file"
        )
    }));
}

#[test]
fn description_only_alias_merge_rejects_generic_descriptions() {
    let groff = r#".TH ALIASES "1"
.SH NAME
aliases \- demo
.SH OPTIONS
.TP
\fB\-a\fR
Enable output
.TP
\fB\-\-all\fR
Enable output
"#;
    let result = parse_manpage_string(groff);
    assert_eq!(result.entries.len(), 2, "entries: {:?}", result.entries);
    assert!(
        !result
            .entries
            .iter()
            .any(|entry| matches!(&entry.switch, OwnedSwitch::Both('a', long) if long == "all")),
        "generic identical descriptions should not synthesize aliases: {:?}",
        result.entries
    );
    assert!(
        result
            .entries
            .iter()
            .any(|entry| matches!(entry.switch, OwnedSwitch::Short('a')))
    );
    assert!(
        result
            .entries
            .iter()
            .any(|entry| matches!(&entry.switch, OwnedSwitch::Long(long) if long == "all"))
    );
}

#[test]
fn clap_subcommand_sections_keep_usage_positionals() {
    let groff = r#".TH APP "1"
.SH NAME
app \- demo
.SH SYNOPSIS
app [OPTIONS] <COMMAND>
.SH SUBCOMMAND
Clone a repository.
Usage: clone [OPTIONS] <repository> [directory]
.TP
\fB\-\-depth\fR \fIDEPTH\fR
Limit history depth.
"#;
    let (_parent, subs) = parse_manpage_with_subs(groff);
    assert_eq!(subs.len(), 1, "subs: {subs:?}");
    let (name, result) = &subs[0];
    assert_eq!(name, "clone");
    assert_eq!(
        result
            .positionals
            .iter()
            .map(|(positional, _)| positional.as_str())
            .collect::<Vec<_>>(),
        vec!["repository", "directory"],
        "positionals: {:?}",
        result.positionals
    );
    assert!(result.entries.iter().any(|entry| {
        matches!(
            &entry.switch,
            OwnedSwitch::Long(long) if long == "depth"
        )
    }));
}
