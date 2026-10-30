use inshellah::parsers::manpage::{
    ManpageResult,
    OwnedParam,
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

fn takes_value(result: &ManpageResult, short: char) -> Option<bool> {
    result
        .entries
        .iter()
        .find(|entry| {
            matches!(&entry.switch, OwnedSwitch::Short(found) | OwnedSwitch::Both(found, _) if *found == short)
        })
        .map(|entry| entry.param.is_some())
}

#[test]
fn mdoc_nested_list_does_not_end_the_option_list() {
    let mdoc = ".Sh NAME\n.Nm tool\n.Nd demo\n.Sh DESCRIPTION\n.Bl -tag -width Ds\n.It Fl \
                a\nPlain switch.\n.It Fl O Ar ctl_cmd\nValid commands are:\n.Bl -tag -width check \
                -compact\n.It Cm check\nCheck the master.\n.It Cm exit\nExit the \
                master.\n.El\n.It Fl p Ar port\nPort to use.\n.El\n";
    let result = parse_manpage_string(mdoc);
    assert_eq!(takes_value(&result, 'O'), Some(true));
    assert_eq!(
        takes_value(&result, 'p'),
        Some(true),
        "entries: {:?}",
        result.entries
    );
    assert_eq!(takes_value(&result, 'a'), Some(false));
}

#[test]
fn mdoc_xo_item_keeps_its_parameter() {
    let mdoc = ".Sh NAME\n.Nm tool\n.Nd demo\n.Sh DESCRIPTION\n.Bl -tag -width Ds\n.It Fl L \
                Xo\n.Sm off\n.Oo Ar bind_address : Oc\n.Ar port : host : hostport\n.Sm \
                on\n.Xc\n.It Fl L Xo\n.Sm off\n.Ar local_socket : remote_socket\n.Sm \
                on\n.Xc\nForward a port.\n.It Fl q\nQuiet.\n.El\n";
    let result = parse_manpage_string(mdoc);
    assert_eq!(
        takes_value(&result, 'L'),
        Some(true),
        "entries: {:?}",
        result.entries
    );
    assert_eq!(takes_value(&result, 'q'), Some(false));
}

#[test]
fn bare_equals_long_option_takes_a_value() {
    let groff = r#".TH APP "1"
.SH NAME
app \- demo
.SH OPTIONS
.PP
\fB\-t\fR, \fB\-\-type=\fR
.RS 4
Unit types to show.
.RE
.PP
\fB\-\-all\fR
.RS 4
Show everything.
.RE
"#;
    let result = parse_manpage_string(groff);
    assert_eq!(
        takes_value(&result, 't'),
        Some(true),
        "entries: {:?}",
        result.entries
    );
    assert!(result.entries.iter().any(|entry| {
        matches!(&entry.switch, OwnedSwitch::Long(long) if long == "all") && entry.param.is_none()
    }));
}

#[test]
fn bold_word_opening_a_prose_paragraph_is_not_a_subcommand() {
    let groff = r#".TH APP "1"
.SH NAME
app \- demo
.SH COMMANDS
.PP
\fBstart\fR \fIUNIT\fR
.RS 4
Start a unit.
.RE
.SS "Environment Commands"
.PP
\fBdaemon\fR
supports an environment block passed to the processes it spawns\&.
"#;
    let result = parse_manpage_string(groff);
    let names: Vec<&str> = result
        .subcommands
        .iter()
        .map(|sub| sub.name.as_str())
        .collect();
    assert_eq!(names, vec!["start"]);
}

fn param_of(result: &ManpageResult, short: char) -> Option<&OwnedParam> {
    result
        .entries
        .iter()
        .find(|entry| {
            matches!(&entry.switch, OwnedSwitch::Short(found) | OwnedSwitch::Both(found, _) if *found == short)
        })
        .and_then(|entry| entry.param.as_ref())
}

fn choice_values(param: &OwnedParam) -> Vec<&str> {
    param
        .choices
        .iter()
        .map(|choice| choice.value.as_str())
        .collect()
}

#[test]
fn mdoc_cm_run_after_a_flag_is_its_choice_list() {
    let mdoc = ".Sh NAME\n.Nm tool\n.Nd demo\n.Sh DESCRIPTION\n.Bl -tag -width Ds\n.It Fl t Cm \
                rsa | ed25519 | ecdsa\nKey type.\n.It Fl M Cm generate\nGenerate.\n.It Fl M Cm \
                screen\nScreen.\n.It Fl q\nQuiet.\n.El\n";
    let result = parse_manpage_string(mdoc);
    let key_type = param_of(&result, 't').expect("-t takes a value");
    assert_eq!(key_type.kind, ParamKind::Mandatory);
    assert_eq!(choice_values(key_type), ["rsa", "ed25519", "ecdsa"]);
    let module = param_of(&result, 'M').expect("-M takes a value");
    assert_eq!(choice_values(module), ["generate", "screen"]);
    assert_eq!(
        result
            .entries
            .iter()
            .filter(|entry| entry.switch == OwnedSwitch::Short('M'))
            .count(),
        1
    );
    assert_eq!(takes_value(&result, 'q'), Some(false));
}

#[test]
fn mdoc_bare_xo_item_keeps_its_flag_and_parameter() {
    let mdoc = ".Sh NAME\n.Nm tool\n.Nd demo\n.Sh DESCRIPTION\n.Bl -tag -width Ds\n.It Xo\n.Fl W \
                Ar host : port\n.Xc\nForward stdio.\n.It Fl q\nQuiet.\n.El\n";
    let result = parse_manpage_string(mdoc);
    let forward = result
        .entries
        .iter()
        .find(|entry| entry.switch == OwnedSwitch::Short('W'))
        .expect("-W kept");
    assert_eq!(
        forward.param.as_ref().map(|param| param.name.as_str()),
        Some("host")
    );
    assert_eq!(forward.desc, "Forward stdio.");
    assert_eq!(takes_value(&result, 'q'), Some(false));
}

#[test]
fn mdoc_unterminated_xo_does_not_swallow_later_options() {
    let mdoc = ".Sh NAME\n.Nm tool\n.Nd demo\n.Sh DESCRIPTION\n.Bl -tag -width Ds\n.It Fl L \
                Xo\n.Ar port\nForward a port.\n.It Fl q\nQuiet.\n.It Fl p Ar port\nPort.\n.El\n";
    let result = parse_manpage_string(mdoc);
    assert_eq!(takes_value(&result, 'L'), Some(true));
    assert_eq!(takes_value(&result, 'q'), Some(false));
    assert_eq!(takes_value(&result, 'p'), Some(true));
}

#[test]
fn mdoc_keyword_glued_to_a_value_is_not_a_choice_list() {
    let mdoc = ".Sh NAME\n.Nm tool\n.Nd demo\n.Sh DESCRIPTION\n.Bl -tag -width Ds\n.It Fl I Cm os \
                Ns = Ns Ar name\nSet the os.\n.It Fl d Cm der | pem ,\nFormat.\n.It Fl \
                q\nQuiet.\n.El\n";
    let result = parse_manpage_string(mdoc);
    let os = param_of(&result, 'I').expect("-I takes a value");
    assert_eq!(os.name, "name");
    assert!(os.choices.is_empty(), "choices: {:?}", os.choices);
    let format = param_of(&result, 'd').expect("-d takes a value");
    assert_eq!(choice_values(format), ["der", "pem"]);
}

#[test]
fn prose_line_opening_with_a_flag_is_not_an_option() {
    let groff = ".TH APP 1\n.SH NAME\napp \\- demo\n.SH OPTIONS\n.SS \\fB--all\\fP\nAdd \
                 everything from the list. Combining\n\\fI--all\\fP with any of the other options \
                 described below is NOT recommended.\n.SS \\fB--cgroups\\fP=\\fImode\\fP\nSet the \
                 mode.\nContainers started with\n\\fB--cgroups=disabled\\fP cannot be frozen and \
                 so do not get this guarantee.\n";
    let result = parse_manpage_string(groff);
    for entry in &result.entries {
        assert!(
            entry
                .param
                .as_ref()
                .is_none_or(|param| !matches!(param.name.as_str(), "with" | "disabled")),
            "prose became a value: {entry:?}"
        );
    }
}

#[test]
fn single_space_option_row_after_a_finished_line_is_kept() {
    let groff = ".TH APP 1\n.SH NAME\napp \\- demo\n.SH OPTIONS\nThe options are:\n-a list all \
                 entries in dir\n-q quiet output, no listing at all\n";
    let result = parse_manpage_string(groff);
    for short in ['a', 'q'] {
        assert!(
            result
                .entries
                .iter()
                .any(|entry| matches!(entry.switch, OwnedSwitch::Short(found) if found == short)),
            "-{short} dropped: {:?}",
            result.entries
        );
    }
}
