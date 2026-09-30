// SPDX-License-Identifier: EUPL-1.2
//! indexing and on-demand scrape pipeline.

mod probe;

use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::parsers::manpage::{
    ManpageEntry, ManpageResult, ManpageSubcommand, OwnedSwitch, extract_synopsis_command,
    parse_manpage_string, parse_manpage_with_subs, read_manpage_file,
};
use crate::pool::{ScrapePool, Submitter, par_map};
use crate::resolver::{self, NodeClass, Outcome, Probe, resolve_node};
use crate::store::{
    command_of_filename, ensure_dir, parse_nu_completions, read_result, write_file, write_native,
    write_result,
};
use crate::subprocess::run_cmd;

use self::probe::{Classify, classify_binary, remaining_ms, skip_name, try_native_completion};

pub use self::probe::{is_executable, parse_help_text, try_help, try_help_args};

const COMMAND_SECTIONS: &[u8] = &[1, 8];

const MAX_RESOLVE_RESULTS: usize = 500;
const MAX_RECURSE_DEPTH: u32 = 10;
const RESOLVE_BUDGET_MULTIPLE: u64 = 8;

// env INSHELLAH_MAX_INDEX_NODES. bounds a pathological tree where fresh names
// every level dodge `self_listed`.
const DEFAULT_MAX_NODES_PER_ROOT: usize = 10_000;

pub fn cmd_name_of_manpage(path: &Path) -> String {
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

pub fn find_manpage_path(mandirs: &[PathBuf], hyphenated: &str) -> Option<PathBuf> {
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
pub fn process_manpage(file: &Path) -> Option<ProcessedManpage> {
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

pub fn list_manpages(mandirs: &[PathBuf]) -> Vec<PathBuf> {
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

pub fn load_ignorelist(path: &Path) -> HashSet<String> {
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

const NUSHELL_NATIVE_COMMANDS_FILE: &str = "nushell-native-commands";

fn list_binaries(
    bindirs: &[PathBuf],
    nushell_commands: &HashSet<String>,
) -> Vec<(String, PathBuf)> {
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
            if skip_name(name) || nushell_commands.contains(name) {
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

fn discover_nushell_native_commands(timeout_ms: u64) -> std::io::Result<HashSet<String>> {
    let script =
        r#"scope commands | where type in [built-in keyword] | get name | sort | to json --raw"#;
    let args = vec![
        "nu".to_string(),
        "--no-config-file".to_string(),
        "--no-std-lib".to_string(),
        "--commands".to_string(),
        script.to_string(),
    ];
    let out = run_cmd(&args, timeout_ms).ok_or_else(|| {
        std::io::Error::other(
            "failed to run `nu` for Nushell native command discovery during indexing",
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(out.trim()).map_err(|e| {
        std::io::Error::other(format!(
            "failed to parse Nushell native command discovery output as JSON: {e}"
        ))
    })?;
    let arr = value.as_array().ok_or_else(|| {
        std::io::Error::other("Nushell native command discovery did not return a JSON array")
    })?;
    let mut commands = HashSet::new();
    for item in arr {
        let Some(name) = item.as_str() else {
            return Err(std::io::Error::other(
                "Nushell native command discovery returned a non-string command name",
            ));
        };
        if !name.is_empty() {
            commands.insert(name.to_string());
        }
    }
    Ok(commands)
}

fn write_nushell_native_commands(dir: &Path, commands: &HashSet<String>) -> std::io::Result<()> {
    let mut commands: Vec<&str> = commands.iter().map(String::as_str).collect();
    commands.sort_unstable();
    let data = serde_json::to_string(&commands)
        .map_err(|e| std::io::Error::other(format!("serialize Nushell native commands: {e}")))?;
    write_file(&dir.join(NUSHELL_NATIVE_COMMANDS_FILE), &data)
}

pub fn manpage_name_has_installed_command(name: &str, binary_names: &HashSet<String>) -> bool {
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
    root_help: Mutex<HashMap<String, u64>>,
    subtree_health: Mutex<HashMap<String, SubtreeHealth>>,
}

// a loaded box times out healthy tools too, so only roots where nothing answered.
const MAX_TIMEOUTS_PER_ROOT: u32 = 3;

#[derive(Default)]
struct SubtreeHealth {
    timeouts: u32,
    answered: bool,
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
    let is_root = job.sub_args.is_empty();
    if !is_root
        && ctx
            .subtree_health
            .lock()
            .get(&job.base_cmd)
            .is_some_and(|health| !health.answered && health.timeouts >= MAX_TIMEOUTS_PER_ROOT)
    {
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
        root_help: Cell::new(if is_root {
            None
        } else {
            ctx.root_help.lock().get(&job.base_cmd).copied()
        }),
        timed_out: Cell::new(false),
        echoed: Cell::new(false),
    };

    // classify only at top level
    if is_root && probe.classify() == NodeClass::Skip {
        return;
    }

    let outcome = resolve_one(&probe, &job.base_cmd, &job.sub_args);
    if is_root && let Some(hash) = probe.root_help.get() {
        ctx.root_help.lock().insert(job.base_cmd.clone(), hash);
    }
    if !is_root {
        let mut health = ctx.subtree_health.lock();
        let health = health.entry(job.base_cmd.clone()).or_default();
        if probe.timed_out.get() {
            health.timeouts += 1;
        } else if matches!(outcome, Outcome::Content { .. }) {
            health.answered = true;
        }
    }

    match outcome {
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
                if !probe.echoed.get() {
                    enqueue_child_jobs(ctx, &job, &children, submit);
                }
            }
        }
    }
}

pub fn cmd_index(
    bindirs: &[PathBuf],
    mandirs: &[PathBuf],
    ignorelist: &HashSet<String>,
    help_only: &HashSet<String>,
    dir: &Path,
    timeout_ms: u64,
    num_workers: usize,
) -> std::io::Result<()> {
    ensure_dir(dir)?;
    let nushell_commands = discover_nushell_native_commands(timeout_ms)?;
    write_nushell_native_commands(dir, &nushell_commands)?;
    let binaries = list_binaries(bindirs, &nushell_commands);
    let profile = ProfileBinaries::new(&binaries, ignorelist);
    let mut indexed = ScrapeCtx::new(dir, mandirs, help_only, timeout_ms)
        .scrape(profile.paths.iter(), num_workers);

    ManpagePass {
        dir,
        mandirs,
        profile: &profile,
        help_only,
        nushell_commands: &nushell_commands,
        timeout_ms,
        num_workers,
    }
    .run(&mut indexed)?;

    println!("indexed {} commands into {}", indexed.len(), dir.display());
    Ok(())
}

/// each command's files come from the package owning the profile's binary for
/// its root, else from the first source that has them.
pub fn cmd_merge(
    profile_dir: &Path,
    sources: &[(PathBuf, PathBuf)],
    ignorelist: &HashSet<String>,
    help_only: &HashSet<String>,
    dir: &Path,
    timeout_ms: u64,
    num_workers: usize,
) -> std::io::Result<()> {
    ensure_dir(dir)?;
    let nushell_commands = discover_nushell_native_commands(timeout_ms)?;
    write_nushell_native_commands(dir, &nushell_commands)?;
    let binaries = list_binaries(&[profile_dir.join("bin")], &nushell_commands);
    let profile = ProfileBinaries::new(&binaries, ignorelist);
    let mandirs = [profile_dir.join("share/man")];

    let owner_of = |root: &str| -> Option<&Path> {
        let target = fs::read_link(profile.paths.get(root)?).ok()?;
        sources
            .iter()
            .find(|(package, _)| target.starts_with(package))
            .map(|(_, index)| index.as_path())
    };

    let mut claimed = HashMap::<String, PathBuf>::new();
    let mut owned = Vec::<(PathBuf, String, String)>::new();

    let mut seen = HashSet::<&PathBuf>::new();

    for (_, index) in sources {
        if !seen.insert(index) {
            continue;
        }
        let Ok(entries) = fs::read_dir(index) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let (Some(stem), Some(ext)) = (
                path.file_stem().and_then(|s| s.to_str()),
                path.extension().and_then(|s| s.to_str()),
            ) else {
                continue;
            };
            if ext != "json" && ext != "nu" {
                continue;
            }
            let cmd = command_of_filename(stem);
            let root = cmd.split(' ').next().unwrap_or_default().to_string();
            let owner = match owner_of(&root) {
                Some(owner) => owner.to_path_buf(),
                None => claimed
                    .entry(root.clone())
                    .or_insert_with(|| index.clone())
                    .clone(),
            };
            if owner == *index {
                owned.push((path, cmd, root));
            }
        }
    }

    // wrapper packages ship no manpage, so their index fell back to `--help`.
    let rescrape: HashMap<&String, &PathBuf> = owned
        .iter()
        .filter(|(path, cmd, root)| {
            cmd == root
                && path.extension().is_some_and(|ext| ext == "json")
                && find_manpage_path(&mandirs, cmd).is_some()
                && path
                    .parent()
                    .and_then(|index| read_result(index, cmd))
                    .is_some_and(|(source, _)| source == "help")
        })
        .filter_map(|(_, cmd, _)| profile.paths.get_key_value(cmd))
        .collect();
    for (path, _, root) in &owned {
        if !rescrape.contains_key(root) {
            fs::copy(path, dir.join(path.file_name().unwrap_or_default()))?;
        }
    }
    let mut indexed: HashSet<String> = owned
        .into_iter()
        .filter(|(_, _, root)| !rescrape.contains_key(root))
        .map(|(_, cmd, _)| cmd)
        .collect();
    indexed.extend(
        ScrapeCtx::new(dir, &mandirs, help_only, timeout_ms)
            .scrape(rescrape.into_iter(), num_workers),
    );

    ManpagePass {
        dir,
        mandirs: &mandirs,
        profile: &profile,
        help_only,
        nushell_commands: &nushell_commands,
        timeout_ms,
        num_workers,
    }
    .run(&mut indexed)?;

    println!("merged {} commands into {}", indexed.len(), dir.display());
    Ok(())
}

impl ScrapeCtx {
    fn new(dir: &Path, mandirs: &[PathBuf], help_only: &HashSet<String>, timeout_ms: u64) -> Self {
        Self {
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
            root_help: Mutex::new(HashMap::new()),
            subtree_health: Mutex::new(HashMap::new()),
        }
    }

    fn scrape<'a>(
        self,
        roots: impl Iterator<Item = (&'a String, &'a PathBuf)>,
        num_workers: usize,
    ) -> HashSet<String> {
        let ctx = Arc::new(self);
        let pool = ScrapePool::new(num_workers, {
            let ctx = ctx.clone();
            move |job: PoolJob, submit: &Submitter<PoolJob>| {
                process_pool_job(&ctx, job, submit);
            }
        });
        for (name, path) in roots {
            pool.submit(PoolJob {
                bin_path: path.clone(),
                base_cmd: name.clone(),
                sub_args: Vec::new(),
                depth: 0,
            });
        }
        pool.wait();
        // no workers alive, so the Arc has a single strong ref
        Arc::try_unwrap(ctx)
            .ok()
            .map(|c| c.indexed.into_inner())
            .unwrap_or_default()
    }
}

struct ProfileBinaries {
    names: HashSet<String>,
    paths: HashMap<String, PathBuf>,
}

impl ProfileBinaries {
    fn new(binaries: &[(String, PathBuf)], ignorelist: &HashSet<String>) -> Self {
        let paths: HashMap<String, PathBuf> = binaries
            .iter()
            .filter(|(name, _)| !ignorelist.contains(name))
            .cloned()
            .collect();
        Self {
            names: paths.keys().cloned().collect(),
            paths,
        }
    }
}

struct ManpagePass<'a> {
    dir: &'a Path,
    mandirs: &'a [PathBuf],
    profile: &'a ProfileBinaries,
    help_only: &'a HashSet<String>,
    nushell_commands: &'a HashSet<String>,
    timeout_ms: u64,
    num_workers: usize,
}

impl ManpagePass<'_> {
    fn run(&self, indexed: &mut HashSet<String>) -> std::io::Result<()> {
        let Self {
            dir,
            mandirs,
            profile,
            help_only,
            nushell_commands,
            timeout_ms,
            num_workers,
        } = *self;
        let binary_names = &profile.names;
        let binary_paths = &profile.paths;

        // shorter filenames sort first so parents precede subpages (nix-env.1
        // before nix-env-install.1)
        let mut manpages = list_manpages(mandirs);
        manpages.sort_by(|a, b| {
            let alen = a.file_name().map(|s| s.len()).unwrap_or(0);
            let blen = b.file_name().map(|s| s.len()).unwrap_or(0);
            alen.cmp(&blen).then_with(|| a.cmp(b))
        });
        let processed = par_map(manpages.iter().collect(), num_workers, |path: &PathBuf| {
            process_manpage(path)
        });

        // serial and in sort order so dedupe and overwrites match a sequential pass.
        let mut plan = WritePlan::default();
        let help_for = |name: &str| {
            let (base_cmd, sub_args) = split_command_for_binary(name)?;
            Some((binary_paths.get(base_cmd)?.clone(), sub_args))
        };

        for (manpage_path, processed) in manpages.iter().zip(processed) {
            let Some((name, result, sub_sections)) = processed else {
                continue;
            };
            if !manpage_name_has_installed_command(&name, binary_names) {
                continue;
            }
            let base_cmd = cmd_name_of_manpage(manpage_path);
            if help_only.contains(&name) {
                continue;
            }
            if nushell_commands.contains(&name) {
                continue;
            }
            if indexed.contains(&name) {
                if let Some(write) = plan.get_mut(&name) {
                    write.duplicates.push(result);
                    continue;
                }
                if merge_indexed_result(dir, &name, "manpage", &result)? {
                    continue;
                }
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
            if nushell_commands.contains(&name) {
                continue;
            }
            indexed.insert(name.clone());
            plan.push(PlannedWrite {
                help: help_for(&name),
                stubs: sub_sections.is_empty(),
                name,
                result,
                duplicates: Vec::new(),
            });
            for (sub_cmd, sub_result) in sub_sections {
                if indexed.contains(&sub_cmd) {
                    continue;
                }
                indexed.insert(sub_cmd.clone());
                plan.push(PlannedWrite {
                    help: help_for(&sub_cmd),
                    stubs: false,
                    name: sub_cmd,
                    result: sub_result,
                    duplicates: Vec::new(),
                });
            }
        }

        // COMMANDS-section subcommands lacking a SUBCOMMAND section or own
        // manpage get a desc-only stub so the completer treats them as leaves.
        // taken after the `--help` supplement, which often supplies the list.
        let plan: Vec<PlannedWrite> = plan.writes.into_iter().flatten().collect();
        let mut stubs: Vec<(String, String)> = Vec::new();
        for written in par_map(plan, num_workers, |write| write.run(dir, timeout_ms)) {
            stubs.extend(written?);
        }
        let mut stub_at: HashMap<String, usize> = HashMap::new();
        for (at, (sub_cmd, _)) in stubs.iter().enumerate() {
            stub_at.insert(sub_cmd.clone(), at);
        }
        for (at, (sub_cmd, description)) in stubs.into_iter().enumerate() {
            if indexed.contains(&sub_cmd) || stub_at[&sub_cmd] != at {
                continue;
            }
            let hyphenated = sub_cmd.replace(' ', "-");
            let mut result = find_manpage_path(mandirs, &hyphenated)
                .and_then(|path| read_manpage_file(&path).ok())
                .map(|contents| parse_manpage_string(&contents))
                .filter(|parsed| !parsed.entries.is_empty() || !parsed.subcommands.is_empty())
                .unwrap_or_default();
            strip_subcmd_prefix(&mut result, &hyphenated);
            if result.description.is_empty() {
                result.description = description;
            }
            write_result(dir, &sub_cmd, "manpage", &result)?;
        }
        Ok(())
    }
}

pub fn resolve_and_cache(
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

fn run_help(path: &Path, sub_args: &[String], timeout_ms: u64) -> Option<String> {
    if sub_args.is_empty() {
        try_help(path, timeout_ms)
    } else {
        let bin_s = path.to_string_lossy().to_string();
        try_help_args(&bin_s, sub_args, timeout_ms)
    }
}

fn group_subcommands_from_help(text: &str) -> Option<Vec<ManpageSubcommand>> {
    let help = parse_help_text(text);
    (!help.subcommands.is_empty()).then_some(help.subcommands)
}

fn help_result_for_command(text: &str, sub_args: &[String]) -> Option<ManpageResult> {
    let result = parse_help_text(text);
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

fn entry_overlaps(result: &ManpageResult, entry: &ManpageEntry) -> bool {
    match &entry.switch {
        OwnedSwitch::Both(short, long) => {
            entry_has_short(result, *short) || entry_has_long(result, long)
        }
        OwnedSwitch::Long(long) => entry_has_long(result, long),
        OwnedSwitch::Short(short) => entry_has_short(result, *short),
    }
}

fn switches_overlap(a: &OwnedSwitch, b: &OwnedSwitch) -> bool {
    match (a, b) {
        (OwnedSwitch::Short(a), OwnedSwitch::Short(b))
        | (OwnedSwitch::Short(a), OwnedSwitch::Both(b, _))
        | (OwnedSwitch::Both(a, _), OwnedSwitch::Short(b)) => a == b,
        (OwnedSwitch::Long(a), OwnedSwitch::Long(b))
        | (OwnedSwitch::Long(a), OwnedSwitch::Both(_, b))
        | (OwnedSwitch::Both(_, a), OwnedSwitch::Long(b)) => a.eq_ignore_ascii_case(b),
        (OwnedSwitch::Both(a_short, a_long), OwnedSwitch::Both(b_short, b_long)) => {
            a_short == b_short || a_long.eq_ignore_ascii_case(b_long)
        }
        (OwnedSwitch::Short(_), OwnedSwitch::Long(_))
        | (OwnedSwitch::Long(_), OwnedSwitch::Short(_)) => false,
    }
}

fn merge_entry_description(existing: &mut ManpageEntry, incoming: &ManpageEntry) -> bool {
    let mut changed = false;
    if existing.desc.is_empty() && !incoming.desc.is_empty() {
        existing.desc = incoming.desc.clone();
        changed = true;
    }
    changed
}

fn merge_matching_entry_descriptions(result: &mut ManpageResult, incoming: &ManpageEntry) -> bool {
    let mut changed = false;
    for existing in &mut result.entries {
        if switches_overlap(&existing.switch, &incoming.switch) {
            changed |= merge_entry_description(existing, incoming);
        }
    }
    changed
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
        let mut alias_changed = false;
        match &help_entry.switch {
            OwnedSwitch::Both(short, long) => {
                if fill_flag_alias_from_help(result, *short, long) {
                    alias_changed = true;
                }
                if entry_overlaps(result, help_entry) {
                    changed |= alias_changed;
                    changed |= merge_matching_entry_descriptions(result, help_entry);
                    continue;
                }
            }
            OwnedSwitch::Long(long) if entry_has_long(result, long) => {
                changed |= merge_matching_entry_descriptions(result, help_entry);
                continue;
            }
            OwnedSwitch::Short(short) if entry_has_short(result, *short) => {
                changed |= merge_matching_entry_descriptions(result, help_entry);
                continue;
            }
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

fn supplement_result_from_duplicate_manpage(
    result: &mut ManpageResult,
    manpage: &ManpageResult,
) -> bool {
    let before_positionals = result.positionals.clone();
    let before_positional_choices = result.positional_choices.clone();
    let changed = supplement_result_from_help(result, manpage);
    result.positionals = before_positionals;
    result.positional_choices = before_positional_choices;
    changed
}

fn merge_sources(existing: &str, incoming: &str) -> String {
    let mut parts: Vec<&str> = existing.split('+').filter(|p| !p.is_empty()).collect();
    for part in incoming.split('+').filter(|p| !p.is_empty()) {
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        incoming.to_string()
    } else {
        parts.join("+")
    }
}

fn merge_indexed_result(
    dir: &Path,
    name: &str,
    incoming_source: &str,
    incoming: &ManpageResult,
) -> std::io::Result<bool> {
    let Some((existing_source, mut existing)) = read_result(dir, name) else {
        return Ok(false);
    };
    if supplement_result_from_duplicate_manpage(&mut existing, incoming) {
        let source = merge_sources(&existing_source, incoming_source);
        write_result(dir, name, &source, &existing)?;
    }
    Ok(true)
}

struct PlannedWrite {
    name: String,
    result: ManpageResult,
    help: Option<(PathBuf, Vec<String>)>,
    stubs: bool,
    duplicates: Vec<ManpageResult>,
}

#[derive(Default)]
struct WritePlan {
    writes: Vec<Option<PlannedWrite>>,
    at: HashMap<String, usize>,
}

impl WritePlan {
    fn push(&mut self, write: PlannedWrite) {
        if let Some(stale) = self.at.insert(write.name.clone(), self.writes.len()) {
            self.writes[stale] = None;
        }
        self.writes.push(Some(write));
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut PlannedWrite> {
        self.writes[*self.at.get(name)?].as_mut()
    }
}

impl PlannedWrite {
    fn run(mut self, dir: &Path, timeout_ms: u64) -> std::io::Result<Vec<(String, String)>> {
        let mut source = String::from("manpage");
        if let Some((path, sub_args)) = &self.help
            && supplement_result_from_help_command(&mut self.result, path, sub_args, timeout_ms)
        {
            source = String::from("manpage+help");
        }
        let stubs = if self.stubs {
            self.result
                .subcommands
                .iter()
                .map(|sc| (format!("{} {}", self.name, sc.name), sc.desc.clone()))
                .collect()
        } else {
            Vec::new()
        };
        for duplicate in &self.duplicates {
            if supplement_result_from_duplicate_manpage(&mut self.result, duplicate) {
                source = merge_sources(&source, "manpage");
            }
        }
        write_result(dir, &self.name, &source, &self.result)?;
        Ok(stubs)
    }
}

fn supplement_result_from_help_text(
    result: &mut ManpageResult,
    text: &str,
    sub_args: &[String],
) -> bool {
    help_result_for_command(text, sub_args)
        .as_ref()
        .is_some_and(|help| supplement_result_from_help(result, help))
}

fn supplement_result_from_help_command(
    result: &mut ManpageResult,
    path: &Path,
    sub_args: &[String],
    timeout_ms: u64,
) -> bool {
    run_help(path, sub_args, timeout_ms)
        .is_some_and(|text| supplement_result_from_help_text(result, &text, sub_args))
}

// group command with a leftover `<command>`/`<subcommands>` placeholder and no
// subcommands populated.
pub fn looks_like_unenumerated_group(r: &ManpageResult) -> bool {
    r.subcommands.is_empty()
        && r.positionals.iter().any(|(n, _)| {
            matches!(
                n.to_ascii_lowercase().as_str(),
                "command" | "commands" | "subcommand" | "subcommands"
            )
        })
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
    root_help: Cell<Option<u64>>,
    timed_out: Cell<bool>,
    echoed: Cell<bool>,
}

impl RealProbe<'_> {
    fn step_timeout(&self) -> u64 {
        self.timeout_ms.min(remaining_ms(self.deadline))
    }

    // some tools print their root help for any verb; its verbs would fan out again.
    fn help(&self, sub_args: &[String]) -> Option<String> {
        let timeout_ms = self.step_timeout();
        let started = Instant::now();
        let Some(text) = run_help(self.path, sub_args, timeout_ms) else {
            if started.elapsed() >= Duration::from_millis(timeout_ms) {
                self.timed_out.set(true);
            }
            return None;
        };
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let hash = hasher.finish();
        if sub_args.is_empty() {
            self.root_help.set(Some(hash));
        } else if self.root_help.get() == Some(hash) {
            self.echoed.set(true);
        }
        Some(text)
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
        self.help(sub_args)
    }

    fn supplement_from_help(&self, result: &mut ManpageResult, sub_args: &[String]) -> bool {
        self.help(sub_args)
            .is_some_and(|text| supplement_result_from_help_text(result, &text, sub_args))
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
        group_subcommands_from_help(&self.help(sub_args)?)
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

pub fn resolve_command_path_and_cache(
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
        root_help: Cell::new(None),
        timed_out: Cell::new(false),
        echoed: Cell::new(false),
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

#[cfg(test)]
mod tests {
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
}
