// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::shared::{
        DynCtx,
        Words,
        run,
    },
    shell::Shell,
};

type Signal = (&'static str, u8, &'static str);

const LINUX_SIGNALS: &[Signal] = &[
    ("HUP", 1, "hangup"),
    ("INT", 2, "interrupt"),
    ("QUIT", 3, "quit"),
    ("ILL", 4, "illegal instruction"),
    ("TRAP", 5, "trace trap"),
    ("ABRT", 6, "abort"),
    ("BUS", 7, "bus error"),
    ("FPE", 8, "arithmetic error"),
    ("KILL", 9, "kill, cannot be caught"),
    ("USR1", 10, "user signal 1"),
    ("SEGV", 11, "segmentation fault"),
    ("USR2", 12, "user signal 2"),
    ("PIPE", 13, "broken pipe"),
    ("ALRM", 14, "alarm clock"),
    ("TERM", 15, "terminate"),
    ("STKFLT", 16, "stack fault"),
    ("CHLD", 17, "child stopped or exited"),
    ("CONT", 18, "continue"),
    ("STOP", 19, "stop, cannot be caught"),
    ("TSTP", 20, "stop from the terminal"),
    ("TTIN", 21, "background read from the terminal"),
    ("TTOU", 22, "background write to the terminal"),
    ("URG", 23, "urgent socket data"),
    ("XCPU", 24, "cpu time limit"),
    ("XFSZ", 25, "file size limit"),
    ("VTALRM", 26, "virtual alarm clock"),
    ("PROF", 27, "profiling timer"),
    ("WINCH", 28, "window size change"),
    ("IO", 29, "i/o possible"),
    ("PWR", 30, "power failure"),
    ("SYS", 31, "bad system call"),
];

const MACOS_SIGNALS: &[Signal] = &[
    ("HUP", 1, "hangup"),
    ("INT", 2, "interrupt"),
    ("QUIT", 3, "quit"),
    ("ILL", 4, "illegal instruction"),
    ("TRAP", 5, "trace trap"),
    ("ABRT", 6, "abort"),
    ("EMT", 7, "emulator trap"),
    ("FPE", 8, "arithmetic error"),
    ("KILL", 9, "kill, cannot be caught"),
    ("BUS", 10, "bus error"),
    ("SEGV", 11, "segmentation fault"),
    ("SYS", 12, "bad system call"),
    ("PIPE", 13, "broken pipe"),
    ("ALRM", 14, "alarm clock"),
    ("TERM", 15, "terminate"),
    ("URG", 16, "urgent socket data"),
    ("STOP", 17, "stop, cannot be caught"),
    ("TSTP", 18, "stop from the terminal"),
    ("CONT", 19, "continue"),
    ("CHLD", 20, "child stopped or exited"),
    ("TTIN", 21, "background read from the terminal"),
    ("TTOU", 22, "background write to the terminal"),
    ("IO", 23, "i/o possible"),
    ("XCPU", 24, "cpu time limit"),
    ("XFSZ", 25, "file size limit"),
    ("VTALRM", 26, "virtual alarm clock"),
    ("PROF", 27, "profiling timer"),
    ("WINCH", 28, "window size change"),
    ("INFO", 29, "status request"),
    ("USR1", 30, "user signal 1"),
    ("USR2", 31, "user signal 2"),
];

const RTMIN: u8 = 34;
const RTMAX: u8 = 64;

/// the flags of the `kill` builtin in bash and zsh, which are not the binary's.
const BUILTIN_FLAGS: &[(&str, &str)] = &[
    ("-s", "signal name"),
    ("-n", "signal number"),
    ("-l", "list signal names"),
    ("-L", "list signal names"),
];

const BUILTIN_SIGNAL_FLAGS: &[&str] = &["-s", "-n", "-l", "-L"];
const BINARY_SIGNAL_FLAGS: &[&str] = &["-s", "--signal", "-l", "--list"];
const PKILL_SIGNAL_FLAGS: &[&str] = &["--signal"];
const USER_FLAGS: &[&str] = &["-u", "--euid", "-U", "--uid"];
const GROUP_FLAGS: &[&str] = &["-G", "--group"];

const fn signal_table() -> &'static [Signal] {
    if cfg!(target_os = "macos") {
        MACOS_SIGNALS
    } else {
        LINUX_SIGNALS
    }
}

fn realtime() -> Vec<(String, u8, String)> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    let named = |name: String, num: u8| (name, num, format!("realtime signal {num}"));
    let mut found = vec![named("RTMIN".to_owned(), RTMIN)];
    found.extend((1..=15).map(|step| named(format!("RTMIN+{step}"), RTMIN + step)));
    found.extend(
        (1..=14)
            .rev()
            .map(|step| named(format!("RTMAX-{step}"), RTMAX - step)),
    );
    found.push(named("RTMAX".to_owned(), RTMAX));
    found
}

fn signals(lead: &str) -> Vec<Candidate> {
    let fixed = signal_table()
        .iter()
        .map(|(name, _, desc)| (name.to_string(), (*desc).to_owned()));
    let rt = realtime().into_iter().map(|(name, _, desc)| (name, desc));
    fixed
        .chain(rt)
        .map(|(name, desc)| Candidate::new(format!("{lead}{name}"), desc))
        .collect()
}

fn numbers(lead: &str) -> Vec<Candidate> {
    let fixed = signal_table()
        .iter()
        .map(|(name, num, _)| (*num, (*name).to_owned()));
    let rt = realtime().into_iter().map(|(name, num, _)| (num, name));
    let mut all: Vec<(u8, String)> = fixed.chain(rt).collect();
    all.sort_by_key(|(num, _)| *num);
    all.into_iter()
        .map(|(num, name)| Candidate::new(format!("{lead}{num}"), name))
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

fn accounts(db: &str, desc: &str, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run(&["getent".into(), db.into()], ctx)?;
    let found = account_names(&out, desc);
    (!found.is_empty()).then_some(found)
}

fn account_names(out: &str, desc: &str) -> Vec<Candidate> {
    out.lines()
        .filter_map(|line| line.split(':').next().filter(|name| !name.is_empty()))
        .map(|name| Candidate::new(name, desc))
        .collect()
}

pub(super) fn complete(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let by_pid = words.command() == "kill";
    let flag = value_flag(words, ctx);
    let signal_flags = if by_pid {
        BINARY_SIGNAL_FLAGS
    } else {
        PKILL_SIGNAL_FLAGS
    };
    if ctx.shell != Shell::Nushell && flag.is_some_and(|name| signal_flags.contains(&name)) {
        return Some(signal_values("", words.current(), false));
    }
    if !by_pid && let Some(name) = flag {
        if USER_FLAGS.contains(&name) {
            return accounts("passwd", "user", ctx);
        }
        if GROUP_FLAGS.contains(&name) {
            return accounts("group", "group", ctx);
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
        let dashed = if ctx.shell == Shell::Nushell || words.past_dashes() || words.arg_count() > 0
        {
            Vec::new()
        } else {
            dashed_signals(typed)
        };
        return (!dashed.is_empty()).then_some(dashed);
    }
    processes(words, ctx)
}

/// `kill` as bash and zsh run it, which no cached manpage describes.
pub(super) fn builtin_kill(words: &Words, ctx: &DynCtx) -> Vec<Candidate> {
    let typed = words.current();
    let zsh = ctx.shell == Shell::Zsh;
    let slot = value_flag(words, ctx).filter(|name| BUILTIN_SIGNAL_FLAGS.contains(name));
    if let Some(name) = slot {
        return signal_values("", typed, zsh && name == "-n");
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
        let mut found: Vec<Candidate> = BUILTIN_FLAGS
            .iter()
            .filter(|(flag, _)| flag.starts_with(typed))
            .map(|(flag, desc)| Candidate::new(*flag, *desc))
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
        let found = account_names(out, "user");
        assert_eq!(values(&found), ["root", "nobody", "messagebus", "bolt"]);
        let groups = account_names("wheel:x:1:bolt\nusers:x:100:\n", "group");
        assert_eq!(values(&groups), ["wheel", "users"]);
    }

    #[test]
    fn each_table_has_its_own_signals() {
        let linux: Vec<&str> = LINUX_SIGNALS.iter().map(|sig| sig.0).collect();
        let macos: Vec<&str> = MACOS_SIGNALS.iter().map(|sig| sig.0).collect();
        assert!(linux.contains(&"STKFLT") && linux.contains(&"PWR"));
        assert!(!linux.contains(&"EMT") && !linux.contains(&"INFO"));
        assert!(macos.contains(&"EMT") && macos.contains(&"INFO"));
        assert!(!macos.contains(&"STKFLT") && !macos.contains(&"PWR"));
    }

    #[test]
    fn linux_adds_the_real_time_range() {
        let names: Vec<String> = realtime().into_iter().map(|sig| sig.0).collect();
        if cfg!(target_os = "linux") {
            assert_eq!(names.len(), 31);
            assert_eq!(names[0], "RTMIN");
            assert!(names.contains(&"RTMIN+15".to_owned()));
            assert!(names.contains(&"RTMAX-14".to_owned()));
            assert_eq!(names.last().map(String::as_str), Some("RTMAX"));
        } else {
            assert!(names.is_empty());
        }
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
    fn numeric_signals_are_numbers_in_order() {
        let found = signal_values("", "", true);
        let nums = values(&found);
        assert_eq!(&nums[..3], ["1", "2", "3"]);
        assert!(nums.contains(&"15"));
    }

    #[test]
    fn dashed_signals_need_a_second_letter_or_capital() {
        assert!(dashed_signals("-").is_empty());
        assert!(dashed_signals("-f").is_empty());
        assert!(values(&dashed_signals("-S")).contains(&"-SIGTERM"));
        assert!(values(&dashed_signals("-te")).contains(&"-TERM"));
    }
}
