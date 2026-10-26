//! Tests ported from ../`inshellah/test/test_inshellah.ml`.
//!
//! Covers the help parser, manpage parser, groff stripping, and nushell
//! generation. The single .nu store parser test (`test_nu_file_parsing`) is
//! not included — it requires porting store.ml first.
#![cfg(test)]

// both parsers now produce the owned model; these aliases keep the ported
// assertions (which match on `Switch::`/`Param::` variants) unchanged.
use inshellah::{
    parsers::{
        help::help_parser,
        manpage::{
            ManpageResult,
            OwnedSwitch as Switch,
            OwnedSwitch,
            ParamKind,
            groff::strip_groff_escapes,
            parse_manpage_string,
            sections::extract_synopsis_command,
        },
        nushell::{
            generate_extern,
            generate_module,
            parse_nu_completions,
        },
    },
    store::{
        json_of_result,
        result_from_json,
    },
};

fn parse(txt: &str) -> ManpageResult {
    match help_parser(txt) {
        Ok((_, result)) => result,
        Err(err) => panic!("parse_help failed: {err:?}"),
    }
}

// --- Help parser tests ---

#[test]
fn gnu_basic() {
    let result = parse("  -a, --all                  do not ignore entries starting with .\n");
    assert_eq!(result.entries.len(), 1);
    let entry = &result.entries[0];
    assert!(matches!(&entry.switch, Switch::Both('a', long) if *long == "all"));
    assert!(entry.param.is_none());
    assert!(!entry.desc.is_empty());
}

#[test]
fn gnu_eq_param() {
    let result = parse("      --block-size=SIZE      scale sizes by SIZE\n");
    assert_eq!(result.entries.len(), 1);
    let entry = &result.entries[0];
    assert!(matches!(&entry.switch, Switch::Long(long) if *long == "block-size"));
    assert!(
        matches!(&entry.param, Some(param) if param.kind == ParamKind::Mandatory && param.name == "SIZE")
    );
}

#[test]
fn gnu_opt_param() {
    let result = parse("      --color[=WHEN]         color the output WHEN\n");
    assert_eq!(result.entries.len(), 1);
    let entry = &result.entries[0];
    assert!(matches!(&entry.switch, Switch::Long(long) if *long == "color"));
    assert!(
        matches!(&entry.param, Some(param) if param.kind == ParamKind::Optional && param.name == "WHEN")
    );
}

#[test]
fn underscore_param() {
    let result = parse("      --time-style=TIME_STYLE  time/date format\n");
    assert_eq!(result.entries.len(), 1);
    let entry = &result.entries[0];
    assert!(
        matches!(&entry.param, Some(param) if param.kind == ParamKind::Mandatory && param.name == "TIME_STYLE")
    );
}

#[test]
fn short_only() {
    let result = parse("  -v                       verbose output\n");
    assert_eq!(result.entries.len(), 1);
    assert!(matches!(result.entries[0].switch, Switch::Short('v')));
}

#[test]
fn long_only() {
    let result = parse("      --help                 display help\n");
    assert_eq!(result.entries.len(), 1);
    assert!(matches!(&result.entries[0].switch, Switch::Long(long) if *long == "help"));
}

#[test]
fn multiline_desc() {
    let txt = "      --block-size=SIZE      with -l, scale sizes by SIZE when printing them;\n                               e.g., '--block-size=M'; see SIZE format below\n";
    let result = parse(txt);
    assert_eq!(result.entries.len(), 1);
    let combined: String = result.entries[0].desc.clone();
    assert!(combined.len() > 50, "desc was: {combined}");
}

#[test]
fn multiple_entries() {
    let txt = "  -a, --all                  do not ignore entries starting with .\n  -A, \
               --almost-all           do not list implied . and ..\n      --author               \
               with -l, print the author of each file\n";
    let result = parse(txt);
    assert_eq!(result.entries.len(), 3);
}

#[test]
fn clap_short_sections() {
    let txt = "INPUT OPTIONS:\n  -e, --regexp=PATTERN       A pattern to search for.\n  -f, \
               --file=PATTERNFILE     Search for patterns from the given file.\nSEARCH OPTIONS:\n  \
               -s, --case-sensitive       Search case sensitively.\n";
    let result = parse(txt);
    assert_eq!(result.entries.len(), 3);
    let entry = &result.entries[0];
    assert!(matches!(&entry.switch, Switch::Both('e', long) if *long == "regexp"));
    assert!(
        matches!(&entry.param, Some(param) if param.kind == ParamKind::Mandatory && param.name == "PATTERN")
    );
}

#[test]
fn clap_long_style() {
    let txt = "  -H, --hidden\n          Include hidden directories and files.\n\n      \
               --no-ignore\n          Do not respect ignore files.\n";
    let result = parse(txt);
    assert_eq!(result.entries.len(), 2);
    let entry = &result.entries[0];
    assert!(matches!(&entry.switch, Switch::Both('H', long) if *long == "hidden"));
    assert!(!entry.desc.is_empty());
}

#[test]
fn clap_long_angle_param() {
    let txt = "      --nonprintable-notation <notation>\n          Set notation for non-printable \
               characters.\n";
    let result = parse(txt);
    assert_eq!(result.entries.len(), 1);
    let entry = &result.entries[0];
    assert!(matches!(&entry.switch, Switch::Long(long) if *long == "nonprintable-notation"));
    assert!(
        matches!(&entry.param, Some(param) if param.kind == ParamKind::Mandatory && param.name == "notation")
    );
}

#[test]
fn space_upper_param() {
    let result = parse("  -f, --foo FOO  foo help\n");
    assert_eq!(result.entries.len(), 1);
    let entry = &result.entries[0];
    assert!(matches!(&entry.switch, Switch::Both('f', long) if *long == "foo"));
    assert!(
        matches!(&entry.param, Some(param) if param.kind == ParamKind::Mandatory && param.name == "FOO")
    );
}

#[test]
fn go_cobra_flags() {
    let txt = "Flags:\n  -D, --debug              Enable debug mode\n  -H, --host string        \
               Daemon socket to connect to\n  -v, --version            Print version information\n";
    let result = parse(txt);
    assert_eq!(result.entries.len(), 3);
    let host = &result.entries[1];
    assert!(matches!(&host.switch, Switch::Both('H', long) if *long == "host"));
    assert!(
        matches!(&host.param, Some(param) if param.kind == ParamKind::Mandatory && param.name == "string")
    );
}

#[test]
fn go_cobra_subcommands() {
    let txt = "Common Commands:\n  run         Create and run a new container from an image\n  \
               exec        Execute a command in a running container\n  build       Build an image \
               from a Dockerfile\n";
    let result = parse(txt);
    assert!(
        !result.subcommands.is_empty(),
        "expected subcommands, got: {:?}",
        result.subcommands.len()
    );
}

#[test]
fn aliased_subcommands_keep_canonical_name() {
    // cargo-style `name, alias` rows: the comma must not abort the entry.
    // the canonical (first) name is kept; the alias is discarded.
    let txt = "Commands:\n  build, b    Compile the current package\n  check, c    Analyze the \
               current package\n  clean       Remove the target directory\n";
    let result = parse(txt);
    let names: Vec<&str> = result
        .subcommands
        .iter()
        .map(|sc| sc.name.as_str())
        .collect();
    assert!(names.contains(&"build"), "subcommands: {names:?}");
    assert!(names.contains(&"check"), "subcommands: {names:?}");
    assert!(names.contains(&"clean"), "subcommands: {names:?}");
    assert!(
        !names.contains(&"b"),
        "alias leaked as subcommand: {names:?}"
    );
    let build = result
        .subcommands
        .iter()
        .find(|sc| sc.name == "build")
        .unwrap();
    assert_eq!(build.desc, "Compile the current package");
}

#[test]
fn help_parser_ignores_value_enums_and_defaults() {
    let txt = "Usage: tar [OPTION...] [FILE]...

 Main operation mode:
  -c, --create               create a new archive

 Archive format selection:

  -H, --format=FORMAT        create archive of the given format

 FORMAT is one of the following:
    gnu                      GNU tar 1.13.x format
    oldgnu                   GNU format as per tar <= 1.12
    pax                      POSIX 1003.1-2001 (pax) format
    posix                    same as pax
    ustar                    POSIX 1003.1-1988 (ustar) format
    v7                       old V7 tar format

*This* tar defaults to:
--format=gnu -f- -b20 --quoting-style=escape
--rmt-command=/nix/store/example/libexec/rmt
";
    let result = parse(txt);
    assert!(
        result.subcommands.is_empty(),
        "enum values became subcommands: {:?}",
        result.subcommands.len()
    );
    assert!(
        !result
            .entries
            .iter()
            .any(|entry| matches!(&entry.switch, Switch::Long(long) if *long == "rmt-command")),
        "default lines should not become flags"
    );
    assert!(
        result
            .entries
            .iter()
            .any(|entry| matches!(&entry.switch, Switch::Both('H', long) if *long == "format")),
        "real option should still be parsed"
    );
}

#[test]
fn busybox_tab() {
    let result = parse("\t-1\tOne column output\n\t-a\tInclude names starting with .\n");
    assert_eq!(result.entries.len(), 2);
    assert!(matches!(result.entries[0].switch, Switch::Short('1')));
}

#[test]
fn no_debug_prints() {
    // the old ocaml parser had print_endline at module load time; this test
    // documents that no such side effects exist in the rust port.
    parse("  -v  verbose\n");
}

#[test]
fn slash_switch_separator() {
    let result = parse("  --verbose / -v             Increase verbosity\n");
    assert_eq!(result.entries.len(), 1);
    let entry = &result.entries[0];
    assert!(matches!(&entry.switch, Switch::Both('v', long) if *long == "verbose"));
    assert!(entry.param.is_none());
    let combined = entry.desc.clone();
    assert_eq!(combined.trim(), "Increase verbosity");
}

// --- Manpage parser tests ---

#[test]
fn manpage_tp_style() {
    let groff = r".SH OPTIONS
.TP
\fB\-a\fR, \fB\-\-all\fR
do not ignore entries starting with .
.TP
\fB\-A\fR, \fB\-\-almost\-all\fR
do not list implied . and ..
.TP
\fB\-\-block\-size\fR=\fISIZE\fR
with \fB\-l\fR, scale sizes by SIZE
.SH AUTHOR
Written by someone.
";
    let result = parse_manpage_string(groff);
    assert_eq!(result.entries.len(), 3, "entries: {:?}", result.entries);
    assert!(matches!(&result.entries[0].switch, OwnedSwitch::Both('a', long) if long == "all"));
    assert!(!result.entries[0].desc.is_empty());
    assert!(matches!(&result.entries[2].switch, OwnedSwitch::Long(long) if long == "block-size"));
    assert!(
        matches!(&result.entries[2].param, Some(param) if param.kind == ParamKind::Mandatory && param.name == "SIZE")
    );
}

#[test]
fn manpage_ip_style() {
    let groff = r#".SH OPTIONS
.IP "\fB\-k\fR, \fB\-\-insecure\fR"
Allow insecure connections.
.IP "\fB\-o\fR, \fB\-\-output\fR \fIfile\fR"
Write output to file.
.SH SEE ALSO
"#;
    let result = parse_manpage_string(groff);
    assert_eq!(result.entries.len(), 2, "entries: {:?}", result.entries);
    assert!(
        matches!(&result.entries[0].switch, OwnedSwitch::Both('k', long) if long == "insecure")
    );
}

#[test]
fn manpage_groff_stripping() {
    let stripped = strip_groff_escapes(r"\fB\-\-color\fR[=\fIWHEN\fR]");
    // font escapes removed
    assert!(!(stripped.contains('f') && stripped.contains('B') && stripped.contains('\\')));
    // dashes converted
    assert!(stripped.contains('-'));
    let stripped2 = strip_groff_escapes(r"\(aqhello\(aq");
    assert!(
        stripped2.contains('\''),
        "expected apostrophe in: {stripped2}"
    );
}

#[test]
fn manpage_getent_databases_from_description() {
    let groff = r".SH SYNOPSIS
.SY getent
.RI [ option \~.\|.\|.\&]
.I database
.IR key \~.\|.\|.
.YS
.SH DESCRIPTION
The
.I database
may be any of those supported by the GNU C Library, listed below:
.TP
.B passwd
When no
.I key
is provided, enumerate the passwd database.
.TP
.B services
When no
.I key
is provided, enumerate the services database.
.SH OPTIONS
.TP
.BI \-\-service\~ service
.TQ
.BI \-s\~ service
Override all databases with the specified service.
.TP
.BI \-\-service\~ database : service
.TQ
.BI \-s\~ database : service
Override only specified databases with the specified service.
.TP
.B \-\-usage
Print a short usage summary and exit.
";
    let result = parse_manpage_string(groff);
    let positional_names: Vec<&str> = result
        .positionals
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(positional_names, vec!["database", "key"]);

    let service = result
        .entries
        .iter()
        .find(|entry| matches!(&entry.switch, OwnedSwitch::Both('s', name) if name == "service"))
        .expect("expected --service(-s)");
    assert!(matches!(
        &service.param,
        Some(param) if param.kind == ParamKind::Mandatory && param.name == "service"
    ));
    assert!(
        !result.entries
            .iter()
            .any(|entry| matches!(&entry.switch, OwnedSwitch::Long(name) if name == "serviceservice" || name == "servicedatabase")),
        "entries: {:?}",
        result.entries
    );

    // database names are prose-mined positional value choices, not real
    // subcommands — they live in their own channel (Phase 4) so they never
    // flow into the real-child paths (recursion, supplement, extern stubs).
    assert!(
        result.subcommands.is_empty(),
        "database choices must not be filed as subcommands: {:?}",
        result.subcommands
    );
    let choices: Vec<&str> = result
        .positional_choices
        .iter()
        .map(|sc| sc.name.as_str())
        .collect();
    assert!(choices.contains(&"passwd"), "choices: {choices:?}");
    assert!(choices.contains(&"services"), "choices: {choices:?}");

    let nu = generate_extern("getent", &result);
    assert!(nu.contains("database: glob"), "nu = {nu}");
    assert!(nu.contains("...key: glob"), "nu = {nu}");
    assert!(nu.contains("--service(-s): string"), "nu = {nu}");
    assert!(!nu.contains("--servicedatabase"), "nu = {nu}");
    // positional choices are NOT real subcommands, so no `getent passwd`
    // extern stub is generated.
    assert!(!nu.contains("export extern \"getent passwd\""), "nu = {nu}");
}

#[test]
fn manpage_b_macro_option_tag_with_embedded_quotes() {
    let groff = r#".SH OPTIONS
.TP
.B "\-s ""\fIprogram\fR [\fIargument \fR...]\fB""\fR, \fB\-\-speller=""\fIprogram\fR [\fIargument \fR...]\fB"""
Use this command to perform spell checking and correcting.
"#;
    let result = parse_manpage_string(groff);
    assert!(
        result
            .entries
            .iter()
            .any(|entry| matches!(entry.switch, OwnedSwitch::Short('s'))),
        "entries: {:?}",
        result.entries
    );
}

#[test]
fn manpage_synopsis_b_macro_bracket_args_keep_spaces() {
    let groff = r#".SH "SYNOPSIS"
.B "rtmon"
.RI "[ " OPTIONS " ] "
.BI "file " FILE
.BR "[ " all
.RI "| " OBJECTS
.RB "]"
.ti -8
.I OBJECTS
.B ":= [" link "]" "[" address "]" "[" route "]"
.SH OPTIONS
"#;
    let result = parse_manpage_string(groff);
    let positional_names: Vec<&str> = result
        .positionals
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        !positional_names.contains(&"ptions")
            && positional_names.contains(&"link")
            && positional_names.contains(&"address"),
        "positionals: {positional_names:?}"
    );
}

#[test]
fn bracketed_angle_positionals_keep_inner_ellipsis() {
    let groff = r#".SH SYNOPSIS
.B bzip2
.RB [ " \-cdfkqstvzVL123456789 " ]
[
.I "filenames \&..."
]
.SH OPTIONS
"#;
    let result = parse_manpage_string(groff);
    assert!(
        result
            .positionals
            .iter()
            .any(|(name, positional)| name == "filenames" && positional.variadic),
        "positionals: {:?}",
        result.positionals
    );
}

#[test]
fn nested_optional_positionals_keep_last_valid_inner_name() {
    let groff = r".SH SYNOPSIS
\fBfc-cat\fR [ \fB-rvVh\fR ]
 [ \fB [ \fIfonts-cache-%version%-files\fB ]  [ \fIdirs\fB ] \fR\fI...\fR ]
.SH OPTIONS
";
    let result = parse_manpage_string(groff);
    assert!(
        result
            .positionals
            .iter()
            .any(|(name, positional)| name == "dirs" && positional.optional && positional.variadic),
        "positionals: {:?}",
        result.positionals
    );
}

#[test]
fn manpage_empty_options() {
    let groff = ".SH NAME\nfoo \\- does stuff\n.SH DESCRIPTION\nDoes stuff.\n";
    let result = parse_manpage_string(groff);
    assert_eq!(result.entries.len(), 0);
}

#[test]
fn manpage_nix3_style() {
    let groff = r#".SH Options
.SS Logging-related options
.IP "\(bu" 3
.UR #opt-verbose
\f(CR--verbose\fR
.UE
/ \f(CR-v\fR
.IP
Increase the logging verbosity level.
.IP "\(bu" 3
.UR #opt-quiet
\f(CR--quiet\fR
.UE
.IP
Decrease the logging verbosity level.
.SH SEE ALSO
"#;
    let result = parse_manpage_string(groff);
    assert_eq!(result.entries.len(), 2, "entries: {:?}", result.entries);
    assert!(matches!(&result.entries[0].switch, OwnedSwitch::Both('v', long) if long == "verbose"));
    assert!(!result.entries[0].desc.is_empty());
    assert!(matches!(&result.entries[1].switch, OwnedSwitch::Long(long) if long == "quiet"));
    assert!(!result.entries[1].desc.is_empty());
}

#[test]
fn manpage_nix3_with_params() {
    let groff = r#".SH Options
.IP "\(bu" 3
.UR #opt-arg
\f(CR--arg\fR
.UE
\fIname\fR \fIexpr\fR
.IP
Pass the value as the argument name to Nix functions.
.IP "\(bu" 3
.UR #opt-include
\f(CR--include\fR
.UE
/ \f(CR-I\fR \fIpath\fR
.IP
Add path to search path entries.
.IP
This option may be given multiple times.
.SH SEE ALSO
"#;
    let result = parse_manpage_string(groff);
    assert_eq!(result.entries.len(), 2, "entries: {:?}", result.entries);
    assert!(matches!(&result.entries[0].switch, OwnedSwitch::Long(long) if long == "arg"));
    assert!(result.entries[0].param.is_some());
    assert!(matches!(&result.entries[1].switch, OwnedSwitch::Both('I', long) if long == "include"));
    assert!(
        matches!(&result.entries[1].param, Some(param) if param.kind == ParamKind::Mandatory && param.name == "path")
    );
}

#[test]
fn synopsis_subcommand() {
    let groff = r#".SH "SYNOPSIS"
.sp
.nf
\fBgit\fR \fBcommit\fR [\fB\-a\fR | \fB\-\-interactive\fR]
.fi
.SH "DESCRIPTION"
"#;
    let cmd = extract_synopsis_command(groff);
    assert_eq!(cmd.as_deref(), Some("git commit"));
}

#[test]
fn synopsis_standalone() {
    let groff = ".SH Synopsis\n.LP\n\\f(CRnix-build\\fR [\\fIpaths\\fR]\n.SH Description\n";
    let cmd = extract_synopsis_command(groff);
    assert_eq!(cmd.as_deref(), Some("nix-build"));
}

#[test]
fn synopsis_nix3() {
    let groff = ".SH Synopsis\n.LP\n\\f(CRnix run\\fR [\\fIoption\\fR] \\fIinstallable\\fR\n.SH \
                 Description\n";
    let cmd = extract_synopsis_command(groff);
    assert_eq!(cmd.as_deref(), Some("nix run"));
}

#[test]
fn italic_synopsis() {
    let groff = ".SH Synopsis\n.LP\n\\f(CRnix-env\\fR \\fIoperation\\fR [\\fIoptions\\fR] \
                 [\\fIarguments\u{2026}\\fR]\n.SH Description\n";
    let cmd = extract_synopsis_command(groff);
    assert_eq!(cmd.as_deref(), Some("nix-env"));
}

#[test]
fn synopsis_italic_command_name() {
    // git-am.1 (and many other git manpages) put the entire command
    // invocation in italics: `\fIgit am\fR [...]`. should still resolve
    // to "git am" rather than treating it as a placeholder.
    let groff = ".SH \"SYNOPSIS\"\n.sp\n.nf\n\\fIgit am\\fR [\\-\\-signoff] \
                 [\\-\\-keep]\n.fi\n.SH \"DESCRIPTION\"\n";
    let cmd = extract_synopsis_command(groff);
    assert_eq!(cmd.as_deref(), Some("git am"));
}

#[test]
fn synopsis_skips_prose_before_invocation() {
    let groff = r".SH SYNOPSIS
The dynamic linker can be run either indirectly by running some
dynamically linked program or shared object
(in which case no command-line options
to the dynamic linker can be passed and, in the ELF case, the dynamic linker
which is stored in the
.B .interp
section of the program is executed) or directly by running:
.P
.I /lib/ld\-linux.so.*
[OPTIONS] [PROGRAM [ARGUMENTS]]
.SH DESCRIPTION
";
    let cmd = extract_synopsis_command(groff);
    assert_eq!(cmd.as_deref(), None);
}

#[test]
fn synopsis_skips_labels_before_invocation() {
    let groff = r#".SH "SYNOPSIS"
.sp
Set up a loop device:
.sp
\fBlosetup\fP [options] \fB\-f\fP|\fIloopdev file\fP
.sp
Get info:
.RS 4
\fBlosetup\fP \fIloopdev\fP
.RE
.SH "DESCRIPTION"
"#;
    let cmd = extract_synopsis_command(groff);
    assert_eq!(cmd.as_deref(), Some("losetup"));
}

#[test]
fn synopsis_b_macro_preserves_command_spaces() {
    let groff = r#".SH "SYNOPSIS"
.sp
.B ip link
.RI " { " COMMAND " | "
.BR help " }"
.SH "DESCRIPTION"
"#;
    let cmd = extract_synopsis_command(groff);
    assert_eq!(cmd.as_deref(), Some("ip link"));
}

#[test]
fn synopsis_br_macro_preserves_quoted_command_spaces() {
    let groff = r#".SH "SYNOPSIS"
.sp
.BR "ip monitor" " [ " all " |"
.IR OBJECT-LIST " ]"
.SH "DESCRIPTION"
"#;
    let cmd = extract_synopsis_command(groff);
    assert_eq!(cmd.as_deref(), Some("ip monitor"));
}

#[test]
fn synopsis_long_b_macro_is_not_prose() {
    let groff = ".SH SYNOPSIS
.ad l
.in +8
.ti -8
.B tipc peer remove address
.IR ADDRESS
.SH OPTIONS
";
    let cmd = extract_synopsis_command(groff);
    assert_eq!(cmd.as_deref(), Some("tipc peer remove address"));
}

#[test]
fn synopsis_ss_heading_is_accepted() {
    let groff = r".SH Name
.LP
\f(CRnix-env --set\fR - set profile to contain a specified derivation
.SS
Synopsis
.LP
\f(CRnix-env\fR \f(CR--set\fR \fIdrvname\fR
.SS
Description
";
    let cmd = extract_synopsis_command(groff);
    assert_eq!(cmd.as_deref(), Some("nix-env"));
}

// --- Font/dedup tests (only the font-spacing one is portable) ---

#[test]
fn font_boundary_spacing() {
    // \fB--max-results\fR\fIcount\fR should become "--max-results count"
    let stripped = strip_groff_escapes(r"\fB\-\-max\-results\fR\fIcount\fR");
    assert!(stripped.contains("--max-results count"), "got: {stripped}");
    // \fB--color\fR[=\fIWHEN\fR] should NOT insert space before =
    let stripped2 = strip_groff_escapes(r"\fB\-\-color\fR[=\fIWHEN\fR]");
    assert!(stripped2.contains("--color[=WHEN]"), "got: {stripped2}");
}

// --- COMMANDS section tests ---

#[test]
fn commands_section_subcommands() {
    let groff = r".SH OPTIONS
.TP
\fB\-\-user\fR
Talk to the service manager of the calling user.
.TP
\fB\-\-system\fR
Talk to the service manager of the system.
.SH COMMANDS
.PP
\fBstart\fR \fIUNIT\fR\&...
.RS 4
Start (activate) one or more units.
.RE
.PP
\fBstop\fR \fIUNIT\fR\&...
.RS 4
Stop (deactivate) one or more units.
.RE
.PP
\fBreload\fR \fIUNIT\fR\&...
.RS 4
Asks all units to reload their configuration.
.RE
.SH SEE ALSO
";
    let result = parse_manpage_string(groff);
    assert_eq!(
        result.entries.len(),
        2,
        "options entries: {:?}",
        result.entries
    );
    assert_eq!(
        result.subcommands.len(),
        3,
        "subcommands: {:?}",
        result.subcommands
    );
    let names: Vec<&str> = result
        .subcommands
        .iter()
        .map(|sc| sc.name.as_str())
        .collect();
    assert!(names.contains(&"start"));
    assert!(names.contains(&"stop"));
    assert!(names.contains(&"reload"));
    let start_sc = result
        .subcommands
        .iter()
        .find(|sc| sc.name == "start")
        .unwrap();
    assert!(!start_sc.desc.is_empty());
}

#[test]
fn commands_section_git_style_refs() {
    let groff = r#".SH OPTIONS
.TP
\fB\-\-version\fR
Show version.
.SH "GIT COMMANDS"
.SS "Main porcelain commands"
.PP
.BR git-add (1)
.RS 4
Add file contents to the index.
.RE
.PP
\fBgit-commit\fR(1)
.RS 4
Record changes to the repository.
.RE
"#;
    let result = parse_manpage_string(groff);
    let names: Vec<&str> = result
        .subcommands
        .iter()
        .map(|sc| sc.name.as_str())
        .collect();
    assert!(
        names.contains(&"git-add"),
        "subcommands: {:?}",
        result.subcommands
    );
    assert!(
        names.contains(&"git-commit"),
        "subcommands: {:?}",
        result.subcommands
    );
    let add = result
        .subcommands
        .iter()
        .find(|sc| sc.name == "git-add")
        .unwrap();
    assert!(add.desc.contains("Add file contents"));
}

// --- Nushell generation tests ---

#[test]
fn nushell_basic() {
    let result = parse("  -a, --all                  do not ignore entries starting with .\n");
    let nu = generate_extern("ls", &result);
    assert!(nu.contains("export extern \"ls\""), "nu = {nu}");
    assert!(nu.contains("--all(-a)"), "nu = {nu}");
    assert!(nu.contains("# do not ignore"), "nu = {nu}");
}

#[test]
fn nushell_param_types() {
    let txt = "  -w, --width=COLS           set output width\n      --block-size=SIZE      scale \
               sizes\n  -o, --output FILE          output file\n";
    let result = parse(txt);
    let nu = generate_extern("ls", &result);
    assert!(nu.contains("--width(-w): int"), "nu = {nu}");
    assert!(nu.contains("--block-size: string"), "nu = {nu}");
    assert!(nu.contains("--output(-o): path"), "nu = {nu}");
}

#[test]
fn nushell_subcommands() {
    let txt = "Common Commands:\n  run         Create and run a new container\n  exec        \
               Execute a command\n\nFlags:\n  -D, --debug              Enable debug mode\n";
    let result = parse(txt);
    let nu = generate_extern("docker", &result);
    assert!(nu.contains("export extern \"docker\""), "nu = {nu}");
    assert!(nu.contains("--debug(-D)"), "nu = {nu}");
    assert!(nu.contains("export extern \"docker run\""), "nu = {nu}");
    assert!(nu.contains("export extern \"docker exec\""), "nu = {nu}");
}

#[test]
fn positional_order_survives_cache_and_generation() {
    let txt = "usage: git clone [<options>] [--] <repository> [directory]\n";
    let result = parse(txt);
    assert_eq!(
        result
            .positionals
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["repository", "directory"]
    );

    let json = json_of_result("help", &result);
    let value = serde_json::from_str(&json).expect("cache json");
    let cached = result_from_json(&value);
    assert_eq!(
        cached
            .positionals
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["repository", "directory"]
    );

    let nu = generate_extern("git clone", &cached);
    let repository = nu.find("repository: glob").expect("repository positional");
    let directory = nu.find("directory?: path").expect("directory positional");
    assert!(repository < directory, "nu = {nu}");
}

#[test]
fn nushell_from_manpage() {
    let groff = r".SH OPTIONS
.TP
\fB\-a\fR, \fB\-\-all\fR
do not ignore entries starting with .
.TP
\fB\-\-block\-size\fR=\fISIZE\fR
scale sizes by SIZE
.SH AUTHOR
";
    let result = parse_manpage_string(groff);
    let nu = generate_extern("ls", &result);
    assert!(nu.contains("export extern \"ls\""), "nu = {nu}");
    assert!(nu.contains("--all(-a)"), "nu = {nu}");
    assert!(nu.contains("--block-size: string"), "nu = {nu}");
}

#[test]
fn nushell_module() {
    let result = parse("  -v, --verbose              verbose output\n");
    let nu = generate_module("myapp", &result);
    assert!(nu.contains("module myapp-completions"), "nu = {nu}");
    assert!(nu.contains("export extern \"myapp\""), "nu = {nu}");
    assert!(nu.contains("--verbose(-v)"), "nu = {nu}");
}

#[test]
fn long_switch_rejects_dash_only_separator_lines() {
    // less's `--help` (and therefore zstdless/bzless wrappers) is full of
    // separator lines like
    // `---------------------------------------------------`. the first two
    // chars look like a `--` prefix; the rest is just dashes. without an
    // alphanumeric-first guard, the parser would treat the whole separator as a
    // flag named `------…`.
    let result = parse(
        "  ---------------------------------------------------\n  -a, --all                  show \
         all\n",
    );
    assert_eq!(
        result.entries.len(),
        1,
        "separator should not parse as a flag, got {} entries",
        result.entries.len()
    );
    assert!(matches!(&result.entries[0].switch, Switch::Both('a', long) if *long == "all"));
}

#[test]
fn dedup_entries_help() {
    let txt = "  -v, --verbose              verbose output\n  --verbose                  verbose \
               mode\n  -v                         be verbose\n";
    let result = parse(txt);
    let nu = generate_extern("test", &result);
    let count = nu.matches("--verbose").count();
    assert_eq!(count, 1, "expected --verbose to appear once, nu = {nu}");
    assert!(nu.contains("--verbose(-v)"), "nu = {nu}");
}

#[test]
fn dedup_manpage_entries() {
    let groff = r".SH OPTIONS
.TP
\fB\-v\fR, \fB\-\-verbose\fR
Be verbose.
.SH DESCRIPTION
Use \fB\-v\fR for verbose output.
Use \fB\-\-verbose\fR to see more.
";
    let result = parse_manpage_string(groff);
    let nu = generate_extern("test", &result);
    assert!(nu.contains("--verbose(-v)"), "nu = {nu}");
    let verbose_lines: Vec<&str> = nu.lines().filter(|line| line.contains("verbose")).collect();
    assert_eq!(
        verbose_lines.len(),
        1,
        "expected 1 verbose line, got: {verbose_lines:?}"
    );
}

#[test]
fn nu_file_parsing() {
    let nu_source = r#"module completions {

  # Unofficial CLI tool
  export extern mytool [
    --help(-h)                # Print help
    --version(-V)             # Print version
  ]

  # List all items
  export extern "mytool list" [
    --raw                     # Output as JSON
    --format(-f): string      # Output format
    --help(-h)                # Print help
    name?: string             # Filter by name
  ]

}

use completions *
"#;
    let result = parse_nu_completions("mytool", nu_source);
    assert_eq!(result.entries.len(), 2, "entries: {:?}", result.entries);
    assert!(
        !result.subcommands.is_empty(),
        "subcommands: {:?}",
        result.subcommands
    );
    assert!(result.subcommands.iter().any(|sc| sc.name == "list"));
    assert_eq!(result.description, "Unofficial CLI tool");

    let r2 = parse_nu_completions("mytool list", nu_source);
    assert_eq!(r2.entries.len(), 3, "list entries: {:?}", r2.entries);
    let has_format = r2
        .entries
        .iter()
        .any(|entry| matches!(&entry.switch, OwnedSwitch::Both('f', long) if long == "format"));
    assert!(
        has_format,
        "list should have --format(-f): {:?}",
        r2.entries
    );
    assert!(!r2.positionals.is_empty(), "list should have a positional");
}

#[test]
fn self_listing_detection() {
    let txt = "systemctl [OPTIONS...] COMMAND ...

Unit Commands:
  start UNIT...                       Start (activate) one or more units
  stop UNIT...                        Stop (deactivate) one or more units
  status [PATTERN...]                 Show runtime status

Options:
  --user                              Talk to the user service manager
  --system                            Talk to the system service manager
";
    let result = parse(txt);
    let has_start = result.subcommands.iter().any(|sc| sc.name == "start");
    assert!(
        has_start,
        "expected start in subcommands: {:?}",
        result
            .subcommands
            .iter()
            .map(|sc| sc.name.as_str())
            .collect::<Vec<_>>()
    );
    assert!(result.entries.len() >= 2);
}
