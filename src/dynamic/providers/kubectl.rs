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

pub(super) const GLOBAL_VALUE_FLAGS: &[&str] = &[
    "--as",
    "--as-group",
    "--as-uid",
    "--as-user-extra",
    "--cache-dir",
    "--certificate-authority",
    "--client-certificate",
    "--client-key",
    "--cluster",
    "--context",
    "--kubeconfig",
    "--kuberc",
    "--namespace",
    "--password",
    "--profile",
    "--profile-output",
    "--proxy-url",
    "--request-timeout",
    "--server",
    "--tls-server-name",
    "--token",
    "--user",
    "--username",
    "--v",
    "--vmodule",
    "-n",
    "-s",
    "-v",
];

const COMMAND_VALUE_FLAGS: &[&str] = &[
    "--address",
    "--chunk-size",
    "--container",
    "--current-replicas",
    "--field-manager",
    "--field-selector",
    "--filename",
    "--grace-period",
    "--limit-bytes",
    "--max-log-requests",
    "--output",
    "--pod-running-timeout",
    "--replicas",
    "--resource-version",
    "--retries",
    "--selector",
    "--since",
    "--since-time",
    "--sort-by",
    "--subresource",
    "--tail",
    "--template",
    "--timeout",
    "-l",
    "-o",
];

const FORWARDED_VALUE_FLAGS: &[&str] = &[
    "--as",
    "--as-group",
    "--as-uid",
    "--as-user-extra",
    "--cache-dir",
    "--certificate-authority",
    "--client-certificate",
    "--client-key",
    "--cluster",
    "--context",
    "--kubeconfig",
    "--kuberc",
    "--password",
    "--proxy-url",
    "--request-timeout",
    "--server",
    "--tls-server-name",
    "--token",
    "--user",
    "--username",
    "-s",
];

const FORWARDED_BOOLEAN_FLAGS: &[&str] = &[
    "--disable-compression",
    "--insecure-skip-tls-verify",
    "--match-server-version",
];

fn takes_value(verb: &str, flag: &str) -> bool {
    GLOBAL_VALUE_FLAGS.contains(&flag)
        || COMMAND_VALUE_FLAGS.contains(&flag)
        || match flag {
            "-c" => matches!(verb, "exec" | "logs" | "cp"),
            "-f" => RESOURCE_VERBS.contains(&verb),
            _ => false,
        }
}

pub(super) fn complete(first_pass: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if !ctx.slot.positional() {
        return None;
    }
    let verb = first_pass.arg(0);
    let words = Words::parse(first_pass.spans(), |flag| takes_value(verb, flag));
    if !words.at_operand(ctx) || words.current().starts_with('-') {
        return None;
    }
    let operands = words.arg_count().checked_sub(1)?;
    match verb {
        _ if RESOURCE_VERBS.contains(&verb) && operands >= 1 && !words.arg(1).contains('/') => {
            kubectl_names(words.arg(1), &words, ctx, "")
        },
        "exec" if !words.past_dashes() && operands == 0 => kubectl_names("pods", &words, ctx, ""),
        "logs" | "port-forward" if operands == 0 => kubectl_names("pods", &words, ctx, ""),
        "cp" => {
            let typed = words.current();
            let bare =
                !typed.is_empty() && !typed.contains([':', '/']) && !typed.starts_with(['.', '~']);
            if bare {
                kubectl_names("pods", &words, ctx, ":")
            } else {
                None
            }
        },
        "rollout"
            if operands == 2
                && ROLLOUT_ACTIONS.contains(&words.arg(1))
                && !words.arg(2).contains('/') =>
        {
            kubectl_names(words.arg(2), &words, ctx, "")
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
    let mut args = Vec::new();
    for flag in FORWARDED_VALUE_FLAGS {
        if let Some(value) = words.value(&[flag]) {
            args.extend([(*flag).to_owned(), value.to_owned()]);
        }
    }
    args.extend(
        FORWARDED_BOOLEAN_FLAGS
            .iter()
            .filter(|flag| words.has(&[flag]))
            .map(|flag| (*flag).to_owned()),
    );
    args
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
        if ctx.limit > 0 && candidates.len() >= ctx.limit {
            break;
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}
