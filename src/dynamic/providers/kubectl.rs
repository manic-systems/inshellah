// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::shared::{
        DynCtx,
        Words,
        run,
    },
};
pub(super) fn complete(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if !words.at_operand(ctx) {
        return None;
    }
    let sub = words.arg(0);
    let resource_verbs = [
        "get", "describe", "delete", "edit", "scale", "annotate", "label",
    ];
    if resource_verbs.contains(&sub) && words.arg_count() >= 2 {
        return kubectl_names(words.arg(1), words, ctx);
    }
    if matches!(sub, "logs" | "exec" | "port-forward") && words.arg_count() >= 1 {
        return kubectl_names("pods", words, ctx);
    }
    if sub == "rollout"
        && words.arg_count() >= 3
        && matches!(
            words.arg(1),
            "history" | "pause" | "restart" | "resume" | "status" | "undo"
        )
    {
        return kubectl_names(words.arg(2), words, ctx);
    }
    None
}

pub struct KubeScope {
    pub args: Vec<String>,
    pub all:  bool,
}

pub fn kubectl_scope(words: &Words) -> KubeScope {
    if words.has(&["-A", "--all-namespaces"]) {
        return KubeScope {
            args: vec!["--all-namespaces".into()],
            all:  true,
        };
    }
    let args = words
        .value(&["-n", "--namespace"])
        .map(|namespace| vec!["-n".to_owned(), namespace.to_owned()])
        .unwrap_or_default();
    KubeScope { args, all: false }
}

fn cluster_args(words: &Words) -> Vec<String> {
    ["--context", "--kubeconfig"]
        .into_iter()
        .filter_map(|flag| {
            words
                .value(&[flag])
                .map(|value| [flag.to_owned(), value.to_owned()])
        })
        .flatten()
        .collect()
}

fn kubectl_names(kind: &str, words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if kind.is_empty() {
        return None;
    }
    let scope = kubectl_scope(words);
    let columns = if scope.all {
        "custom-columns=NAMESPACE:.metadata.namespace,NAME:.metadata.name"
    } else {
        "custom-columns=NAME:.metadata.name"
    };
    let mut args: Vec<String> = vec![ctx.bin("kubectl"), "get".into(), kind.into()];
    args.extend(cluster_args(words));
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
