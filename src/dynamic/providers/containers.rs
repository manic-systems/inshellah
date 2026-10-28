// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::shared::{
        DynCtx,
        Words,
        run,
    },
};

const REPEATED_CONTAINER_VERBS: &[&str] = &[
    "inspect", "start", "stop", "restart", "rm", "kill", "wait", "pause", "unpause",
];
const SINGLE_CONTAINER_VERBS: &[&str] = &[
    "exec", "logs", "attach", "top", "port", "commit", "diff", "export",
];
const REPEATED_IMAGE_VERBS: &[&str] = &["rmi", "save"];
const SINGLE_IMAGE_VERBS: &[&str] = &["run", "create", "tag", "push", "pull", "history"];

pub(super) const SELECTOR_FLAGS: &[&str] = &[
    "-H",
    "--host",
    "-c",
    "--context",
    "--connection",
    "--config",
    "--url",
    "--tlscacert",
    "--tlscert",
    "--tlskey",
    "--identity",
    "--root",
    "--runroot",
];

const FORWARDED_BOOLEAN_FLAGS: &[&str] = &[
    "-r",
    "--remote",
    "--tls",
    "--tlsverify",
    "--transient-store",
];

pub(super) fn complete(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if !words.at_operand(ctx) || words.current().starts_with('-') {
        return None;
    }
    let (verb, skipped) = if words.arg(0) == "container" && words.arg_count() > 1 {
        (words.arg(1), 2)
    } else {
        (words.arg(0), 1)
    };
    let operands = words.arg_count().checked_sub(skipped)?;
    let podman = words.command() == "podman";
    if REPEATED_CONTAINER_VERBS.contains(&verb) || (podman && verb == "logs") {
        return container_names(words, ctx, "");
    }
    if SINGLE_CONTAINER_VERBS.contains(&verb) {
        return (operands == 0)
            .then(|| container_names(words, ctx, ""))
            .flatten();
    }
    if verb == "cp" {
        let typed = words.current();
        let bare =
            !typed.is_empty() && !typed.contains([':', '/']) && !typed.starts_with(['.', '~']);
        return bare.then(|| container_names(words, ctx, ":")).flatten();
    }
    if REPEATED_IMAGE_VERBS.contains(&verb) || (podman && verb == "pull") {
        return image_names(words, ctx);
    }
    if SINGLE_IMAGE_VERBS.contains(&verb) {
        return (operands == 0).then(|| image_names(words, ctx)).flatten();
    }
    None
}

fn base_args(words: &Words, ctx: &DynCtx) -> Vec<String> {
    // podman globals are cobra persistent flags, so one typed after the verb
    // still selects the backend unless the verb defines that flag itself.
    let persistent = |flag: &str| {
        words.command() == "podman" && !ctx.value_flags.iter().any(|local| local == flag)
    };
    let mut args = vec![ctx.bin(words.command())];
    for flag in SELECTOR_FLAGS {
        let typed = words
            .global_value(&[flag])
            .or_else(|| persistent(flag).then(|| words.value(&[flag])).flatten());
        if let Some(value) = typed {
            args.extend([(*flag).to_owned(), value.to_owned()]);
        }
    }
    args.extend(
        FORWARDED_BOOLEAN_FLAGS
            .iter()
            .filter(|flag| words.global_has(&[flag]) || (persistent(flag) && words.has(&[flag])))
            .map(|flag| (*flag).to_owned()),
    );
    args
}

fn container_names(words: &Words, ctx: &DynCtx, suffix: &str) -> Option<Vec<Candidate>> {
    let mut args = base_args(words, ctx);
    args.extend(["ps".into(), "-a".into()]);
    args.extend(ctx.limit_args("--last", words.current()));
    args.extend(
        ["--format", "{{.Names}}\t{{.Image}}"]
            .into_iter()
            .map(str::to_owned),
    );
    let out = run(&args, ctx)?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let mut parts = line.splitn(2, '\t');
        let Some(name) = parts.next() else { continue };
        let image = parts.next().unwrap_or("");
        candidates.push(Candidate::new(format!("{name}{suffix}"), image));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn image_names(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args = base_args(words, ctx);
    args.extend(
        ["images", "--format", "{{.Repository}}:{{.Tag}}\t{{.Size}}"]
            .into_iter()
            .map(str::to_owned),
    );
    let out = run(&args, ctx)?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let mut parts = line.splitn(2, '\t');
        let Some(repo) = parts.next() else { continue };
        let size = parts.next().unwrap_or("");
        if repo.ends_with(":<none>") {
            continue;
        }
        candidates.push(Candidate::new(repo, size));
    }
    (!candidates.is_empty()).then_some(candidates)
}
