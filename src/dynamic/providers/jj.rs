// SPDX-License-Identifier: EUPL-1.2
//! jj completes itself under `COMPLETE=fish`, one `value\tdescription` per
//! line.

use crate::{
    complete::Candidate,
    dynamic::{
        adapters::clap,
        shared::{
            DynCtx,
            Matching,
            Words,
            filter_candidates,
        },
    },
};

pub(super) fn complete(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let spans = words.spans();
    if let Some(candidates) = clap(spans, ctx) {
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
    filter_candidates(clap(&widened, ctx)?, word, Matching::Value)
}
