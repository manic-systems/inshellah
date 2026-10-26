// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::{
        Files,
        shared::{
            DynCtx,
            Matching,
        },
    },
};

pub mod adb;
pub mod containers;
pub mod git;
pub mod jj;
pub mod kubectl;
pub mod nix;
pub mod process;
pub mod project;
pub mod ssh;
pub mod systemd;

type CompleteFn = fn(&[String], &DynCtx<'_>) -> Option<Vec<Candidate>>;

/// where a provider sits relative to static completion. `Value` preempts static
/// flags; `Fallback` answers only when static produced nothing.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Value,
    Fallback,
}

trait Provider {
    fn commands(&self) -> &'static [&'static str];
    fn kind(&self) -> Kind;
    fn files(&self) -> Files;
    fn matching(&self) -> Matching;
    fn complete(&self, spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>>;
}

struct FunctionProvider {
    commands: &'static [&'static str],
    kind:     Kind,
    files:    Files,
    matching: Matching,
    complete: CompleteFn,
}

impl Provider for FunctionProvider {
    fn commands(&self) -> &'static [&'static str] {
        self.commands
    }

    fn kind(&self) -> Kind {
        self.kind
    }

    fn files(&self) -> Files {
        self.files
    }

    fn matching(&self) -> Matching {
        self.matching
    }

    fn complete(&self, spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
        (self.complete)(spans, ctx)
    }
}

const PROVIDERS: &[FunctionProvider] = &[
    FunctionProvider {
        commands: &["adb"],
        kind:     Kind::Value,
        files:    Files::Replace,
        matching: Matching::Value,
        complete: adb::complete,
    },
    FunctionProvider {
        commands: &["nix"],
        kind:     Kind::Fallback,
        files:    Files::Alongside,
        matching: Matching::Value,
        complete: nix::complete,
    },
    FunctionProvider {
        commands: &[
            "systemctl",
            "journalctl",
            "coredumpctl",
            "loginctl",
            "machinectl",
            "networkctl",
        ],
        kind:     Kind::Fallback,
        files:    Files::Replace,
        matching: Matching::Value,
        complete: systemd::complete,
    },
    FunctionProvider {
        commands: &["ssh", "sftp"],
        kind:     Kind::Fallback,
        files:    Files::Replace,
        matching: Matching::Value,
        complete: ssh::complete,
    },
    FunctionProvider {
        commands: &["scp"],
        kind:     Kind::Fallback,
        files:    Files::Alongside,
        matching: Matching::Value,
        complete: ssh::complete,
    },
    FunctionProvider {
        commands: &["docker", "podman"],
        kind:     Kind::Fallback,
        files:    Files::Replace,
        matching: Matching::Value,
        complete: containers::complete,
    },
    FunctionProvider {
        commands: &["kubectl"],
        kind:     Kind::Fallback,
        files:    Files::Replace,
        matching: Matching::Value,
        complete: kubectl::complete,
    },
    FunctionProvider {
        commands: &["git"],
        kind:     Kind::Fallback,
        files:    Files::Alongside,
        matching: Matching::Value,
        complete: git::complete,
    },
    FunctionProvider {
        commands: &["jj"],
        kind:     Kind::Value,
        files:    Files::Replace,
        matching: Matching::Value,
        complete: jj::complete,
    },
    FunctionProvider {
        commands: &["npm", "pnpm", "yarn", "make", "just", "cargo", "zig"],
        kind:     Kind::Fallback,
        files:    Files::Replace,
        matching: Matching::Value,
        complete: project::complete,
    },
    FunctionProvider {
        commands: &["kill", "pkill"],
        kind:     Kind::Fallback,
        files:    Files::Replace,
        matching: Matching::ValueOrDescription,
        complete: process::complete,
    },
];

fn provider_for(cmd: &str, kind: Kind) -> Option<&'static FunctionProvider> {
    PROVIDERS
        .iter()
        .find(|provider| provider.kind() == kind && provider.commands().contains(&cmd))
}

/// fallback providers, called when static completion was empty. value providers
/// answer earlier via `value_completions`.
pub fn dispatch(spans: &[String], ctx: &DynCtx<'_>) -> Option<(Vec<Candidate>, Files, Matching)> {
    let cmd = spans.first()?.as_str();
    let provider = provider_for(cmd, Kind::Fallback)?;
    let candidates = provider.complete(spans, ctx)?;
    Some((candidates, provider.files(), provider.matching()))
}

/// value providers that answer before static flag completion. `Some` (even
/// empty) suppresses static; `None` falls through.
pub fn value_completions(spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
    let cmd = spans.first()?.as_str();
    let provider = provider_for(cmd, Kind::Value)?;
    provider.complete(spans, ctx)
}
