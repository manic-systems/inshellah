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
        Answer,
        Files,
        Reply,
        shared::{
            DynCtx,
            run_ended,
            run_with,
        },
    },
    indexer::probe::image_strings,
    store::ExeFact,
    subprocess::{
        Run,
        run_quiet_ended,
    },
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

impl Detection {
    /// what `bin` answers, from its image and then a confirmation run.
    pub fn scan(bin: &Path, timeout_ms: u64) -> Option<Self> {
        let needles: Vec<&str> = Protocol::ALL
            .into_iter()
            .flat_map(Protocol::signature)
            .copied()
            .collect();
        let found = image_strings(bin, &needles);
        let linked = Protocol::ALL.into_iter().find(|protocol| {
            protocol
                .signature()
                .iter()
                .all(|needle| found.contains(*needle))
        });
        let Some(protocol) = linked else {
            return Some(Self(None));
        };
        let confirmed = protocol.confirm(bin, timeout_ms)?;
        Some(Self(confirmed.then_some(protocol)))
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

    /// `COMPLETE=fish prog --help` proves clap, None when that run failed.
    fn confirm(self, bin: &Path, timeout_ms: u64) -> Option<bool> {
        match self {
            Self::Cobra => Some(true),
            Self::Clap => {
                let args = [bin.to_string_lossy().into_owned(), "--help".to_owned()];
                let run = run_quiet_ended(&args, timeout_ms, |cmd| {
                    cmd.env("COMPLETE", "fish");
                });
                match run {
                    Run::Succeeded(out) => Some(out.contains("COMPLETE=fish")),
                    Run::Errored(out) => out.contains("COMPLETE=fish").then_some(true),
                    Run::Failed => None,
                }
            },
        }
    }

    pub(super) fn complete(self, spans: &[String], ctx: &DynCtx) -> Reply {
        match self {
            Self::Cobra => cobra(spans, ctx),
            Self::Clap => {
                clap(spans, ctx).map_or(Reply::Silent, |candidates| {
                    Reply::Answered(Answer {
                        candidates,
                        files: Files::Replace,
                    })
                })
            },
        }
    }
}

/// cobra ends each reply with a `:DIRECTIVE` line, anything else is foreign.
fn cobra(spans: &[String], ctx: &DynCtx) -> Reply {
    let mut args = vec![ctx.bin(ctx.cmd_name), "__complete".to_owned()];
    args.extend(spans.iter().skip(1).cloned());

    let out = match run_ended(&args, ctx, |cmd| {
        cmd.env("COBRA_ACTIVE_HELP", "0");
    }) {
        Run::Succeeded(out) => out,
        Run::Errored(_) | Run::Failed => return Reply::Silent,
    };

    let trimmed = out.trim_end();
    let (body, last) = trimmed.rsplit_once('\n').unwrap_or(("", trimmed));
    let Some(directive) = last
        .strip_prefix(':')
        .and_then(|code| code.parse::<u32>().ok())
    else {
        return Reply::Foreign;
    };
    if directive & (COBRA_ERROR | COBRA_FILTER_FILE_EXT | COBRA_FILTER_DIRS) != 0 {
        return Reply::Silent;
    }

    let candidates = body
        .lines()
        .filter(|line| !line.starts_with("_activeHelp_"))
        .filter_map(candidate)
        .collect();
    let files = if directive & COBRA_NO_FILE_COMP == 0 {
        Files::Alongside
    } else {
        Files::Never
    };
    Reply::Answered(Answer { candidates, files })
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
