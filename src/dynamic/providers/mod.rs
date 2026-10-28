// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::{
        Answer,
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
            self.value_flags.contains(&flag)
                || ctx.value_flags.iter().any(|known| known == flag)
                || ends_in_value_flag(flag, short_value_flags(ctx.cmd_name))
        });
        (self.complete)(&words, ctx)
    }
}

/// short flags that take a value when they end a cluster like `-fNL`.
fn short_value_flags(cmd: &str) -> &'static str {
    match cmd {
        "ssh" => "BbcDEeFIiJLlmOopPQRSWw",
        "scp" => "cDFiJloPSX",
        "sftp" => "BbcDEFIiJloPRSsX",
        "journalctl" => "DFMSUcgopstu",
        _ => "",
    }
}

fn ends_in_value_flag(word: &str, value_flags: &str) -> bool {
    let Some(cluster) = word
        .strip_prefix('-')
        .filter(|cluster| !cluster.starts_with('-'))
    else {
        return false;
    };
    cluster
        .char_indices()
        .find(|(_, flag)| value_flags.contains(*flag))
        .is_some_and(|(at, flag)| at + flag.len_utf8() == cluster.len())
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
            "--job-mode",
            "-S",
            "--since",
            "-U",
            "--until",
            "-F",
            "--field",
            "-D",
            "--directory",
            "--file",
            "-g",
            "--grep",
            "-c",
            "--cursor",
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
        value_flags: containers::GLOBAL_VALUE_FLAGS,
        complete:    containers::complete,
    },
    FunctionProvider {
        commands:    &["kubectl"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: kubectl::GLOBAL_VALUE_FLAGS,
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
        commands:    &["npm", "pnpm", "yarn", "make", "just", "cargo"],
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
        commands:    &["zig"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &[
            "-p",
            "--prefix",
            "--prefix-lib-dir",
            "--prefix-exe-dir",
            "--prefix-include-dir",
            "--build-file",
            "--build-runner",
            "--cache-dir",
            "--global-cache-dir",
            "--zig-lib-dir",
            "--color",
            "--debounce",
            "--debug-log",
            "--error-style",
            "--host",
            "--libc",
            "--libc-runtimes",
            "--maxrss",
            "--multiline-errors",
            "--search-prefix",
            "--seed",
            "--summary",
            "--sysroot",
            "--system",
            "--test-timeout",
        ],
        complete:    project::complete,
    },
    FunctionProvider {
        commands:    &["kill", "pkill"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::ValueOrDescription,
        value_flags: &[
            "-s",
            "--signal",
            "-q",
            "--queue",
            "--timeout",
            "-u",
            "--euid",
            "-U",
            "--uid",
            "-g",
            "--pgroup",
            "-G",
            "--group",
            "-t",
            "--terminal",
            "-P",
            "--parent",
            "-p",
            "--pid",
            "-O",
            "--older",
            "-r",
            "--runstates",
            "-F",
            "--pidfile",
            "-c",
            "--pidns",
            "--ns",
            "--nslist",
        ],
        complete:    process::complete,
    },
];

fn provider_for(cmd: &str, kind: Kind) -> Option<&'static FunctionProvider> {
    PROVIDERS
        .iter()
        .find(|provider| provider.kind() == kind && provider.commands().contains(&cmd))
}

pub fn kind_of(cmd: &str) -> Option<&'static str> {
    PROVIDERS
        .iter()
        .find(|provider| provider.commands().contains(&cmd))
        .map(|provider| {
            match provider.kind() {
                Kind::Value => "value provider",
                Kind::Fallback => "fallback provider",
            }
        })
}

/// fallback providers, called when static completion was empty. value providers
/// answer earlier via `value_completions`.
pub fn dispatch(spans: &[String], ctx: &DynCtx<'_>) -> Option<Answer> {
    let cmd = spans.first()?.as_str();
    let provider = provider_for(cmd, Kind::Fallback)?;
    Some(Answer {
        candidates: provider.complete(spans, ctx)?,
        files:      provider.files(),
    })
}

/// a provider's own matching rule, plain value matching otherwise.
pub fn matching_of(cmd: &str) -> Matching {
    provider_for(cmd, Kind::Fallback).map_or(Matching::Value, Provider::matching)
}

/// value providers that answer before static flag completion. `Some` (even
/// empty) suppresses static; `None` falls through.
pub fn value_completions(spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
    let cmd = spans.first()?.as_str();
    let provider = provider_for(cmd, Kind::Value)?;
    provider.complete(spans, ctx)
}
