// SPDX-License-Identifier: EUPL-1.2
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::complete::Candidate;
use crate::dynamic::shared::DynCtx;

pub(super) fn complete(_spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if !ctx.slot.positional() && ctx.slot.value_flag() != Some("-J") {
        return None;
    }
    let mut seen = HashSet::new();
    let mut out: Vec<Candidate> = Vec::new();
    if let Some(home) = env::var_os("HOME") {
        let cfg_path = PathBuf::from(&home).join(".ssh/config");
        if let Ok(contents) = fs::read_to_string(&cfg_path) {
            for line in contents.lines() {
                let trimmed = line.trim_start();
                let host_line = trimmed
                    .strip_prefix("Host ")
                    .or_else(|| trimmed.strip_prefix("host "))
                    .or_else(|| trimmed.strip_prefix("HOST "));
                let Some(rest) = host_line else { continue };
                for host in rest.split_whitespace() {
                    if host.contains('*') || host.is_empty() {
                        continue;
                    }
                    if seen.insert(host.to_owned()) {
                        out.push(Candidate::new(host, ""));
                    }
                }
            }
        }
        let kh_path = PathBuf::from(&home).join(".ssh/known_hosts");
        if let Ok(contents) = fs::read_to_string(&kh_path) {
            for line in contents.lines() {
                let Some(first) = line.split_whitespace().next() else {
                    continue;
                };
                if first.starts_with('|') || first.starts_with('[') {
                    continue;
                }
                for host in first.split(',') {
                    if host.is_empty() {
                        continue;
                    }
                    if seen.insert(host.to_owned()) {
                        out.push(Candidate::new(host, ""));
                    }
                }
            }
        }
    }
    (!out.is_empty()).then_some(out)
}
