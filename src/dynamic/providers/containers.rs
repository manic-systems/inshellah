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

pub(super) const GLOBAL_VALUE_FLAGS: &[&str] = &[
    "--config",
    "--cgroup-manager",
    "--cdi-spec-dir",
    "--connection",
    "--conmon",
    "--context",
    "--db-backend",
    "--events-backend",
    "--host",
    "--hooks-dir",
    "--identity",
    "--imagestore",
    "--log-level",
    "--module",
    "--namespace",
    "--network-cmd-path",
    "--network-config-dir",
    "--out",
    "--root",
    "--runroot",
    "--runtime",
    "--runtime-flag",
    "--ssh",
    "--storage-driver",
    "--storage-opt",
    "--tmpdir",
    "--tls-ca",
    "--tls-cert",
    "--tls-key",
    "--tlscacert",
    "--tlscert",
    "--tlskey",
    "--url",
    "--volumepath",
    "-H",
    "-c",
    "-l",
];

const COMMAND_VALUE_FLAGS: &[&str] = &[
    "--add-host",
    "--annotation",
    "--arch",
    "--authfile",
    "--author",
    "--blkio-weight",
    "--blkio-weight-device",
    "--cap-add",
    "--cap-drop",
    "--cert-dir",
    "--cgroup-conf",
    "--cgroup-parent",
    "--cgroupns",
    "--cgroups",
    "--change",
    "--chrootdirs",
    "--cidfile",
    "--conmon-pidfile",
    "--cpu-count",
    "--cpu-percent",
    "--cpu-period",
    "--cpu-quota",
    "--cpu-rt-period",
    "--cpu-rt-runtime",
    "--cpu-shares",
    "--cpus",
    "--cpuset-cpus",
    "--cpuset-mems",
    "--creds",
    "--decryption-key",
    "--detach-keys",
    "--device",
    "--device-cgroup-rule",
    "--device-read-bps",
    "--device-read-iops",
    "--device-write-bps",
    "--device-write-iops",
    "--dns",
    "--dns-option",
    "--dns-search",
    "--domainname",
    "--entrypoint",
    "--env",
    "--env-file",
    "--env-merge",
    "--expose",
    "--filter",
    "--format",
    "--gidmap",
    "--gpus",
    "--group-add",
    "--group-entry",
    "--health-cmd",
    "--health-interval",
    "--health-log-destination",
    "--health-max-log-count",
    "--health-max-log-size",
    "--health-on-failure",
    "--health-retries",
    "--health-start-interval",
    "--health-start-period",
    "--health-startup-cmd",
    "--health-startup-interval",
    "--health-startup-retries",
    "--health-startup-success",
    "--health-startup-timeout",
    "--health-timeout",
    "--hostname",
    "--hosts-file",
    "--hostuser",
    "--image-volume",
    "--init-ctr",
    "--init-path",
    "--io-maxbandwidth",
    "--io-maxiops",
    "--ip",
    "--ip6",
    "--ipc",
    "--isolation",
    "--label",
    "--label-file",
    "--link",
    "--link-local-ip",
    "--log-driver",
    "--log-opt",
    "--mac-address",
    "--memory",
    "--memory-reservation",
    "--memory-swap",
    "--memory-swappiness",
    "--message",
    "--mount",
    "--name",
    "--net",
    "--network",
    "--network-alias",
    "--oom-score-adj",
    "--os",
    "--output",
    "--passwd-entry",
    "--personality",
    "--pid",
    "--pidfile",
    "--pids-limit",
    "--platform",
    "--pod",
    "--pod-id-file",
    "--preserve-fd",
    "--preserve-fds",
    "--publish",
    "--pull",
    "--rdt-class",
    "--requires",
    "--restart",
    "--retry",
    "--retry-delay",
    "--runtime",
    "--sdnotify",
    "--seccomp-policy",
    "--secret",
    "--security-opt",
    "--shm-size",
    "--shm-size-systemd",
    "--signal",
    "--stop-signal",
    "--stop-timeout",
    "--subgidname",
    "--subuidname",
    "--sysctl",
    "--systemd",
    "--tmpfs",
    "--tz",
    "--uidmap",
    "--ulimit",
    "--umask",
    "--unsetenv",
    "--until",
    "--user",
    "--userns",
    "--uts",
    "--variant",
    "--volume",
    "--volume-driver",
    "--volumes-from",
    "--workdir",
];

const FORWARDED_VALUE_FLAGS: &[&[&str]] = &[
    &["-H", "--host"],
    &["-c", "--context", "--connection"],
    &["--config"],
    &["--db-backend"],
    &["--identity"],
    &["--imagestore"],
    &["--namespace"],
    &["--root"],
    &["--runroot"],
    &["--ssh"],
    &["--storage-driver"],
    &["--storage-opt"],
    &["--tls-ca", "--tlscacert"],
    &["--tls-cert", "--tlscert"],
    &["--tls-key", "--tlskey"],
    &["--url"],
    &["--volumepath"],
];

const FORWARDED_BOOLEAN_FLAGS: &[&str] = &["--remote", "--tls", "--tlsverify", "--transient-store"];

fn takes_value(command: &str, verb: &str, flag: &str) -> bool {
    GLOBAL_VALUE_FLAGS.contains(&flag)
        || COMMAND_VALUE_FLAGS.contains(&flag)
        || command_value_flag(command, verb, flag)
}

fn command_value_flag(command: &str, verb: &str, flag: &str) -> bool {
    let podman = command == "podman";
    match verb {
        "exec" => matches!(flag, "-e" | "-u" | "-w"),
        "run" | "create" => {
            matches!(
                flag,
                "-a" | "-c" | "-e" | "-h" | "-l" | "-m" | "-p" | "-u" | "-v" | "-w"
            ) || (podman && flag == "--timeout")
        },
        "start" => matches!(flag, "--checkpoint" | "--checkpoint-dir"),
        "inspect" => matches!(flag, "-f" | "--type") || (podman && flag == "-t"),
        "kill" => flag == "-s",
        "logs" => matches!(flag, "--since" | "--tail" | "--until") || (!podman && flag == "-n"),
        "commit" => {
            matches!(flag, "-a" | "-c" | "-m")
                || (podman && matches!(flag, "--config" | "--iidfile"))
        },
        "export" | "save" => flag == "-o",
        "stop" | "restart" => {
            flag == "-t" || (!podman && flag == "--timeout") || (podman && flag == "--time")
        },
        "wait" => podman && matches!(flag, "-i" | "--condition" | "--interval"),
        "pull" => {
            podman
                && matches!(
                    flag,
                    "--arch"
                        | "--authfile"
                        | "--cert-dir"
                        | "--creds"
                        | "--decryption-key"
                        | "--os"
                        | "--policy"
                        | "--retry"
                        | "--retry-delay"
                        | "--variant"
                )
        },
        _ => false,
    }
}

pub(super) fn complete(first_pass: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if !ctx.slot.positional() {
        return None;
    }
    let command = first_pass.command();
    let (verb, skipped) = if first_pass.arg(0) == "container" && first_pass.arg_count() > 1 {
        (first_pass.arg(1), 2)
    } else {
        (first_pass.arg(0), 1)
    };
    let words = Words::parse(first_pass.spans(), |flag| takes_value(command, verb, flag));
    if !words.at_operand(ctx) || words.current().starts_with('-') {
        return None;
    }
    let operands = words.arg_count().checked_sub(skipped)?;
    let podman = command == "podman";
    if REPEATED_CONTAINER_VERBS.contains(&verb) || (podman && verb == "logs") {
        return container_names(&words, ctx, "");
    }
    if SINGLE_CONTAINER_VERBS.contains(&verb) {
        return (operands == 0)
            .then(|| container_names(&words, ctx, ""))
            .flatten();
    }
    if verb == "cp" {
        let typed = words.current();
        let bare =
            !typed.is_empty() && !typed.contains([':', '/']) && !typed.starts_with(['.', '~']);
        return bare.then(|| container_names(&words, ctx, ":")).flatten();
    }
    if REPEATED_IMAGE_VERBS.contains(&verb) || (podman && verb == "pull") {
        return image_names(&words, ctx);
    }
    if SINGLE_IMAGE_VERBS.contains(&verb) {
        return (operands == 0).then(|| image_names(&words, ctx)).flatten();
    }
    None
}

fn base_args(words: &Words, ctx: &DynCtx) -> Vec<String> {
    let mut args = vec![ctx.bin(words.command())];
    for aliases in FORWARDED_VALUE_FLAGS {
        if let Some((flag, value)) = words.global_flag(aliases) {
            args.extend([flag.to_owned(), value.to_owned()]);
        }
    }
    args.extend(
        FORWARDED_BOOLEAN_FLAGS
            .iter()
            .filter(|flag| words.global_has(&[flag]))
            .map(|flag| (*flag).to_owned()),
    );
    args
}

fn container_names(words: &Words, ctx: &DynCtx, suffix: &str) -> Option<Vec<Candidate>> {
    let mut args = base_args(words, ctx);
    args.extend(["ps".into(), "-a".into()]);
    args.extend(ctx.limit_args("--last"));
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
        if ctx.limit > 0 && candidates.len() >= ctx.limit {
            break;
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}
