// SPDX-License-Identifier: EUPL-1.2
//! inshellah CLI.
//!
//! subcommands:
//!   index PREFIX...     scan PREFIX/bin and PREFIX/share/man, write JSON cache
//!   manpage FILE        parse a single manpage, emit nushell extern
//!   manpage-dir DIR     batch-process manpages under DIR
//!   complete CMD ARG... nushell external completer; reads the cache,
//!                       falls back to on-the-fly --help if uncached
//!   query CMD           print stored data for CMD
//!   dump                list indexed commands
//!   completions         emit nushell completion definitions for inshellah itself

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

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
    all_commands, default_store_path, ensure_dir, file_type_of, filename_of_command, lookup,
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
       default 10000 — bounds runaway recursion on pathological trees)
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
"
    );
}

// --- file classification ---

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

/// skip filenames that aren't real commands (e.g. doc/locale paths).
fn skip_name(name: &str) -> bool {
    name.starts_with('.')
        || name.ends_with(".so")
        || name.ends_with(".a")
        || name.ends_with(".la")
        || name.contains('/')
}

// --- executable image scanning ---

/// is `magic` the leading 4 bytes of an executable image we know how to
/// string-scan on *this* platform? the scan itself is byte-oriented and
/// format-agnostic; this gate just keeps us from slurping data files that
/// happen to carry the executable bit.
///
/// recognition is strictly per-platform: a macOS build honours only Mach-O
/// (thin 32/64-bit either endianness, plus fat/universal), every other
/// (ELF) target honours only ELF. keeping them mutually exclusive means a
/// Linux build never treats `CA FE BA BE` as an image — that's FAT_MAGIC to
/// Mach-O but also a Java class file, which a Linux box can plausibly carry.
fn is_scannable_magic(magic: &[u8; 4]) -> bool {
    #[cfg(target_os = "macos")]
    {
        matches!(
            magic,
            [0xce, 0xfa, 0xed, 0xfe]   // MH_MAGIC    (thin 32-bit, little-endian)
                | [0xcf, 0xfa, 0xed, 0xfe] // MH_MAGIC_64 (thin 64-bit, little-endian)
                | [0xfe, 0xed, 0xfa, 0xce] // MH_MAGIC    (thin 32-bit, big-endian)
                | [0xfe, 0xed, 0xfa, 0xcf] // MH_MAGIC_64 (thin 64-bit, big-endian)
                | [0xca, 0xfe, 0xba, 0xbe] // FAT_MAGIC   (universal)
                | [0xca, 0xfe, 0xba, 0xbf] // FAT_MAGIC_64
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        magic == b"\x7fELF"
    }
}

/// scan an executable image (ELF on Linux, Mach-O on macOS) for string needles.
/// returns the set of needles that appeared. on read failure all needles are
/// reported found (conservative — we'd rather try --help than skip).
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
        // not a recognised executable image — return empty so caller decides
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

// --- nix wrapper detection ---

fn read_to_string_capped(path: &Path, cap: usize) -> Option<String> {
    let real = fs::canonicalize(path).ok()?;
    let md = fs::metadata(&real).ok()?;
    if md.len() as usize > cap {
        return None;
    }
    fs::read_to_string(&real).ok()
}

/// detect nix-generated c wrappers; return the real binary path.
fn nix_wrapper_target(path: &Path) -> Option<PathBuf> {
    let contents = read_to_string_capped(path, 65536)?;
    if !contents.contains("makeCWrapper") {
        return None;
    }
    // pattern: /nix/store/<hash>-<name>/bin/<exe>
    extract_nix_bin_path(&contents)
}

/// detect nix-generated bash/sh wrappers.
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
        // find end of the path (whitespace, quote, or null)
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

// --- binary classification ---

#[derive(Debug, Clone, PartialEq, Eq)]
enum Classify {
    /// can try --help
    TryHelp,
    /// the tool likely speaks the "nushell" completion subcommand
    HasNativeCompletions,
    /// skip — doesn't look like a CLI we can extract from
    Skip,
}

/// classify an executable image by scanning for help/completion needles.
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

/// classify a binary by its actual nature: script, native image, or nix
/// wrapper. native images are ELF on Linux and Mach-O on macOS.
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

// --- help text extraction ---

/// try `--help`, then `-h`, returning the first non-empty output (with
/// ANSI escapes stripped). each attempt gets the same per-call timeout.
/// we deliberately skip the third historical `help`-subcommand variant:
/// if neither flag yielded usable text, a positional `help` is unlikely
/// to do anything different and the extra spawn dominates indexing cost.
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

/// look for words that contain a known needle within the text (used to
/// find subcommand names that might be a native-completion command).
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

/// try to get native nushell completions from a binary that supports them.
fn try_native_completion(bin: &Path, timeout_ms: u64) -> Option<String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let help_text = try_help_until(bin, timeout_ms, deadline)?;
    // look for words like "completion", "completions" — typical subcommand
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

// --- subcommand recursion ---

const MAX_RESOLVE_RESULTS: usize = 500;
const MAX_RECURSE_DEPTH: u32 = 10;
const RESOLVE_BUDGET_MULTIPLE: u64 = 8;

/// default per-root cap on indexed subcommand nodes, overridable via
/// `INSHELLAH_MAX_INDEX_NODES`. real CLIs — even giants like gcloud/aws/
/// kubectl — top out in the low thousands of total subcommands, so 10k is
/// already beyond belief; it still bounds a pathological tree (one that emits
/// fresh subcommand names at every level, so `self_listed` never fires) to a
/// fixed amount of work instead of breadth^depth runaway. see
/// `enqueue_child_jobs`.
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

/// try `bin sub_path... --help` first, then `... -h` if --help came back
/// empty or "No manual entry…". used by deep subcommand recursion.
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

// --- manpage handling ---

fn cmd_name_of_manpage(path: &Path) -> String {
    let mut base = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    if base.ends_with(".gz") {
        base.truncate(base.len() - 3);
    }
    // strip section suffix: "ls.1" -> "ls"
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

/// derive the command name a manpage documents. the SYNOPSIS section
/// is authoritative because manpage filenames are ambiguous —
/// "btrfs-check.8" could mean either a standalone binary `btrfs-check`
/// or the subcommand `btrfs check`. we clamp to the number of
/// hyphen-separated parts in the filename to prevent synopsis lines
/// like "btrfs check [options] <device>" from absorbing the device
/// placeholder into the command name.
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

/// process a manpage and return (cmd_name, main_result, per-subcommand results).
/// the sub_results come from clap-style `.SH SUBCOMMAND` sections — each is
/// a self-contained command with its own flags.
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
    // namespace the sub-section names under the resolved cmd name:
    // e.g. nh's SUBCOMMAND "os" becomes the stored command "nh os".
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

// --- index command ---

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

    // fuzzy_score / completion_json ranking + escaping are covered by the
    // canonical module's own tests (src/complete.rs).

    #[test]
    fn completion_dir_mandir_resolves_to_prefix_share_man() {
        // <prefix>/share/inshellah -> <prefix>/share/man, no doubled "share".
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
        // positional first, then each --prefix segment, in order.
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
        // a PNG header, a shebang, plain text — none are images on any platform.
        assert!(!is_scannable_magic(&[0x89, b'P', b'N', b'G']));
        assert!(!is_scannable_magic(b"#!/b"));
        assert!(!is_scannable_magic(b"text"));
    }

    // recognition is strictly per-platform: each build honours only its
    // native container and rejects the other.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_scans_mach_o_only() {
        // thin 64-bit little-endian — the common arm64/x86_64 layout.
        assert!(is_scannable_magic(&[0xcf, 0xfa, 0xed, 0xfe]));
        // fat/universal.
        assert!(is_scannable_magic(&[0xca, 0xfe, 0xba, 0xbe]));
        // ELF is not a native macOS image.
        assert!(!is_scannable_magic(b"\x7fELF"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn elf_targets_scan_elf_only() {
        assert!(is_scannable_magic(b"\x7fELF"));
        // Mach-O magics are rejected; FAT_MAGIC also collides with java class.
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

/// shared state passed to every pool worker. nothing inside mutates
/// except `indexed`, which is wrapped in a parking_lot::Mutex.
struct ScrapeCtx {
    cache_dir: PathBuf,
    mandirs: Vec<PathBuf>,
    help_only: HashSet<String>,
    indexed: Mutex<HashSet<String>>,
    timeout_ms: u64,
    /// per-root fan-out budget and its bookkeeping (see `enqueue_child_jobs`).
    /// `node_counts` tallies enqueued children per root command; `truncated`
    /// records roots already warned about so the message fires once.
    node_budget: usize,
    node_counts: Mutex<std::collections::HashMap<String, usize>>,
    truncated: Mutex<HashSet<String>>,
}

#[derive(Debug)]
struct PoolJob {
    bin_path: PathBuf,
    /// the binary's basename — e.g. "git". stays constant across the
    /// whole recursion tree for this binary.
    base_cmd: String,
    /// chain of subcommand tokens past the base. empty for the
    /// top-level scrape, ["clone"] for `git clone`, ["stash","apply"]
    /// for `git stash apply`.
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

/// some manpages list subcommands with the parent's name as a prefix —
/// git.1 has \fBgit-add\fR(1), \fBgit-remote-ext\fR(1), etc. downstream
/// expects bare subcommand names ("add", "remote-ext") so they dispatch
/// as `git add` / `git remote-ext`. strips a leading "{base}-" wherever
/// present; a no-op when the manpage already uses bare names.
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

/// enqueue one child job per discovered subcommand token. tokens are already
/// filtered by the resolver core (`child_tokens`: len >= 2, not a flag, not
/// `help`).
///
/// two bounds keep a pathological tree from exploding into breadth^depth work.
/// `MAX_RECURSE_DEPTH` caps how deep we descend; the per-root node budget caps
/// the total subcommands indexed under a single root. the depth cap alone is
/// not enough — `self_listed` in the resolver only drops a child that echoes
/// its parent's menu, so a tool that invents fresh names at every level walks
/// straight past it, and depth^breadth is still enormous at depth 10.
fn enqueue_child_jobs(
    ctx: &ScrapeCtx,
    job: &PoolJob,
    children: &[String],
    submit: &Submitter<PoolJob>,
) {
    // depth check is `> MAX`, not `>=`, so the last discovered layer is still
    // indexed rather than cut off — deep clap/kubectl/gcloud trees go far.
    if job.depth > MAX_RECURSE_DEPTH {
        return;
    }
    // per-root fan-out budget. base_cmd is constant down the whole tree, so a
    // single command's subtree is bounded without touching the breadth of a
    // full system scan: every top-level binary gets its own allowance.
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

/// per-job handler called by every worker. populates the cache + enqueues
/// child jobs (one per discovered subcommand) onto the same pool. shares the
/// resolver core with the runtime path; the only index-specific concerns are
/// the `Skip` classification (don't index non-CLIs), the `--help-only` list
/// (skip the manpage source), and persisting before marking a command indexed.
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
        // the pool bounds total work and each subprocess is timeout-capped;
        // there is no per-job wall-clock budget, so leave the deadline open.
        deadline: Instant::now() + Duration::from_secs(86_400),
        skip_manpage: ctx.help_only.contains(&job.base_cmd)
            || ctx.help_only.contains(&full_cmd),
    };

    // non-CLIs are skipped entirely (top-level classification only).
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
            // mark indexed only after a successful write, so a failed persist
            // doesn't leave a command falsely recorded as done.
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

    // phase 1: parallel scrape of every eligible binary via the BFS pool.
    // shared state lives in an Arc<ScrapeCtx>; the `indexed` set is the
    // one mutable bit and uses parking_lot::Mutex.
    let ctx = Arc::new(ScrapeCtx {
        cache_dir: dir.to_path_buf(),
        mandirs: mandirs.to_vec(),
        help_only: help_only.clone(),
        indexed: Mutex::new(HashSet::new()),
        timeout_ms,
        // 0 / unparseable falls back to the default; we never honour an
        // unbounded budget here — that is the runaway this guards against.
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
    // unwrap the indexed set back out for phase 2 — by this point no
    // workers are alive so the Arc has only one strong reference.
    let mut indexed: HashSet<String> = Arc::try_unwrap(ctx)
        .ok()
        .map(|c| c.indexed.into_inner())
        .unwrap_or_default();

    // process manpages for commands not yet indexed (unless they're in help-only).
    // shorter filenames sort first so parent manpages (e.g. nix-env.1) are
    // processed before subpage manpages (nix-env-install.1).
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
        // clap-style SUBCOMMAND sections produce real, fully-populated
        // sub-files (each with its own flags + positionals); they take
        // priority over COMMANDS-section leaf stubs.
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
        // for COMMANDS-section subcommands that aren't already covered by
        // a SUBCOMMAND section (or a per-subcommand manpage), write a
        // description-only stub so the completer treats them as leaves.
        // a real per-subcommand manpage processed later will overwrite the
        // stub since we deliberately don't add it to `indexed`.
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

// --- manpage subcommand ---

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

// --- query / dump / complete ---

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

/// derive man directories to search for a binary: the install prefix
/// colocated with `<prefix>/bin/<name>`, plus common system locations.
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

/// canonical identity for a flag, keyed on the long name when present so
/// `-v`/`--verbose` (manpage) and `--verbose` (help) compare equal.
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

/// dev-time source-divergence audit: parse a command's manpage and its
/// `--help` independently and report where they disagree, so parser gaps
/// (structure one source captures and the other drops) surface instead of
/// being silently masked by the manpage>help fallback. `cmd_args` is the
/// full command path, e.g. ["jj", "bookmark"].
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

    // the jj-class gap: the manpage body enumerates no children but help
    // does. note whether sibling `cmd-sub.1` pages cover them (the manpage
    // route is intact, just not in the parent body) or not (help is the
    // only source).
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

/// does this result look like a group command whose children we failed to
/// enumerate — a leftover `<command>`/`<subcommands>` synopsis placeholder
/// with no subcommands populated?
fn looks_like_unenumerated_group(r: &ManpageResult) -> bool {
    r.subcommands.is_empty()
        && r.positionals.iter().any(|(n, _)| {
            matches!(
                n.to_ascii_lowercase().as_str(),
                "command" | "commands" | "subcommand" | "subcommands"
            )
        })
}

/// scan a prefix's man1 pages for group commands whose manpage body
/// enumerates no children, then probe `--help` to see whether the children
/// are recoverable there. reports parser gaps (body should enumerate but
/// doesn't) and help-only gaps (no sibling page; help is the only source).
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

/// purge the on-the-fly user cache. only the writable user dir is cleared;
/// read-only system overlays are never touched.
fn cmd_purge(user_dir: &Path) {
    match purge_dir(user_dir) {
        Ok(n) => println!("purged {n} cached entries from {}", user_dir.display()),
        Err(e) => {
            eprintln!("purge failed: {e}");
            std::process::exit(1);
        }
    }
}

/// look up a command's path in $PATH.
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

/// compute completion match quality. zero means no match.
///
/// scoring tiers:
/// - exact match: 1000
/// - prefix match: 900 + length bonus
/// - subsequence match: per-character score with bonuses for word boundaries
///   and consecutive matches
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

/// dynamically scrape --help for a command not in the cache, write the result
/// into the user store, and return its parsed form. discovered subcommands
/// are also written.
fn resolve_and_cache(
    user_dir: &Path,
    mandirs: &[PathBuf],
    cmd_name: &str,
    path: &Path,
    timeout_ms: u64,
) -> Option<ManpageResult> {
    resolve_command_path_and_cache(user_dir, mandirs, cmd_name, &[], path, timeout_ms)
}

/// index every descendant manpage of a group command (`cmd-*.N`) into the
/// user cache, resolving each page's real space-separated name from its
/// own content. returns whether anything was indexed. lets `subcommands_of`
/// surface a group's children when its parent page didn't enumerate them.
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

/// scrape a group command's children from `--help` (subcommands only).
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
            Some(existing) if existing.desc.is_empty() && !help_sub.desc.is_empty() => {
                existing.desc = help_sub.desc.clone();
                changed = true;
            }
            Some(_) => {}
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

/// a manpage resolved to a group command whose body enumerated no children.
/// recover them without discarding the manpage's flags: prefer the manpage
/// route (index the sibling `cmd-sub.N` pages, which `subcommands_of` then
/// surfaces); fall back to `--help` only when no sibling pages exist. used
/// by the on-the-fly resolver, which (unlike `index`) doesn't otherwise
/// walk the sibling pages.
/// Filesystem/subprocess-backed [`Probe`] for one binary. Bound to a single
/// binary path; the resolver core drives it per node. This is the one place
/// the manpage⊕help supplement and group-recovery I/O lives, shared by the
/// runtime driver below and the indexer's pool driver.
struct RealProbe<'a> {
    path: &'a Path,
    mandirs: &'a [PathBuf],
    user_dir: &'a Path,
    timeout_ms: u64,
    deadline: Instant,
    /// the indexer's `--help-only` list forces some commands past the manpage
    /// source straight to `--help`; runtime resolution never sets this.
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
        // prefer the manpage route: index the sibling `cmd-sub.N` pages, which
        // a later `subcommands_of` lookup surfaces (so return None — nothing to
        // graft inline). only fall back to --help when no sibling pages exist.
        if index_sibling_manpages(self.user_dir, self.mandirs, hyphenated) {
            return None;
        }
        group_subcommands_from_help(self.path, sub_args, self.step_timeout())
    }
}

/// the parser/strip/group-detection callbacks the resolver core needs,
/// gathered once so the two call sites stay identical.
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
            // only the --help branch eagerly populates the subtree; manpage
            // children are found on demand via their own sibling pages.
            if source == "help" {
                resolve_subtree(&probe, base_cmd, sub_args, children, deadline);
            }
            Some(result)
        }
    }
}

/// Breadth-first resolve+cache of the subtree under a node, bounded by the
/// result cap and the shared deadline. Replaces the old
/// `help_resolve`/`recurse_subcommand` pair, which expressed this same shape.
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

        tokens.push(token.clone());
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

    // skip past elevation wrappers (sudo, doas) to find the real command
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

    if spans.is_empty() {
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

    // dynamic resolve: if nothing matches or only a parent matched, try --help
    let resolve_tokens: Vec<String> = lookup_tokens
        .iter()
        .filter(|t| !t.is_empty())
        .cloned()
        .collect();
    let resolve_depth = resolve_tokens.len();
    let need_resolve = match &found {
        Some((_, _, depth)) => *depth < resolve_depth,
        None => resolve_depth > 0,
    };
    if need_resolve
        && let Some(path) = explicit_cmd_path
            .as_ref()
            .cloned()
            .or_else(|| find_in_path(&cmd_name))
    {
        // build extended mandirs from the binary's own prefix as well
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

    // flag completions are gated on a configurable trigger: by default a
    // leading "-", but the user may add other characters or opt into
    // surfacing flags on an empty token (right after a space).
    let typing_flag = cfg.triggers_flags(&last_token);
    let fallback_subcommands = match &found {
        Some((matched_name, r, _)) if r.subcommands.is_empty() => {
            subcommands_of(&dirs, matched_name)
        }
        _ => Vec::new(),
    };
    // positional value choices (getent databases) fill the same argument slot
    // as subcommands, so they suppress the file/dynamic handoff the same way.
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
    // answer argument prefixes. when the token starts with "-", keep flags.
    let want_files = !typing_flag && !has_subs && (last_token.is_empty() || candidates.is_empty());
    if want_files || candidates.is_empty() {
        // spans are post-elevation, so `sudo nix ...` reaches the dynamic
        // dispatch as `[nix, ...]` and hits the nix branch.
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

// --- completions self-emission ---

fn cmd_completions() {
    // Emit hand-maintained completions for the current CLI. The parser-driven
    // generator is aimed at arbitrary commands; inshellah's own surface is
    // small enough that explicit subcommand externs give better completions.
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

// --- argument parsing ---

struct IndexArgs {
    prefixes: Vec<PathBuf>,
    dir: Option<PathBuf>,
    ignore: Option<PathBuf>,
    help_only: Option<PathBuf>,
    timeout_ms: u64,
    workers: usize,
}

fn parse_index_args(args: &[String]) -> IndexArgs {
    let mut out = IndexArgs {
        prefixes: Vec::new(),
        dir: None,
        ignore: None,
        help_only: None,
        timeout_ms: DEFAULT_TIMEOUT_MS,
        workers: default_workers(),
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                i += 1;
                if i < args.len() {
                    out.dir = Some(PathBuf::from(&args[i]));
                }
            }
            "--ignore" => {
                i += 1;
                if i < args.len() {
                    out.ignore = Some(PathBuf::from(&args[i]));
                }
            }
            "--help-only" => {
                i += 1;
                if i < args.len() {
                    out.help_only = Some(PathBuf::from(&args[i]));
                }
            }
            // additional scrape prefixes beyond the positional ones, as a
            // colon-separated list. lets callers (notably the nix module's
            // extraScrapePackages) roll up extra packages without relying on
            // positional ordering.
            "--prefix" => {
                i += 1;
                if i < args.len() {
                    out.prefixes.extend(
                        args[i]
                            .split(':')
                            .filter(|s| !s.is_empty())
                            .map(PathBuf::from),
                    );
                }
            }
            "--timeout-ms" => {
                i += 1;
                if i < args.len()
                    && let Ok(n) = args[i].parse::<u64>()
                {
                    out.timeout_ms = n;
                }
            }
            "--workers" => {
                i += 1;
                if i < args.len()
                    && let Ok(n) = args[i].parse::<usize>()
                {
                    out.workers = n.max(1);
                }
            }
            other => {
                out.prefixes.push(PathBuf::from(other));
            }
        }
        i += 1;
    }
    out
}

/// best-effort thread count default: `available_parallelism` (1.59+), else 4.
fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

fn man_dir_of_prefix(prefix: &Path) -> PathBuf {
    prefix.join("share/man")
}

/// derive the manpage dir colocated with a read-only system completion dir.
/// the completer is pointed at `<prefix>/share/inshellah`, so the install
/// prefix is two levels up and its manpages live at `<prefix>/share/man` —
/// the same bin↔share/man colocation `index` and the binary-prefix walk
/// assume. portable across Linux and macOS prefixes (nix profile, Homebrew,
/// /usr, CommandLineTools).
fn mandir_for_completion_dir(dir: &Path) -> Option<PathBuf> {
    dir.parent().and_then(Path::parent).map(man_dir_of_prefix)
}

/// parse --dir PATH[:PATH...], optional --timeout-ms N, plus any
/// positional args. when --dir isn't supplied, returns the default cache
/// dir as the single entry. the timeout is `None` when `--timeout-ms`
/// isn't passed, so the caller can fall back to the configured default.
fn parse_dir_args(args: &[String]) -> (Vec<String>, Vec<PathBuf>, Option<u64>) {
    let mut positional = Vec::new();
    let mut dirs: Option<Vec<PathBuf>> = None;
    let mut timeout_ms: Option<u64> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                i += 1;
                if i < args.len() {
                    dirs = Some(args[i].split(':').map(PathBuf::from).collect());
                }
            }
            "--timeout-ms" => {
                i += 1;
                if i < args.len()
                    && let Ok(n) = args[i].parse::<u64>()
                {
                    timeout_ms = Some(n);
                }
            }
            _ => {
                positional.push(args[i].clone());
            }
        }
        i += 1;
    }
    let dirs = dirs.unwrap_or_else(|| vec![default_store_path()]);
    (positional, dirs, timeout_ms)
}

fn main() {
    // restore default SIGPIPE handling so piping output into `head`, `grep -m`,
    // or a completer that stops reading exits quietly instead of panicking on a
    // broken-pipe write (rust ignores SIGPIPE by default, turning it into an
    // `io::ErrorKind::BrokenPipe` that `println!` unwraps into a panic).
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
        std::process::exit(1);
    }
    match args[1].as_str() {
        "index" => {
            let parsed = parse_index_args(&args[2..]);
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
        "manpage" => {
            if args.len() < 3 {
                eprintln!("error: manpage requires a FILE argument");
                std::process::exit(1);
            }
            if let Err(e) = cmd_manpage(Path::new(&args[2])) {
                eprintln!("manpage failed: {e}");
                std::process::exit(1);
            }
        }
        "manpage-dir" => {
            if args.len() < 3 {
                eprintln!("error: manpage-dir requires a DIR argument");
                std::process::exit(1);
            }
            if let Err(e) = cmd_manpage_dir(Path::new(&args[2])) {
                eprintln!("manpage-dir failed: {e}");
                std::process::exit(1);
            }
        }
        "complete" => {
            let cfg = Config::from_env();
            let (positional, dirs, timeout_override) = parse_dir_args(&args[2..]);
            // explicit --timeout-ms wins; otherwise fall back to the
            // configured default (INSHELLAH_TIMEOUT_MS or the compiled one).
            let timeout_ms = timeout_override.unwrap_or(cfg.timeout_ms);
            // first dir is the writable user cache; rest are read-only system dirs
            let (user_dir, system_dirs): (PathBuf, Vec<PathBuf>) = match dirs.split_first() {
                Some((first, rest)) => (first.clone(), rest.to_vec()),
                None => (default_store_path(), Vec::new()),
            };
            // mandirs default to the share/man colocated with each system
            // completion dir's install prefix (<prefix>/share/inshellah).
            let mandirs: Vec<PathBuf> = system_dirs
                .iter()
                .filter_map(|d| mandir_for_completion_dir(d))
                .filter(|p| p.is_dir())
                .collect();
            cmd_complete(
                &positional,
                &user_dir,
                &system_dirs,
                &mandirs,
                timeout_ms,
                &cfg,
            );
        }
        "query" => {
            let (positional, dirs, _timeout_ms) = parse_dir_args(&args[2..]);
            if positional.is_empty() {
                eprintln!("error: query requires a CMD argument");
                std::process::exit(1);
            }
            let cmd = positional.join(" ");
            if let Err(e) = cmd_query(&cmd, &dirs) {
                eprintln!("query failed: {e}");
                std::process::exit(1);
            }
        }
        "dump" => {
            let (_, dirs, _timeout_ms) = parse_dir_args(&args[2..]);
            cmd_dump(&dirs);
        }
        "diff" => {
            let cfg = Config::from_env();
            // `--scan PREFIX` sweeps a prefix for group commands with gaps;
            // otherwise `diff CMD [SUB...]` audits one command.
            if let Some(pos) = args.iter().position(|a| a == "--scan") {
                let Some(prefix) = args.get(pos + 1) else {
                    eprintln!("error: --scan requires a PREFIX path");
                    std::process::exit(1);
                };
                cmd_diff_scan(Path::new(prefix), cfg.timeout_ms);
            } else {
                let (positional, dirs, timeout_override) = parse_dir_args(&args[2..]);
                if positional.is_empty() {
                    eprintln!("error: diff requires a CMD argument");
                    std::process::exit(1);
                }
                cmd_diff(
                    &positional,
                    &dirs,
                    timeout_override.unwrap_or(cfg.timeout_ms),
                );
            }
        }
        "purge" => {
            let (_, dirs, _timeout_ms) = parse_dir_args(&args[2..]);
            // only the first (writable user) dir is purged; the rest are
            // read-only system overlays we must never delete from.
            let user_dir = dirs.first().cloned().unwrap_or_else(default_store_path);
            cmd_purge(&user_dir);
        }
        "completions" => cmd_completions(),
        "--help" | "-h" | "help" => usage(),
        other => {
            eprintln!("unknown subcommand: {other}");
            usage();
            std::process::exit(1);
        }
    }
    // make warning go away
    let _ = filename_of_command;
}
