// SPDX-License-Identifier: EUPL-1.2
//! inshellah CLI.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use pound::Parse;

use inshellah::complete::{Candidate, generate_candidates};
use inshellah::config::{Config, DEFAULT_TIMEOUT_MS};
use inshellah::dynamic::{dynamic_complete_with_path, dynamic_value_completions};
use inshellah::parsers::help::help_parser;
use inshellah::parsers::manpage::{
    ManpageEntry, ManpageResult, ManpageSubcommand, OwnedSwitch, extract_synopsis_command,
    parse_manpage_string, parse_manpage_with_subs, read_manpage_file,
};
use inshellah::parsers::nushell::{generate_extern, is_nushell_builtin};
use inshellah::pool::{ScrapePool, Submitter};
use inshellah::resolver::{self, NodeClass, Outcome, Probe, resolve_node};
use inshellah::store::{
    self, all_commands, default_store_path, ensure_dir, file_type_of, filename_of_command, lookup,
    lookup_raw, parse_nu_completions, purge_dir, subcommands_of, write_native, write_result,
};
use inshellah::subprocess::run_cmd;

const COMMAND_SECTIONS: &[u8] = &[1, 8];

fn usage() {
    eprintln!(
        "inshellah - nushell completions engine

Usage:
  inshellah index PREFIX... [--dir PATH] [--ignore FILE] [--help-only FILE]
                            [--prefix PATH[:PATH...]] [--timeout-ms N] [--workers N]
      Index completions into a directory of JSON/nu files.
      PREFIX is a directory containing bin/ and share/man/.
      Default dir: $XDG_CACHE_HOME/inshellah
      --ignore FILE     skip listed commands entirely
      --help-only FILE  skip manpages for listed commands, use --help instead
      --prefix PATHS    extra scrape prefixes, colon-separated (in addition
                        to the positional PREFIX args)
      --timeout-ms N    per-subprocess timeout in milliseconds (default 200)
      --workers N       parallel scrape workers (default: cpu count)
      (env INSHELLAH_MAX_INDEX_NODES caps subcommand nodes per root command;
       default 10000, bounds runaway recursion on pathological trees)
  inshellah complete CMD [ARGS...] [--dir PATH[:PATH...]] [--timeout-ms N]
      Nushell custom completer. Outputs JSON completion candidates.
      Falls back to --help resolution if command is not indexed.
      --dir takes colon-separated paths. The first path is the writable
      user cache; additional paths are read-only system directories.
  inshellah query CMD [--dir PATH[:PATH...]]
      Print stored completion data for CMD.
  inshellah dump [--dir PATH[:PATH...]]
      List indexed commands.
  inshellah diff CMD [SUB...] [--dir EXTRA_MANDIR] [--timeout-ms N]
      Audit source divergence: parse CMD's manpage and --help separately
      and report subcommand/flag gaps between them (dev tool).
  inshellah purge [--dir PATH[:PATH...]]
      Delete the on-the-fly user cache (.json/.nu files). Only the first
      --dir (the writable user cache) is cleared; system dirs are untouched.
      Default dir: $XDG_CACHE_HOME/inshellah
  inshellah manpage FILE            Parse a manpage and emit nushell extern
  inshellah manpage-dir DIR         Batch-process manpages under DIR
  inshellah completions             Generate nushell completions for inshellah

Configuration (environment, read by `complete`):
  INSHELLAH_FLAG_TRIGGERS   chars that surface flags (default \"-\"; e.g. \"-+\")
  INSHELLAH_FLAG_ON_EMPTY   1 to also surface flags on an empty token
  INSHELLAH_MAX_COMPLETIONS cap on candidates returned (0 = no cap)
  INSHELLAH_TIMEOUT_MS      default --help resolve timeout (--timeout-ms wins)
  INSHELLAH_CACHE_TTL_SECS  rescrape user-cached sets older than N seconds (default 604800; 0 = never)
"
    );
}

fn is_executable(path: &Path) -> bool {
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

fn skip_name(name: &str) -> bool {
    name.starts_with('.')
        || name.ends_with(".so")
        || name.ends_with(".a")
        || name.ends_with(".la")
        || name.contains('/')
}

// macOS honours Mach-O, others ELF, so linux never treats `CA FE BA BE`
// (also a java class) as an image.
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
enum Classify {
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

fn classify_binary(_bindir: &Path, full: &Path) -> Classify {
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

fn try_help(bin: &Path, timeout_ms: u64) -> Option<String> {
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

fn try_native_completion(bin: &Path, timeout_ms: u64) -> Option<String> {
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

const MAX_RESOLVE_RESULTS: usize = 500;
const MAX_RECURSE_DEPTH: u32 = 10;
const RESOLVE_BUDGET_MULTIPLE: u64 = 8;

// env INSHELLAH_MAX_INDEX_NODES. bounds a pathological tree where fresh names
// every level dodge `self_listed`.
const DEFAULT_MAX_NODES_PER_ROOT: usize = 10_000;

fn remaining_ms(deadline: Instant) -> u64 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn parse_help_text(text: &str) -> ManpageResult {
    let cleaned: String = fast_strip_ansi::strip_ansi_string(text).into_owned();
    match help_parser(&cleaned) {
        Ok((_, r)) => r,
        Err(_) => ManpageResult::default(),
    }
}

// falls back to `-h` when --help is empty or "No manual entry".
fn try_help_args(bin_s: &str, sub_args: &[String], timeout_ms: u64) -> Option<String> {
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

fn cmd_name_of_manpage(path: &Path) -> String {
    let mut base = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    if base.ends_with(".gz") {
        base.truncate(base.len() - 3);
    }
    // strip section suffix, "ls.1" -> "ls"
    if let Some(dot) = base.rfind('.') {
        base.truncate(dot);
    }
    base
}

fn find_manpage_path(mandirs: &[PathBuf], hyphenated: &str) -> Option<PathBuf> {
    for mandir in mandirs {
        for section in COMMAND_SECTIONS {
            let secdir = mandir.join(format!("man{section}"));
            for ext in ["", ".gz"] {
                let path = secdir.join(format!("{hyphenated}.{section}{ext}"));
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

// synopsis wins because filenames are ambiguous: "btrfs-check.8" could be
// `btrfs-check` or `btrfs check`. clamp to the filename's hyphen-part count so
// the synopsis can't absorb a placeholder.
fn resolve_manpage_cmd_name(file: &Path, contents: &str) -> String {
    let fallback = cmd_name_of_manpage(file);
    let max_words = fallback.matches('-').count() + 1;
    match extract_synopsis_command(contents) {
        Some(name) => {
            let words: Vec<&str> = name.split(' ').filter(|w| !w.is_empty()).collect();
            if words.len() > max_words {
                words[..max_words].join(" ")
            } else {
                name
            }
        }
        None => fallback,
    }
}

type NamedManpageResult = (String, ManpageResult);
type ProcessedManpage = (String, ManpageResult, Vec<NamedManpageResult>);

// subs come from clap-style `.SH SUBCOMMAND` sections.
fn process_manpage(file: &Path) -> Option<ProcessedManpage> {
    let contents = read_manpage_file(file).ok()?;
    let (mut result, sub_sections) = parse_manpage_with_subs(&contents);
    if result.entries.is_empty()
        && result.subcommands.is_empty()
        && result.positional_choices.is_empty()
        && sub_sections.is_empty()
    {
        return None;
    }
    let name = resolve_manpage_cmd_name(file, &contents);
    if name.is_empty() {
        return None;
    }
    strip_manpage_subcmd_prefixes(&mut result, file, &name);
    // namespace sub-sections under the cmd name: nh "os" -> "nh os"
    let subs: Vec<(String, ManpageResult)> = sub_sections
        .into_iter()
        .map(|(sub_name, sub_result)| (format!("{name} {sub_name}"), sub_result))
        .collect();
    Some((name, result, subs))
}

fn list_manpages(mandirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for mandir in mandirs {
        for section in COMMAND_SECTIONS {
            let secdir = mandir.join(format!("man{section}"));
            if let Ok(entries) = fs::read_dir(&secdir) {
                for entry in entries.flatten() {
                    out.push(entry.path());
                }
            }
        }
    }
    out
}

fn load_ignorelist(path: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Ok(contents) = fs::read_to_string(path) {
        for line in contents.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('#') {
                out.insert(line.to_string());
            }
        }
    }
    out
}

fn list_binaries(bindirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut all: Vec<(String, PathBuf)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for bd in bindirs {
        let Ok(entries) = fs::read_dir(bd) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if skip_name(name) || is_nushell_builtin(name) {
                continue;
            }
            if !is_executable(&path) {
                continue;
            }
            if seen.insert(name.to_string()) {
                all.push((name.to_string(), path));
            }
        }
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));
    all
}

fn manpage_name_has_installed_command(name: &str, binary_names: &HashSet<String>) -> bool {
    if binary_names.contains(name) {
        return true;
    }
    name.split_once(' ')
        .map(|(parent, _)| binary_names.contains(parent))
        .unwrap_or(false)
}

fn split_command_for_binary(name: &str) -> Option<(&str, Vec<String>)> {
    let mut parts = name.split_whitespace();
    let base = parts.next()?;
    Some((base, parts.map(str::to_string).collect()))
}

#[cfg(test)]
mod main_tests {
    use super::*;

    #[test]
    fn manpage_names_must_match_installed_binary_or_subcommand_parent() {
        let binary_names = HashSet::from(["git".to_string(), "getent".to_string()]);

        assert!(manpage_name_has_installed_command("git", &binary_names));
        assert!(manpage_name_has_installed_command("git add", &binary_names));
        assert!(manpage_name_has_installed_command(
            "getent passwd",
            &binary_names
        ));
        assert!(!manpage_name_has_installed_command("ld.so", &binary_names));
        assert!(!manpage_name_has_installed_command(
            "git-add",
            &binary_names
        ));
    }

    #[test]
    fn completion_dir_mandir_resolves_to_prefix_share_man() {
        assert_eq!(
            mandir_for_completion_dir(Path::new("/run/current-system/sw/share/inshellah")),
            Some(PathBuf::from("/run/current-system/sw/share/man"))
        );
        assert_eq!(
            mandir_for_completion_dir(Path::new("/etc/profiles/per-user/alice/share/inshellah")),
            Some(PathBuf::from("/etc/profiles/per-user/alice/share/man"))
        );
    }

    #[test]
    fn index_prefix_flag_appends_colon_separated_prefixes() {
        let args = [
            "/sys".to_string(),
            "--prefix".to_string(),
            "/a:/b/c".to_string(),
            "--prefix".to_string(),
            "/d".to_string(),
        ];
        let parsed = parse_index_args(&args);
        assert_eq!(
            parsed.prefixes,
            vec![
                PathBuf::from("/sys"),
                PathBuf::from("/a"),
                PathBuf::from("/b/c"),
                PathBuf::from("/d"),
            ]
        );
    }

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

struct ScrapeCtx {
    cache_dir: PathBuf,
    mandirs: Vec<PathBuf>,
    help_only: HashSet<String>,
    indexed: Mutex<HashSet<String>>,
    timeout_ms: u64,
    node_budget: usize,
    node_counts: Mutex<std::collections::HashMap<String, usize>>,
    // roots already warned about, so the budget warning fires once each
    truncated: Mutex<HashSet<String>>,
}

#[derive(Debug)]
struct PoolJob {
    bin_path: PathBuf,
    base_cmd: String,
    // tokens past the base: ["stash","apply"] for `git stash apply`
    sub_args: Vec<String>,
    depth: u32,
}

impl PoolJob {
    fn full_cmd(&self) -> String {
        if self.sub_args.is_empty() {
            self.base_cmd.clone()
        } else {
            format!("{} {}", self.base_cmd, self.sub_args.join(" "))
        }
    }
}

// some manpages prefix subcommands with the parent name (git.1 lists git-add);
// strip the leading "{base}-" so they dispatch as `git add`.
fn strip_subcmd_prefix(result: &mut ManpageResult, base: &str) {
    let prefix = format!("{base}-");
    for sc in &mut result.subcommands {
        if let Some(rest) = sc.name.strip_prefix(&prefix) {
            sc.name = rest.to_string();
        }
    }
}

fn strip_manpage_subcmd_prefixes(result: &mut ManpageResult, file: &Path, cmd_name: &str) {
    let filename_base = cmd_name_of_manpage(file);
    if !filename_base.is_empty() {
        strip_subcmd_prefix(result, &filename_base);
    }
    let hyphenated_cmd = cmd_name.replace(' ', "-");
    if !hyphenated_cmd.is_empty() && hyphenated_cmd != filename_base {
        strip_subcmd_prefix(result, &hyphenated_cmd);
    }
}

// depth and node budget bound a breadth^depth blowup; `self_listed` only
// catches a child echoing its parent, so fresh names every level slip past.
fn enqueue_child_jobs(
    ctx: &ScrapeCtx,
    job: &PoolJob,
    children: &[String],
    submit: &Submitter<PoolJob>,
) {
    // `>` not `>=`, so the last discovered layer is still indexed
    if job.depth > MAX_RECURSE_DEPTH {
        return;
    }
    // per-root allowance keyed on base_cmd, so a full system scan stays
    // unbounded in breadth
    let mut counts = ctx.node_counts.lock();
    let tally = counts.entry(job.base_cmd.clone()).or_insert(0);
    for name in children {
        if *tally >= ctx.node_budget {
            if ctx.truncated.lock().insert(job.base_cmd.clone()) {
                eprintln!(
                    "warning: {} subcommand tree hit the {}-node budget; truncating",
                    job.base_cmd, ctx.node_budget
                );
            }
            break;
        }
        *tally += 1;
        let mut next = job.sub_args.clone();
        next.push(name.clone());
        submit.submit(PoolJob {
            bin_path: job.bin_path.clone(),
            base_cmd: job.base_cmd.clone(),
            sub_args: next,
            depth: job.depth + 1,
        });
    }
}

fn process_pool_job(ctx: &ScrapeCtx, job: PoolJob, submit: &Submitter<PoolJob>) {
    let full_cmd = job.full_cmd();
    if ctx.indexed.lock().contains(&full_cmd) {
        return;
    }
    let probe = RealProbe {
        path: &job.bin_path,
        mandirs: &ctx.mandirs,
        user_dir: &ctx.cache_dir,
        timeout_ms: ctx.timeout_ms,
        // pool bounds total work + per-subprocess timeouts, so no per-job budget
        deadline: Instant::now() + Duration::from_secs(86_400),
        skip_manpage: ctx.help_only.contains(&job.base_cmd) || ctx.help_only.contains(&full_cmd),
    };

    // classify only at top level
    if job.sub_args.is_empty() && probe.classify() == NodeClass::Skip {
        return;
    }

    match resolve_one(&probe, &job.base_cmd, &job.sub_args) {
        Outcome::Native { nu } => {
            if write_native(&ctx.cache_dir, &full_cmd, &nu).is_ok() {
                ctx.indexed.lock().insert(full_cmd);
            }
        }
        Outcome::Empty => {}
        Outcome::Content {
            result,
            source,
            children,
        } => {
            if write_result(&ctx.cache_dir, &full_cmd, source, &result).is_ok() {
                ctx.indexed.lock().insert(full_cmd);
                enqueue_child_jobs(ctx, &job, &children, submit);
            }
        }
    }
}

fn cmd_index(
    bindirs: &[PathBuf],
    mandirs: &[PathBuf],
    ignorelist: &HashSet<String>,
    help_only: &HashSet<String>,
    dir: &Path,
    timeout_ms: u64,
    num_workers: usize,
) -> std::io::Result<()> {
    ensure_dir(dir)?;
    let binaries = list_binaries(bindirs);
    let binary_names: HashSet<String> = binaries
        .iter()
        .filter(|(name, _)| !ignorelist.contains(name))
        .map(|(name, _)| name.clone())
        .collect();
    let binary_paths: std::collections::HashMap<String, PathBuf> = binaries
        .iter()
        .filter(|(name, _)| !ignorelist.contains(name))
        .cloned()
        .collect();

    let ctx = Arc::new(ScrapeCtx {
        cache_dir: dir.to_path_buf(),
        mandirs: mandirs.to_vec(),
        help_only: help_only.clone(),
        indexed: Mutex::new(HashSet::new()),
        timeout_ms,
        // 0/unparseable -> default; never unbounded
        node_budget: std::env::var("INSHELLAH_MAX_INDEX_NODES")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_MAX_NODES_PER_ROOT),
        node_counts: Mutex::new(std::collections::HashMap::new()),
        truncated: Mutex::new(HashSet::new()),
    });
    let pool = ScrapePool::new(num_workers, {
        let ctx = ctx.clone();
        move |job: PoolJob, submit: &Submitter<PoolJob>| {
            process_pool_job(&ctx, job, submit);
        }
    });
    for (name, path) in &binaries {
        if ignorelist.contains(name) {
            continue;
        }
        pool.submit(PoolJob {
            bin_path: path.clone(),
            base_cmd: name.clone(),
            sub_args: Vec::new(),
            depth: 0,
        });
    }
    pool.wait();
    // no workers alive, so the Arc has a single strong ref
    let mut indexed: HashSet<String> = Arc::try_unwrap(ctx)
        .ok()
        .map(|c| c.indexed.into_inner())
        .unwrap_or_default();

    // shorter filenames sort first so parents precede subpages (nix-env.1
    // before nix-env-install.1)
    let mut manpages = list_manpages(mandirs);
    manpages.sort_by(|a, b| {
        let alen = a.file_name().map(|s| s.len()).unwrap_or(0);
        let blen = b.file_name().map(|s| s.len()).unwrap_or(0);
        alen.cmp(&blen).then_with(|| a.cmp(b))
    });
    for manpage_path in manpages {
        let Some((name, mut result, sub_sections)) = process_manpage(&manpage_path) else {
            continue;
        };
        if !manpage_name_has_installed_command(&name, &binary_names) {
            continue;
        }
        let base_cmd = cmd_name_of_manpage(&manpage_path);
        if indexed.contains(&name) {
            if name != base_cmd {
                eprintln!(
                    "warning: {} extracted cmd \"{}\" (already indexed), skipping",
                    manpage_path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(""),
                    name
                );
            }
            continue;
        }
        if help_only.contains(&name) {
            continue;
        }
        if is_nushell_builtin(&name) {
            continue;
        }
        let mut source = "manpage";
        if let Some((base_cmd, sub_args)) = split_command_for_binary(&name)
            && let Some(path) = binary_paths.get(base_cmd)
            && supplement_result_from_help_command(&mut result, path, &sub_args, timeout_ms)
        {
            source = "manpage+help";
        }
        write_result(dir, &name, source, &result)?;
        indexed.insert(name.clone());
        for (sub_cmd, sub_result) in &sub_sections {
            if indexed.contains(sub_cmd) {
                continue;
            }
            let mut sub_result = sub_result.clone();
            let mut sub_source = "manpage";
            if let Some((base_cmd, sub_args)) = split_command_for_binary(sub_cmd)
                && let Some(path) = binary_paths.get(base_cmd)
                && supplement_result_from_help_command(&mut sub_result, path, &sub_args, timeout_ms)
            {
                sub_source = "manpage+help";
            }
            write_result(dir, sub_cmd, sub_source, &sub_result)?;
            indexed.insert(sub_cmd.clone());
        }
        // COMMANDS-section subcommands lacking a SUBCOMMAND section or own
        // manpage get a desc-only stub so the completer treats them as leaves.
        // left out of `indexed` so a real per-subcommand manpage overwrites it.
        if sub_sections.is_empty() {
            for sc in &result.subcommands {
                let sub_cmd = format!("{name} {}", sc.name);
                if indexed.contains(&sub_cmd) {
                    continue;
                }
                let stub = ManpageResult {
                    entries: Vec::new(),
                    subcommands: Vec::new(),
                    positional_choices: Vec::new(),
                    positionals: Default::default(),
                    description: sc.desc.clone(),
                };
                write_result(dir, &sub_cmd, "manpage", &stub)?;
            }
        }
    }

    println!("indexed {} commands into {}", indexed.len(), dir.display());
    Ok(())
}

fn cmd_manpage(file: &Path) -> std::io::Result<()> {
    if let Some((name, result, sub_sections)) = process_manpage(file) {
        print!("{}", generate_extern(&name, &result));
        for (sub_cmd, sub_result) in sub_sections {
            print!("{}", generate_extern(&sub_cmd, &sub_result));
        }
    }
    Ok(())
}

fn cmd_manpage_dir(dir: &Path) -> std::io::Result<()> {
    for section in COMMAND_SECTIONS {
        let secdir = dir.join(format!("man{section}"));
        let Ok(entries) = fs::read_dir(&secdir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some((name, result, sub_sections)) = process_manpage(&path) {
                print!("{}", generate_extern(&name, &result));
                for (sub_cmd, sub_result) in sub_sections {
                    print!("{}", generate_extern(&sub_cmd, &sub_result));
                }
            }
        }
    }
    Ok(())
}

fn cmd_query(cmd: &str, dirs: &[PathBuf]) -> std::io::Result<()> {
    match lookup_raw(dirs, cmd) {
        Some(data) => {
            print!("{data}");
            Ok(())
        }
        None => {
            eprintln!("not found: {cmd}");
            std::process::exit(1);
        }
    }
}

fn mandirs_for_bin(bin: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(prefix) = bin.parent().and_then(|p| p.parent()) {
        out.push(prefix.join("share/man"));
    }
    for p in [
        "/run/current-system/sw/share/man",
        "/usr/share/man",
        "/usr/local/share/man",
    ] {
        out.push(PathBuf::from(p));
    }
    out.into_iter().filter(|p| p.is_dir()).collect()
}

// keyed on the long name when present so `-v`/`--verbose` (manpage) and
// `--verbose` (help) compare equal.
fn switch_key(e: &ManpageEntry) -> String {
    match &e.switch {
        OwnedSwitch::Both(_, l) | OwnedSwitch::Long(l) => format!("--{l}"),
        OwnedSwitch::Short(c) => format!("-{c}"),
    }
}

fn diff_sets(label: &str, man: &[String], help: &[String]) {
    let sa: std::collections::BTreeSet<&str> = man.iter().map(String::as_str).collect();
    let sb: std::collections::BTreeSet<&str> = help.iter().map(String::as_str).collect();
    let shared = sa.intersection(&sb).count();
    println!(
        "  {label}: {} man, {} help, {shared} shared",
        man.len(),
        help.len()
    );
    let man_only: Vec<&str> = sa.difference(&sb).copied().collect();
    let help_only: Vec<&str> = sb.difference(&sa).copied().collect();
    if !man_only.is_empty() {
        println!("    man-only:  {}", man_only.join(" "));
    }
    if !help_only.is_empty() {
        println!("    help-only: {}", help_only.join(" "));
    }
}

// dev tool: parse manpage and `--help` independently and report where they
// disagree, so parser gaps aren't masked by the manpage>help fallback.
// `cmd_args` is the full path, e.g. ["jj", "bookmark"].
fn cmd_diff(cmd_args: &[String], extra_mandirs: &[PathBuf], timeout_ms: u64) {
    let Some((base, sub_args)) = cmd_args.split_first() else {
        eprintln!("error: diff requires a CMD argument");
        std::process::exit(1);
    };
    let Some(bin) = find_in_path(base) else {
        eprintln!("error: {base} not found in PATH");
        std::process::exit(1);
    };
    let mut mandirs = mandirs_for_bin(&bin);
    mandirs.extend(extra_mandirs.iter().cloned());
    let hyphenated = if sub_args.is_empty() {
        base.clone()
    } else {
        format!("{base}-{}", sub_args.join("-"))
    };
    let full = cmd_args.join(" ");

    let man_path = find_manpage_path(&mandirs, &hyphenated);
    let man = man_path
        .as_ref()
        .and_then(|p| read_manpage_file(p).ok())
        .map(|c| parse_manpage_string(&c));
    let bin_s = bin.to_string_lossy().to_string();
    let help_text = if sub_args.is_empty() {
        try_help(&bin, timeout_ms)
    } else {
        try_help_args(&bin_s, sub_args, timeout_ms)
    };
    let help = help_text.as_deref().map(parse_help_text);

    println!("# diff {full}");
    println!(
        "  manpage: {}",
        man_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".into())
    );
    println!(
        "  help:    {}",
        if help_text.is_some() { "ok" } else { "(none)" }
    );

    let subs = |r: &Option<ManpageResult>| -> Vec<String> {
        r.as_ref()
            .map(|r| r.subcommands.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default()
    };
    let flags = |r: &Option<ManpageResult>| -> Vec<String> {
        r.as_ref()
            .map(|r| r.entries.iter().map(switch_key).collect())
            .unwrap_or_default()
    };
    let man_subs = subs(&man);
    let help_subs = subs(&help);
    diff_sets("subcommands", &man_subs, &help_subs);
    diff_sets("flags", &flags(&man), &flags(&help));

    // manpage body enumerates no children but help does. note whether sibling
    // `cmd-sub.1` pages cover them, or help is the only source.
    if man_subs.is_empty() && !help_subs.is_empty() {
        let covered = help_subs
            .iter()
            .filter(|s| find_manpage_path(&mandirs, &format!("{hyphenated}-{s}")).is_some())
            .count();
        println!(
            "  GAP: manpage body has 0 subcommands, help has {}; sibling pages cover {covered}/{}",
            help_subs.len(),
            help_subs.len()
        );
    }
}

// group command with a leftover `<command>`/`<subcommands>` placeholder and no
// subcommands populated.
fn looks_like_unenumerated_group(r: &ManpageResult) -> bool {
    r.subcommands.is_empty()
        && r.positionals.iter().any(|(n, _)| {
            matches!(
                n.to_ascii_lowercase().as_str(),
                "command" | "commands" | "subcommand" | "subcommands"
            )
        })
}

// dev tool: scan a prefix for group commands whose body enumerates no children,
// probe `--help`, and report parser gaps (body should enumerate but doesn't) vs
// help-only gaps (no sibling page).
fn cmd_diff_scan(prefix: &Path, timeout_ms: u64) {
    let mandirs = vec![prefix.join("share/man")];
    let mut suspects = 0u32;
    let mut help_recoverable = 0u32;
    let mut sibling_covered = 0u32;
    for page in list_manpages(&mandirs) {
        let Ok(contents) = read_manpage_file(&page) else {
            continue;
        };
        let man = parse_manpage_string(&contents);
        if !looks_like_unenumerated_group(&man) {
            continue;
        }
        // map "jj-bookmark.1.gz" -> command tokens ["jj","bookmark"].
        let stem = page
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.split('.').next().unwrap_or(n))
            .unwrap_or("");
        let toks: Vec<String> = stem.split('-').map(str::to_string).collect();
        let Some((base, sub)) = toks.split_first() else {
            continue;
        };
        let Some(bin) = find_in_path(base) else {
            continue;
        };
        suspects += 1;
        let bin_s = bin.to_string_lossy().to_string();
        let help_text = if sub.is_empty() {
            try_help(&bin, timeout_ms)
        } else {
            try_help_args(&bin_s, sub, timeout_ms)
        };
        let help_subs: Vec<String> = help_text
            .as_deref()
            .map(parse_help_text)
            .map(|r| r.subcommands.into_iter().map(|s| s.name).collect())
            .unwrap_or_default();
        if help_subs.is_empty() {
            continue;
        }
        let covered = help_subs
            .iter()
            .filter(|s| find_manpage_path(&mandirs, &format!("{stem}-{s}")).is_some())
            .count();
        let kind = if covered == help_subs.len() {
            sibling_covered += 1;
            "sibling-covered (body parser gap)"
        } else {
            help_recoverable += 1;
            "help-only"
        };
        println!(
            "{}: body=0 help={} siblings={}/{}  [{kind}]",
            toks.join(" "),
            help_subs.len(),
            covered,
            help_subs.len()
        );
    }
    eprintln!(
        "scanned: {suspects} group suspects, {sibling_covered} body-parser gaps, {help_recoverable} help-only"
    );
}

fn cmd_dump(dirs: &[PathBuf]) {
    let cmds = all_commands(dirs);
    println!("{} commands", cmds.len());
    for cmd in &cmds {
        let src = file_type_of(dirs, cmd).unwrap_or_else(|| "?".to_string());
        println!("{src:>8}  {cmd}");
    }
}

fn cmd_purge(user_dir: &Path) {
    match purge_dir(user_dir) {
        Ok(n) => println!("purged {n} cached entries from {}", user_dir.display()),
        Err(e) => {
            eprintln!("purge failed: {e}");
            std::process::exit(1);
        }
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in path_var.split(':') {
        let candidate = Path::new(dir).join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn executable_span_path(span: &str) -> Option<PathBuf> {
    if !span.contains('/') {
        return None;
    }
    let path = PathBuf::from(span);
    is_executable(&path).then_some(path)
}

fn command_name_for_path(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

// `null` is nushell's no-match form.
fn print_completion_candidates(candidates: &[String]) {
    if candidates.is_empty() {
        println!("null");
    } else {
        let mut out = io::stdout().lock();
        out.write_all(b"[").expect("write completion output");
        for (idx, candidate) in candidates.iter().enumerate() {
            if idx > 0 {
                out.write_all(b",").expect("write completion output");
            }
            out.write_all(candidate.as_bytes())
                .expect("write completion output");
        }
        out.write_all(b"]\n").expect("write completion output");
    }
}

fn resolve_and_cache(
    user_dir: &Path,
    mandirs: &[PathBuf],
    cmd_name: &str,
    path: &Path,
    timeout_ms: u64,
) -> Option<ManpageResult> {
    resolve_command_path_and_cache(user_dir, mandirs, cmd_name, &[], path, timeout_ms)
}

// resolves each `cmd-*.N` page's real space-separated name from its content, so
// `subcommands_of` can surface children the parent page didn't enumerate.
fn index_sibling_manpages(user_dir: &Path, mandirs: &[PathBuf], hyphenated: &str) -> bool {
    let prefix = format!("{hyphenated}-");
    let mut any = false;
    for mandir in mandirs {
        for section in COMMAND_SECTIONS {
            let secdir = mandir.join(format!("man{section}"));
            let Ok(entries) = fs::read_dir(&secdir) else {
                continue;
            };
            for entry in entries.flatten() {
                let fname = entry.file_name();
                let Some(fname) = fname.to_str() else {
                    continue;
                };
                let stem = fname.split('.').next().unwrap_or(fname);
                if !stem.starts_with(&prefix) {
                    continue;
                }
                if let Some((name, result, subs)) = process_manpage(&entry.path()) {
                    let _ = write_result(user_dir, &name, "manpage", &result);
                    for (sub_name, sub_result) in subs {
                        let _ = write_result(user_dir, &sub_name, "manpage", &sub_result);
                    }
                    any = true;
                }
            }
        }
    }
    any
}

fn group_subcommands_from_help(
    path: &Path,
    sub_args: &[String],
    timeout_ms: u64,
) -> Option<Vec<ManpageSubcommand>> {
    let text = if sub_args.is_empty() {
        try_help(path, timeout_ms)
    } else {
        let bin_s = path.to_string_lossy().to_string();
        try_help_args(&bin_s, sub_args, timeout_ms)
    }?;
    let help = parse_help_text(&text);
    (!help.subcommands.is_empty()).then_some(help.subcommands)
}

fn help_result_for_command(
    path: &Path,
    sub_args: &[String],
    timeout_ms: u64,
) -> Option<ManpageResult> {
    let text = if sub_args.is_empty() {
        try_help(path, timeout_ms)
    } else {
        let bin_s = path.to_string_lossy().to_string();
        try_help_args(&bin_s, sub_args, timeout_ms)
    }?;
    let result = parse_help_text(&text);
    if let Some(leaf) = sub_args.last()
        && result
            .subcommands
            .iter()
            .any(|sc| sc.name.eq_ignore_ascii_case(leaf))
    {
        return None;
    }
    Some(result)
}

fn entry_has_long(result: &ManpageResult, long: &str) -> bool {
    result.entries.iter().any(|e| match &e.switch {
        OwnedSwitch::Long(name) | OwnedSwitch::Both(_, name) => name == long,
        OwnedSwitch::Short(_) => false,
    })
}

fn entry_has_short(result: &ManpageResult, short: char) -> bool {
    result.entries.iter().any(|e| match &e.switch {
        OwnedSwitch::Short(c) | OwnedSwitch::Both(c, _) => *c == short,
        OwnedSwitch::Long(_) => false,
    })
}

fn fill_flag_alias_from_help(result: &mut ManpageResult, short: char, long: &str) -> bool {
    if !entry_has_long(result, long) {
        for entry in &mut result.entries {
            if matches!(entry.switch, OwnedSwitch::Short(c) if c == short) {
                entry.switch = OwnedSwitch::Both(short, long.to_string());
                return true;
            }
        }
    }
    if !entry_has_short(result, short) {
        for entry in &mut result.entries {
            if matches!(&entry.switch, OwnedSwitch::Long(name) if name == long) {
                entry.switch = OwnedSwitch::Both(short, long.to_string());
                return true;
            }
        }
    }
    false
}

fn supplement_result_from_help(result: &mut ManpageResult, help: &ManpageResult) -> bool {
    let mut changed = false;

    if result.description.is_empty() && !help.description.is_empty() {
        result.description = help.description.clone();
        changed = true;
    }

    for help_entry in &help.entries {
        match &help_entry.switch {
            OwnedSwitch::Both(short, long) => {
                if fill_flag_alias_from_help(result, *short, long) {
                    changed = true;
                    continue;
                }
                if entry_has_short(result, *short) || entry_has_long(result, long) {
                    continue;
                }
            }
            OwnedSwitch::Long(long) if entry_has_long(result, long) => continue,
            OwnedSwitch::Short(short) if entry_has_short(result, *short) => continue,
            _ => {}
        }
        result.entries.push(help_entry.clone());
        changed = true;
    }

    for help_sub in &help.subcommands {
        match result
            .subcommands
            .iter_mut()
            .find(|sc| sc.name.eq_ignore_ascii_case(&help_sub.name))
        {
            Some(existing) => {
                if existing.desc.is_empty() && !help_sub.desc.is_empty() {
                    existing.desc = help_sub.desc.clone();
                    changed = true;
                }
                // manpage subs carry no aliases; adopt help's (`help | h`).
                if existing.aliases.is_empty() && !help_sub.aliases.is_empty() {
                    existing.aliases = help_sub.aliases.clone();
                    changed = true;
                }
            }
            None => {
                result.subcommands.push(help_sub.clone());
                changed = true;
            }
        }
    }

    for (name, positional) in &help.positionals {
        if !result
            .positionals
            .iter()
            .any(|(existing, _)| existing.eq_ignore_ascii_case(name))
        {
            result.positionals.push((name.clone(), positional.clone()));
            changed = true;
        }
    }

    if changed {
        result.normalize();
    }
    changed
}

fn supplement_result_from_help_command(
    result: &mut ManpageResult,
    path: &Path,
    sub_args: &[String],
    timeout_ms: u64,
) -> bool {
    help_result_for_command(path, sub_args, timeout_ms)
        .as_ref()
        .is_some_and(|help| supplement_result_from_help(result, help))
}

// filesystem/subprocess-backed `Probe` for one binary. the one place the
// manpage+help supplement and group-recovery I/O lives, shared by runtime and
// pool drivers.
struct RealProbe<'a> {
    path: &'a Path,
    mandirs: &'a [PathBuf],
    user_dir: &'a Path,
    timeout_ms: u64,
    deadline: Instant,
    // indexer's `--help-only` list forces straight to `--help`; runtime
    // resolution never sets this.
    skip_manpage: bool,
}

impl RealProbe<'_> {
    fn step_timeout(&self) -> u64 {
        self.timeout_ms.min(remaining_ms(self.deadline))
    }
}

impl Probe for RealProbe<'_> {
    fn classify(&self) -> NodeClass {
        match classify_binary(self.path, self.path) {
            Classify::TryHelp => NodeClass::TryHelp,
            Classify::HasNativeCompletions => NodeClass::HasNativeCompletions,
            Classify::Skip => NodeClass::Skip,
        }
    }

    fn native_completions(&self) -> Option<String> {
        try_native_completion(self.path, self.timeout_ms)
    }

    fn manpage(&self, hyphenated: &str) -> Option<String> {
        if self.skip_manpage {
            return None;
        }
        let mp_path = find_manpage_path(self.mandirs, hyphenated)?;
        read_manpage_file(&mp_path).ok()
    }

    fn help_text(&self, sub_args: &[String]) -> Option<String> {
        if sub_args.is_empty() {
            try_help(self.path, self.step_timeout())
        } else {
            let bin_s = self.path.to_string_lossy().to_string();
            try_help_args(&bin_s, sub_args, self.step_timeout())
        }
    }

    fn supplement_from_help(&self, result: &mut ManpageResult, sub_args: &[String]) -> bool {
        supplement_result_from_help_command(result, self.path, sub_args, self.step_timeout())
    }

    fn group_children(
        &self,
        hyphenated: &str,
        sub_args: &[String],
    ) -> Option<Vec<ManpageSubcommand>> {
        // prefer the manpage route: index sibling `cmd-sub.N` pages, which a
        // later `subcommands_of` surfaces, so return None with nothing to graft.
        // fall back to --help only when no sibling pages exist.
        if index_sibling_manpages(self.user_dir, self.mandirs, hyphenated) {
            return None;
        }
        group_subcommands_from_help(self.path, sub_args, self.step_timeout())
    }
}

// resolver core wired with this crate's parser/strip/group-detection callbacks.
fn resolve_one(probe: &RealProbe, base_cmd: &str, sub_args: &[String]) -> Outcome {
    resolve_node(
        probe,
        base_cmd,
        sub_args,
        &parse_manpage_string,
        &parse_help_text,
        &strip_subcmd_prefix,
        &looks_like_unenumerated_group,
    )
}

fn resolve_command_path_and_cache(
    user_dir: &Path,
    mandirs: &[PathBuf],
    base_cmd: &str,
    sub_args: &[String],
    path: &Path,
    timeout_ms: u64,
) -> Option<ManpageResult> {
    let deadline =
        Instant::now() + Duration::from_millis(timeout_ms.saturating_mul(RESOLVE_BUDGET_MULTIPLE));
    let probe = RealProbe {
        path,
        mandirs,
        user_dir,
        timeout_ms,
        deadline,
        skip_manpage: false,
    };
    let full = resolver::full_cmd(base_cmd, sub_args);
    match resolve_one(&probe, base_cmd, sub_args) {
        Outcome::Native { nu } => {
            let _ = write_native(user_dir, base_cmd, &nu);
            Some(parse_nu_completions(&full, &nu))
        }
        Outcome::Empty => None,
        Outcome::Content {
            result,
            source,
            children,
        } => {
            let _ = write_result(user_dir, &full, source, &result);
            // only --help eagerly populates the subtree; manpage children are
            // found on demand via sibling pages.
            if source == "help" {
                resolve_subtree(&probe, base_cmd, sub_args, children, deadline);
            }
            Some(result)
        }
    }
}

// bounded by the result cap and shared deadline.
fn resolve_subtree(
    probe: &RealProbe,
    base_cmd: &str,
    base_sub_args: &[String],
    roots: Vec<String>,
    deadline: Instant,
) {
    let mut queue: std::collections::VecDeque<Vec<String>> = roots
        .into_iter()
        .map(|c| {
            let mut v = base_sub_args.to_vec();
            v.push(c);
            v
        })
        .collect();
    let mut count = 0usize;
    while let Some(sub) = queue.pop_front() {
        if count >= MAX_RESOLVE_RESULTS || Instant::now() >= deadline {
            break;
        }
        if (sub.len() - base_sub_args.len()) as u32 > MAX_RECURSE_DEPTH {
            continue;
        }
        if let Outcome::Content {
            result,
            source,
            children,
        } = resolve_one(probe, base_cmd, &sub)
        {
            let full = resolver::full_cmd(base_cmd, &sub);
            let _ = write_result(probe.user_dir, &full, source, &result);
            count += 1;
            for c in children {
                let mut next = sub.clone();
                next.push(c);
                queue.push_back(next);
            }
        }
    }
}

const ELEVATION_COMMANDS: &[&str] = &["sudo", "doas", "pkexec", "su", "run0"];

fn switch_takes_value(result: &ManpageResult, token: &str) -> bool {
    if token.contains('=') {
        return false;
    }
    result.entries.iter().any(|entry| {
        if entry.param.is_none() {
            return false;
        }
        match &entry.switch {
            OwnedSwitch::Long(long) => token == format!("--{long}"),
            OwnedSwitch::Short(short) => {
                let mut chars = token.chars();
                matches!(
                    (chars.next(), chars.next(), chars.next()),
                    (Some('-'), Some(c), None) if c == *short
                )
            }
            OwnedSwitch::Both(short, long) => {
                token == format!("--{long}") || {
                    let mut chars = token.chars();
                    matches!(
                        (chars.next(), chars.next(), chars.next()),
                        (Some('-'), Some(c), None) if c == *short
                    )
                }
            }
        }
    })
}

fn lookup_path_tokens(dirs: &[PathBuf], cmd_name: &str, rest: &[String]) -> Vec<String> {
    let mut tokens = vec![cmd_name.to_string()];
    let mut current = lookup(dirs, cmd_name);
    let mut skip_next_value = false;

    for token in rest {
        if token.is_empty() {
            tokens.push(token.clone());
            continue;
        }
        if skip_next_value {
            skip_next_value = false;
            continue;
        }
        if token.starts_with('-') {
            if current
                .as_ref()
                .is_some_and(|result| switch_takes_value(result, token))
            {
                skip_next_value = true;
            }
            continue;
        }

        // map an alias to its canonical child name (cargo `b` -> build) so the
        // path resolves to the real node and descends into its flags/subs.
        let resolved = current
            .as_ref()
            .and_then(|r| canonical_for_alias(r, token))
            .unwrap_or_else(|| token.clone());
        tokens.push(resolved);
        let name = tokens
            .iter()
            .filter(|t| !t.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        current = lookup(dirs, &name).or(current);
    }

    tokens
}

/// canonical child name for a token that matches one of `result`'s subcommand
/// aliases (not its name), else None.
fn canonical_for_alias(result: &ManpageResult, token: &str) -> Option<String> {
    result.subcommands.iter().find_map(|sc| {
        sc.aliases
            .iter()
            .any(|a| a.eq_ignore_ascii_case(token))
            .then(|| sc.name.clone())
    })
}

// a user-cache set is stale when its newest file is older than the ttl. ttl 0
// disables; system-dir hits aren't in the user cache so they never go stale.
fn cache_is_stale(
    user_dir: &Path,
    found: Option<&(String, ManpageResult, usize)>,
    ttl_secs: u64,
) -> bool {
    ttl_secs > 0
        && found.is_some_and(|(name, _, _)| {
            store::user_cache_age(user_dir, name).is_some_and(|age| age.as_secs() > ttl_secs)
        })
}

fn cmd_complete(
    spans: &[String],
    user_dir: &Path,
    system_dirs: &[PathBuf],
    mandirs: &[PathBuf],
    timeout_ms: u64,
    cfg: &Config,
) {
    let mut dirs: Vec<PathBuf> = Vec::with_capacity(system_dirs.len() + 1);
    dirs.push(user_dir.to_path_buf());
    dirs.extend(system_dirs.iter().cloned());

    // skip past elevation wrappers (sudo, doas) to the real command.
    let mut explicit_cmd_path: Option<PathBuf> = None;
    let mut spans: Vec<String> = match spans.first() {
        Some(first) if ELEVATION_COMMANDS.contains(&first.as_str()) => {
            let rest = &spans[1..];
            let mut real_spans = None;
            for (idx, s) in rest.iter().enumerate() {
                if let Some(path) = executable_span_path(s)
                    && let Some(name) = command_name_for_path(&path)
                {
                    let mut target = rest[idx..].to_vec();
                    target[0] = name;
                    explicit_cmd_path = Some(path);
                    real_spans = Some(target);
                    break;
                }
                if !s.is_empty()
                    && !s.starts_with('-')
                    && (lookup(&dirs, s).is_some() || find_in_path(s).is_some())
                {
                    real_spans = Some(rest[idx..].to_vec());
                    break;
                }
            }
            real_spans.unwrap_or_else(|| spans.to_vec())
        }
        _ => spans.to_vec(),
    };
    if explicit_cmd_path.is_none()
        && let Some(first) = spans.first()
        && let Some(path) = executable_span_path(first)
        && let Some(name) = command_name_for_path(&path)
    {
        spans[0] = name;
        explicit_cmd_path = Some(path);
    }

    if spans.is_empty() || (explicit_cmd_path.is_none() && find_in_path(&spans[0]).is_none()) {
        println!("null");
        return;
    }

    let cmd_name = spans[0].clone();
    let rest: Vec<String> = spans[1..].to_vec();

    if let Some(candidates) =
        dynamic_value_completions(&cmd_name, &rest, explicit_cmd_path.as_deref(), timeout_ms)
    {
        print_completion_candidates(&candidates);
        return;
    }

    let last_token = rest.last().cloned().unwrap_or_default();
    let complete_rest: &[String] = if last_token.is_empty() || rest.is_empty() {
        &rest
    } else {
        &rest[..rest.len() - 1]
    };
    let mut lookup_tokens = lookup_path_tokens(&dirs, &cmd_name, complete_rest);
    if last_token.is_empty()
        && !rest.is_empty()
        && !lookup_tokens.last().is_some_and(|t| t.is_empty())
    {
        lookup_tokens.push(String::new());
    }

    // try longest-prefix match: "git stash apply" → "git stash" → "git"
    let find_result = |toks: &[String]| -> Option<(String, ManpageResult, usize)> {
        let n = toks.len();
        for drop in 0..n {
            let prefix = &toks[..n - drop];
            if prefix.is_empty() {
                continue;
            }
            let name = prefix.join(" ");
            if let Some(r) = lookup(&dirs, &name) {
                return Some((name, r, prefix.len()));
            }
        }
        None
    };

    let mut found = find_result(&lookup_tokens);

    // nothing matched or only a parent matched, so try --help
    let resolve_tokens: Vec<String> = lookup_tokens
        .iter()
        .filter(|t| !t.is_empty())
        .cloned()
        .collect();
    let resolve_depth = resolve_tokens.len();
    // a stale hit re-resolves through the same path: the resolve block overwrites
    // the cache and falls back to the stale value if rescrape fails.
    let need_resolve = cache_is_stale(user_dir, found.as_ref(), cfg.cache_ttl_secs)
        || match &found {
            Some((_, _, depth)) => *depth < resolve_depth,
            None => resolve_depth > 0,
        };
    if need_resolve
        && let Some(path) = explicit_cmd_path
            .as_ref()
            .cloned()
            .or_else(|| find_in_path(&cmd_name))
    {
        // also search the binary's own prefix for manpages
        let mut all_mandirs = mandirs.to_vec();
        if let Some(parent) = path.parent()
            && let Some(prefix) = parent.parent()
        {
            let share_man = prefix.join("share/man");
            if share_man.is_dir() {
                all_mandirs.push(share_man);
            }
        }
        let sub_args = if resolve_tokens.len() > 1 {
            resolve_tokens[1..].to_vec()
        } else {
            Vec::new()
        };
        let resolved = if sub_args.is_empty() {
            resolve_and_cache(user_dir, &all_mandirs, &cmd_name, &path, timeout_ms)
        } else {
            resolve_command_path_and_cache(
                user_dir,
                &all_mandirs,
                &cmd_name,
                &sub_args,
                &path,
                timeout_ms,
            )
        };
        if resolved.is_some() {
            found = find_result(&lookup_tokens);
        }
    }

    let typing_flag = cfg.triggers_flags(&last_token);
    let fallback_subcommands = match &found {
        Some((matched_name, r, _)) if r.subcommands.is_empty() => {
            subcommands_of(&dirs, matched_name)
        }
        _ => Vec::new(),
    };
    // positional value choices (getent databases) fill the same slot as
    // subcommands, so they suppress the file/dynamic handoff too.
    let has_subs = match &found {
        Some((_, r, _)) => {
            !r.subcommands.is_empty()
                || !r.positional_choices.is_empty()
                || !fallback_subcommands.is_empty()
        }
        None => false,
    };
    let candidates: Vec<String> = match &found {
        None => Vec::new(),
        Some((_, r, depth)) => generate_candidates(
            r,
            *depth,
            resolve_depth,
            &last_token,
            &fallback_subcommands,
            typing_flag,
            cfg,
        )
        .into_iter()
        .map(Candidate::into_json)
        .collect(),
    };
    // hand off at non-flag leaf positions so file and dynamic completers can
    // answer argument prefixes. a leading "-" keeps flags.
    let want_files = !typing_flag && !has_subs && (last_token.is_empty() || candidates.is_empty());
    if want_files || candidates.is_empty() {
        // spans are post-elevation, so `sudo nix ...` reaches this as
        // `[nix, ...]` and hits the nix branch.
        if let Some(dyn_candidates) =
            dynamic_complete_with_path(&spans, explicit_cmd_path.as_deref(), cfg)
        {
            print_completion_candidates(&dyn_candidates);
        } else {
            println!("null");
        }
    } else {
        print_completion_candidates(&candidates);
    }
}

fn cmd_completions() {
    // inshellah's own surface is small enough that explicit externs beat the
    // parser-driven generator aimed at arbitrary cmds.
    print!(
        r#"module inshellah-completions {{
export extern "inshellah" [
    --help(-h)                      # show help
]

export extern "inshellah index" [
    ...prefix: path
    --dir: path                     # completion output directory
    --ignore: path                  # file of commands to skip
    --help-only: path               # file of commands to scrape with --help only
    --prefix: string                # extra colon-separated scrape prefixes
    --timeout-ms: int               # per-subprocess timeout in milliseconds
    --workers: int                  # parallel scrape workers
]

export extern "inshellah complete" [
    cmd: string
    ...args: string
    --dir: string                   # writable cache plus read-only dirs
    --timeout-ms: int               # on-the-fly scrape timeout in milliseconds
]

export extern "inshellah query" [
    cmd: string
    ...subcommand: string
    --dir: string                   # completion directories to read
]

export extern "inshellah dump" [
    --dir: string                   # completion directories to read
]

export extern "inshellah diff" [
    cmd?: string
    ...subcommand: string
    --dir: path                     # extra man directory to inspect
    --timeout-ms: int               # help scrape timeout in milliseconds
    --scan: path                    # scan a prefix for source divergence
]

export extern "inshellah purge" [
    --dir: string                   # writable cache plus read-only dirs
]

export extern "inshellah manpage" [
    file: path
]

export extern "inshellah manpage-dir" [
    dir: path
]

export extern "inshellah completions" []
}}

use inshellah-completions *
"#
    );
}

struct IndexArgs {
    prefixes: Vec<PathBuf>,
    dir: Option<PathBuf>,
    ignore: Option<PathBuf>,
    help_only: Option<PathBuf>,
    timeout_ms: u64,
    workers: usize,
}

#[derive(Parse, Debug)]
#[pound(name = "inshellah")]
enum Cli {
    /// index completions into a directory of JSON/nu files
    Index {
        #[pound(positional, value_name = "PREFIX")]
        prefixes: Vec<PathBuf>,
        #[pound(long)]
        dir: Option<PathBuf>,
        #[pound(long)]
        ignore: Option<PathBuf>,
        #[pound(long)]
        help_only: Option<PathBuf>,
        #[pound(long = "prefix", value_name = "PATHS")]
        extra_prefixes: Vec<String>,
        #[pound(long)]
        timeout_ms: Option<String>,
        #[pound(long)]
        workers: Option<String>,
    },
    /// parse a manpage and emit nushell extern
    Manpage { file: PathBuf },
    /// batch-process manpages under a directory
    ManpageDir { dir: PathBuf },
    /// nushell custom completer
    Complete {
        #[pound(long)]
        dir: Option<String>,
        #[pound(long)]
        timeout_ms: Option<String>,
        #[pound(positional, value_name = "SPAN")]
        spans: Vec<String>,
    },
    /// print stored completion data
    Query {
        #[pound(long)]
        dir: Option<String>,
        #[pound(positional, value_name = "CMD")]
        cmd: Vec<String>,
    },
    /// list indexed commands
    Dump {
        #[pound(long)]
        dir: Option<String>,
    },
    /// audit source divergence
    Diff {
        #[pound(long)]
        scan: Option<PathBuf>,
        #[pound(long)]
        dir: Option<String>,
        #[pound(long)]
        timeout_ms: Option<String>,
        #[pound(positional, value_name = "CMD")]
        cmd: Vec<String>,
    },
    /// delete the on-the-fly user cache
    Purge {
        #[pound(long)]
        dir: Option<String>,
    },
    /// generate nushell completions for inshellah
    Completions,
    #[pound(hidden)]
    Help,
}

#[cfg(test)]
#[derive(Parse, Debug)]
#[pound(name = "inshellah index")]
struct IndexCli {
    #[pound(positional, value_name = "PREFIX")]
    prefixes: Vec<PathBuf>,
    #[pound(long)]
    dir: Option<PathBuf>,
    #[pound(long)]
    ignore: Option<PathBuf>,
    #[pound(long)]
    help_only: Option<PathBuf>,
    #[pound(long = "prefix", value_name = "PATHS")]
    extra_prefixes: Vec<String>,
    #[pound(long)]
    timeout_ms: Option<String>,
    #[pound(long)]
    workers: Option<String>,
}

#[cfg(test)]
impl From<IndexCli> for IndexArgs {
    fn from(parsed: IndexCli) -> Self {
        index_args_from_parts(
            parsed.prefixes,
            parsed.dir,
            parsed.ignore,
            parsed.help_only,
            parsed.extra_prefixes,
            parsed.timeout_ms.as_deref(),
            parsed.workers.as_deref(),
        )
    }
}

fn index_args_from_parts(
    mut prefixes: Vec<PathBuf>,
    dir: Option<PathBuf>,
    ignore: Option<PathBuf>,
    help_only: Option<PathBuf>,
    extra_prefixes: Vec<String>,
    timeout_ms: Option<&str>,
    workers: Option<&str>,
) -> IndexArgs {
    prefixes.extend(split_colon_paths(extra_prefixes.iter().map(String::as_str)));
    IndexArgs {
        prefixes,
        dir,
        ignore,
        help_only,
        timeout_ms: timeout_ms
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(DEFAULT_TIMEOUT_MS),
        workers: workers
            .and_then(|n| n.parse::<usize>().ok())
            .map(|n| n.max(1))
            .unwrap_or_else(default_workers),
    }
}

#[cfg(test)]
fn parse_index_args(args: &[String]) -> IndexArgs {
    IndexCli::parse_from(args.iter().map(String::as_str)).into()
}

fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

fn man_dir_of_prefix(prefix: &Path) -> PathBuf {
    prefix.join("share/man")
}

// completer is pointed at `<prefix>/share/inshellah`, so manpages sit two
// levels up at `<prefix>/share/man`, the bin/share-man colocation `index`
// assumes.
fn mandir_for_completion_dir(dir: &Path) -> Option<PathBuf> {
    dir.parent().and_then(Path::parent).map(man_dir_of_prefix)
}

// timeout is `None` when unset so the caller can fall back to the configured
// default.
fn split_colon_paths<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<PathBuf> {
    values
        .into_iter()
        .flat_map(|value| value.split(':'))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn completion_dirs(dir: Option<&str>) -> Vec<PathBuf> {
    dir.map(|d| d.split(':').map(PathBuf::from).collect())
        .unwrap_or_else(|| vec![default_store_path()])
}

fn parse_timeout_ms(timeout_ms: Option<&str>) -> Option<u64> {
    timeout_ms.and_then(|n| n.parse::<u64>().ok())
}

const COMPLETE_DASH_ARG_SENTINEL: &str = "__INSHELLAH_COMPLETE_DASH_ARG__";
const COMPLETE_DOUBLE_DASH_SENTINEL: &str = "__INSHELLAH_LITERAL_DOUBLE_DASH__";

fn normalize_cli_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut args: Vec<String> = args.into_iter().collect();
    if args.first().is_some_and(|arg| arg == "complete") {
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--dir" || args[i] == "--timeout-ms" {
                i += 2;
                continue;
            }
            if args[i].starts_with("--dir=") || args[i].starts_with("--timeout-ms=") {
                i += 1;
                continue;
            }
            if args[i] == "--" {
                args[i] = COMPLETE_DOUBLE_DASH_SENTINEL.to_string();
            } else if args[i].starts_with('-') {
                args[i] = format!("{COMPLETE_DASH_ARG_SENTINEL}{}", args[i]);
            }
            i += 1;
        }
    }
    args
}

fn restore_complete_spans(spans: &mut [String]) {
    for span in spans {
        if span == COMPLETE_DOUBLE_DASH_SENTINEL {
            *span = "--".to_string();
        } else if let Some(rest) = span.strip_prefix(COMPLETE_DASH_ARG_SENTINEL) {
            *span = rest.to_string();
        }
    }
}

fn main() {
    // rust ignores SIGPIPE, so a broken-pipe write becomes a BrokenPipe error
    // that `println!` panics on. restore the default so piping into `head` exits
    // quietly.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if raw_args.is_empty() {
        usage();
        std::process::exit(1);
    }
    if raw_args
        .first()
        .is_some_and(|arg| arg == "-h" || arg == "--help")
    {
        usage();
        return;
    }
    let args = normalize_cli_args(raw_args);
    match Cli::parse_from(args.iter().map(String::as_str)) {
        Cli::Index {
            prefixes,
            dir,
            ignore,
            help_only,
            extra_prefixes,
            timeout_ms,
            workers,
        } => {
            let parsed = index_args_from_parts(
                prefixes,
                dir,
                ignore,
                help_only,
                extra_prefixes,
                timeout_ms.as_deref(),
                workers.as_deref(),
            );
            if parsed.prefixes.is_empty() {
                eprintln!("error: index requires at least one PREFIX");
                std::process::exit(1);
            }
            let dir = parsed.dir.unwrap_or_else(default_store_path);
            let ignorelist = parsed
                .ignore
                .as_deref()
                .map(load_ignorelist)
                .unwrap_or_default();
            let help_only = parsed
                .help_only
                .as_deref()
                .map(load_ignorelist)
                .unwrap_or_default();
            let bindirs: Vec<PathBuf> = parsed.prefixes.iter().map(|p| p.join("bin")).collect();
            let mandirs: Vec<PathBuf> = parsed
                .prefixes
                .iter()
                .map(|p| man_dir_of_prefix(p))
                .collect();
            if let Err(e) = cmd_index(
                &bindirs,
                &mandirs,
                &ignorelist,
                &help_only,
                &dir,
                parsed.timeout_ms,
                parsed.workers,
            ) {
                eprintln!("index failed: {e}");
                std::process::exit(1);
            }
        }
        Cli::Manpage { file } => {
            if let Err(e) = cmd_manpage(&file) {
                eprintln!("manpage failed: {e}");
                std::process::exit(1);
            }
        }
        Cli::ManpageDir { dir } => {
            if let Err(e) = cmd_manpage_dir(&dir) {
                eprintln!("manpage-dir failed: {e}");
                std::process::exit(1);
            }
        }
        Cli::Complete {
            dir,
            timeout_ms,
            mut spans,
        } => {
            restore_complete_spans(&mut spans);
            let cfg = Config::from_env();
            let dirs = completion_dirs(dir.as_deref());
            let timeout_override = parse_timeout_ms(timeout_ms.as_deref());
            let timeout_ms = timeout_override.unwrap_or(cfg.timeout_ms);
            // first dir is the writable user cache; rest are read-only system dirs
            let (user_dir, system_dirs): (PathBuf, Vec<PathBuf>) = match dirs.split_first() {
                Some((first, rest)) => (first.clone(), rest.to_vec()),
                None => (default_store_path(), Vec::new()),
            };
            let mandirs: Vec<PathBuf> = system_dirs
                .iter()
                .filter_map(|d| mandir_for_completion_dir(d))
                .filter(|p| p.is_dir())
                .collect();
            cmd_complete(&spans, &user_dir, &system_dirs, &mandirs, timeout_ms, &cfg);
        }
        Cli::Query { dir, cmd } => {
            let dirs = completion_dirs(dir.as_deref());
            if cmd.is_empty() {
                eprintln!("error: query requires a CMD argument");
                std::process::exit(1);
            }
            let cmd = cmd.join(" ");
            if let Err(e) = cmd_query(&cmd, &dirs) {
                eprintln!("query failed: {e}");
                std::process::exit(1);
            }
        }
        Cli::Dump { dir } => {
            let dirs = completion_dirs(dir.as_deref());
            cmd_dump(&dirs);
        }
        Cli::Diff {
            scan,
            dir,
            timeout_ms,
            cmd,
        } => {
            let cfg = Config::from_env();
            if let Some(prefix) = scan {
                cmd_diff_scan(&prefix, cfg.timeout_ms);
            } else {
                let dirs = completion_dirs(dir.as_deref());
                let timeout_override = parse_timeout_ms(timeout_ms.as_deref());
                if cmd.is_empty() {
                    eprintln!("error: diff requires a CMD argument");
                    std::process::exit(1);
                }
                cmd_diff(&cmd, &dirs, timeout_override.unwrap_or(cfg.timeout_ms));
            }
        }
        Cli::Purge { dir } => {
            let dirs = completion_dirs(dir.as_deref());
            // only the writable user dir is purged, never the system overlays
            let user_dir = dirs.first().cloned().unwrap_or_else(default_store_path);
            cmd_purge(&user_dir);
        }
        Cli::Completions => cmd_completions(),
        Cli::Help => usage(),
    }
    let _ = filename_of_command;
}
