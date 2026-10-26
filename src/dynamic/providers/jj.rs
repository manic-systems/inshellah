// SPDX-License-Identifier: EUPL-1.2
//! jj completes itself under `COMPLETE=fish`, one `value\tdescription` per
//! line.

use crate::{
    complete::Candidate,
    dynamic::shared::{
        DynCtx,
        Matching,
        filter_candidates,
        run_with,
    },
};

pub(super) fn complete(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if let Some(candidates) = native(spans, ctx) {
        return Some(candidates);
    }

    // jj only prefix-matches, so `jj lg` retries blank and fuzzy-filters.
    let (word, head) = spans.split_last()?;
    let plain = word
        .chars()
        .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '/'));
    if word.is_empty() || word.starts_with('-') || !plain {
        return None;
    }

    let mut widened = head.to_vec();
    widened.push(String::new());
    filter_candidates(native(&widened, ctx)?, word, Matching::Value)
}

fn native(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args = vec![ctx.bin("jj"), "--".to_owned()];
    args.extend_from_slice(spans);

    let out = run_with(&args, ctx, |cmd| {
        cmd.env("COMPLETE", "fish");
    })?;

    let candidates: Vec<Candidate> = out
        .lines()
        .filter_map(|line| {
            let (raw_value, description) = line.split_once('\t').unwrap_or((line, ""));
            let value = raw_value.trim_end();
            (!value.is_empty()).then(|| Candidate::new(value, description.trim()))
        })
        .collect();

    (!candidates.is_empty()).then_some(candidates)
}
