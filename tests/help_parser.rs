// SPDX-License-Identifier: EUPL-1.2
//! help parser cases captured from real cobra, pflag, docker and git output.
#![cfg(test)]

use inshellah::parsers::{
    help::help_parser,
    manpage::{
        ManpageEntry,
        ManpageResult,
        OwnedParam,
        OwnedSwitch,
        ParamKind,
        ParamOrigin,
    },
};

fn parse(text: &str) -> ManpageResult {
    help_parser(text).expect("help parse").1
}

fn long_of(entry: &ManpageEntry) -> Option<&str> {
    match &entry.switch {
        OwnedSwitch::Long(long) | OwnedSwitch::Both(_, long) => Some(long),
        OwnedSwitch::Short(_) => None,
    }
}

fn entry<'result>(result: &'result ManpageResult, long: &str) -> &'result ManpageEntry {
    result
        .entries
        .iter()
        .find(|entry| long_of(entry) == Some(long))
        .unwrap_or_else(|| panic!("no entry for --{long} in {:?}", result.entries))
}

fn param_name<'result>(result: &'result ManpageResult, long: &str) -> Option<&'result str> {
    entry(result, long)
        .param
        .as_ref()
        .map(|param| param.name.as_str())
}

const KUBECTL_LOGS: &str = "Options:
    --all-containers=false:
\tGet all containers' logs in the pod(s).

    -c, --container='':
\tPrint the logs of this container

    -f, --follow=false:
\tSpecify if the logs should be streamed.

    --limit-bytes=0:
\tMaximum bytes of logs to return. Defaults to no limit.

    --since=0s:
\tOnly return logs newer than a relative duration like 5s, 2m, or 3h.

    --tail=-1:
\tLines of recent log file to display.

    -o, --output=json:
\tOutput format.

Usage:
  kubectl logs [-f] [-p] (POD | TYPE/NAME) [-c CONTAINER] [options]
";

#[test]
fn cobra_defaults_tell_switches_from_valued_flags() {
    let result = parse(KUBECTL_LOGS);
    for switch in ["all-containers", "follow"] {
        assert!(entry(&result, switch).param.is_none(), "--{switch}");
    }
    for valued in ["container", "limit-bytes", "since", "tail", "output"] {
        assert_eq!(param_name(&result, valued), Some("value"), "--{valued}");
    }
    assert_eq!(
        entry(&result, "follow").desc,
        "Specify if the logs should be streamed."
    );
}

#[test]
fn pflag_type_words_are_value_types() {
    let result = parse(
        "Options:
  -e, --env stringArray                          Set environment variables in container
      --env-host                                 Use all current host environment variables
      --cpus float                               Number of CPUs
  -m, --memory int                               Memory limit
      --stop-timeout duration                    Timeout to stop a container
",
    );
    for (long, name) in [
        ("env", "stringArray"),
        ("cpus", "float"),
        ("memory", "int"),
        ("stop-timeout", "duration"),
    ] {
        let param = entry(&result, long).param.as_ref().expect(long);
        assert_eq!(param.name, name);
        assert!(param.is_exact(), "--{long}");
    }
    assert!(entry(&result, "env-host").param.is_none());
}

#[test]
fn flag_line_ending_in_a_colon_is_not_a_section_header() {
    let result = parse(
        "Options:
  -t, --tty                              Allocate a pseudo-TTY
  -u, --user string                      Username or UID (format:
                                         <name|uid>[:<group|gid>])
      --userns string                    User namespace to use
",
    );
    assert_eq!(param_name(&result, "user"), Some("string"));
    assert_eq!(result.entries.len(), 3);
}

#[test]
fn entry_descriptions_with_header_words_stay_entries() {
    let result = parse(
        "Commands:
  query        print stored completion data
  dump         list indexed commands
  diff         report flag and subcommand gaps
  purge        delete the cache
",
    );
    let names: Vec<&str> = result
        .subcommands
        .iter()
        .map(|sub| sub.name.as_str())
        .collect();
    assert_eq!(names, ["query", "dump", "diff", "purge"]);
}

#[test]
fn git_negatable_long_with_an_option_value_is_kept() {
    let result = parse(
        "    -s, --[no-]strategy <strategy>
                          merge strategy to use
    -X, --[no-]strategy-option <option=value>
                          option for selected merge strategy
    -S, --[no-]gpg-sign[=<key-id>]
                          GPG sign commit
",
    );
    assert_eq!(param_name(&result, "strategy-option"), Some("option=value"));
    assert_eq!(param_name(&result, "strategy"), Some("strategy"));
}

#[test]
fn only_a_lowercase_word_before_prose_is_a_guess() {
    let result = parse(
        "  -n, --name=name       the attribute name
      --max-wait=num    seconds to wait
      --timeout <num>   seconds before giving up
      --chroot path\tchange the root
  -f progfile
      --ignore-command-error ignore exit codes
",
    );
    for long in ["name", "max-wait", "timeout", "chroot"] {
        let param = entry(&result, long).param.as_ref().expect(long);
        assert!(param.is_exact(), "--{long}");
    }
    assert!(
        result
            .entries
            .iter()
            .any(|entry| entry.switch == OwnedSwitch::Short('f') && entry.param.is_some())
    );
    assert_eq!(
        entry(&result, "ignore-command-error")
            .param
            .as_ref()
            .expect("spaced word")
            .origin,
        ParamOrigin::ProseGuess
    );
}

#[test]
fn optional_arguments_header_opens_an_options_section() {
    let result = parse(
        "Optional arguments:
    --input <inputfile>: Reads <inputfile> for options generated by
       qmake.

    --aab: Build an Android App Bundle.
",
    );
    assert_eq!(param_name(&result, "input"), Some("inputfile"));
    assert!(entry(&result, "aab").param.is_none());
}

fn param_of<'result>(result: &'result ManpageResult, long: &str) -> &'result OwnedParam {
    entry(result, long)
        .param
        .as_ref()
        .unwrap_or_else(|| panic!("--{long} takes a value in {:?}", result.entries))
}

#[test]
fn bracketed_equals_values_stay_optional() {
    let result = parse(
        "Options:
  -n --lines[=[+]INTEGER]    Number of journal entries to show
     --diff[=yes|no]         Show differences
  -u --unit=UNIT             Show logs from the specified unit
",
    );
    let lines = param_of(&result, "lines");
    assert_eq!(lines.kind, ParamKind::Optional);
    assert_eq!(lines.name, "[+]INTEGER");
    let diff = param_of(&result, "diff");
    assert_eq!(diff.kind, ParamKind::Optional);
    let choices: Vec<_> = diff
        .choices
        .iter()
        .map(|choice| choice.value.as_str())
        .collect();
    assert_eq!(choices, ["yes", "no"]);
    assert_eq!(param_of(&result, "unit").kind, ParamKind::Mandatory);
}

#[test]
fn bracket_prefixed_equals_value_is_mandatory() {
    let result = parse(
        "Options:
  -H --host=[USER@]HOST      Operate on a remote host
",
    );
    let host = param_of(&result, "host");
    assert_eq!(host.kind, ParamKind::Mandatory);
    assert_eq!(host.name, "[USER@]HOST");
}

#[test]
fn equals_angle_and_ellipsis_values_take_a_value() {
    let result = parse(
        "Options:
      --timeout=<timeout>   Time to wait
  -r, --require=...         Preload module
  -A --debugger-arguments=\u{2026}  Arguments for the debugger
  -q, --quiet               Be quiet
",
    );
    assert_eq!(param_of(&result, "timeout").name, "timeout");
    assert_eq!(param_of(&result, "require").kind, ParamKind::Mandatory);
    assert_eq!(
        param_of(&result, "debugger-arguments").kind,
        ParamKind::Mandatory
    );
    assert!(entry(&result, "quiet").param.is_none());
}

#[test]
fn argparse_long_first_pair_keeps_short_and_value() {
    let result = parse(
        "options:
  -h, --help            show this help message and exit
  --width, -w WIDTH     width of the output
  --verbose, -v         print more
",
    );
    let width = entry(&result, "width");
    assert_eq!(width.switch, OwnedSwitch::Both('w', "width".to_owned()));
    assert_eq!(param_of(&result, "width").name, "WIDTH");
    assert_eq!(
        entry(&result, "verbose").switch,
        OwnedSwitch::Both('v', "verbose".to_owned())
    );
    assert!(entry(&result, "verbose").param.is_none());
}

#[test]
fn single_letter_value_name_before_prose_is_exact() {
    let result = parse(
        "Options:
  -b n or --buffers=n   number of buffers to use
",
    );
    let buffers = result
        .entries
        .iter()
        .find(|entry| matches!(entry.switch, OwnedSwitch::Short('b')))
        .and_then(|entry| entry.param.as_ref())
        .expect("-b takes a value");
    assert!(buffers.is_exact());
}

#[test]
fn iptables_long_then_short_rows_keep_both_and_the_value() {
    let result = parse(
        "Options:
  --table\t-t table\ttable to manipulate (default: `filter')
  --set-counters -c PKTS BYTES\tset the counter during insert/append
  --numeric\t-n\t\tnumeric output of addresses and ports
",
    );
    assert_eq!(
        entry(&result, "table").switch,
        OwnedSwitch::Both('t', "table".to_owned())
    );
    assert_eq!(param_of(&result, "table").name, "table");
    assert_eq!(param_of(&result, "set-counters").name, "PKTS");
    assert_eq!(
        entry(&result, "numeric").switch,
        OwnedSwitch::Both('n', "numeric".to_owned())
    );
    assert!(entry(&result, "numeric").param.is_none());
}

#[test]
fn flag_opening_a_description_is_not_a_short_alias() {
    let result = parse(
        "Options:
      --null                 -T reads null-terminated names; implies
                             --verbatim-files-from
  -T, --files-from=FILE      get names to extract or create from FILE
",
    );
    assert_eq!(
        entry(&result, "null").switch,
        OwnedSwitch::Long("null".to_owned())
    );
    assert!(entry(&result, "null").param.is_none());
}

#[test]
fn punctuation_short_flags_pair_with_their_long_names() {
    let result = parse(
        "Options:
    -., --hidden
        Search hidden files and directories.
    -?, --help
        Show help.
",
    );
    assert_eq!(
        entry(&result, "hidden").switch,
        OwnedSwitch::Both('.', "hidden".to_owned())
    );
    assert_eq!(
        entry(&result, "help").switch,
        OwnedSwitch::Both('?', "help".to_owned())
    );
}

fn choice_values(result: &ManpageResult, long: &str) -> Vec<String> {
    entry(result, long)
        .param
        .as_ref()
        .map(|param| {
            param
                .choices
                .iter()
                .map(|found| found.value.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn wrapped_possible_values_keep_dotted_and_alternate_spellings() {
    let result = parse(
        "SORTING OPTIONS:
  -s, --sort <FIELD>             which field to sort by [default: name] [possible values:
                                   name, .name, ext,
                                   size, none]
  -t, --time <FIELD>             which timestamp field to show [possible values:
                                   mod|modified, acc|accessed]
      --time-style <STYLE>       how to format timestamps [possible values:
                                   default, iso, \"+<CUSTOM_FORMAT>\"]
",
    );
    assert_eq!(choice_values(&result, "sort"), [
        "name", ".name", "ext", "size", "none"
    ]);
    assert_eq!(choice_values(&result, "time"), [
        "mod", "modified", "acc", "accessed"
    ]);
    assert_eq!(choice_values(&result, "time-style"), ["default", "iso"]);
}

#[test]
fn quoted_alternative_labels_under_a_colon_header_are_choices() {
    let result = parse(
        "Options:
  -t, --type <filetype>
          Filter the search by type:
            'f' or 'file':         regular files
            'd' or 'dir' or 'directory':    directories
            'l' or 'symlink':      symbolic links

            'x' or 'executable':   executables

          This option can be specified more than once.
",
    );
    assert_eq!(choice_values(&result, "type"), [
        "f",
        "file",
        "d",
        "dir",
        "directory",
        "l",
        "symlink",
        "x",
        "executable"
    ]);
}

#[test]
fn quoted_label_lists_past_the_first_line_are_not_choices() {
    let result = parse(
        "Options:
  -S, --size <size>
          Limit results based on the size of files.
          Allowed unit values:
              'b':  bytes
              'k':  kilobytes
",
    );
    assert!(choice_values(&result, "size").is_empty());
}

#[test]
fn lowercase_placeholder_before_a_long_form_that_repeats_it() {
    let result = parse(
        "Options:
  -F format, --format=format
        Specify the file format.
  -e word, --regexp=other
        Pattern.
",
    );
    assert_eq!(
        entry(&result, "format").switch,
        OwnedSwitch::Both('F', "format".to_owned())
    );
    assert!(entry(&result, "format").param.is_some());
    assert!(
        result
            .entries
            .iter()
            .all(|found| long_of(found) != Some("regexp"))
    );
}

#[test]
fn doc_wide_metavar_sentence_gives_choices_to_the_matching_param() {
    let result = parse(
        "Usage: ls [OPTION]... [FILE]...

  -F, --classify[=WHEN]
         append indicator to entries WHEN
      --color[=WHEN]
         color the output WHEN; more info below
      --block-size=SIZE
         scale sizes by SIZE
      --hyperlink[=BOOL]
         hyperlink file names

The WHEN argument defaults to 'always' and can also be 'auto' or 'never'.
The BOOL argument is mostly harmless.
",
    );
    assert_eq!(choice_values(&result, "color"), ["always", "auto", "never"]);
    assert_eq!(choice_values(&result, "classify"), [
        "always", "auto", "never"
    ]);
    assert!(choice_values(&result, "block-size").is_empty());
    assert!(choice_values(&result, "hyperlink").is_empty());
}

#[test]
fn usage_line_flags_fill_in_what_the_options_section_lacks() {
    let result = parse(
        "usage: git [-v | --version] [-h | --help] [-C <path>] [-c <name>=<value>]
           [--exec-path[=<path>]] [--html-path]
           [-p | --paginate | -P | --no-pager] [--git-dir=<path>]
           [--term-{new,bad}=<term>] [-xyz] <command> [<args>]

Options:
  -h, --help   show help
",
    );
    assert_eq!(
        entry(&result, "version").switch,
        OwnedSwitch::Both('v', "version".to_owned())
    );
    assert_eq!(
        entry(&result, "no-pager").switch,
        OwnedSwitch::Both('P', "no-pager".to_owned())
    );
    assert_eq!(param_name(&result, "git-dir"), Some("path"));
    assert_eq!(param_name(&result, "exec-path"), Some("path"));
    let help = result
        .entries
        .iter()
        .filter(|found| long_of(found) == Some("help"))
        .count();
    assert_eq!(help, 1);
    let dash_c = result
        .entries
        .iter()
        .find(|found| found.switch == OwnedSwitch::Short('C'))
        .expect("-C kept");
    assert_eq!(
        dash_c.param.as_ref().map(|param| param.name.as_str()),
        Some("path")
    );
    assert!(result.entries.iter().all(|found| {
        !matches!(
            &found.switch,
            OwnedSwitch::Long(long) if long.starts_with("term")
        ) && found.switch != OwnedSwitch::Short('x')
    }));
}

#[test]
fn usage_only_short_flags_keep_the_word_after_them_as_their_param() {
    let result = parse(
        "Usage: ip6tables-restore [-c] [-w secs] [-T table] [-M command] [file]\n\t   [ \
         --wait=<seconds>\n\t   [ --table=<TABLE> ]\n",
    );
    let param_of = |short: char| {
        result
            .entries
            .iter()
            .find(|entry| matches!(entry.switch, OwnedSwitch::Short(found) if found == short))
            .and_then(|entry| entry.param.as_ref())
            .map(|param| param.name.as_str())
    };
    assert_eq!(param_of('w'), Some("secs"));
    assert_eq!(param_of('T'), Some("table"));
    assert_eq!(param_of('M'), Some("command"));
    assert_eq!(param_of('c'), None);
    assert_eq!(param_name(&result, "wait"), Some("seconds"));
    assert_eq!(param_name(&result, "table"), Some("TABLE"));
}

#[test]
fn bracketed_short_switch_clusters_expand_and_junk_does_not() {
    let result = parse("Usage: fsck.btrfs [-aApy] [<device>...]\n");
    for short in ['a', 'A', 'p', 'y'] {
        assert!(
            result
                .entries
                .iter()
                .any(|entry| matches!(entry.switch, OwnedSwitch::Short(found) if found == short)),
            "-{short} missing in {:?}",
            result.entries
        );
    }
    for junk in [
        "Usage: fmt [-WIDTH] [OPTION]... [FILE]...\n",
        "Usage: psfxtable [-i infont] [-nt]\n",
        "Usage: tool [-lName] [-m size]\n",
    ] {
        let parsed = parse(junk);
        assert!(
            !parsed.entries.iter().any(|entry| {
                matches!(
                    entry.switch,
                    OwnedSwitch::Short('W' | 'D' | 'n' | 't' | 'N' | 'a' | 'e')
                )
            }),
            "{junk}: {:?}",
            parsed.entries
        );
    }
}

#[test]
fn flag_and_value_name_squeezed_together_is_not_a_cluster() {
    let parsed = parse("Usage: gzip [-acfhLV] [-Ssuffix] [name...]\n");
    assert!(
        !parsed
            .entries
            .iter()
            .any(|entry| matches!(entry.switch, OwnedSwitch::Short('u' | 'i' | 'x'))),
        "{:?}",
        parsed.entries
    );
}

#[test]
fn long_bracket_value_names_stay_and_camel_case_flags_are_not_clusters() {
    let result = parse("Usage: ssh [-i identity_file] [-noVerify]\n");
    assert_eq!(result.entries.len(), 1, "{:?}", result.entries);
    let param = result.entries[0].param.as_ref().expect("param");
    assert_eq!(param.name, "identity_file");
}

#[test]
fn example_value_in_a_usage_bracket_is_not_the_param_name() {
    let result = parse("Usage: tool [-t ext4] [-o ro] [--mode rw] [-w secs]\n");
    assert_eq!(param_of(&result, "mode").name, "value");
    let names: Vec<_> = result
        .entries
        .iter()
        .filter_map(|entry| entry.param.as_ref().map(|param| param.name.as_str()))
        .collect();
    assert_eq!(names, ["value", "value", "value", "secs"]);
}
