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
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use inshellah::config::{Config, DEFAULT_TIMEOUT_MS};
use inshellah::dynamic::dynamic_complete;
use inshellah::parsers::help::help_parser;
use inshellah::parsers::manpage::{
    ManpageEntry, ManpageResult, ManpageSubcommand, OwnedParam, OwnedSwitch,
    extract_synopsis_command, parse_manpage_string, parse_manpage_with_subs, read_manpage_file,
};
use inshellah::parsers::nushell::{generate_extern, generate_module, is_nushell_builtin};
use inshellah::pool::{ScrapePool, Submitter};
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
fn try_help(bin: &Path, timeout_ms: u64) -> Option<String> {
    let bin_s = bin.to_string_lossy().to_string();
    for variant in [&["--help"][..], &["-h"][..]] {
        let mut args = vec![bin_s.clone()];
        args.extend(variant.iter().map(|s| s.to_string()));
        if let Some(out) = run_cmd(&args, timeout_ms) {
            let cleaned = fast_strip_ansi::strip_ansi_string(&out);
            if !cleaned.trim().is_empty() {
                return Some(cleaned.to_string());
            }
        }
    }
    None
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
    let help_text = try_help(bin, timeout_ms)?;
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
            if let Some(out) = run_cmd(&args_form, timeout_ms) {
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
const MAX_RECURSE_DEPTH: u32 = 5;
const RESOLVE_BUDGET_MULTIPLE: u64 = 8;

fn remaining_ms(deadline: Instant) -> u64 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn parse_help_text(text: &str) -> ManpageResult {
    let cleaned: String = fast_strip_ansi::strip_ansi_string(text).into_owned();
    match help_parser(&cleaned) {
        Ok((_, r)) => (&r).into(),
        Err(_) => ManpageResult::default(),
    }
}

/// recursively resolve subcommands, returning a vec of (cmd_path, result)
/// where cmd_path is the full "git stash apply" form. used by the
/// dynamic-resolve path in `cmd_complete`; the batch indexer uses the
/// pool instead, which expresses this same BFS shape with workers.
fn help_resolve(
    bin: &Path,
    cmd: &str,
    depth: u32,
    timeout_ms: u64,
    deadline: Instant,
    acc: &mut Vec<(String, ManpageResult)>,
) {
    if acc.len() >= MAX_RESOLVE_RESULTS || Instant::now() >= deadline {
        return;
    }
    let Some(help_text) = try_help(bin, timeout_ms.min(remaining_ms(deadline))) else {
        return;
    };
    let result = parse_help_text(&help_text);
    acc.push((cmd.to_string(), result));
    let initial_subs: Vec<String> = acc
        .last()
        .map(|(_, r)| {
            r.subcommands
                .iter()
                .map(|sc| sc.name.clone())
                .filter(|n| n.len() >= 2 && !n.starts_with('-'))
                .collect()
        })
        .unwrap_or_default();
    let bin_s = bin.to_string_lossy().to_string();
    for sub in initial_subs {
        recurse_subcommand(
            &bin_s,
            cmd,
            std::slice::from_ref(&sub),
            depth + 1,
            timeout_ms,
            deadline,
            acc,
        );
    }
}

fn recurse_subcommand(
    bin_s: &str,
    base_cmd: &str,
    sub_args: &[String],
    depth: u32,
    timeout_ms: u64,
    deadline: Instant,
    acc: &mut Vec<(String, ManpageResult)>,
) {
    if acc.len() >= MAX_RESOLVE_RESULTS || depth > MAX_RECURSE_DEPTH || Instant::now() >= deadline {
        return;
    }
    let full_cmd = format!("{base_cmd} {}", sub_args.join(" "));
    let Some(text) = try_help_args(bin_s, sub_args, timeout_ms.min(remaining_ms(deadline))) else {
        return;
    };
    let result = parse_help_text(&text);
    if result.entries.is_empty() && result.subcommands.is_empty() && result.positionals.is_empty() {
        return;
    }
    if let Some(leaf) = sub_args.last() {
        let self_listed = result
            .subcommands
            .iter()
            .any(|sc| sc.name.eq_ignore_ascii_case(leaf));
        if self_listed {
            return;
        }
    }
    let inner_subs: Vec<String> = result
        .subcommands
        .iter()
        .map(|sc| sc.name.clone())
        .filter(|n| n.len() >= 2 && !n.starts_with('-') && n != "help")
        .collect();
    acc.push((full_cmd, result));
    for sub in inner_subs {
        if acc.len() >= MAX_RESOLVE_RESULTS {
            break;
        }
        let mut next = sub_args.to_vec();
        next.push(sub);
        recurse_subcommand(bin_s, base_cmd, &next, depth + 1, timeout_ms, deadline, acc);
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
    if result.entries.is_empty() && result.subcommands.is_empty() && sub_sections.is_empty() {
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
    fn fuzzy_score_keeps_completion_ranking_shape() {
        assert_eq!(fuzzy_score("", "build"), 1);
        assert_eq!(fuzzy_score("build", "build"), 1000);
        assert_eq!(fuzzy_score("BUILD", "build"), 1000);
        assert_eq!(fuzzy_score("bl", "build"), 60);
        assert_eq!(fuzzy_score("bl", "bundle"), 60);
        assert_eq!(fuzzy_score("bl", "branch-list"), 100);
        assert_eq!(fuzzy_score("bl", "blacklist"), 922);
        assert_eq!(fuzzy_score("bl", "table"), 40);
    }

    #[test]
    fn completion_json_escapes_without_changing_shape() {
        assert_eq!(
            completion_json("a\"b", "line\nnext"),
            r#"{"value":"a\"b","description":"line\nnext"}"#
        );
    }

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

}

/// shared state passed to every pool worker. nothing inside mutates
/// except `indexed`, which is wrapped in a parking_lot::Mutex.
struct ScrapeCtx {
    cache_dir: PathBuf,
    mandirs: Vec<PathBuf>,
    help_only: HashSet<String>,
    indexed: Mutex<HashSet<String>>,
    timeout_ms: u64,
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

/// hyphenated form used to look up a manpage for a (possibly nested)
/// command — "git" for top-level, "git-remote" for `git remote`,
/// "git-stash-apply" for `git stash apply`.
fn hyphenated_cmd(job: &PoolJob) -> String {
    if job.sub_args.is_empty() {
        job.base_cmd.clone()
    } else {
        format!("{}-{}", job.base_cmd, job.sub_args.join("-"))
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

/// enqueue child jobs for each discovered subcommand. shared between the
/// manpage and help branches of process_pool_job.
fn enqueue_subcommands(
    job: &PoolJob,
    subcommands: &[ManpageSubcommand],
    submit: &Submitter<PoolJob>,
) {
    // matches the sequential recurse_subcommand depth check (`depth > MAX`),
    // not `>=`, so we get 6 levels (0..=5) of recursion. without this we
    // were cutting off the last layer of deep clap trees like jay.
    if job.depth > MAX_RECURSE_DEPTH {
        return;
    }
    for sc in subcommands {
        if sc.name.len() < 2 || sc.name.starts_with('-') || sc.name == "help" {
            continue;
        }
        let mut next = job.sub_args.clone();
        next.push(sc.name.clone());
        submit.submit(PoolJob {
            bin_path: job.bin_path.clone(),
            base_cmd: job.base_cmd.clone(),
            sub_args: next,
            depth: job.depth + 1,
        });
    }
}

/// per-job handler called by every worker. populates the cache + enqueues
/// child jobs (one per discovered subcommand) onto the same pool.
///
/// source priority is: (1) native completions, (2) manpage, (3) --help.
/// --help text is fetched at step 1 only as a probe for the completions
/// subcommand; it is not mined for content unless steps 1 and 2 both miss.
fn process_pool_job(ctx: &ScrapeCtx, job: PoolJob, submit: &Submitter<PoolJob>) {
    let full_cmd = job.full_cmd();
    if ctx.indexed.lock().contains(&full_cmd) {
        return;
    }
    let bin_s = job.bin_path.to_string_lossy().to_string();

    // 1. native completions (top-level only — sub-commands don't ship
    //    their own completion payloads). classify_binary scans the ELF for
    //    "complet" needles, and try_native_completion confirms by invoking
    //    the completions subcommand.
    if job.sub_args.is_empty() {
        let class = classify_binary(&job.bin_path, &job.bin_path);
        if matches!(class, Classify::Skip) {
            return;
        }
        if matches!(class, Classify::HasNativeCompletions)
            && let Some(nu) = try_native_completion(&job.bin_path, ctx.timeout_ms)
        {
            let _ = write_native(&ctx.cache_dir, &full_cmd, &nu);
            ctx.indexed.lock().insert(full_cmd);
            return;
        }
    }

    // 2. manpage as primary content source — structured documentation
    //    over the curated --help summary.
    if !ctx.help_only.contains(&job.base_cmd) && !ctx.help_only.contains(&full_cmd) {
        let hyphenated = hyphenated_cmd(&job);
        if let Some(mp_path) = find_manpage_path(&ctx.mandirs, &hyphenated)
            && let Ok(contents) = read_manpage_file(&mp_path)
        {
            let mut mp_result = parse_manpage_string(&contents);
            if !mp_result.entries.is_empty() || !mp_result.subcommands.is_empty() {
                strip_subcmd_prefix(&mut mp_result, &hyphenated);
                let _ = write_result(&ctx.cache_dir, &full_cmd, "manpage", &mp_result);
                ctx.indexed.lock().insert(full_cmd);
                enqueue_subcommands(&job, &mp_result.subcommands, submit);
                return;
            }
        }
    }

    // 3. fallback: scrape --help text for content.
    let text = if job.sub_args.is_empty() {
        try_help(&job.bin_path, ctx.timeout_ms)
    } else {
        try_help_args(&bin_s, &job.sub_args, ctx.timeout_ms)
    };
    let Some(text) = text else { return };

    let result = parse_help_text(&text);
    if result.entries.is_empty() && result.subcommands.is_empty() && result.positionals.is_empty() {
        return;
    }

    // self-listing detection for sub-probes: if the leaf token shows up in
    // the result's subcommand list, the binary probably echoed the parent
    // help (didn't recognize the token). discard.
    if let Some(leaf) = job.sub_args.last()
        && result
            .subcommands
            .iter()
            .any(|sc| sc.name.eq_ignore_ascii_case(leaf))
    {
        return;
    }

    let _ = write_result(&ctx.cache_dir, &full_cmd, "help", &result);
    ctx.indexed.lock().insert(full_cmd);
    enqueue_subcommands(&job, &result.subcommands, submit);
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

    // phase 1: parallel scrape of every eligible binary via the BFS pool.
    // shared state lives in an Arc<ScrapeCtx>; the `indexed` set is the
    // one mutable bit and uses parking_lot::Mutex.
    let ctx = Arc::new(ScrapeCtx {
        cache_dir: dir.to_path_buf(),
        mandirs: mandirs.to_vec(),
        help_only: help_only.clone(),
        indexed: Mutex::new(HashSet::new()),
        timeout_ms,
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
        let Some((name, result, sub_sections)) = process_manpage(&manpage_path) else {
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
        // clap-style SUBCOMMAND sections produce real, fully-populated
        // sub-files (each with its own flags + positionals); they take
        // priority over COMMANDS-section leaf stubs.
        write_result(dir, &name, "manpage", &result)?;
        indexed.insert(name.clone());
        for (sub_cmd, sub_result) in &sub_sections {
            if indexed.contains(sub_cmd) {
                continue;
            }
            write_result(dir, sub_cmd, "manpage", sub_result)?;
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
        if help_text.is_some() {
            "ok"
        } else {
            "(none)"
        }
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
fn fuzzy_score(needle: &str, haystack: &str) -> i32 {
    let needle_len = needle.len();
    let haystack_len = haystack.len();
    if needle_len == 0 {
        return 1;
    }
    if needle_len > haystack_len {
        return 0;
    }
    if needle == haystack {
        return 1000;
    }

    let needle = needle.as_bytes();
    let haystack = haystack.as_bytes();
    if starts_with_ignore_ascii_case(haystack, needle) {
        return 900 + (needle_len as i32 * 100 / haystack_len as i32);
    }

    let mut needle_idx = 0usize;
    let mut score = 0i32;
    let mut prev_match: Option<usize> = None;

    for (hay_idx, &c) in haystack.iter().enumerate() {
        if needle_idx >= needle_len {
            break;
        }
        if c.eq_ignore_ascii_case(&needle[needle_idx]) {
            let boundary = hay_idx == 0
                || haystack[hay_idx - 1] == b'-'
                || haystack[hay_idx - 1] == b'_'
                || (haystack[hay_idx - 1].is_ascii_lowercase()
                    && haystack[hay_idx].is_ascii_uppercase());
            let consecutive = prev_match == Some(hay_idx.saturating_sub(1));
            score += if boundary { 50 } else { 10 };
            if consecutive {
                score += 20;
            }
            needle_idx += 1;
            prev_match = Some(hay_idx);
            continue;
        }
    }

    if needle_idx == needle_len { score } else { 0 }
}

fn starts_with_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len()
        && haystack
            .iter()
            .zip(needle)
            .all(|(&hay, &needle)| hay.eq_ignore_ascii_case(&needle))
}

fn push_json_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

fn completion_json(value: &str, desc: &str) -> String {
    let mut out = String::with_capacity(value.len() + desc.len() + 30);
    out.push_str(r#"{"value":""#);
    push_json_escaped(&mut out, value);
    out.push_str(r#"","description":""#);
    push_json_escaped(&mut out, desc);
    out.push_str(r#""}"#);
    out
}

fn entry_completion_desc(e: &ManpageEntry) -> String {
    match &e.param {
        Some(OwnedParam::Mandatory(p)) => {
            if e.desc.is_empty() {
                format!("<{p}>")
            } else {
                format!("{} <{p}>", e.desc)
            }
        }
        Some(OwnedParam::Optional(p)) => {
            if e.desc.is_empty() {
                format!("[{p}]")
            } else {
                format!("{} [{p}]", e.desc)
            }
        }
        None => e.desc.clone(),
    }
}

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
    timeout_ms: u64,
) -> Vec<String> {
    let args = vec![
        path.to_string_lossy().to_string(),
        "devices".to_string(),
        "-l".to_string(),
    ];
    let Some(output) = run_cmd(&args, timeout_ms) else {
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
                        completion_json(
                            &format!("{replacement_prefix}{}", &device.serial),
                            &device.desc,
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
                            completion_json(
                                &format!("{replacement_prefix}{id}"),
                                &format!("{} {}", &device.serial, &device.desc),
                            ),
                        ));
                    }
                }
            }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, json)| json).collect()
}

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
    timeout_ms: u64,
) -> Vec<String> {
    let mut args = vec![path.to_string_lossy().to_string()];
    args.extend(selector_args.iter().cloned());
    args.extend(
        ["shell", "pm", "list", "packages"]
            .into_iter()
            .map(str::to_string),
    );
    let Some(output) = run_cmd(&args, timeout_ms) else {
        return Vec::new();
    };
    let mut scored = Vec::new();
    for package in output.lines().filter_map(parse_adb_package_line) {
        let score = prefix_score(prefix, package);
        if score > 0 {
            scored.push((score, completion_json(package, "package")));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, json)| json).collect()
}

fn dynamic_value_completions(
    cmd_name: &str,
    rest: &[String],
    explicit_cmd_path: Option<&Path>,
    timeout_ms: u64,
) -> Option<Vec<String>> {
    if cmd_name != "adb" {
        return None;
    }
    let path = explicit_cmd_path
        .map(Path::to_path_buf)
        .or_else(|| find_in_path(cmd_name))?;
    if let Some(completion) = adb_device_completion(rest) {
        return Some(adb_device_candidates(&path, completion, timeout_ms));
    }
    if let Some(prefix) = adb_package_completion_prefix(rest) {
        let selectors = adb_selector_args(rest);
        return Some(adb_package_candidates(
            &path, &selectors, prefix, timeout_ms,
        ));
    }
    None
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
    let full_cmd = if sub_args.is_empty() {
        base_cmd.to_string()
    } else {
        format!("{base_cmd} {}", sub_args.join(" "))
    };
    let hyphenated = if sub_args.is_empty() {
        base_cmd.to_string()
    } else {
        format!("{base_cmd}-{}", sub_args.join("-"))
    };

    // 1. native completions
    if matches!(classify_binary(path, path), Classify::HasNativeCompletions)
        && let Some(nu) = try_native_completion(path, timeout_ms)
    {
        let _ = write_native(user_dir, base_cmd, &nu);
        return Some(parse_nu_completions(&full_cmd, &nu));
    }
    // 2. manpage as primary content source.
    if let Some(mp_path) = find_manpage_path(mandirs, &hyphenated)
        && let Ok(contents) = read_manpage_file(&mp_path)
    {
        let mut result = parse_manpage_string(&contents);
        if !result.entries.is_empty() || !result.subcommands.is_empty() {
            strip_subcmd_prefix(&mut result, &hyphenated);
            let _ = write_result(user_dir, &full_cmd, "manpage", &result);
            return Some(result);
        }
    }
    // 3. fallback: scrape --help text.
    let text = if sub_args.is_empty() {
        try_help(path, timeout_ms.min(remaining_ms(deadline)))
    } else {
        let bin_s = path.to_string_lossy().to_string();
        try_help_args(&bin_s, sub_args, timeout_ms.min(remaining_ms(deadline)))
    }?;
    let parsed = parse_help_text(&text);
    if parsed.entries.is_empty() && parsed.subcommands.is_empty() && parsed.positionals.is_empty() {
        return None;
    }
    if let Some(leaf) = sub_args.last()
        && parsed
            .subcommands
            .iter()
            .any(|sc| sc.name.eq_ignore_ascii_case(leaf))
    {
        return None;
    }

    let _ = write_result(user_dir, &full_cmd, "help", &parsed);
    if sub_args.is_empty() {
        let mut sub_acc: Vec<(String, ManpageResult)> = Vec::new();
        help_resolve(path, base_cmd, 1, timeout_ms, deadline, &mut sub_acc);
        for (cmd, r) in sub_acc.into_iter().skip(1) {
            let _ = write_result(user_dir, &cmd, "help", &r);
        }
    } else {
        let bin_s = path.to_string_lossy().to_string();
        let inner_subs: Vec<String> = parsed
            .subcommands
            .iter()
            .map(|sc| sc.name.clone())
            .filter(|n| n.len() >= 2 && !n.starts_with('-') && n != "help")
            .collect();
        let mut sub_acc: Vec<(String, ManpageResult)> = Vec::new();
        for sub in inner_subs {
            let mut next = sub_args.to_vec();
            next.push(sub);
            recurse_subcommand(
                &bin_s,
                base_cmd,
                &next,
                sub_args.len() as u32 + 2,
                timeout_ms,
                deadline,
                &mut sub_acc,
            );
        }
        for (cmd, r) in sub_acc {
            let _ = write_result(user_dir, &cmd, "help", &r);
        }
    }
    Some(parsed)
}

const ELEVATION_COMMANDS: &[&str] = &["sudo", "doas", "pkexec", "su", "run0"];

fn cmd_complete(
    spans: &[String],
    user_dir: &Path,
    system_dirs: &[PathBuf],
    mandirs: &[PathBuf],
    timeout_ms: u64,
    cfg: &Config,
) {
    let mut dirs: Vec<PathBuf> = system_dirs.to_vec();
    dirs.push(user_dir.to_path_buf());

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

    // strip intermediate flag tokens — they aren't part of subcommand path
    let mut tokens: Vec<String> = vec![cmd_name.clone()];
    if !rest.is_empty() {
        let (last, leading) = rest.split_last().unwrap();
        for t in leading {
            if !t.starts_with('-') || t.is_empty() {
                tokens.push(t.clone());
            }
        }
        tokens.push(last.clone());
    }

    let last_token = rest.last().cloned().unwrap_or_default();
    // lookup tokens exclude the partial unless the user has typed a trailing space
    let lookup_tokens: Vec<String> = if last_token.is_empty() {
        tokens.clone()
    } else if tokens.len() > 1 {
        tokens[..tokens.len() - 1].to_vec()
    } else {
        vec![cmd_name.clone()]
    };

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
    let has_subs = match &found {
        Some((_, r, _)) => !r.subcommands.is_empty() || !fallback_subcommands.is_empty(),
        None => false,
    };
    let candidates: Vec<String> = match &found {
        None => Vec::new(),
        Some((_, r, depth)) => {
            let subs: &[ManpageSubcommand] = if !r.subcommands.is_empty() {
                &r.subcommands
            } else {
                &fallback_subcommands
            };
            let mut scored: Vec<(i32, String)> = Vec::with_capacity(
                (if *depth >= resolve_depth {
                    subs.len()
                } else {
                    0
                }) + if typing_flag { r.entries.len() } else { 0 },
            );
            // subcommand candidates (skip if match is too shallow). when
            // `systemctl status` isn't in the cache, `find_result` falls
            // back to `systemctl` at depth 1; we must NOT then offer
            // `systemctl`'s subs (`poweroff`, `preset`, ...) — the user has
            // already typed past that point. requiring depth >= resolve_depth
            // (the count of complete, non-partial tokens) keeps subs
            // exclusive to a full-prefix match and lets the dynamic completer
            // — systemctl unit names, etc. — take over otherwise.
            //
            // also: when the typed token *exactly* equals a candidate we
            // drop it. the user has already written the full word; echoing
            // it back masks any downstream dynamic completer.
            if *depth >= resolve_depth {
                for sc in subs {
                    if !last_token.is_empty() && last_token == sc.name {
                        continue;
                    }
                    let s = fuzzy_score(&last_token, &sc.name);
                    if s > 0 {
                        scored.push((s, completion_json(&sc.name, &sc.desc)));
                    }
                }
            }
            // flag candidates. the needle — and whether it scores against
            // the bare flag name or the dashed form — depends on which
            // trigger the user typed (see Config::flag_needle). the default
            // "-" trigger keeps the dashed form, so ranking is unchanged.
            if typing_flag {
                let fneedle = cfg.flag_needle(&last_token);
                let score_against = |dashed: &str, bare_name: &str| -> i32 {
                    if fneedle.bare {
                        fuzzy_score(fneedle.needle, bare_name)
                    } else {
                        fuzzy_score(fneedle.needle, dashed)
                    }
                };
                for e in &r.entries {
                    let (flag, aka, score) = match &e.switch {
                        OwnedSwitch::Long(l) => {
                            let flag = format!("--{l}");
                            let score = score_against(&flag, l);
                            (flag, None, score)
                        }
                        OwnedSwitch::Short(c) => {
                            let flag = format!("-{c}");
                            let short_bare = c.to_string();
                            let score = score_against(&flag, &short_bare);
                            (flag, None, score)
                        }
                        OwnedSwitch::Both(c, l) => {
                            let long_flag = format!("--{l}");
                            let short_flag = format!("-{c}");
                            let short_bare = c.to_string();
                            let ls = score_against(&long_flag, l);
                            let ss = score_against(&short_flag, &short_bare);
                            if ss > ls {
                                (short_flag, Some(long_flag), ss)
                            } else {
                                (long_flag, Some(short_flag), ls)
                            }
                        }
                    };
                    if !last_token.is_empty() && last_token == flag {
                        continue;
                    }
                    if score > 0 {
                        let base_desc = entry_completion_desc(e);
                        let desc = match aka {
                            Some(aka) => format!("(aka {aka}) {base_desc}"),
                            None => base_desc,
                        };
                        scored.push((score, completion_json(&flag, &desc)));
                    }
                }
            }
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            if cfg.max_completions > 0 {
                scored.truncate(cfg.max_completions);
            }
            scored.into_iter().map(|(_, json)| json).collect()
        }
    };
    // hand off at non-flag leaf positions so file and dynamic completers can
    // answer argument prefixes. when the token starts with "-", keep flags.
    let want_files = !typing_flag && !has_subs && (last_token.is_empty() || candidates.is_empty());
    if want_files || candidates.is_empty() {
        // spans are post-elevation, so `sudo nix ...` reaches the dynamic
        // dispatch as `[nix, ...]` and hits the nix branch.
        if let Some(dyn_candidates) = dynamic_complete(&spans, cfg) {
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
    // emit completions for inshellah itself.
    let entries: Vec<ManpageEntry> = vec![ManpageEntry {
        switch: OwnedSwitch::Both('h', "help".to_string()),
        param: None,
        desc: "show help".to_string(),
    }];
    let subs = [
        "index",
        "manpage",
        "manpage-dir",
        "complete",
        "query",
        "dump",
        "diff",
        "purge",
        "completions",
    ];
    let mut subcommands = Vec::new();
    for s in subs {
        subcommands.push(ManpageSubcommand {
            name: s.to_string(),
            desc: String::new(),
        });
    }
    let result = ManpageResult {
        entries,
        subcommands,
        positionals: Default::default(),
        description: "nushell completions engine".to_string(),
    };
    print!("{}", generate_module("inshellah", &result));
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
                    out.prefixes
                        .extend(args[i].split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
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
                cmd_diff(&positional, &dirs, timeout_override.unwrap_or(cfg.timeout_ms));
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
