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

/// where a command accepts its global flags. cobra persistent flags work after
/// the subcommand too, git and docker globals only before it.
#[derive(Clone, Copy, PartialEq)]
enum Globals {
    Leading,
    Anywhere,
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
    /// flags whose value is the next word, beyond what the cache knows. most
    /// are global flags, and zig lists its `zig build` flags here.
    value_flags: &'static [&'static str],
    globals:     Globals,
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
        let known = |flag: &str, flags: &[String]| flags.iter().any(|known| known == flag);
        let global_scope = |leading: bool| leading || self.globals == Globals::Anywhere;
        let takes_value = |flag: &str, leading: bool| {
            let global = self.value_flags.contains(&flag) || known(flag, ctx.flags.global_value);
            (known(flag, ctx.flags.value) && !leading) || (global && global_scope(leading))
        };
        let is_switch = |flag: &str, leading: bool| {
            (known(flag, ctx.flags.short) && !leading)
                || (known(flag, ctx.flags.global_short) && global_scope(leading))
        };
        let words = Words::parse(spans, takes_value, is_switch);
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
        globals:     Globals::Leading,
        complete:    adb::complete,
    },
    FunctionProvider {
        commands:    &["nix"],
        kind:        Kind::Fallback,
        files:       Files::Alongside,
        matching:    Matching::Value,
        value_flags: &[],
        globals:     Globals::Leading,
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
            "-u",
            "--unit",
            "--user-unit",
        ],
        globals:     Globals::Anywhere,
        complete:    systemd::complete,
    },
    FunctionProvider {
        commands:    &["ssh", "sftp"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &["-J", "-F"],
        globals:     Globals::Leading,
        complete:    ssh::complete,
    },
    FunctionProvider {
        commands:    &["scp"],
        kind:        Kind::Fallback,
        files:       Files::Alongside,
        matching:    Matching::Value,
        value_flags: &["-J", "-F"],
        globals:     Globals::Anywhere,
        complete:    ssh::complete,
    },
    FunctionProvider {
        commands:    &["docker"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: containers::SELECTOR_FLAGS,
        globals:     Globals::Leading,
        complete:    containers::complete,
    },
    FunctionProvider {
        commands:    &["podman"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: containers::SELECTOR_FLAGS,
        globals:     Globals::Anywhere,
        complete:    containers::complete,
    },
    FunctionProvider {
        commands:    &["kubectl"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: kubectl::VALUE_FLAGS,
        globals:     Globals::Anywhere,
        complete:    kubectl::complete,
    },
    FunctionProvider {
        commands:    &["git"],
        kind:        Kind::Fallback,
        files:       Files::Alongside,
        matching:    Matching::Value,
        value_flags: &["-C", "-c", "--git-dir", "--work-tree", "--namespace"],
        globals:     Globals::Leading,
        complete:    git::complete,
    },
    FunctionProvider {
        commands:    &["jj"],
        kind:        Kind::Value,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &[],
        globals:     Globals::Leading,
        complete:    jj::complete,
    },
    FunctionProvider {
        commands:    &["npm", "pnpm", "yarn"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &["--prefix", "-C", "--dir", "--cwd"],
        globals:     Globals::Anywhere,
        complete:    project::complete,
    },
    FunctionProvider {
        commands:    &["make"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &["-C", "--directory", "-f", "--file", "--makefile"],
        globals:     Globals::Anywhere,
        complete:    project::complete,
    },
    FunctionProvider {
        commands:    &["just"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &["-f", "--justfile", "-d", "--working-directory"],
        globals:     Globals::Anywhere,
        complete:    project::complete,
    },
    FunctionProvider {
        commands:    &["cargo"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &["-C", "--manifest-path"],
        globals:     Globals::Anywhere,
        complete:    project::complete,
    },
    FunctionProvider {
        commands:    &["zig"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: project::ZIG_BUILD_FLAGS,
        globals:     Globals::Anywhere,
        complete:    project::complete,
    },
    FunctionProvider {
        commands:    &["kill", "pkill"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::ValueOrDescription,
        value_flags: &[],
        globals:     Globals::Anywhere,
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
