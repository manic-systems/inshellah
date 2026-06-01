// SPDX-License-Identifier: EUPL-1.2
//! adb device-serial and package-name value completions. unlike the other
//! providers this preempts static flag completion (see providers::value_completions):
//! `adb -s <tab>` must offer live serials, not adb's own flags.

use std::path::{Path, PathBuf};

use crate::complete::starts_with_ignore_ascii_case;

use super::super::shared::{Candidate, DynCtx, find_in_path, run_scrape};

#[derive(Clone, Debug)]
struct AdbDevice {
    serial: String,
    desc: String,
    transport_id: Option<String>,
}

enum AdbDeviceCompletion<'a> {
    Serial {
        prefix: &'a str,
        replacement_prefix: &'static str,
    },
    TransportId {
        prefix: &'a str,
        replacement_prefix: &'static str,
    },
}

/// preempt entry point: value completions for adb selectors and package args,
/// or None to let static flag completion answer. `spans[0]` is the command.
pub(super) fn complete(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let rest = &spans[1..];
    let path = resolve_adb(ctx)?;
    if let Some(completion) = adb_device_completion(rest) {
        return Some(adb_device_candidates(&path, completion, ctx));
    }
    if let Some(prefix) = adb_package_completion_prefix(rest) {
        let selectors = adb_selector_args(rest);
        return Some(adb_package_candidates(&path, &selectors, prefix, ctx));
    }
    None
}

fn resolve_adb(ctx: &DynCtx) -> Option<PathBuf> {
    ctx.explicit_cmd_path
        .map(Path::to_path_buf)
        .or_else(|| find_in_path(ctx.cmd_name))
}

fn adb_device_completion(rest: &[String]) -> Option<AdbDeviceCompletion<'_>> {
    if !adb_command_tokens(rest).is_empty() {
        return None;
    }
    let current = rest.last().map(String::as_str).unwrap_or("");
    if let Some(prefix) = current.strip_prefix("--serial=") {
        return Some(AdbDeviceCompletion::Serial {
            prefix,
            replacement_prefix: "--serial=",
        });
    }
    if let Some(prefix) = current.strip_prefix("--one-device=") {
        return Some(AdbDeviceCompletion::Serial {
            prefix,
            replacement_prefix: "--one-device=",
        });
    }
    if let Some(prefix) = current.strip_prefix("--transport-id=") {
        return Some(AdbDeviceCompletion::TransportId {
            prefix,
            replacement_prefix: "--transport-id=",
        });
    }
    if rest.len() >= 2 {
        let prev = rest[rest.len() - 2].as_str();
        if prev == "-s" || prev == "--serial" || prev == "--one-device" {
            return Some(AdbDeviceCompletion::Serial {
                prefix: current,
                replacement_prefix: "",
            });
        }
        if prev == "-t" || prev == "--transport-id" {
            return Some(AdbDeviceCompletion::TransportId {
                prefix: current,
                replacement_prefix: "",
            });
        }
    }
    None
}

fn parse_adb_devices(output: &str) -> Vec<AdbDevice> {
    let mut out = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('*')
            || trimmed.eq_ignore_ascii_case("List of devices attached")
        {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let serial = parts[0];
        let state = if parts.get(1) == Some(&"no") && parts.get(2) == Some(&"permissions") {
            "no permissions"
        } else {
            parts[1]
        };
        if serial.eq_ignore_ascii_case("list") {
            continue;
        }
        if !is_adb_device_state(state) {
            continue;
        }

        let mut details = Vec::new();
        let mut transport_id = None;
        let detail_start = if state == "no permissions" { 3 } else { 2 };
        for part in parts.iter().skip(detail_start) {
            if let Some(model) = part.strip_prefix("model:") {
                details.push(model.replace('_', " "));
            } else if let Some(product) = part.strip_prefix("product:") {
                details.push(product.replace('_', " "));
            } else if let Some(id) = part.strip_prefix("transport_id:") {
                transport_id = Some(id.to_string());
            }
        }
        let desc = if details.is_empty() {
            state.to_string()
        } else {
            format!("{state} {}", details.join(" "))
        };
        out.push(AdbDevice {
            serial: serial.to_string(),
            desc,
            transport_id,
        });
    }
    out
}

fn is_adb_device_state(state: &str) -> bool {
    matches!(
        state,
        "device"
            | "offline"
            | "unauthorized"
            | "recovery"
            | "sideload"
            | "rescue"
            | "no permissions"
    )
}

fn adb_device_candidates(
    path: &Path,
    completion: AdbDeviceCompletion<'_>,
    ctx: &DynCtx,
) -> Vec<Candidate> {
    let args = vec![
        path.to_string_lossy().to_string(),
        "devices".to_string(),
        "-l".to_string(),
    ];
    let Some(output) = run_scrape(&args, ctx) else {
        return Vec::new();
    };
    let mut scored = Vec::new();
    for device in parse_adb_devices(&output) {
        match &completion {
            AdbDeviceCompletion::Serial {
                prefix,
                replacement_prefix,
            } => {
                let score = prefix_score(prefix, &device.serial);
                if score > 0 {
                    scored.push((
                        score,
                        Candidate::new(
                            format!("{replacement_prefix}{}", &device.serial),
                            device.desc.clone(),
                        ),
                    ));
                }
            }
            AdbDeviceCompletion::TransportId {
                prefix,
                replacement_prefix,
            } => {
                if let Some(id) = &device.transport_id {
                    let score = prefix_score(prefix, id);
                    if score > 0 {
                        scored.push((
                            score,
                            Candidate::new(
                                format!("{replacement_prefix}{id}"),
                                format!("{} {}", &device.serial, &device.desc),
                            ),
                        ));
                    }
                }
            }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, c)| c).collect()
}

/// adb-specific prefix match: exact serial wins, then case-insensitive prefix.
/// not the dynamic fuzzy_score (serials aren't subcommands).
fn prefix_score(prefix: &str, value: &str) -> i32 {
    if prefix.is_empty() {
        return 1;
    }
    if prefix.len() == value.len() && prefix.eq_ignore_ascii_case(value) {
        1000
    } else if starts_with_ignore_ascii_case(value.as_bytes(), prefix.as_bytes()) {
        900
    } else {
        0
    }
}

fn adb_selector_args(rest: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let token = rest[i].as_str();
        if matches!(token, "-s" | "--serial" | "-t" | "--transport-id") {
            if i + 1 < rest.len() && !rest[i + 1].is_empty() {
                out.push(rest[i].clone());
                out.push(rest[i + 1].clone());
                i += 2;
                continue;
            }
        } else if (token.starts_with("--serial=") || token.starts_with("--transport-id="))
            && !token.ends_with('=')
        {
            out.push(rest[i].clone());
        }
        i += 1;
    }
    out
}

fn adb_command_tokens(rest: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let token = rest[i].as_str();
        if matches!(
            token,
            "-s" | "--serial" | "-t" | "--transport-id" | "--one-device"
        ) {
            i += if i + 1 < rest.len() { 2 } else { 1 };
            continue;
        }
        if token.starts_with("--serial=")
            || token.starts_with("--transport-id=")
            || token.starts_with("--one-device=")
        {
            i += 1;
            continue;
        }
        out.push(token);
        i += 1;
    }
    out
}

fn adb_package_completion_prefix(rest: &[String]) -> Option<&str> {
    let tokens = adb_command_tokens(rest);
    let first = *tokens.first()?;
    if first == "uninstall" {
        return package_prefix_for_arg_tail(&tokens[1..], &["--user"]);
    }
    if tokens.len() >= 4 && tokens[0] == "shell" && tokens[1] == "pm" {
        let action = tokens[2];
        if matches!(action, "clear" | "disable-user" | "enable") {
            return package_prefix_for_arg_tail(&tokens[3..], &["--user"]);
        }
    }
    if tokens.len() >= 4 && tokens[0] == "shell" && tokens[1] == "am" && tokens[2] == "force-stop" {
        return package_prefix_for_arg_tail(&tokens[3..], &["--user"]);
    }
    None
}

fn package_prefix_for_arg_tail<'a>(args: &[&'a str], value_flags: &[&str]) -> Option<&'a str> {
    let current = *args.last()?;
    if current.starts_with('-') {
        return None;
    }
    if args.len() >= 2 && value_flags.contains(&args[args.len() - 2]) {
        return None;
    }
    let mut positional_count = 0usize;
    let mut i = 0usize;
    let end = args.len().saturating_sub(1);
    while i < end {
        let token = args[i];
        if token.starts_with('-') {
            i += if value_flags.contains(&token) && i + 1 < end {
                2
            } else {
                1
            };
        } else {
            positional_count += 1;
            i += 1;
        }
    }
    (positional_count == 0).then_some(current)
}

fn parse_adb_package_line(line: &str) -> Option<&str> {
    let package = line.trim().strip_prefix("package:")?;
    let package = package
        .rsplit_once('=')
        .map(|(_, rhs)| rhs)
        .unwrap_or(package)
        .trim();
    (!package.is_empty()).then_some(package)
}

fn adb_package_candidates(
    path: &Path,
    selector_args: &[String],
    prefix: &str,
    ctx: &DynCtx,
) -> Vec<Candidate> {
    let mut args = vec![path.to_string_lossy().to_string()];
    args.extend(selector_args.iter().cloned());
    args.extend(
        ["shell", "pm", "list", "packages"]
            .into_iter()
            .map(str::to_string),
    );
    let Some(output) = run_scrape(&args, ctx) else {
        return Vec::new();
    };
    let mut scored = Vec::new();
    for package in output.lines().filter_map(parse_adb_package_line) {
        let score = prefix_score(prefix, package);
        if score > 0 {
            scored.push((score, Candidate::new(package, "package")));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, c)| c).collect()
}
