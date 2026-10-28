// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::shared::{
        DynCtx,
        Words,
        run,
    },
};

const RESOURCE_VERBS: &[&str] = &[
    "get", "describe", "delete", "edit", "scale", "annotate", "label",
];

const ROLLOUT_ACTIONS: &[&str] = &["history", "pause", "restart", "resume", "status", "undo"];

pub(super) const VALUE_FLAGS: &[&str] = &[
    "-n",
    "--namespace",
    "--context",
    "--kubeconfig",
    "--cluster",
    "--user",
    "--server",
    "-s",
];

pub(super) fn complete(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if !words.at_operand(ctx) || words.current().starts_with('-') {
        return None;
    }
    let verb = words.arg(0);
    let operands = words.arg_count().checked_sub(1)?;
    match verb {
        _ if RESOURCE_VERBS.contains(&verb) && operands >= 1 && !words.arg(1).contains('/') => {
            kubectl_names(words.arg(1), words, ctx, "")
        },
        "exec" if !words.past_dashes() && operands == 0 => kubectl_names("pods", words, ctx, ""),
        "logs" | "port-forward" if operands == 0 => kubectl_names("pods", words, ctx, ""),
        "cp" => {
            let typed = words.current();
            let bare =
                !typed.is_empty() && !typed.contains([':', '/']) && !typed.starts_with(['.', '~']);
            if bare {
                kubectl_names("pods", words, ctx, ":")
            } else {
                None
            }
        },
        "rollout"
            if operands == 2
                && ROLLOUT_ACTIONS.contains(&words.arg(1))
                && !words.arg(2).contains('/') =>
        {
            kubectl_names(words.arg(2), words, ctx, "")
        },
        _ => None,
    }
}

pub struct KubeScope {
    pub args: Vec<String>,
    pub all:  bool,
}

pub fn kubectl_scope(words: &Words) -> KubeScope {
    if words.has(&["-A", "--all-namespaces"])
        && words.value(&["-A", "--all-namespaces"]) != Some("false")
    {
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
    VALUE_FLAGS
        .iter()
        .filter(|flag| !matches!(**flag, "-n" | "--namespace"))
        .filter_map(|flag| {
            words
                .value(&[flag])
                .map(|value| [(*flag).to_owned(), value.to_owned()])
        })
        .flatten()
        .collect()
}

fn kubectl_names(kind: &str, words: &Words, ctx: &DynCtx, suffix: &str) -> Option<Vec<Candidate>> {
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
            candidates.push(Candidate::new(
                format!("{}{suffix}", parts[1]),
                format!("{kind} in {}", parts[0]),
            ));
        } else {
            candidates.push(Candidate::new(format!("{trimmed}{suffix}"), kind));
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}
