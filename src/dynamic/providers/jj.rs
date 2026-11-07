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
        return Some(without_bare_aliases(candidates));
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
    filter_candidates(
        without_bare_aliases(clap(&widened, ctx)?),
        word,
        Matching::Value,
    )
}

/// clap lists a subcommand alias as its own row with no help, while the
/// described row already names it as `[default alias: NAME]`.
fn without_bare_aliases(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let aliases: Vec<String> = candidates
        .iter()
        .filter_map(|candidate| {
            let (_, rest) = candidate.description.split_once("[default alias: ")?;
            Some(rest.split_once(']')?.0.to_owned())
        })
        .collect();
    candidates
        .into_iter()
        .filter(|candidate| {
            !candidate.description.is_empty() || !aliases.contains(&candidate.value)
        })
        .collect()
}
