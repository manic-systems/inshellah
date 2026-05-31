// SPDX-License-Identifier: EUPL-1.2
use super::super::shared::{Candidate, DynCtx, run};
pub(super) fn complete(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let by_pid = spans[0] == "kill";
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
