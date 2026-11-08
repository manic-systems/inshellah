// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::{
        Candidate,
        directory_candidates,
    },
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
type FilesFn = fn(&Words<'_>, &DynCtx<'_>) -> Files;
type AnswerFn = fn(&Words<'_>, &DynCtx<'_>) -> Option<Answer>;

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
    fn matching(&self) -> Matching;
    fn complete(&self, spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>>;
    fn answer(&self, spans: &[String], ctx: &DynCtx<'_>) -> Option<Answer>;
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
    /// narrows `files` for a slot that can never be a path.
    files_at:    Option<FilesFn>,
    /// flags whose value is a directory, which the engine lists itself.
    dir_flags:   &'static [&'static str],
    /// replaces `complete` and `files_at` for a backend that says itself
    /// whether a path fits.
    answer_fn:   Option<AnswerFn>,
}

impl Provider for FunctionProvider {
    fn commands(&self) -> &'static [&'static str] {
        self.commands
    }

    fn kind(&self) -> Kind {
        self.kind
    }

    fn matching(&self) -> Matching {
        self.matching
    }

    fn complete(&self, spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
        (self.complete)(&self.words(spans, ctx), ctx)
    }

    fn answer(&self, spans: &[String], ctx: &DynCtx<'_>) -> Option<Answer> {
        let words = self.words(spans, ctx);
        if let Some(answer) = self.answer_fn {
            return answer(&words, ctx);
        }
        if let Some(candidates) = self.directories(&words, ctx) {
            return Some(Answer {
                candidates,
                files: Files::Replace,
            });
        }
        let candidates = (self.complete)(&words, ctx)?;
        let files = self.files_at.map_or(self.files, |at| at(&words, ctx));
        Some(Answer { candidates, files })
    }
}

impl FunctionProvider {
    fn directories(&self, words: &Words<'_>, ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
        let flag = words.value_slot(ctx)?;
        let global = self.globals == Globals::Anywhere || words.arg_count() == 0;
        let attached = words
            .spans()
            .last()
            .is_some_and(|last| last.starts_with("--") && last.contains('='));
        let typed = words.current();
        if !self.dir_flags.contains(&flag) || !global || typed.starts_with('$') {
            return None;
        }
        if attached && typed.starts_with('~') {
            return None;
        }
        Some(directory_candidates(typed, ctx.limit))
    }

    fn words<'span>(&self, spans: &'span [String], ctx: &DynCtx<'_>) -> Words<'span> {
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
        Words::parse(spans, takes_value, is_switch)
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
        files_at:    None,
        dir_flags:   &[],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["nix"],
        kind:        Kind::Fallback,
        files:       Files::Alongside,
        matching:    Matching::Value,
        value_flags: &[],
        globals:     Globals::Leading,
        complete:    nix::complete,
        files_at:    None,
        dir_flags:   &[],
        answer_fn:   Some(nix::answer),
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
        files_at:    None,
        dir_flags:   &[],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["ssh", "sftp"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &["-J", "-F"],
        globals:     Globals::Leading,
        complete:    ssh::complete,
        files_at:    None,
        dir_flags:   &[],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["scp"],
        kind:        Kind::Fallback,
        files:       Files::Alongside,
        matching:    Matching::Value,
        value_flags: &["-J", "-F"],
        globals:     Globals::Anywhere,
        complete:    ssh::complete,
        files_at:    None,
        dir_flags:   &[],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["docker"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: containers::SELECTOR_FLAGS,
        globals:     Globals::Leading,
        complete:    containers::complete,
        files_at:    None,
        dir_flags:   &[],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["podman"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: containers::SELECTOR_FLAGS,
        globals:     Globals::Anywhere,
        complete:    containers::complete,
        files_at:    None,
        dir_flags:   &[],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["kubectl"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: kubectl::VALUE_FLAGS,
        globals:     Globals::Anywhere,
        complete:    kubectl::complete,
        files_at:    None,
        dir_flags:   &[],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["git"],
        kind:        Kind::Fallback,
        files:       Files::Alongside,
        matching:    Matching::Value,
        value_flags: &["-C", "-c", "--git-dir", "--work-tree", "--namespace"],
        globals:     Globals::Leading,
        complete:    git::complete,
        files_at:    Some(git::files),
        dir_flags:   &["-C", "--git-dir", "--work-tree"],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["jj"],
        kind:        Kind::Value,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &[],
        globals:     Globals::Leading,
        complete:    jj::complete,
        files_at:    None,
        dir_flags:   &[],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["npm", "pnpm", "yarn"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &["--prefix", "-C", "--dir", "--cwd"],
        globals:     Globals::Anywhere,
        complete:    project::complete,
        files_at:    None,
        dir_flags:   &["--prefix", "-C", "--dir", "--cwd"],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["make"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &["-C", "--directory", "-f", "--file", "--makefile"],
        globals:     Globals::Anywhere,
        complete:    project::complete,
        files_at:    None,
        dir_flags:   &["-C", "--directory"],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["just"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &["-f", "--justfile", "-d", "--working-directory"],
        globals:     Globals::Anywhere,
        complete:    project::complete,
        files_at:    None,
        dir_flags:   &["-d", "--working-directory"],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["cargo"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: &["-C", "--manifest-path"],
        globals:     Globals::Anywhere,
        complete:    project::complete,
        files_at:    None,
        dir_flags:   &["-C"],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["zig"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::Value,
        value_flags: project::ZIG_BUILD_FLAGS,
        globals:     Globals::Anywhere,
        complete:    project::complete,
        files_at:    None,
        dir_flags:   &[],
        answer_fn:   None,
    },
    FunctionProvider {
        commands:    &["kill", "pkill"],
        kind:        Kind::Fallback,
        files:       Files::Replace,
        matching:    Matching::ValueOrDescription,
        value_flags: &[],
        globals:     Globals::Anywhere,
        complete:    process::complete,
        files_at:    None,
        dir_flags:   &[],
        answer_fn:   None,
    },
];

fn provider_for(cmd: &str, kind: Kind) -> Option<&'static FunctionProvider> {
    PROVIDERS
        .iter()
        .find(|provider| provider.kind() == kind && provider.commands().contains(&cmd))
}

/// whether the provider for `cmd` knows `flag` to take the next word, where
/// `leading` is true before the subcommand.
pub fn takes_value(cmd: &str, flag: &str, leading: bool) -> bool {
    PROVIDERS
        .iter()
        .find(|provider| provider.commands().contains(&cmd))
        .is_some_and(|provider| {
            provider.value_flags.contains(&flag)
                && (leading || provider.globals == Globals::Anywhere)
        })
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
    provider.answer(spans, ctx)
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
