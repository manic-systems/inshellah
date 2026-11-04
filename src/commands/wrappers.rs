// SPDX-License-Identifier: EUPL-1.2
//! commands that run another command, so completion follows the one they run.
//! sudo and friends are handled by their own lookup in `complete`.

/// the argument grammar of one wrapper, enough to find the command it runs.
struct Wrapper {
    name:              &'static str,
    /// flags whose value is the next word, as `-n 5` or `--adjustment 5`.
    takes_value:       &'static [&'static str],
    /// flags after which no command follows, or whose grammar is too irregular
    /// to skip, like `env -S`.
    no_command:        &'static [&'static str],
    /// words between the flags and the command, like timeout's duration.
    operands:          usize,
    /// flags that stand in for the operands.
    replaces_operands: &'static [&'static str],
    /// flags after which the operands may be left out, as chrt's priority
    /// after a policy that has no use for one.
    optional_operands: &'static [&'static str],
    /// `NAME=value` words may precede the command.
    assignments:       bool,
    /// a bare `-` among the flags, as in `env - cmd`.
    bare_dash:         bool,
}

const WRAPPERS: &[Wrapper] = &[
    Wrapper {
        name:              "nohup",
        takes_value:       &[],
        no_command:        &[],
        operands:          0,
        replaces_operands: &[],
        optional_operands: &[],
        assignments:       false,
        bare_dash:         false,
    },
    Wrapper {
        name:              "env",
        takes_value:       &[
            "-u",
            "--unset",
            "-C",
            "--chdir",
            "-a",
            "--argv0",
            "-P",
            "--env0-from",
        ],
        no_command:        &["-S", "--split-string"],
        operands:          0,
        replaces_operands: &[],
        optional_operands: &[],
        assignments:       true,
        bare_dash:         true,
    },
    Wrapper {
        name:              "nice",
        takes_value:       &["-n", "--adjustment"],
        no_command:        &[],
        operands:          0,
        replaces_operands: &[],
        optional_operands: &[],
        assignments:       false,
        bare_dash:         false,
    },
    Wrapper {
        name:              "ionice",
        takes_value:       &["-c", "--class", "-n", "--classdata"],
        no_command:        &["-p", "--pid", "-P", "--pgid", "-u", "--uid"],
        operands:          0,
        replaces_operands: &[],
        optional_operands: &[],
        assignments:       false,
        bare_dash:         false,
    },
    Wrapper {
        name:              "timeout",
        takes_value:       &["-s", "--signal", "-k", "--kill-after"],
        no_command:        &[],
        operands:          1,
        replaces_operands: &[],
        optional_operands: &[],
        assignments:       false,
        bare_dash:         false,
    },
    Wrapper {
        name:              "xargs",
        takes_value:       &[
            "-a",
            "--arg-file",
            "-d",
            "--delimiter",
            "-E",
            "-I",
            "-L",
            "--max-lines",
            "-n",
            "--max-args",
            "-P",
            "--max-procs",
            "-s",
            "--max-chars",
            "--process-slot-var",
        ],
        no_command:        &[],
        operands:          0,
        replaces_operands: &[],
        optional_operands: &[],
        assignments:       false,
        bare_dash:         false,
    },
    Wrapper {
        name:              "time",
        takes_value:       &["-f", "--format", "-o", "--output"],
        no_command:        &[],
        operands:          0,
        replaces_operands: &[],
        optional_operands: &[],
        assignments:       false,
        bare_dash:         false,
    },
    Wrapper {
        name:              "command",
        takes_value:       &[],
        no_command:        &["-v", "-V"],
        operands:          0,
        replaces_operands: &[],
        optional_operands: &[],
        assignments:       false,
        bare_dash:         false,
    },
    Wrapper {
        name:              "exec",
        takes_value:       &["-a"],
        no_command:        &[],
        operands:          0,
        replaces_operands: &[],
        optional_operands: &[],
        assignments:       false,
        bare_dash:         false,
    },
    Wrapper {
        name:              "stdbuf",
        takes_value:       &["-i", "--input", "-o", "--output", "-e", "--error"],
        no_command:        &[],
        operands:          0,
        replaces_operands: &[],
        optional_operands: &[],
        assignments:       false,
        bare_dash:         false,
    },
    Wrapper {
        name:              "setsid",
        takes_value:       &[],
        no_command:        &[],
        operands:          0,
        replaces_operands: &[],
        optional_operands: &[],
        assignments:       false,
        bare_dash:         false,
    },
    Wrapper {
        name:              "chrt",
        takes_value:       &[
            "-T",
            "--sched-runtime",
            "-P",
            "--sched-period",
            "-D",
            "--sched-deadline",
        ],
        no_command:        &["-p", "--pid", "-m", "--max"],
        operands:          1,
        replaces_operands: &[],
        optional_operands: &[
            "-b",
            "--batch",
            "-d",
            "--deadline",
            "-e",
            "--ext",
            "-i",
            "--idle",
            "-o",
            "--other",
        ],
        assignments:       false,
        bare_dash:         false,
    },
    Wrapper {
        name:              "taskset",
        takes_value:       &["-c", "--cpu-list"],
        no_command:        &["-p", "--pid"],
        operands:          1,
        replaces_operands: &["-c", "--cpu-list"],
        optional_operands: &[],
        assignments:       false,
        bare_dash:         false,
    },
];

fn wrapper_of(word: &str) -> Option<&'static Wrapper> {
    let name = if word.starts_with('/') {
        word.rsplit('/').next()?
    } else {
        word
    };
    WRAPPERS.iter().find(|wrapper| wrapper.name == name)
}

/// the index of the command `spans[0]` runs, when that command is already
/// typed. the last span is the word under the cursor, so a command that is
/// still being typed, or a flag value not yet typed, gives None.
pub fn wrapped_command(spans: &[String]) -> Option<usize> {
    let wrapper = wrapper_of(spans.first()?)?;
    let cursor = spans.len() - 1;
    let mut at = 1;
    let mut operands = wrapper.operands;
    let mut optional = false;
    while at < cursor {
        let word = spans[at].as_str();
        if word == "--" {
            at += 1;
            break;
        }
        if word == "-" && wrapper.bare_dash {
            at += 1;
            continue;
        }
        if !word.starts_with('-') || word == "-" {
            break;
        }
        let scan = scan_flag(wrapper, word)?;
        if scan.replaced {
            operands = 0;
        }
        optional |= scan.relaxed;
        at += 1 + usize::from(scan.consumed);
    }
    for _ in 0..operands {
        let numeric =
            |word: &str| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_digit());
        if optional && !spans.get(at).is_some_and(|word| numeric(word)) {
            break;
        }
        at += 1;
    }
    while wrapper.assignments && at < cursor && is_assignment(&spans[at]) {
        at += 1;
    }
    (at < cursor && !spans[at].is_empty() && !spans[at].starts_with('-')).then_some(at)
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=')
        .is_some_and(|(name, _)| !name.is_empty() && !name.starts_with('-'))
}

/// what a flag word means for finding the command.
#[derive(Default)]
struct FlagScan {
    /// the flag takes the next word.
    consumed: bool,
    /// the flag replaces the wrapper's operands.
    replaced: bool,
    /// the flag makes the wrapper's operands optional.
    relaxed:  bool,
}

/// None when the flag rules out a command.
fn scan_flag(wrapper: &Wrapper, word: &str) -> Option<FlagScan> {
    if word.starts_with("--") {
        let (name, attached) = word
            .split_once('=')
            .map_or((word, false), |(name, _)| (name, true));
        if wrapper.no_command.contains(&name) {
            return None;
        }
        return Some(FlagScan {
            consumed: !attached && wrapper.takes_value.contains(&name),
            replaced: wrapper.replaces_operands.contains(&name),
            relaxed:  wrapper.optional_operands.contains(&name),
        });
    }
    let mut scan = FlagScan::default();
    for (idx, letter) in word.char_indices().skip(1) {
        let flag = format!("-{letter}");
        if wrapper.no_command.contains(&flag.as_str()) {
            return None;
        }
        scan.replaced |= wrapper.replaces_operands.contains(&flag.as_str());
        scan.relaxed |= wrapper.optional_operands.contains(&flag.as_str());
        if wrapper.takes_value.contains(&flag.as_str()) {
            let attached = idx + letter.len_utf8() < word.len();
            scan.consumed = !attached;
            return Some(scan);
        }
    }
    Some(scan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: &str) -> Option<usize> {
        let spans: Vec<String> = line.split(' ').map(str::to_owned).collect();
        wrapped_command(&spans)
    }

    #[test]
    fn plain_wrappers_run_the_next_word() {
        assert_eq!(at("nohup git "), Some(1));
        assert_eq!(at("setsid -f git "), Some(2));
        assert_eq!(at("time -p git "), Some(2));
        assert_eq!(at("exec -a name git "), Some(3));
        assert_eq!(at("command -p git "), Some(2));
        assert_eq!(at("/usr/bin/env git "), Some(1));
    }

    #[test]
    fn env_skips_flags_and_assignments() {
        assert_eq!(at("env FOO=1 BAR=2 git "), Some(3));
        assert_eq!(at("env -i -u HOME FOO=1 git "), Some(5));
        assert_eq!(at("env --unset HOME git "), Some(3));
        assert_eq!(at("env --unset=HOME git "), Some(2));
        assert_eq!(at("env -uHOME git "), Some(2));
        assert_eq!(at("env -C /tmp git "), Some(3));
        assert_eq!(at("env -S 'git status' "), None);
    }

    #[test]
    fn flag_values_are_skipped() {
        assert_eq!(at("nice -n 5 git "), Some(3));
        assert_eq!(at("nice -n5 git "), Some(2));
        assert_eq!(at("nice -10 git "), Some(2));
        assert_eq!(at("nice --adjustment=5 git "), Some(2));
        assert_eq!(at("ionice -c 2 -n 4 git "), Some(5));
        assert_eq!(at("ionice -t git "), Some(2));
        assert_eq!(at("stdbuf -oL -e0 git "), Some(3));
        assert_eq!(at("stdbuf -o L git "), Some(3));
        assert_eq!(at("xargs -0 -n 1 git "), Some(4));
        assert_eq!(at("xargs -I {} git "), Some(3));
        assert_eq!(at("xargs -r0 git "), Some(2));
    }

    #[test]
    fn operands_precede_the_command() {
        assert_eq!(at("timeout 5 git "), Some(2));
        assert_eq!(at("timeout -s KILL --foreground 5s git "), Some(5));
        assert_eq!(at("timeout --signal=KILL 5 git "), Some(3));
        assert_eq!(at("chrt -f 10 git "), Some(3));
        assert_eq!(at("taskset 0x3 git "), Some(2));
        assert_eq!(at("taskset -c 0,1 git "), Some(3));
        assert_eq!(at("taskset --cpu-list=0 git "), Some(2));
    }

    #[test]
    fn chrt_priority_is_optional_for_policies_without_one() {
        assert_eq!(at("chrt -b git "), Some(2));
        assert_eq!(at("chrt --idle git "), Some(2));
        assert_eq!(at("chrt -o -v git "), Some(3));
        assert_eq!(at("chrt -b 0 git "), Some(3));
        assert_eq!(at("chrt -i -R 0 git "), Some(4));
        assert_eq!(at("chrt -f git "), None);
        assert_eq!(at("chrt -b "), None);
    }

    #[test]
    fn env_knows_its_newer_flags_and_a_bare_dash() {
        assert_eq!(at("env --env0-from FILE git "), Some(3));
        assert_eq!(at("env --env0-from=FILE git "), Some(2));
        assert_eq!(at("env - git "), Some(2));
        assert_eq!(at("env - FOO=1 git "), Some(3));
        assert_eq!(at("env -i - git "), Some(3));
        assert_eq!(at("nohup - "), None);
    }

    #[test]
    fn nothing_runs_when_the_cursor_is_in_the_wrapper() {
        assert_eq!(at("nice -n "), None);
        assert_eq!(at("timeout "), None);
        assert_eq!(at("timeout 5 "), None);
        assert_eq!(at("env FOO=1 "), None);
        assert_eq!(at("nohup "), None);
        assert_eq!(at("nohup gi"), None);
        assert_eq!(at("git status "), None);
    }

    #[test]
    fn modes_without_a_command_stay_put() {
        assert_eq!(at("command -v git "), None);
        assert_eq!(at("chrt -p 10 1234 "), None);
        assert_eq!(at("taskset -p 0x1 1234 "), None);
        assert_eq!(at("ionice -p 1234 "), None);
    }

    #[test]
    fn a_double_dash_ends_the_flags() {
        assert_eq!(at("nice -- git "), Some(2));
        assert_eq!(at("env -- git "), Some(2));
    }
}
