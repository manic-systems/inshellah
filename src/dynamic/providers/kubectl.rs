// SPDX-License-Identifier: EUPL-1.2
use super::super::shared::{Candidate, DynCtx, run};
pub(super) fn complete(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let sub = spans.get(1).map(String::as_str).unwrap_or("");
    let resource_verbs = [
        "get", "describe", "delete", "edit", "scale", "annotate", "label",
    ];
    if resource_verbs.contains(&sub) && spans.len() >= 4 {
        let kind = spans.get(2).map(String::as_str).unwrap_or("");
        return kubectl_names(kind, spans, ctx);
    }
    if (sub == "logs" || sub == "exec" || sub == "port-forward") && spans.len() >= 3 {
        return kubectl_names("pods", spans, ctx);
    }
    if sub == "rollout" && spans.len() >= 5 {
        let action = spans.get(2).map(String::as_str).unwrap_or("");
        let kind = spans.get(3).map(String::as_str).unwrap_or("");
        if matches!(
            action,
            "history" | "pause" | "restart" | "resume" | "status" | "undo"
        ) {
            return kubectl_names(kind, spans, ctx);
        }
    }
    None
}

pub(crate) struct KubeScope {
    pub(crate) args: Vec<String>,
    pub(crate) all: bool,
}

pub(crate) fn kubectl_scope(spans: &[String]) -> KubeScope {
    let all_namespaces = spans.iter().any(|s| s == "-A" || s == "--all-namespaces");
    if all_namespaces {
        return KubeScope {
            args: vec!["--all-namespaces".into()],
            all: true,
        };
    }
    let mut namespace: Option<String> = None;
    let mut i = 0usize;
    while i < spans.len() {
        let s = &spans[i];
        if let Some(rest) = s.strip_prefix("--namespace=") {
            namespace = Some(rest.to_string());
        } else if (s == "-n" || s == "--namespace") && i + 1 < spans.len() {
            namespace = Some(spans[i + 1].clone());
            i += 1;
        }
        i += 1;
    }
    match namespace {
        Some(ns) if !ns.is_empty() => KubeScope {
            args: vec!["-n".into(), ns],
            all: false,
        },
        _ => KubeScope {
            args: Vec::new(),
            all: false,
        },
    }
}

fn kubectl_names(kind: &str, spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if kind.is_empty() || kind.starts_with('-') {
        return None;
    }
    let scope = kubectl_scope(spans);
    let columns = if scope.all {
        "custom-columns=NAMESPACE:.metadata.namespace,NAME:.metadata.name"
    } else {
        "custom-columns=NAME:.metadata.name"
    };
    let mut args: Vec<String> = vec![ctx.bin("kubectl"), "get".into(), kind.into()];
    args.extend(scope.args.iter().cloned());
    args.extend(
        ["--no-headers", "-o", columns]
            .into_iter()
            .map(str::to_string),
    );
    let out = run(&args, ctx)?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if scope.all {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            candidates.push(Candidate::new(parts[1], format!("{kind} in {}", parts[0])));
        } else {
            candidates.push(Candidate::new(line, kind));
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}
