// SPDX-License-Identifier: EUPL-1.2
use std::borrow::Cow;

use crate::{
    complete::Candidate,
    dynamic::shared::{
        DynCtx,
        Words,
        run,
    },
    shell::Shell,
};

#[derive(Clone)]
struct Signal {
    name:   Cow<'static, str>,
    number: u8,
    desc:   Cow<'static, str>,
}

impl Signal {
    const fn fixed(name: &'static str, number: u8, desc: &'static str) -> Self {
        Self {
            name: Cow::Borrowed(name),
            number,
            desc: Cow::Borrowed(desc),
        }
    }

    fn realtime(name: String, number: u8) -> Self {
        Self {
            name: Cow::Owned(name),
            number,
            desc: Cow::Owned(format!("realtime signal {number}")),
        }
    }
}

#[derive(Clone, Copy)]
enum Account {
    User,
    Group,
}

impl Account {
    fn of_flag(flag: &str) -> Option<Self> {
        if USER_FLAGS.contains(&flag) {
            Some(Self::User)
        } else if GROUP_FLAGS.contains(&flag) {
            Some(Self::Group)
        } else {
            None
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Group => "group",
        }
    }
}

struct BuiltinFlag {
    flag: &'static str,
    desc: &'static str,
    only: Option<Shell>,
}

const LINUX_SIGNALS: &[Signal] = &[
    Signal::fixed("HUP", 1, "hangup"),
    Signal::fixed("INT", 2, "interrupt"),
    Signal::fixed("QUIT", 3, "quit"),
    Signal::fixed("ILL", 4, "illegal instruction"),
    Signal::fixed("TRAP", 5, "trace trap"),
    Signal::fixed("ABRT", 6, "abort"),
    Signal::fixed("BUS", 7, "bus error"),
    Signal::fixed("FPE", 8, "arithmetic error"),
    Signal::fixed("KILL", 9, "kill, cannot be caught"),
    Signal::fixed("USR1", 10, "user signal 1"),
    Signal::fixed("SEGV", 11, "segmentation fault"),
    Signal::fixed("USR2", 12, "user signal 2"),
    Signal::fixed("PIPE", 13, "broken pipe"),
    Signal::fixed("ALRM", 14, "alarm clock"),
    Signal::fixed("TERM", 15, "terminate"),
    Signal::fixed("STKFLT", 16, "stack fault"),
    Signal::fixed("CHLD", 17, "child stopped or exited"),
    Signal::fixed("CONT", 18, "continue"),
    Signal::fixed("STOP", 19, "stop, cannot be caught"),
    Signal::fixed("TSTP", 20, "stop from the terminal"),
    Signal::fixed("TTIN", 21, "background read from the terminal"),
    Signal::fixed("TTOU", 22, "background write to the terminal"),
    Signal::fixed("URG", 23, "urgent socket data"),
    Signal::fixed("XCPU", 24, "cpu time limit"),
    Signal::fixed("XFSZ", 25, "file size limit"),
    Signal::fixed("VTALRM", 26, "virtual alarm clock"),
    Signal::fixed("PROF", 27, "profiling timer"),
    Signal::fixed("WINCH", 28, "window size change"),
    Signal::fixed("IO", 29, "i/o possible"),
    Signal::fixed("PWR", 30, "power failure"),
    Signal::fixed("SYS", 31, "bad system call"),
];

const BSD_SIGNALS: &[Signal] = &[
    Signal::fixed("HUP", 1, "hangup"),
    Signal::fixed("INT", 2, "interrupt"),
    Signal::fixed("QUIT", 3, "quit"),
    Signal::fixed("ILL", 4, "illegal instruction"),
    Signal::fixed("TRAP", 5, "trace trap"),
    Signal::fixed("ABRT", 6, "abort"),
    Signal::fixed("EMT", 7, "emulator trap"),
    Signal::fixed("FPE", 8, "arithmetic error"),
    Signal::fixed("KILL", 9, "kill, cannot be caught"),
    Signal::fixed("BUS", 10, "bus error"),
    Signal::fixed("SEGV", 11, "segmentation fault"),
    Signal::fixed("SYS", 12, "bad system call"),
    Signal::fixed("PIPE", 13, "broken pipe"),
    Signal::fixed("ALRM", 14, "alarm clock"),
    Signal::fixed("TERM", 15, "terminate"),
    Signal::fixed("URG", 16, "urgent socket data"),
    Signal::fixed("STOP", 17, "stop, cannot be caught"),
    Signal::fixed("TSTP", 18, "stop from the terminal"),
    Signal::fixed("CONT", 19, "continue"),
    Signal::fixed("CHLD", 20, "child stopped or exited"),
    Signal::fixed("TTIN", 21, "background read from the terminal"),
    Signal::fixed("TTOU", 22, "background write to the terminal"),
    Signal::fixed("IO", 23, "i/o possible"),
    Signal::fixed("XCPU", 24, "cpu time limit"),
    Signal::fixed("XFSZ", 25, "file size limit"),
    Signal::fixed("VTALRM", 26, "virtual alarm clock"),
    Signal::fixed("PROF", 27, "profiling timer"),
    Signal::fixed("WINCH", 28, "window size change"),
    Signal::fixed("INFO", 29, "status request"),
    Signal::fixed("USR1", 30, "user signal 1"),
    Signal::fixed("USR2", 31, "user signal 2"),
];

const RTMIN: u8 = 34;
const RTMAX: u8 = 64;

/// the flags of the `kill` builtin in bash and zsh, which are not the binary's.
const BUILTIN_FLAGS: &[BuiltinFlag] = &[
    BuiltinFlag {
        flag: "-s",
        desc: "signal name",
        only: None,
    },
    BuiltinFlag {
        flag: "-n",
        desc: "signal number",
        only: None,
    },
    BuiltinFlag {
        flag: "-l",
        desc: "list signal names",
        only: None,
    },
    BuiltinFlag {
        flag: "-L",
        desc: "list signal names",
        only: Some(Shell::Bash),
    },
    BuiltinFlag {
        flag: "-L",
        desc: "list signal names with their numbers",
        only: Some(Shell::Zsh),
    },
    BuiltinFlag {
        flag: "-q",
        desc: "integer value sent with the signal",
        only: Some(Shell::Zsh),
    },
];

const BUILTIN_SIGNAL_FLAGS: &[&str] = &["-s", "-n", "-l", "-L"];
const BINARY_SIGNAL_FLAGS: &[&str] = &["-s", "--signal", "-l", "--list"];
const PKILL_SIGNAL_FLAGS: &[&str] = &["--signal"];
const USER_FLAGS: &[&str] = &["-u", "--euid", "-U", "--uid"];
const GROUP_FLAGS: &[&str] = &["-G", "--group"];

const fn signal_table() -> &'static [Signal] {
    if cfg!(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )) {
        BSD_SIGNALS
    } else {
        LINUX_SIGNALS
    }
}

fn realtime() -> Vec<Signal> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    let mut found = vec![Signal::realtime("RTMIN".to_owned(), RTMIN)];
    found.extend((1..=15).map(|step| Signal::realtime(format!("RTMIN+{step}"), RTMIN + step)));
    found.extend(
        (1..=14)
            .rev()
            .map(|step| Signal::realtime(format!("RTMAX-{step}"), RTMAX - step)),
    );
    found.push(Signal::realtime("RTMAX".to_owned(), RTMAX));
    found
}

fn all_signals() -> impl Iterator<Item = Signal> {
    signal_table().iter().cloned().chain(realtime())
}

fn signals(lead: &str) -> Vec<Candidate> {
    all_signals()
        .map(|sig| Candidate::new(format!("{lead}{}", sig.name), sig.desc))
        .collect()
}

fn numbers(lead: &str) -> Vec<Candidate> {
    let mut all: Vec<Signal> = all_signals().collect();
    all.sort_by_key(|sig| sig.number);
    all.into_iter()
        .map(|sig| Candidate::new(format!("{lead}{}", sig.number), sig.name))
        .collect()
}

/// the signal names a partly typed `partial` can still become, with the `SIG`
/// spellings once it is heading there. `numeric` offers numbers instead.
fn signal_values(lead: &str, partial: &str, numeric: bool) -> Vec<Candidate> {
    if numeric {
        return numbers(lead);
    }
    let mut found = signals(lead);
    if partial.starts_with("SIG") || (partial.len() >= 2 && "SIG".starts_with(partial)) {
        found.extend(signals(&format!("{lead}SIG")));
    }
    found
}

/// `-TERM` and `-SIGTERM` for the signal a pkill or kill names as its first
/// word.
fn dashed_signals(typed: &str) -> Vec<Candidate> {
    let mut found = Vec::new();
    if let Some(rest) = typed.strip_prefix('-')
        && (rest.len() >= 2 || rest.starts_with(|ch: char| ch.is_ascii_uppercase()))
    {
        found.extend(signals("-"));
        found.extend(signals("-SIG"));
    }
    found
}

fn value_flag<'words>(words: &'words Words, ctx: &'words DynCtx) -> Option<&'words str> {
    words.value_slot(ctx).or_else(|| ctx.slot.value_flag())
}

fn accounts(kind: Account, lead: &str, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    // macos ships no getent, its directory service lists names one per line.
    let argv: Vec<String> = if cfg!(target_os = "macos") {
        let dir = match kind {
            Account::User => "/Users",
            Account::Group => "/Groups",
        };
        vec!["dscl".into(), ".".into(), "-list".into(), dir.into()]
    } else {
        let db = match kind {
            Account::User => "passwd",
            Account::Group => "group",
        };
        vec!["getent".into(), db.into()]
    };
    let out = run(&argv, ctx)?;
    let found = account_names(&out, kind.label(), lead);
    (!found.is_empty()).then_some(found)
}

fn account_names(out: &str, desc: &str, lead: &str) -> Vec<Candidate> {
    out.lines()
        .filter_map(|line| line.split(':').next().filter(|name| !name.is_empty()))
        .map(|name| Candidate::new(format!("{lead}{name}"), desc))
        .collect()
}

/// a user or group flag with its value attached, as in `-uroot` or `-furoot`,
/// split into the text up to the value and the kind of account it names.
fn attached_account<'typed>(typed: &'typed str, ctx: &DynCtx) -> Option<(&'typed str, Account)> {
    let cluster = typed
        .strip_prefix('-')
        .filter(|cluster| !cluster.starts_with('-'))?;
    for (at, letter) in cluster.char_indices() {
        let flag = format!("-{letter}");
        let Some(kind) = Account::of_flag(&flag) else {
            if ctx.flags.short.contains(&flag) || ctx.flags.global_short.contains(&flag) {
                continue;
            }
            return None;
        };
        let split = 1 + at + letter.len_utf8();
        return (split < typed.len()).then(|| (typed.split_at(split).0, kind));
    }
    None
}

pub(super) fn complete(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let command = words.command();
    let by_pid = command == "kill";
    let flag = value_flag(words, ctx);
    let signal_flags = if by_pid {
        BINARY_SIGNAL_FLAGS
    } else {
        PKILL_SIGNAL_FLAGS
    };
    if command != "pgrep"
        && ctx.shell != Shell::Nushell
        && flag.is_some_and(|name| signal_flags.contains(&name))
    {
        return Some(signal_values("", words.current(), false));
    }
    if !by_pid {
        if let Some(kind) = flag.and_then(Account::of_flag) {
            return accounts(kind, "", ctx);
        }
        if words.value_slot(ctx).is_none()
            && !words.past_dashes()
            && let Some((lead, kind)) = attached_account(words.current(), ctx)
        {
            return accounts(kind, lead, ctx);
        }
    }
    if !words.at_operand(ctx) {
        return None;
    }
    // util-linux kill --timeout takes a millisecond count and then a signal.
    let awaiting_signal = by_pid && words.has(&["--timeout"]) && words.arg_count() == 0;
    let pattern_given = !by_pid && words.arg_count() > 0;
    if awaiting_signal || pattern_given {
        return None;
    }
    let typed = words.current();
    if typed.starts_with('-') {
        let dashed = if ctx.shell == Shell::Nushell
            || command == "pgrep"
            || words.past_dashes()
            || words.arg_count() > 0
        {
            Vec::new()
        } else {
            dashed_signals(typed)
        };
        return (!dashed.is_empty()).then_some(dashed);
    }
    processes(words, ctx)
}

/// whether the word under the cursor is the leading `-SIGNAL` of kill or pkill,
/// which static flag completion must not shadow as it reads `-TE` as flags.
pub(super) fn is_signal_word(spans: &[String], shell: Shell) -> bool {
    let [command, typed] = spans else {
        return false;
    };
    let name = |ch: char| ch.is_ascii_uppercase() || ch.is_ascii_digit() || "+-".contains(ch);
    shell != Shell::Nushell
        && matches!(command.as_str(), "kill" | "pkill")
        && typed.strip_prefix('-').is_some_and(|rest| {
            rest.len() >= 2
                && rest.starts_with(|ch: char| ch.is_ascii_uppercase())
                && rest.chars().all(name)
        })
}

/// whether the word under the cursor is a lone capital after `-`, such as
/// `-T`, which names no flag of kill or pkill by itself but starts a signal.
pub(super) fn is_lone_capital(spans: &[String], shell: Shell) -> bool {
    let [command, typed] = spans else {
        return false;
    };
    shell != Shell::Nushell
        && matches!(command.as_str(), "kill" | "pkill")
        && typed.strip_prefix('-').is_some_and(|rest| {
            rest.len() == 1 && rest.starts_with(|ch: char| ch.is_ascii_uppercase())
        })
}

/// `kill` as bash and zsh run it, which no cached manpage describes.
pub(super) fn builtin_kill(words: &Words, ctx: &DynCtx) -> Vec<Candidate> {
    let typed = words.current();
    let zsh = ctx.shell == Shell::Zsh;
    let slot = value_flag(words, ctx).filter(|name| BUILTIN_SIGNAL_FLAGS.contains(name));
    if let Some(name) = slot {
        return signal_values("", typed, zsh && name == "-n");
    }
    if zsh && value_flag(words, ctx) == Some("-q") {
        return Vec::new();
    }
    if words.has(&["-l", "-L"]) {
        return signals("");
    }
    if let Some(rest) = typed.strip_prefix('-')
        && !words.past_dashes()
    {
        if rest.starts_with(['s', 'n']) && rest.len() > 1 {
            let (lead, partial) = typed.split_at(2);
            return signal_values(lead, partial, zsh && rest.starts_with('n'));
        }
        if words.arg_count() > 0 {
            return Vec::new();
        }
        let mut found: Vec<Candidate> = BUILTIN_FLAGS
            .iter()
            .filter(|entry| {
                entry.flag.starts_with(typed) && entry.only.is_none_or(|shell| shell == ctx.shell)
            })
            .map(|entry| Candidate::new(entry.flag, entry.desc))
            .collect();
        found.extend(dashed_signals(typed));
        return found;
    }
    processes(words, ctx).unwrap_or_default()
}

fn processes(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let by_pid = words.command() == "kill";
    let out = run(
        &[
            "ps".into(),
            "-eo".into(),
            "pid,comm".into(),
            "--no-headers".into(),
        ],
        ctx,
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim();
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let pid = parts[0];
        let comm = parts[1..].join(" ");
        if by_pid {
            candidates.push(Candidate::new(pid, comm));
        } else {
            candidates.push(Candidate::new(comm, pid));
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(found: &[Candidate]) -> Vec<&str> {
        found.iter().map(|cand| cand.value.as_str()).collect()
    }

    #[test]
    fn every_account_is_offered_whatever_its_id() {
        let out = "root:x:0:0::/root:/bin/sh\nnobody:x:65534:65534::/:/bin/false\nmessagebus:x:4:\
                   4::/:\nbolt:x:1000:100::/h:/bin/sh\n:x:5:5\n";
        let found = account_names(out, "user", "");
        assert_eq!(values(&found), ["root", "nobody", "messagebus", "bolt"]);
        let groups = account_names("wheel:x:1:bolt\nusers:x:100:\n", "group", "");
        assert_eq!(values(&groups), ["wheel", "users"]);
    }

    #[test]
    fn each_table_has_its_own_signals() {
        let linux: Vec<&str> = LINUX_SIGNALS.iter().map(|sig| &*sig.name).collect();
        let bsd: Vec<&str> = BSD_SIGNALS.iter().map(|sig| &*sig.name).collect();
        assert!(linux.contains(&"STKFLT") && linux.contains(&"PWR"));
        assert!(!linux.contains(&"EMT") && !linux.contains(&"INFO"));
        assert!(bsd.contains(&"EMT") && bsd.contains(&"INFO"));
        assert!(!bsd.contains(&"STKFLT") && !bsd.contains(&"PWR"));
    }

    #[test]
    fn sig_spellings_follow_a_partial_sig() {
        let plain = signal_values("", "TE", false);
        assert!(!values(&plain).contains(&"SIGTERM"));
        let spelled = signal_values("", "SIGTE", false);
        assert!(values(&spelled).contains(&"SIGTERM"));
        let attached = signal_values("-s", "SI", false);
        assert!(values(&attached).contains(&"-sSIGTERM"));
        assert!(values(&attached).contains(&"-sTERM"));
    }

    #[test]
    fn a_leading_capitalised_word_is_a_signal() {
        let spans = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>()
        };
        assert!(is_signal_word(&spans(&["pkill", "-TE"]), Shell::Bash));
        assert!(is_signal_word(&spans(&["pkill", "-RTMIN+1"]), Shell::Zsh));
        assert!(!is_signal_word(&spans(&["pkill", "-TE"]), Shell::Nushell));
        assert!(!is_signal_word(&spans(&["pkill", "-Uro"]), Shell::Bash));
        assert!(!is_signal_word(&spans(&["pkill", "-P"]), Shell::Bash));
        assert!(!is_signal_word(
            &spans(&["pkill", "-f", "-TE"]),
            Shell::Bash
        ));
        assert!(!is_signal_word(&spans(&["pgrep", "-TE"]), Shell::Bash));
    }

    #[test]
    fn dashed_signals_need_a_second_letter_or_capital() {
        assert!(dashed_signals("-").is_empty());
        assert!(dashed_signals("-f").is_empty());
        assert!(values(&dashed_signals("-S")).contains(&"-SIGTERM"));
        assert!(values(&dashed_signals("-te")).contains(&"-TERM"));
    }
}
