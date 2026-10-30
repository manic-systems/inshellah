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
