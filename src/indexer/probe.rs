// SPDX-License-Identifier: EUPL-1.2
//! binary classification and help/native completion probing.

use std::{
    borrow::Cow,
    collections::HashSet,
    fs,
    io::Read as _,
    path::{
        Path,
        PathBuf,
    },
    time::{
        Duration,
        Instant,
    },
};

use crate::{
    parsers::{
        help::help_parser,
        manpage::ManpageResult,
    },
    subprocess::run_cmd,
};

fn is_script(path: &Path) -> bool {
    let Ok(real) = fs::canonicalize(path) else {
        return false;
    };
    let Ok(mut file) = fs::File::open(&real) else {
        return false;
    };
    let mut buf = [0_u8; 2];
    file.read_exact(&mut buf).is_ok_and(|()| &buf == b"#!")
}

pub(super) fn skip_name(name: &str) -> bool {
    name.starts_with('.')
        || name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| matches!(ext, "so" | "a" | "la"))
        || name.contains('/')
}

const fn is_scannable_magic(magic: [u8; 4]) -> bool {
    #[cfg(target_os = "macos")]
    {
        matches!(
            magic,
            [0xCE | 0xCF, 0xFA, 0xED, 0xFE]
                | [0xFE, 0xED, 0xFA, 0xCE | 0xCF]
                | [0xCA, 0xFE, 0xBA, 0xBE | 0xBF]
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        matches!(magic, [0x7F, b'E', b'L', b'F'])
    }
}

/// the needles a binary image holds, None when it cannot be read.
fn image_scan(path: &Path, needles: &[&str]) -> Option<HashSet<String>> {
    let mut found: HashSet<String> = HashSet::new();
    let real = fs::canonicalize(path).ok()?;
    let mut file = fs::File::open(&real).ok()?;
    let mut magic = [0_u8; 4];
    if file.read_exact(&mut magic).is_err() {
        return Some(found);
    }
    if !is_scannable_magic(magic) {
        // empty lets the caller decide
        return Some(found);
    }
    let max_needle = needles.iter().map(|needle| needle.len()).max().unwrap_or(0);
    let chunk_size = 64 * 1024_usize;
    let mut buf = vec![0_u8; chunk_size + max_needle];
    let mut carry = 0_usize;
    let needles_b: Vec<&[u8]> = needles.iter().map(|needle| needle.as_bytes()).collect();
    loop {
        let n: usize = file
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
                found.insert(key.to_owned());
            }
        }
        if found.len() == needles.len() {
            break;
        }
        let new_carry = max_needle.min(total);
        buf.copy_within(total - new_carry..total, 0);
        carry = new_carry;
    }
    Some(found)
}

/// the real image behind a nix wrapper, else `full` itself.
pub fn image_behind(full: &Path) -> PathBuf {
    nix_wrapper_target(full)
        .or_else(|| nix_script_wrapper_target(full))
        .unwrap_or_else(|| full.to_path_buf())
}

/// the needles the image behind `full` holds, empty when it cannot be read.
pub fn image_strings(full: &Path, needles: &[&str]) -> HashSet<String> {
    image_scan(&image_behind(full), needles).unwrap_or_default()
}

fn read_to_string_capped(path: &Path, cap: usize) -> Option<String> {
    let real = fs::canonicalize(path).ok()?;
    let md = fs::metadata(&real).ok()?;
    if usize::try_from(md.len()).map_or(true, |len| len > cap) {
        return None;
    }
    fs::read_to_string(&real).ok()
}

fn nix_wrapper_target(path: &Path) -> Option<PathBuf> {
    let contents = read_to_string_capped(path, 64 * 1024)?;
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
    // the shebang names the interpreter, the exec line names the target.
    contents
        .lines()
        .filter(|line| line.trim_start().starts_with("exec"))
        .find_map(extract_nix_bin_path)
}

fn extract_nix_bin_path(contents: &str) -> Option<PathBuf> {
    let needle = "/nix/store/";
    let bytes = contents.as_bytes();
    let mut idx = 0;
    while let Some(rel) = contents.get(idx..).and_then(|rest| rest.find(needle)) {
        let start = idx + rel;
        // path ends at whitespace, quote, or null
        let mut end = start + needle.len();
        while end < bytes.len() {
            let byte = bytes[end];
            if byte == b' '
                || byte == b'\t'
                || byte == b'\n'
                || byte == b'\r'
                || byte == b'"'
                || byte == b'\''
                || byte == 0
            {
                break;
            }
            end += 1;
        }
        let Some(candidate) = contents.get(start..end) else {
            break;
        };
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

// read failure reports all needles so the caller falls back to --help rather
// than skipping.
fn classify_image(path: &Path) -> Classify {
    let needles = ["-h", "--help", "complet"];
    let found = image_scan(path, &needles)
        .unwrap_or_else(|| needles.iter().map(|needle| (*needle).to_owned()).collect());
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
    classify_image(&image_behind(full))
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
        args.extend(variant.iter().map(ToString::to_string));
        let started = Instant::now();
        if let Some(out) = run_cmd(&args, attempt_ms) {
            let cleaned = strip_ansi(&out);
            if !cleaned.trim().is_empty() {
                return Some(cleaned.to_string());
            }
        } else if timed_out(started, attempt_ms) {
            return None;
        }
    }
    None
}

// a hung `--help` hangs on `-h` too.
fn timed_out(started: Instant, timeout_ms: u64) -> bool {
    started.elapsed() >= Duration::from_millis(timeout_ms)
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
    for token in text.split(|ch: char| ch.is_whitespace() || ch == ',' || ch == '|') {
        let word = token.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '-' && ch != '_');
        if word.len() < 2 || word.starts_with('-') {
            continue;
        }
        for needle in needles {
            if word.contains(needle) && !seen.contains(word) {
                seen.insert(word.to_owned());
                out.push(word.to_owned());
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
            vec![bin_s.clone(), sub.clone(), "nushell".to_owned()],
            vec![
                bin_s.clone(),
                sub.clone(),
                "--shell".to_owned(),
                "nushell".to_owned(),
            ],
            vec![bin_s.clone(), sub.clone(), "--shell=nushell".to_owned()],
        ] {
            let attempt_ms = timeout_ms.min(remaining_ms(deadline));
            if attempt_ms == 0 {
                return None;
            }
            if let Some(out) = run_cmd(&args_form, attempt_ms) {
                let cleaned = strip_ansi(&out);
                if is_nushell_source(&cleaned) {
                    return Some(cleaned.to_string());
                }
            }
        }
    }
    None
}

pub(super) fn remaining_ms(deadline: Instant) -> u64 {
    u64::try_from(
        deadline
            .saturating_duration_since(Instant::now())
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

// the ECMA-48 escapes and the C0 controls a terminal acts on, keeping tab, LF
// and CR
fn strip_ansi(text: &str) -> Cow<'_, str> {
    const ESC: u8 = 0x1B;
    let is_control =
        |byte: u8| (byte < 0x20 && !matches!(byte, b'\t' | b'\n' | b'\r')) || byte == 0x7F;

    let bytes = text.as_bytes();
    let byte_at = |idx: usize| bytes.get(idx).copied();

    // a branch-free fold per 32-byte chunk vectorizes, scanning 4x faster than
    // `position`
    let next_control = |from: usize| {
        let mut base = from;
        for chunk in bytes[from..].chunks(32) {
            if chunk
                .iter()
                .fold(false, |any, &byte| any | is_control(byte))
            {
                return base + chunk.iter().position(|&byte| is_control(byte)).unwrap_or(0);
            }
            base += chunk.len();
        }
        bytes.len()
    };

    let mut at = next_control(0);
    if at == bytes.len() {
        return Cow::Borrowed(text);
    }

    let mut out = Vec::with_capacity(bytes.len());
    let mut start = 0;
    while at < bytes.len() {
        out.extend_from_slice(&bytes[start..at]);
        at += 1;
        if bytes[at - 1] == ESC {
            match byte_at(at) {
                Some(b'[') => {
                    at += 1;
                    while let Some(param) = byte_at(at)
                        && param != ESC
                        && ((b' '..=b'?').contains(&param) || is_control(param))
                    {
                        at += 1;
                    }
                    if let Some(b'@'..=b'~') = byte_at(at) {
                        at += 1;
                    }
                },
                Some(b']' | b'P' | b'X' | b'^' | b'_') => {
                    at += 1;
                    while let Some(body) = byte_at(at)
                        && body != ESC
                    {
                        at += 1;
                        if body == 0x07 {
                            break;
                        }
                    }
                },
                Some(b' '..=b'/') => {
                    at += 1;
                    while let Some(b' '..=b'/') = byte_at(at) {
                        at += 1;
                    }
                    if let Some(b'0'..=b'~') = byte_at(at) {
                        at += 1;
                    }
                },
                Some(b'0'..=b'~') => at += 1,
                _ => {},
            }
        }
        start = at;
        at = next_control(at);
    }
    out.extend_from_slice(&bytes[start..]);

    // SAFETY: each removed run starts at an ASCII byte and ends before one or
    // at the end of `text`, so every UTF-8 sequence survives whole
    Cow::Owned(unsafe { String::from_utf8_unchecked(out) })
}

pub fn parse_help_text(text: &str) -> ManpageResult {
    let cleaned = strip_ansi(text);
    match help_parser(&cleaned) {
        Ok((_, parsed)) => parsed,
        Err(_) => ManpageResult::default(),
    }
}

// falls back to `-h` when --help is empty or "No manual entry".
pub fn try_help_args(bin_s: &str, sub_args: &[String], timeout_ms: u64) -> Option<String> {
    let mut primary_args: Vec<String> = vec![bin_s.to_owned()];
    primary_args.extend(sub_args.iter().cloned());
    primary_args.push("--help".to_owned());
    let started = Instant::now();
    let primary = run_cmd(&primary_args, timeout_ms);
    if primary.is_none() && timed_out(started, timeout_ms) {
        return None;
    }
    let primary_text = primary.as_deref().map(|raw| strip_ansi(raw).into_owned());
    let primary_useful = primary_text.as_ref().is_some_and(|text| {
        let trimmed = text.trim();
        !trimmed.is_empty()
            && !trimmed.starts_with("No manual entry")
            && !trimmed.starts_with("man:")
    });
    if primary_useful {
        return primary_text;
    }
    let mut fallback_args: Vec<String> = vec![bin_s.to_owned()];
    fallback_args.extend(sub_args.iter().cloned());
    fallback_args.push("-h".to_owned());
    if let Some(out) = run_cmd(&fallback_args, timeout_ms) {
        let cleaned = strip_ansi(&out).into_owned();
        if !cleaned.trim().is_empty() {
            return Some(cleaned);
        }
    }
    primary_text
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        io,
        process,
        thread,
        time::{
            SystemTime,
            UNIX_EPOCH,
        },
    };

    use super::*;

    #[test]
    fn non_executable_magic_is_never_scannable() {
        assert!(!is_scannable_magic([0x89, b'P', b'N', b'G']));
        assert!(!is_scannable_magic(*b"#!/b"));
        assert!(!is_scannable_magic(*b"text"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_scans_mach_o_only() {
        assert!(is_scannable_magic([0xCF, 0xFA, 0xED, 0xFE]));
        assert!(is_scannable_magic([0xCA, 0xFE, 0xBA, 0xBE]));
        assert!(!is_scannable_magic(*b"\x7fELF"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn elf_targets_scan_elf_only() {
        assert!(is_scannable_magic(*b"\x7fELF"));
        assert!(!is_scannable_magic([0xCA, 0xFE, 0xBA, 0xBE]));
        assert!(!is_scannable_magic([0xCF, 0xFA, 0xED, 0xFE]));
    }

    #[test]
    fn remaining_ms_saturates_at_zero() {
        let past = Instant::now();
        thread::sleep(Duration::from_millis(2));
        assert_eq!(remaining_ms(past), 0, "elapsed deadline must yield 0");

        let future = Instant::now() + Duration::from_millis(500);
        let remaining = remaining_ms(future);
        assert!(
            remaining > 0 && remaining <= 500,
            "remaining {remaining} out of (0, 500]"
        );
    }

    #[test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "temp dir cleanup is best-effort and must not fail the test"
    )]
    fn native_completion_probe_uses_one_shared_deadline() {
        use std::os::unix::fs::PermissionsExt as _;

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "inshellah-native-deadline-{}-{nanos}",
            process::id()
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

        let _: io::Result<()> = fs::remove_dir_all(root);
    }
}
