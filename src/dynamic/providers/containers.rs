// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::shared::{
        DynCtx,
        Words,
        run,
    },
};
const DOCKER_CONTAINER_VERBS: &[&str] = &[
    "exec", "logs", "inspect", "start", "stop", "restart", "rm", "kill", "attach", "cp", "top",
    "wait", "pause", "unpause", "port", "commit", "diff", "export",
];
const DOCKER_IMAGE_VERBS: &[&str] = &[
    "run", "rmi", "tag", "push", "pull", "history", "save", "create",
];
const ENDPOINT_FLAGS: &[&[&str]] = &[
    &["-H", "--host"],
    &["-c", "--context", "--connection"],
    &["--config"],
    &["--url"],
    &["--identity"],
];

pub(super) fn complete(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if !words.at_operand(ctx) {
        return None;
    }
    let mut base = vec![ctx.bin(words.command())];
    for aliases in ENDPOINT_FLAGS {
        if let Some((flag, value)) = words.global_flag(aliases) {
            base.extend([flag.to_owned(), value.to_owned()]);
        }
    }
    let sub = words.arg(0);
    if DOCKER_CONTAINER_VERBS.contains(&sub) {
        let mut args = base;
        args.push("ps".into());
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
            candidates.push(Candidate::new(name, image));
        }
        (!candidates.is_empty()).then_some(candidates)
    } else if DOCKER_IMAGE_VERBS.contains(&sub) {
        let mut args = base;
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
    } else {
        None
    }
}
