// SPDX-License-Identifier: EUPL-1.2
//! `inshellah explain`.

use std::{
    io::{
        self,
        Write as _,
    },
    iter,
    path::{
        Path,
        PathBuf,
    },
};

use inshellah::{
    complete::{
        Candidate,
        Response,
    },
    config::Config,
    dynamic::provider_kind,
    store::{
        ExeIdentity,
        file_type_of,
        user_cache_age,
    },
};

use super::complete::decide;

pub fn run(
    spans: &[String],
    user_dir: &Path,
    system_dirs: &[PathBuf],
    mandirs: &[PathBuf],
    timeout_ms: u64,
    cfg: &Config,
) -> io::Result<()> {
    let decision = decide(spans, user_dir, system_dirs, mandirs, timeout_ms, cfg);
    let trace = &decision.trace;
    let mut out = io::stdout().lock();

    writeln!(out, "command   {}", trace.spans.join(" "))?;
    match trace.bin.as_deref() {
        Some(bin) => {
            match ExeIdentity::of(bin) {
                Some(identity) => writeln!(out, "binary    {}", identity.path().display())?,
                None => writeln!(out, "binary    {} (unreadable)", bin.display())?,
            }
        },
        None => writeln!(out, "binary    not on PATH")?,
    }
    for (dir, stored) in &trace.dropped {
        writeln!(
            out,
            "stale     {} came from {}",
            dir.display(),
            stored.path().display()
        )?;
    }
    if !trace.path.is_empty() {
        writeln!(out, "path      {}", trace.path.join(" "))?;
    }

    let dirs = iter::once(user_dir)
        .chain(system_dirs.iter().map(PathBuf::as_path))
        .filter(|dir| !trace.dropped.iter().any(|(stale, _)| stale == dir));
    match &trace.node {
        Some(node) => {
            let hit = dirs.into_iter().find_map(|dir| {
                file_type_of(&[dir.to_path_buf()], node).map(|source| (dir, source))
            });
            match hit {
                Some((dir, source)) => {
                    let age = (dir == user_dir)
                        .then(|| user_cache_age(user_dir, node))
                        .flatten()
                        .map_or_else(String::new, |age| {
                            format!(", {}h old", age.as_secs() / 3600)
                        });
                    writeln!(
                        out,
                        "cache     {node} from {} ({source}{age})",
                        dir.display()
                    )?;
                },
                None => writeln!(out, "cache     {node}")?,
            }
        },
        None if trace.bin.is_none() => {},
        None => writeln!(out, "cache     no entry")?,
    }

    if let Some(slot) = &trace.slot {
        writeln!(out, "slot      {slot}")?;
    }
    if let Some(cmd) = trace.spans.first() {
        match (provider_kind(cmd), trace.dynamic) {
            (Some(kind), Some(count)) => writeln!(out, "provider  {kind}, {count} candidate(s)")?,
            (Some(kind), None) => writeln!(out, "provider  {kind}, no answer")?,
            (None, _) => writeln!(out, "provider  none")?,
        }
    }

    let preview = |candidates: &[Candidate]| {
        candidates
            .iter()
            .take(5)
            .map(|candidate| candidate.value.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    match &decision.response {
        Response::Defer => writeln!(out, "answer    null")?,
        Response::Candidates(candidates) => {
            writeln!(
                out,
                "answer    {} candidate(s): {}",
                candidates.len(),
                preview(candidates)
            )?;
        },
        Response::WithFiles(candidates) => {
            writeln!(
                out,
                "answer    {} candidate(s) plus files: {}",
                candidates.len(),
                preview(candidates)
            )?;
        },
    }
    writeln!(out, "reason    {}", decision.reason)
}
