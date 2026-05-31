// SPDX-License-Identifier: EUPL-1.2
use super::shared::{Candidate, DynCtx};

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

trait Provider {
    fn commands(&self) -> &'static [&'static str];
    fn complete(&self, spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>>;
}

struct FunctionProvider {
    commands: &'static [&'static str],
    complete: CompleteFn,
}

impl Provider for FunctionProvider {
    fn commands(&self) -> &'static [&'static str] {
        self.commands
    }

    fn complete(&self, spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
        (self.complete)(spans, ctx)
    }
}

const PROVIDERS: &[FunctionProvider] = &[
    FunctionProvider {
        commands: &["nix"],
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
        complete: systemd::complete,
    },
    FunctionProvider {
        commands: &["ssh", "scp", "sftp"],
        complete: ssh::complete,
    },
    FunctionProvider {
        commands: &["docker", "podman"],
        complete: containers::complete,
    },
    FunctionProvider {
        commands: &["kubectl"],
        complete: kubectl::complete,
    },
    FunctionProvider {
        commands: &["git"],
        complete: git::complete,
    },
    FunctionProvider {
        commands: &["jj"],
        complete: jj::complete,
    },
    FunctionProvider {
        commands: &["npm", "pnpm", "yarn", "make", "just", "cargo"],
        complete: project::complete,
    },
    FunctionProvider {
        commands: &["kill", "pkill"],
        complete: process::complete,
    },
];

pub(crate) fn dispatch(spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
    let cmd = spans.first()?.as_str();
    PROVIDERS
        .iter()
        .find(|provider| provider.commands().contains(&cmd))
        .and_then(|provider| provider.complete(spans, ctx))
}
