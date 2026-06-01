// SPDX-License-Identifier: EUPL-1.2
use super::shared::{Candidate, DynCtx};

pub(crate) mod adb;
pub(crate) mod containers;
pub(crate) mod git;
pub(crate) mod jj;
pub(crate) mod kubectl;
pub(crate) mod nix;
pub(crate) mod process;
pub(crate) mod project;
pub(crate) mod ssh;
pub(crate) mod systemd;

type CompleteFn = fn(&[String], &DynCtx<'_>) -> Option<Vec<Candidate>>;

/// where a provider sits relative to static completion. `Value` providers
/// (adb selectors) preempt static flags; `Fallback` providers answer only
/// when static completion produced nothing.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Value,
    Fallback,
}

trait Provider {
    fn commands(&self) -> &'static [&'static str];
    fn kind(&self) -> Kind;
    fn complete(&self, spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>>;
}

struct FunctionProvider {
    commands: &'static [&'static str],
    kind: Kind,
    complete: CompleteFn,
}

impl Provider for FunctionProvider {
    fn commands(&self) -> &'static [&'static str] {
        self.commands
    }

    fn kind(&self) -> Kind {
        self.kind
    }

    fn complete(&self, spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
        (self.complete)(spans, ctx)
    }
}

const PROVIDERS: &[FunctionProvider] = &[
    FunctionProvider {
        commands: &["adb"],
        kind: Kind::Value,
        complete: adb::complete,
    },
    FunctionProvider {
        commands: &["nix"],
        kind: Kind::Fallback,
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
        kind: Kind::Fallback,
        complete: systemd::complete,
    },
    FunctionProvider {
        commands: &["ssh", "scp", "sftp"],
        kind: Kind::Fallback,
        complete: ssh::complete,
    },
    FunctionProvider {
        commands: &["docker", "podman"],
        kind: Kind::Fallback,
        complete: containers::complete,
    },
    FunctionProvider {
        commands: &["kubectl"],
        kind: Kind::Fallback,
        complete: kubectl::complete,
    },
    FunctionProvider {
        commands: &["git"],
        kind: Kind::Fallback,
        complete: git::complete,
    },
    FunctionProvider {
        commands: &["jj"],
        kind: Kind::Fallback,
        complete: jj::complete,
    },
    FunctionProvider {
        commands: &["npm", "pnpm", "yarn", "make", "just", "cargo"],
        kind: Kind::Fallback,
        complete: project::complete,
    },
    FunctionProvider {
        commands: &["kill", "pkill"],
        kind: Kind::Fallback,
        complete: process::complete,
    },
];

fn provider_for(cmd: &str, kind: Kind) -> Option<&'static FunctionProvider> {
    PROVIDERS
        .iter()
        .find(|provider| provider.kind() == kind && provider.commands().contains(&cmd))
}

/// handoff dispatch: fallback providers, called when static completion was
/// empty. value providers (adb) are excluded here; they answer via
/// `value_completions` before static runs.
pub(crate) fn dispatch(spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
    let cmd = spans.first()?.as_str();
    provider_for(cmd, Kind::Fallback).and_then(|provider| provider.complete(spans, ctx))
}

/// preempt dispatch: value providers (adb) that answer before static flag
/// completion. `Some` (even empty) suppresses static; `None` falls through.
pub(crate) fn value_completions(spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
    let cmd = spans.first()?.as_str();
    provider_for(cmd, Kind::Value).and_then(|provider| provider.complete(spans, ctx))
}
