// SPDX-License-Identifier: EUPL-1.2
use crate::complete::Candidate;
use crate::dynamic::shared::{DynCtx, run};
pub(super) fn complete(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if !ctx.slot.positional() {
        return None;
    }
    let sub = spans.get(1).map_or("", String::as_str);
    let resource_verbs = [
        "get", "describe", "delete", "edit", "scale", "annotate", "label",
    ];
    if resource_verbs.contains(&sub) && spans.len() >= 4 {
        let kind = spans.get(2).map_or("", String::as_str);
        return kubectl_names(kind, spans, ctx);
    }
    if (sub == "logs" || sub == "exec" || sub == "port-forward") && spans.len() >= 3 {
        return kubectl_names("pods", spans, ctx);
    }
    if sub == "rollout" && spans.len() >= 5 {
        let action = spans.get(2).map_or("", String::as_str);
        let kind = spans.get(3).map_or("", String::as_str);
        if matches!(
            action,
            "history" | "pause" | "restart" | "resume" | "status" | "undo"
        ) {
            return kubectl_names(kind, spans, ctx);
        }
    }
    None
}

pub struct KubeScope {
    pub args: Vec<String>,
    pub all: bool,
}

pub fn kubectl_scope(spans: &[String]) -> KubeScope {
    let all_namespaces = spans
        .iter()
        .any(|span| span == "-A" || span == "--all-namespaces");
    if all_namespaces {
        return KubeScope {
            args: vec!["--all-namespaces".into()],
            all: true,
        };
    }
    let mut namespace: Option<String> = None;
    let mut i = 0_usize;
    while i < spans.len() {
        let span = &spans[i];
        if let Some(rest) = span.strip_prefix("--namespace=") {
            namespace = Some(rest.to_owned());
        } else if (span == "-n" || span == "--namespace") && i + 1 < spans.len() {
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

fn cluster_args(spans: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    let mut iter = spans.iter();
    while let Some(span) = iter.next() {
        let (flag, inline) = match span.split_once('=') {
            Some((flag, value)) => (flag, Some(value)),
            None => (span.as_str(), None),
        };
        if !matches!(flag, "--context" | "--kubeconfig") {
            continue;
        }
        if let Some(value) = inline
            .or_else(|| iter.next().map(String::as_str))
            .filter(|value| !value.is_empty())
        {
            args.extend([flag.to_owned(), value.to_owned()]);
        }
    }
    args
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
    args.extend(cluster_args(spans));
    args.extend(scope.args.iter().cloned());
    args.extend(
        ["--no-headers", "-o", columns]
            .into_iter()
            .map(str::to_owned),
    );
    let out = run(&args, ctx)?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if scope.all {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            candidates.push(Candidate::new(parts[1], format!("{kind} in {}", parts[0])));
        } else {
            candidates.push(Candidate::new(trimmed, kind));
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}
