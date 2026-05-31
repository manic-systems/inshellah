// SPDX-License-Identifier: EUPL-1.2
use super::super::shared::{Candidate, DynCtx, run};
const DOCKER_CONTAINER_VERBS: &[&str] = &[
    "exec", "logs", "inspect", "start", "stop", "restart", "rm", "kill", "attach", "cp", "top",
    "wait", "pause", "unpause", "port", "commit", "diff", "export",
];
const DOCKER_IMAGE_VERBS: &[&str] = &[
    "run", "rmi", "tag", "push", "pull", "history", "save", "create",
];

pub(super) fn complete(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let cmd = ctx.bin(spans[0].as_str());
    let sub = spans.get(1).map(String::as_str).unwrap_or("");
    if DOCKER_CONTAINER_VERBS.contains(&sub) {
        let mut args = vec![cmd, "ps".into()];
        args.extend(ctx.limit_args("--last"));
        args.extend(
            ["--format", "{{.Names}}\t{{.Image}}"]
                .into_iter()
                .map(str::to_string),
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
        let args: Vec<String> = vec![
            cmd,
            "images".into(),
            "--format".into(),
            "{{.Repository}}:{{.Tag}}\t{{.Size}}".into(),
        ];
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
