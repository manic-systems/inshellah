// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::shared::{
        DynCtx,
        Words,
        run,
    },
};
pub(super) fn complete(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if !words.at_operand(ctx) {
        return None;
    }
    let by_pid = words.command() == "kill";
    // util-linux kill --timeout takes a millisecond count and then a signal.
    let awaiting_signal = by_pid && words.has(&["--timeout"]) && words.arg_count() == 0;
    let pattern_given = !by_pid && words.arg_count() > 0;
    if awaiting_signal || pattern_given {
        return None;
    }
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
