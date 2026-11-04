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

const SIGNALS: &[(&str, &str)] = &[
    ("HUP", "hangup"),
    ("INT", "interrupt"),
    ("QUIT", "quit"),
    ("ILL", "illegal instruction"),
    ("TRAP", "trace trap"),
    ("ABRT", "abort"),
    ("BUS", "bus error"),
    ("FPE", "arithmetic error"),
    ("KILL", "kill, cannot be caught"),
    ("USR1", "user signal 1"),
    ("SEGV", "segmentation fault"),
    ("USR2", "user signal 2"),
    ("PIPE", "broken pipe"),
    ("ALRM", "alarm clock"),
    ("TERM", "terminate"),
    ("STKFLT", "stack fault"),
    ("CHLD", "child stopped or exited"),
    ("CONT", "continue"),
    ("STOP", "stop, cannot be caught"),
    ("TSTP", "stop from the terminal"),
    ("TTIN", "background read from the terminal"),
    ("TTOU", "background write to the terminal"),
    ("URG", "urgent socket data"),
    ("XCPU", "cpu time limit"),
    ("XFSZ", "file size limit"),
    ("VTALRM", "virtual alarm clock"),
    ("PROF", "profiling timer"),
    ("WINCH", "window size change"),
    ("IO", "i/o possible"),
    ("PWR", "power failure"),
    ("SYS", "bad system call"),
];

/// the flags of the `kill` builtin in bash and zsh, which are not the binary's.
const BUILTIN_FLAGS: &[(&str, &str)] = &[
    ("-s", "signal name"),
    ("-n", "signal number"),
    ("-l", "list signal names"),
    ("-L", "list signal names"),
];

const BUILTIN_SIGNAL_FLAGS: &[&str] = &["-s", "-n", "-l", "-L"];
const BINARY_SIGNAL_FLAGS: &[&str] = &["-s", "--signal", "-l", "--list"];

fn signals(lead: &str) -> Vec<Candidate> {
    SIGNALS
        .iter()
        .map(|(name, desc)| Candidate::new(format!("{lead}{name}"), *desc))
        .collect()
}

pub(super) fn complete(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let by_pid = words.command() == "kill";
    if by_pid
        && ctx.shell != Shell::Nushell
        && words
            .value_slot(ctx)
            .or_else(|| ctx.slot.value_flag())
            .is_some_and(|flag| BINARY_SIGNAL_FLAGS.contains(&flag))
    {
        return Some(signals(""));
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
    processes(words, ctx)
}

/// `kill` as bash and zsh run it, which no cached manpage describes.
pub(super) fn builtin_kill(words: &Words, ctx: &DynCtx) -> Vec<Candidate> {
    let typed = words.current();
    let signal_value = words
        .value_slot(ctx)
        .or_else(|| ctx.slot.value_flag())
        .is_some_and(|flag| BUILTIN_SIGNAL_FLAGS.contains(&flag));
    if signal_value || words.has(&["-l", "-L"]) {
        return signals("");
    }
    if let Some(rest) = typed.strip_prefix('-') {
        let mut found: Vec<Candidate> = BUILTIN_FLAGS
            .iter()
            .filter(|(flag, _)| flag.starts_with(typed))
            .map(|(flag, desc)| Candidate::new(*flag, *desc))
            .collect();
        if rest.len() >= 2 || rest.starts_with(|ch: char| ch.is_ascii_uppercase()) {
            found.extend(signals("-"));
            found.extend(signals("-SIG"));
        }
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
