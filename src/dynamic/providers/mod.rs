// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::{
        Files,
        shared::{
            DynCtx,
            Matching,
            Words,
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

type CompleteFn = fn(&Words<'_>, &DynCtx<'_>) -> Option<Vec<Candidate>>;

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
    commands:    &'static [&'static str],
    kind:        Kind,
    files:       Files,
    matching:    Matching,
    /// global flags whose value is the next word, beyond what the cache knows.
    value_flags: &'static [&'static str],
    complete:    CompleteFn,
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
        let words = Words::parse(spans, |flag| {
            self.value_flags.contains(&flag) || ctx.value_flags.iter().any(|known| known == flag)
        });
        (self.complete)(&words, ctx)
    }
}

const PROVIDERS: &[FunctionProvider] = &[
    FunctionProvider {
        commands:    &["adb"],
        kind:        Kind::Value,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &[],
        complete:    adb::complete,
    },
    FunctionProvider {
        commands:    &["nix"],
        kind:        Kind::Fallback,
        files:       Files::Alongside,
        matching:    Matching::Value,
        value_flags: &[],
        complete:    nix::complete,
    },
    FunctionProvider {
        commands:    &[
            "systemctl",
            "journalctl",
            "coredumpctl",
            "loginctl",
            "machinectl",
            "networkctl",
        ],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &[
            "-H",
            "--host",
            "-M",
            "--machine",
            "-t",
            "--type",
            "-p",
            "--property",
            "--state",
            "-n",
            "--lines",
            "-o",
            "--output",
            "--root",
            "-s",
            "--signal",
            "--kill-whom",
            "-u",
            "--unit",
            "--user-unit",
        ],
        complete:    systemd::complete,
    },
    FunctionProvider {
        commands:    &["ssh", "sftp"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &[],
        complete:    ssh::complete,
    },
    FunctionProvider {
        commands:    &["scp"],
        kind:        Kind::Fallback,
        files:       Files::Alongside,
        matching:    Matching::Value,
        value_flags: &[],
        complete:    ssh::complete,
    },
    FunctionProvider {
        commands:    &["docker", "podman"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &[
            "-H",
            "--host",
            "-c",
            "--context",
            "--config",
            "--connection",
            "--url",
            "--identity",
            "-l",
            "--log-level",
        ],
        complete:    containers::complete,
    },
    FunctionProvider {
        commands:    &["kubectl"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &[
            "-n",
            "--namespace",
            "--context",
            "--kubeconfig",
            "--cluster",
            "--user",
            "-s",
            "--server",
            "-l",
            "--selector",
            "-o",
            "--output",
            "-f",
            "--filename",
            "-c",
            "--container",
        ],
        complete:    kubectl::complete,
    },
    FunctionProvider {
        commands:    &["git"],
        kind:        Kind::Fallback,
        files:       Files::Alongside,
        matching:    Matching::Value,
        value_flags: &["-C", "-c", "--git-dir", "--work-tree", "--namespace"],
        complete:    git::complete,
    },
    FunctionProvider {
        commands:    &["jj"],
        kind:        Kind::Value,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &[],
        complete:    jj::complete,
    },
    FunctionProvider {
        commands:    &["npm", "pnpm", "yarn", "make", "just", "cargo", "zig"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &[
            "-p",
            "--package",
            "--manifest-path",
            "-C",
            "--prefix",
            "--dir",
            "--cwd",
            "-f",
            "--file",
            "--makefile",
            "--directory",
            "--justfile",
            "-d",
            "--working-directory",
            "--build-file",
        ],
        complete:    project::complete,
    },
    FunctionProvider {
        commands:    &["kill", "pkill"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::ValueOrDescription,
        value_flags: &[],
        complete:    process::complete,
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
