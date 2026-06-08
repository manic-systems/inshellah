// SPDX-License-Identifier: EUPL-1.2
//! binary classification and help/native completion probing.

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::parsers::help::help_parser;
use crate::parsers::manpage::ManpageResult;
use crate::subprocess::run_cmd;

pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
        .unwrap_or(false)
}

fn is_script(path: &Path) -> bool {
    let real = match fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let Ok(mut f) = fs::File::open(&real) else {
        return false;
    };
    let mut buf = [0u8; 2];
    f.read_exact(&mut buf)
        .map(|_| &buf == b"#!")
        .unwrap_or(false)
}

pub(super) fn skip_name(name: &str) -> bool {
    name.starts_with('.')
        || name.ends_with(".so")
        || name.ends_with(".a")
        || name.ends_with(".la")
        || name.contains('/')
}

fn is_scannable_magic(magic: &[u8; 4]) -> bool {
    #[cfg(target_os = "macos")]
    {
        matches!(
            magic,
            [0xce, 0xfa, 0xed, 0xfe]
                | [0xcf, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xce]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xca, 0xfe, 0xba, 0xbf]
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        magic == b"\x7fELF"
    }
}

// read failure reports all needles so the caller falls back to --help rather
// than skipping.
fn image_scan(path: &Path, needles: &[&str]) -> HashSet<String> {
    let mut found: HashSet<String> = HashSet::new();
    let real = match fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => {
            for n in needles {
                found.insert((*n).to_string());
            }
            return found;
        }
    };
    let Ok(mut f) = fs::File::open(&real) else {
        for n in needles {
            found.insert((*n).to_string());
        }
        return found;
    };
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() {
        return found;
    }
    if !is_scannable_magic(&magic) {
        // empty lets the caller decide
        return found;
    }
    let max_needle = needles.iter().map(|s| s.len()).max().unwrap_or(0);
    let chunk_size = 65536usize;
    let mut buf = vec![0u8; chunk_size + max_needle];
    let mut carry = 0usize;
    let needles_b: Vec<&[u8]> = needles.iter().map(|s| s.as_bytes()).collect();
    loop {
        let n: usize = f
            .read(&mut buf[carry..carry + chunk_size])
            .unwrap_or_default();
        if n == 0 {
            break;
        }
        let total = carry + n;
        for (i, needle) in needles_b.iter().enumerate() {
            let key = needles[i];
            if found.contains(key) {
                continue;
            }
            if needle.len() > total {
                continue;
            }
            let win = &buf[..total];
            if win.windows(needle.len()).any(|w| w == *needle) {
                found.insert(key.to_string());
            }
        }
        if found.len() == needles.len() {
            break;
        }
        let new_carry = max_needle.min(total);
        buf.copy_within(total - new_carry..total, 0);
        carry = new_carry;
    }
    found
}

fn read_to_string_capped(path: &Path, cap: usize) -> Option<String> {
    let real = fs::canonicalize(path).ok()?;
    let md = fs::metadata(&real).ok()?;
    if md.len() as usize > cap {
        return None;
    }
    fs::read_to_string(&real).ok()
}

fn nix_wrapper_target(path: &Path) -> Option<PathBuf> {
    let contents = read_to_string_capped(path, 65536)?;
    if !contents.contains("makeCWrapper") {
        return None;
    }
    extract_nix_bin_path(&contents)
}

fn nix_script_wrapper_target(path: &Path) -> Option<PathBuf> {
    let contents = read_to_string_capped(path, 4096)?;
    if !contents.starts_with("#!") {
        return None;
    }
    if !contents.contains("/nix/store/") {
        return None;
    }
    if !(contents.contains("exec ") || contents.contains("exec\t")) {
        return None;
    }
    extract_nix_bin_path(&contents)
}

fn extract_nix_bin_path(contents: &str) -> Option<PathBuf> {
    let needle = "/nix/store/";
    let bytes = contents.as_bytes();
    let mut idx = 0;
    while let Some(rel) = contents[idx..].find(needle) {
        let start = idx + rel;
        // path ends at whitespace, quote, or null
        let mut end = start + needle.len();
        while end < bytes.len() {
            let b = bytes[end];
            if b == b' '
                || b == b'\t'
                || b == b'\n'
                || b == b'\r'
                || b == b'"'
                || b == b'\''
                || b == 0
            {
                break;
            }
            end += 1;
        }
        let candidate = &contents[start..end];
        if candidate.contains("/bin/") {
            let path = PathBuf::from(candidate);
            if path.exists() {
                return Some(path);
            }
        }
        idx = end;
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Classify {
    TryHelp,
    HasNativeCompletions,
    Skip,
}

fn classify_image(path: &Path) -> Classify {
    let found = image_scan(path, &["-h", "--help", "complet"]);
    if found.contains("complet") {
        Classify::HasNativeCompletions
    } else if found.contains("-h") || found.contains("--help") {
        Classify::TryHelp
    } else {
        Classify::Skip
    }
}

pub(super) fn classify_binary(_bindir: &Path, full: &Path) -> Classify {
    if is_script(full) {
        return Classify::TryHelp;
    }
    if let Some(target) = nix_wrapper_target(full) {
        return classify_image(&target);
    }
    if let Some(target) = nix_script_wrapper_target(full) {
        return classify_image(&target);
    }
    classify_image(full)
}

// skips the positional `help` variant: unlikely to differ, extra spawn
// dominates.
fn try_help_until(bin: &Path, timeout_ms: u64, deadline: Instant) -> Option<String> {
    let bin_s = bin.to_string_lossy().to_string();
    for variant in [&["--help"][..], &["-h"][..]] {
        let attempt_ms = timeout_ms.min(remaining_ms(deadline));
        if attempt_ms == 0 {
            return None;
        }
        let mut args = vec![bin_s.clone()];
        args.extend(variant.iter().map(|s| s.to_string()));
        if let Some(out) = run_cmd(&args, attempt_ms) {
            let cleaned = fast_strip_ansi::strip_ansi_string(&out);
            if !cleaned.trim().is_empty() {
                return Some(cleaned.to_string());
            }
        }
    }
    None
}

pub fn try_help(bin: &Path, timeout_ms: u64) -> Option<String> {
    try_help_until(
        bin,
        timeout_ms,
        Instant::now() + Duration::from_millis(timeout_ms),
    )
}

fn is_nushell_source(text: &str) -> bool {
    text.len() > 20
        && (text.contains("export extern")
            || text.contains("export def")
            || (text.contains("module ") && text.contains("export")))
}

fn extract_matching_words(text: &str, needles: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for token in text.split(|c: char| c.is_whitespace() || c == ',' || c == '|') {
        let word = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
        if word.len() < 2 || word.starts_with('-') {
            continue;
        }
        for needle in needles {
            if word.contains(needle) && !seen.contains(word) {
                seen.insert(word.to_string());
                out.push(word.to_string());
                break;
            }
        }
    }
    out
}

pub(super) fn try_native_completion(bin: &Path, timeout_ms: u64) -> Option<String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let help_text = try_help_until(bin, timeout_ms, deadline)?;
    let candidates = extract_matching_words(&help_text, &["complet"]);
    let bin_s = bin.to_string_lossy().to_string();
    for sub in &candidates {
        for args_form in [
            vec![bin_s.clone(), sub.clone(), "nushell".to_string()],
            vec![
                bin_s.clone(),
                sub.clone(),
                "--shell".to_string(),
                "nushell".to_string(),
            ],
            vec![bin_s.clone(), sub.clone(), "--shell=nushell".to_string()],
        ] {
            let attempt_ms = timeout_ms.min(remaining_ms(deadline));
            if attempt_ms == 0 {
                return None;
            }
            if let Some(out) = run_cmd(&args_form, attempt_ms) {
                let cleaned = fast_strip_ansi::strip_ansi_string(&out);
                if is_nushell_source(&cleaned) {
                    return Some(cleaned.to_string());
                }
            }
        }
    }
    None
}

pub(super) fn remaining_ms(deadline: Instant) -> u64 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .min(u64::MAX as u128) as u64
}

pub fn parse_help_text(text: &str) -> ManpageResult {
    let cleaned: String = fast_strip_ansi::strip_ansi_string(text).into_owned();
    match help_parser(&cleaned) {
        Ok((_, r)) => r,
        Err(_) => ManpageResult::default(),
    }
}

// falls back to `-h` when --help is empty or "No manual entry".
pub fn try_help_args(bin_s: &str, sub_args: &[String], timeout_ms: u64) -> Option<String> {
    let mut primary_args: Vec<String> = vec![bin_s.to_string()];
    primary_args.extend(sub_args.iter().cloned());
    primary_args.push("--help".to_string());
    let primary = run_cmd(&primary_args, timeout_ms);
    let primary_text = primary
        .as_deref()
        .map(|s| fast_strip_ansi::strip_ansi_string(s).into_owned());
    let primary_useful = primary_text
        .as_ref()
        .map(|t| {
            let trimmed = t.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with("No manual entry")
                && !trimmed.starts_with("man:")
        })
        .unwrap_or(false);
    if primary_useful {
        return primary_text;
    }
    let mut fallback_args: Vec<String> = vec![bin_s.to_string()];
    fallback_args.extend(sub_args.iter().cloned());
    fallback_args.push("-h".to_string());
    if let Some(out) = run_cmd(&fallback_args, timeout_ms) {
        let cleaned = fast_strip_ansi::strip_ansi_string(&out).into_owned();
        if !cleaned.trim().is_empty() {
            return Some(cleaned);
        }
    }
    primary_text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_executable_magic_is_never_scannable() {
        assert!(!is_scannable_magic(&[0x89, b'P', b'N', b'G']));
        assert!(!is_scannable_magic(b"#!/b"));
        assert!(!is_scannable_magic(b"text"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_scans_mach_o_only() {
        assert!(is_scannable_magic(&[0xcf, 0xfa, 0xed, 0xfe]));
        assert!(is_scannable_magic(&[0xca, 0xfe, 0xba, 0xbe]));
        assert!(!is_scannable_magic(b"\x7fELF"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn elf_targets_scan_elf_only() {
        assert!(is_scannable_magic(b"\x7fELF"));
        assert!(!is_scannable_magic(&[0xca, 0xfe, 0xba, 0xbe]));
        assert!(!is_scannable_magic(&[0xcf, 0xfa, 0xed, 0xfe]));
    }

    #[test]
    fn remaining_ms_saturates_at_zero() {
        let past = Instant::now();
        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(remaining_ms(past), 0, "elapsed deadline must yield 0");

        let future = Instant::now() + Duration::from_millis(500);
        let r = remaining_ms(future);
        assert!(r > 0 && r <= 500, "remaining {r} out of (0, 500]");
    }

    #[test]
    fn native_completion_probe_uses_one_shared_deadline() {
        use std::os::unix::fs::PermissionsExt;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "inshellah-native-deadline-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("temp dir");
        let bin = root.join("nativeish");
        fs::write(
            &bin,
            r#"#!/bin/sh
if [ "$1" = "--help" ] || [ "$1" = "-h" ]; then
  printf 'Usage: nativeish\nCommands:\n  completionalpha\n  completionbeta\n  completiongamma\n'
  exit 0
fi
sleep 1
"#,
        )
        .expect("write script");
        let mut perms = fs::metadata(&bin).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&bin, perms).expect("chmod");

        let start = Instant::now();
        let out = try_native_completion(&bin, 80);
        let elapsed = start.elapsed();
        assert!(out.is_none());
        assert!(
            elapsed < Duration::from_millis(500),
            "native probing multiplied timeout across candidates: {elapsed:?}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
