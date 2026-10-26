// SPDX-License-Identifier: EUPL-1.2
//! completers a program ships, asked over its CLI library's protocol.

use std::{
    fmt::{
        self,
        Display,
        Formatter,
    },
    path::Path,
    str::FromStr,
};

use crate::{
    complete::Candidate,
    dynamic::{
        Files,
        shared::{
            DynCtx,
            run_with,
        },
    },
    indexer::probe::image_strings,
    store::ExeFact,
};

const COBRA_ERROR: u32 = 1;
const COBRA_NO_FILE_COMP: u32 = 4;
const COBRA_FILTER_FILE_EXT: u32 = 8;
const COBRA_FILTER_DIRS: u32 = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// `prog __complete ARGS...`, answered by spf13/cobra.
    Cobra,
    /// `COMPLETE=fish prog -- prog ARGS...`, answered by clap's `CompleteEnv`.
    Clap,
}

impl Display for Protocol {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cobra => "cobra",
            Self::Clap => "clap",
        })
    }
}

/// what a scan found, cached so a binary without a protocol is not rescanned.
#[derive(Clone, Copy)]
pub struct Detection(pub Option<Protocol>);

impl Display for Detection {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(protocol) => protocol.fmt(f),
            None => f.write_str("none"),
        }
    }
}

impl FromStr for Detection {
    type Err = ();

    fn from_str(name: &str) -> Result<Self, ()> {
        if name == "none" {
            return Ok(Self(None));
        }
        Protocol::ALL
            .into_iter()
            .find(|protocol| protocol.to_string() == name)
            .map(|protocol| Self(Some(protocol)))
            .ok_or(())
    }
}

impl ExeFact for Detection {
    const EXTENSION: &'static str = "protocol";
}

impl Protocol {
    const ALL: [Self; 2] = [Self::Cobra, Self::Clap];

    /// compiled-in strings that prove it, asking an unproven binary runs it.
    const fn signature(self) -> &'static [&'static str] {
        match self {
            Self::Cobra => &["spf13/cobra", "__complete"],
            Self::Clap => &["_CLAP_COMPLETE_INDEX"],
        }
    }

    /// the protocol `bin` answers, read from its image without running it.
    pub fn detect(bin: &Path) -> Option<Self> {
        let needles: Vec<&str> = Self::ALL
            .into_iter()
            .flat_map(Self::signature)
            .copied()
            .collect();
        let found = image_strings(bin, &needles);
        Self::ALL.into_iter().find(|protocol| {
            protocol
                .signature()
                .iter()
                .all(|needle| found.contains(*needle))
        })
    }

    pub(super) fn complete(
        self,
        spans: &[String],
        ctx: &DynCtx,
    ) -> Option<(Vec<Candidate>, Files)> {
        match self {
            Self::Cobra => cobra(spans, ctx),
            Self::Clap => clap(spans, ctx).map(|candidates| (candidates, Files::Replace)),
        }
    }
}

fn cobra(spans: &[String], ctx: &DynCtx) -> Option<(Vec<Candidate>, Files)> {
    let mut args = vec![ctx.bin(ctx.cmd_name), "__complete".to_owned()];
    args.extend_from_slice(spans.get(1..)?);

    let out = run_with(&args, ctx, |cmd| {
        cmd.env("COBRA_ACTIVE_HELP", "0");
    })?;

    let trimmed = out.trim_end();
    let (body, last) = trimmed.rsplit_once('\n').unwrap_or(("", trimmed));
    let directive: u32 = last.strip_prefix(':')?.parse().ok()?;
    if directive & (COBRA_ERROR | COBRA_FILTER_FILE_EXT | COBRA_FILTER_DIRS) != 0 {
        return None;
    }

    let candidates = body
        .lines()
        .filter(|line| !line.starts_with("_activeHelp_"))
        .filter_map(candidate)
        .collect();
    let files = if directive & COBRA_NO_FILE_COMP == 0 {
        Files::Alongside
    } else {
        Files::Replace
    };
    Some((candidates, files))
}

pub(super) fn clap(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args = vec![ctx.bin(ctx.cmd_name), "--".to_owned()];
    args.extend_from_slice(spans);

    let out = run_with(&args, ctx, |cmd| {
        cmd.env("COMPLETE", "fish");
    })?;

    let candidates: Vec<Candidate> = out.lines().filter_map(candidate).collect();
    (!candidates.is_empty()).then_some(candidates)
}

fn candidate(line: &str) -> Option<Candidate> {
    let (raw_value, description) = line.split_once('\t').unwrap_or((line, ""));
    let value = raw_value.trim_end();
    (!value.is_empty()).then(|| Candidate::new(value, description.trim()))
}
