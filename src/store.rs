// SPDX-License-Identifier: EUPL-1.2
//! filesystem store for parsed completion data. filenames are reversible
//! ("git add" -> v1%git%20add.json). lookups span the user cache + system dirs.

use std::{
    collections::{
        BTreeSet,
        HashMap,
    },
    env,
    ffi::OsStr,
    fmt::{
        self,
        Display,
        Formatter,
        Write as _,
    },
    fs,
    io,
    iter,
    path::{
        Path,
        PathBuf,
    },
    process,
    str::FromStr,
    sync::atomic::{
        AtomicU64,
        Ordering,
    },
    time::{
        Duration,
        SystemTime,
        UNIX_EPOCH,
    },
};

use serde_json::Value;

use crate::{
    parsers::{
        manpage::{
            ManpageEntry,
            ManpageResult,
            ManpageSubcommand,
            OwnedParam,
            OwnedSwitch,
            ParamChoice,
            ParamKind,
        },
        nushell::parse_nu_completions,
    },
    types::Positional,
};

pub fn default_store_path() -> PathBuf {
    if let Ok(xdg) = env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("inshellah");
    }
    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(".cache/inshellah");
    }
    PathBuf::from(".cache/inshellah")
}

/// # Errors
/// fails when `dir` or one of its parents cannot be created.
pub fn ensure_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

const ENCODED_FILENAME_PREFIX: &str = "v1%";

struct PercentEncoded<'text>(&'text str);

impl Display for PercentEncoded<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for byte in self.0.bytes() {
            match byte {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' => {
                    f.write_char(char::from(byte))?;
                },
                _ => write!(f, "%{byte:02X}")?,
            }
        }
        Ok(())
    }
}

/// percent-encode every byte outside a portable filename subset.
pub fn filename_of_command(cmd: &str) -> String {
    format!("{ENCODED_FILENAME_PREFIX}{}", PercentEncoded(cmd))
}

/// age since the newest user-cache file for `command` was written.
///
/// i.e. when this set was last resolved. checks `user_dir` only; system/index
/// dirs are never auto-refreshed. None when no user-cache file exists.
pub fn user_cache_age(user_dir: &Path, command: &str) -> Option<Duration> {
    let now = SystemTime::now();
    let base = filename_of_command(command);
    let newest = ["nu", "json"]
        .map(|ext| user_dir.join(format!("{base}.{ext}")))
        .into_iter()
        .filter_map(|path| fs::metadata(&path).and_then(|meta| meta.modified()).ok())
        .max()?;
    // future-dated mtime (clock skew) saturates to age 0.
    Some(now.duration_since(newest).unwrap_or_default())
}

const fn decode_hex(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

fn decode_encoded_filename(encoded: &str) -> Option<String> {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = *bytes.get(i + 1)?;
            let lo = *bytes.get(i + 2)?;
            out.push((decode_hex(hi)? << 4) | decode_hex(lo)?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// None for anything `filename_of_command` could not have produced.
pub fn command_of_filename(base: &str) -> Option<String> {
    decode_encoded_filename(base.strip_prefix(ENCODED_FILENAME_PREFIX)?)
}

struct JsonEscaped<'text>(&'text str);

impl Display for JsonEscaped<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for ch in self.0.chars() {
            match ch {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                '\n' => f.write_str("\\n")?,
                '\r' => f.write_str("\\r")?,
                '\t' => f.write_str("\\t")?,
                '\x08' => f.write_str("\\b")?,
                '\x0c' => f.write_str("\\f")?,
                control if control < '\x20' => {
                    write!(f, "\\u{:04x}", u32::from(control))?;
                },
                other => f.write_char(other)?,
            }
        }
        Ok(())
    }
}

fn json_string(text: &str) -> String {
    format!("\"{}\"", JsonEscaped(text))
}

fn json_switch(switch: &OwnedSwitch) -> String {
    match switch {
        OwnedSwitch::Short(short) => {
            format!(
                r#"{{"type":"short","char":{}}}"#,
                json_string(&short.to_string())
            )
        },
        OwnedSwitch::Long(long) => {
            format!(r#"{{"type":"long","name":{}}}"#, json_string(long))
        },
        OwnedSwitch::Both(short, long) => {
            format!(
                r#"{{"type":"both","char":{},"name":{}}}"#,
                json_string(&short.to_string()),
                json_string(long)
            )
        },
    }
}

fn json_choice(choice: &ParamChoice) -> String {
    format!(
        r#"{{"value":{},"desc":{}}}"#,
        json_string(&choice.value),
        json_string(&choice.desc)
    )
}

fn json_param(maybe_param: Option<&OwnedParam>) -> String {
    let Some(param) = maybe_param else {
        return "null".to_owned();
    };
    let kind = match param.kind {
        ParamKind::Mandatory => "mandatory",
        ParamKind::Optional => "optional",
        ParamKind::OptionalWord => "optional-word",
    };
    format!(
        r#"{{"kind":"{kind}","name":{},"choices":{}}}"#,
        json_string(&param.name),
        json_list(&param.choices, json_choice)
    )
}

fn json_entry(entry: &ManpageEntry) -> String {
    format!(
        r#"{{"switch":{},"param":{},"desc":{}}}"#,
        json_switch(&entry.switch),
        json_param(entry.param.as_ref()),
        json_string(&entry.desc)
    )
}

fn json_subcommand(sc: &ManpageSubcommand) -> String {
    let aliases = sc
        .aliases
        .iter()
        .map(|alias| json_string(alias))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"name":{},"desc":{},"aliases":[{}]}}"#,
        json_string(&sc.name),
        json_string(&sc.desc),
        aliases
    )
}

fn json_positional(name: &str, positional: &Positional) -> String {
    format!(
        r#"{{"name":{},"optional":{},"variadic":{}}}"#,
        json_string(name),
        positional.optional,
        positional.variadic
    )
}

pub fn json_strings(items: &[&str]) -> String {
    json_list(items, |item| json_string(item))
}

fn json_list<T, F>(items: &[T], render: F) -> String
where
    F: Fn(&T) -> String,
{
    let parts: Vec<String> = items.iter().map(render).collect();
    format!("[{}]", parts.join(","))
}

pub fn json_of_result(source: &str, result: &ManpageResult) -> String {
    let entries = json_list(&result.entries, json_entry);
    let subcommands = json_list(&result.subcommands, json_subcommand);
    let positional_choices = json_list(&result.positional_choices, json_subcommand);
    let positionals_parts: Vec<String> = result
        .positionals
        .iter()
        .map(|(name, positional)| json_positional(name, positional))
        .collect();
    let positionals = format!("[{}]", positionals_parts.join(","));
    format!(
        r#"{{"source":{},"description":{},"entries":{},"subcommands":{},"positional_choices":{},"positionals":{}}}"#,
        json_string(source),
        json_string(&result.description),
        entries,
        subcommands,
        positional_choices,
        positionals,
    )
}

/// atomic: same-dir temp file then rename.
///
/// # Errors
/// fails when the parent dir, the temp file, or the rename fails.
#[expect(
    clippy::let_underscore_must_use,
    reason = "temp file cleanup is best effort, the rename error is what the caller sees"
)]
pub fn write_file(path: &Path, contents: &str) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let parent = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => {
            fs::create_dir_all(dir)?;
            dir
        },
        _ => Path::new("."),
    };
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = parent.join(format!(".{stem}.{}.{seq}.tmp", process::id()));
    fs::write(&tmp, contents)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _: io::Result<()> = fs::remove_file(&tmp);
            Err(err)
        },
    }
}

/// # Errors
/// fails when the json write or the stale .nu removal fails.
pub fn write_result(
    dir: &Path,
    command: &str,
    source: &str,
    result: &ManpageResult,
) -> io::Result<()> {
    let base = filename_of_command(command);
    write_file(
        &dir.join(format!("{base}.json")),
        &json_of_result(source, result),
    )?;

    // lookup prefers .nu, so a leftover one would shadow the json just written.
    remove_if_present(&dir.join(format!("{base}.nu")))
}

/// store a native-nushell completion blob the binary supplied itself.
///
/// # Errors
/// fails when the .nu write or the stale json removal fails.
pub fn write_native(dir: &Path, command: &str, data: &str) -> io::Result<()> {
    let base = filename_of_command(command);
    write_file(&dir.join(format!("{base}.nu")), data)?;
    // the manpage pass merges into an existing json, and that write would
    // then drop this file.
    remove_if_present(&dir.join(format!("{base}.json")))
}

/// the binary a command's cache entries came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExeIdentity {
    path:     PathBuf,
    len:      u64,
    modified: u64,
}

impl ExeIdentity {
    pub fn of(bin: &Path) -> Option<Self> {
        let path = fs::canonicalize(bin).ok()?;
        let meta = fs::metadata(&path).ok()?;
        let modified = meta
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_secs();
        Some(Self {
            path,
            len: meta.len(),
            modified,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Display for ExeIdentity {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}\t{}\t{}",
            self.len,
            self.modified,
            self.path.display()
        )
    }
}

impl FromStr for ExeIdentity {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let mut fields = text.trim_end_matches('\n').splitn(3, '\t');
        let len = fields
            .next()
            .and_then(|field| field.parse().ok())
            .ok_or(())?;
        let modified = fields
            .next()
            .and_then(|field| field.parse().ok())
            .ok_or(())?;
        let path = fields.next().filter(|field| !field.is_empty()).ok_or(())?;
        Ok(Self {
            path: PathBuf::from(path),
            len,
            modified,
        })
    }
}

/// # Errors
/// fails when the sidecar write fails.
pub fn write_exe(dir: &Path, command: &str, identity: &ExeIdentity) -> io::Result<()> {
    let base = filename_of_command(command);
    write_file(&dir.join(format!("{base}.exe")), &identity.to_string())
}

pub fn read_exe(dir: &Path, command: &str) -> Option<ExeIdentity> {
    let base = filename_of_command(command);
    read_file(&dir.join(format!("{base}.exe")))?.parse().ok()
}

const MISS_TTL: Duration = Duration::from_mins(10);

/// mark `command` as unusable for binary `exe`, so completion skips probing it.
///
/// # Errors
/// fails when the marker write fails.
pub fn record_miss(dir: &Path, command: &str, exe: Option<&ExeIdentity>) -> io::Result<()> {
    let base = filename_of_command(command);
    let content = exe.map(ToString::to_string).unwrap_or_default();
    write_file(&dir.join(format!("{base}.miss")), &content)
}

pub fn recent_miss(dir: &Path, command: &str, exe: Option<&ExeIdentity>) -> bool {
    let path = dir.join(format!("{}.miss", filename_of_command(command)));
    let fresh = fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < MISS_TTL);
    fresh
        && read_file(&path).is_some_and(|stored| stored.parse::<ExeIdentity>().ok().as_ref() == exe)
}

/// a binary's fact, cached beside its command until the binary changes.
pub trait ExeFact: Display + FromStr {
    const EXTENSION: &'static str;
}

/// # Errors
/// fails when the sidecar write fails.
pub fn write_fact<Fact>(dir: &Path, command: &str, exe: &ExeIdentity, fact: &Fact) -> io::Result<()>
where
    Fact: ExeFact,
{
    let base = filename_of_command(command);
    write_file(
        &dir.join(format!("{base}.{}", Fact::EXTENSION)),
        &format!("{fact}\t{exe}"),
    )
}

pub fn read_fact<Fact>(dir: &Path, command: &str, exe: &ExeIdentity) -> Option<Fact>
where
    Fact: ExeFact,
{
    let base = filename_of_command(command);
    let stored = read_file(&dir.join(format!("{base}.{}", Fact::EXTENSION)))?;
    let (fact, identity) = stored.split_once('\t')?;
    (identity.parse::<ExeIdentity>().ok()? == *exe)
        .then(|| fact.parse().ok())
        .flatten()
}

/// remove `command`'s entries and its subcommands', returning how many went.
///
/// # Errors
/// fails when `dir` cannot be read or an entry cannot be removed.
pub fn purge_command(dir: &Path, command: &str) -> io::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    let prefix = format!("{command} ");
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let owned = path
            .file_stem()
            .and_then(OsStr::to_str)
            .and_then(command_of_filename)
            .is_some_and(|stored| stored == command || stored.starts_with(&prefix));
        let cache_ext = path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| matches!(ext, "json" | "nu" | "exe" | "miss" | "protocol"));
        if owned && cache_ext && path.is_file() {
            fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

fn read_file(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn read_json_result(path: &Path) -> Option<(String, ManpageResult)> {
    let data = read_file(path)?;
    let value = serde_json::from_str::<Value>(&data).ok()?;
    let source = value
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("json")
        .to_owned();
    Some((source, result_from_json(&value)))
}

pub fn read_result(dir: &Path, command: &str) -> Option<(String, ManpageResult)> {
    let path = dir.join(format!("{}.json", filename_of_command(command)));
    read_json_result(&path)
}

fn source_from_json_data(data: &str) -> String {
    serde_json::from_str::<Value>(data)
        .ok()
        .and_then(|value| {
            value
                .get("source")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .unwrap_or_else(|| "json".to_owned())
}

fn switch_from_json(value: &Value) -> Option<OwnedSwitch> {
    let kind = value.get("type")?.as_str()?;
    match kind {
        "short" => {
            let short = value.get("char")?.as_str()?.chars().next()?;
            Some(OwnedSwitch::Short(short))
        },
        "long" => Some(OwnedSwitch::Long(value.get("name")?.as_str()?.to_owned())),
        "both" => {
            let short = value.get("char")?.as_str()?.chars().next()?;
            let long = value.get("name")?.as_str()?.to_owned();
            Some(OwnedSwitch::Both(short, long))
        },
        _ => None,
    }
}

fn param_from_json(value: &Value) -> Option<OwnedParam> {
    if value.is_null() {
        return None;
    }
    let kind = match value.get("kind")?.as_str()? {
        "mandatory" => ParamKind::Mandatory,
        "optional" => ParamKind::Optional,
        "optional-word" => ParamKind::OptionalWord,
        _ => return None,
    };
    let name = value.get("name")?.as_str()?.to_owned();
    let choices = value
        .get("choices")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(choice_from_json).collect())
        .unwrap_or_default();
    Some(OwnedParam {
        kind,
        name,
        choices,
    })
}

fn choice_from_json(value: &Value) -> Option<ParamChoice> {
    Some(ParamChoice {
        value: value.get("value")?.as_str()?.to_owned(),
        desc:  value
            .get("desc")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
    })
}

fn entry_from_json(value: &Value) -> Option<ManpageEntry> {
    let switch = switch_from_json(value.get("switch")?)?;
    let param = value.get("param").and_then(param_from_json);
    let desc = value
        .get("desc")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    Some(ManpageEntry {
        switch,
        param,
        desc,
    })
}

fn subcommand_from_json(value: &Value) -> Option<ManpageSubcommand> {
    let name = value.get("name")?.as_str()?.to_owned();
    let desc = value
        .get("desc")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    // absent in pre-alias caches; defaults to none.
    let aliases = value
        .get("aliases")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|alias| alias.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Some(ManpageSubcommand {
        name,
        desc,
        aliases,
    })
}

fn positional_from_json(value: &Value) -> Option<(String, Positional)> {
    let name = value.get("name")?.as_str()?.to_owned();
    let optional = value
        .get("optional")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let variadic = value
        .get("variadic")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some((name, Positional { optional, variadic }))
}

pub fn result_from_json(value: &Value) -> ManpageResult {
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let entries = value
        .get("entries")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(entry_from_json).collect())
        .unwrap_or_default();
    let subcommands = value
        .get("subcommands")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(subcommand_from_json).collect())
        .unwrap_or_default();
    let positional_choices = value
        .get("positional_choices")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(subcommand_from_json).collect())
        .unwrap_or_default();
    let positionals = value
        .get("positionals")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(positional_from_json).collect())
        .unwrap_or_default();
    ManpageResult {
        entries,
        subcommands,
        positional_choices,
        positionals,
        description,
    }
}

/// native nushell before JSON. a subcommand also checks its parent's .nu,
/// since clap-generated files hold every extern block in one file.
pub fn lookup(dirs: &[PathBuf], command: &str) -> Option<ManpageResult> {
    let base_name = filename_of_command(command);
    let parent_base = command
        .split_once(' ')
        .map(|(parent, _)| filename_of_command(parent));
    for directory in dirs {
        let native = iter::once(&base_name).chain(&parent_base).find_map(|base| {
            let data = read_file(&directory.join(format!("{base}.nu")))?;
            Some(parse_nu_completions(command, &data)).filter(ManpageResult::has_structured_content)
        });
        if native.is_some() {
            return native;
        }
        if let Some((_, result)) = read_json_result(&directory.join(format!("{base_name}.json"))) {
            return Some(result);
        }
    }
    None
}

pub fn lookup_raw(dirs: &[PathBuf], command: &str) -> Option<String> {
    let base_name = filename_of_command(command);
    for directory in dirs {
        if let Some(data) = read_file(&directory.join(format!("{base_name}.nu")))
            && parse_nu_completions(command, &data).has_structured_content()
        {
            return Some(data);
        }
        if let Some(data) = read_file(&directory.join(format!("{base_name}.json"))) {
            return Some(data);
        }
    }
    None
}

fn chop_extension(filename: &str) -> Option<&str> {
    filename
        .strip_suffix(".json")
        .or_else(|| filename.strip_suffix(".nu"))
}

pub fn all_commands(dirs: &[PathBuf]) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for directory in dirs {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && let Some(base) = chop_extension(name)
                && let Some(command) = command_of_filename(base)
            {
                out.insert(command);
            }
        }
    }
    out.into_iter().collect()
}

/// only cache extensions go, so a misaimed dir keeps unrelated files.
/// missing dir counts as already empty.
///
/// # Errors
/// fails when `dir` cannot be read or a cache file cannot be removed.
pub fn purge_dir(dir: &Path) -> io::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_cache_file = path
            .file_name()
            .and_then(OsStr::to_str)
            .and_then(|name| {
                chop_extension(name)
                    .or_else(|| name.strip_suffix(".exe"))
                    .or_else(|| name.strip_suffix(".miss"))
                    .or_else(|| name.strip_suffix(".protocol"))
            })
            .is_some();
        if is_cache_file && path.is_file() {
            fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn subcommands_of(dirs: &[PathBuf], command: &str) -> Vec<ManpageSubcommand> {
    let mut seen: HashMap<String, ManpageSubcommand> = HashMap::new();
    let command_prefix = format!("{command} ");
    for directory in dirs {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let os_filename = entry.file_name();
            let Some(filename) = os_filename.to_str() else {
                continue;
            };
            let is_json = Path::new(filename).extension() == Some(OsStr::new("json"));
            let Some(base) = chop_extension(filename) else {
                continue;
            };
            let Some(stored_command) = command_of_filename(base) else {
                continue;
            };
            let Some(rest) = stored_command.strip_prefix(&command_prefix) else {
                continue;
            };
            if rest.is_empty() || rest.contains(' ') {
                continue;
            }
            if seen.contains_key(rest) {
                continue;
            }
            let desc = if is_json {
                read_file(&entry.path())
                    .and_then(|data| serde_json::from_str::<Value>(&data).ok())
                    .and_then(|value| {
                        value
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_default()
            } else {
                String::new()
            };
            seen.insert(
                rest.to_owned(),
                ManpageSubcommand::new(rest.to_owned(), desc),
            );
        }
    }
    let mut out: Vec<ManpageSubcommand> = seen.into_values().collect();
    out.sort_by(|left, right| left.name.cmp(&right.name));
    out
}

/// the JSON "source" field, or "native" for a .nu blob.
pub fn file_type_of(dirs: &[PathBuf], command: &str) -> Option<String> {
    let base = filename_of_command(command);
    for directory in dirs {
        if let Some(data) = read_file(&directory.join(format!("{base}.nu")))
            && parse_nu_completions(command, &data).has_structured_content()
        {
            return Some("native".to_owned());
        }
        let json_path = directory.join(format!("{base}.json"));
        if json_path.exists() {
            let source = read_file(&json_path)
                .map_or_else(|| "json".to_owned(), |data| source_from_json_data(&data));
            return Some(source);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::{
        slice,
        time::UNIX_EPOCH,
    };

    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("inshellah-store-{tag}-{}-{nanos}", process::id()))
    }

    #[test]
    fn write_file_creates_parents_and_writes_contents() {
        let dir = unique_dir("write");
        let path = dir.join("nested/git_add.json");
        write_file(&path, "{\"ok\":true}").unwrap();
        assert_eq!(read_file(&path).as_deref(), Some("{\"ok\":true}"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_file_overwrites_and_leaves_no_temp_files() {
        let dir = unique_dir("overwrite");
        let path = dir.join("git.nu");
        write_file(&path, "old").unwrap();
        write_file(&path, "new").unwrap();
        assert_eq!(read_file(&path).as_deref(), Some("new"));

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|leftover| leftover.extension() == Some(OsStr::new("tmp")))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn encoded_filenames_round_trip_spaces_and_underscores() {
        let spaced = filename_of_command("demo foo");
        let underscored = filename_of_command("demo foo_bar");
        assert_ne!(spaced, underscored);
        assert_eq!(command_of_filename(&spaced).as_deref(), Some("demo foo"));
        assert_eq!(
            command_of_filename(&underscored).as_deref(),
            Some("demo foo_bar")
        );
        assert_eq!(command_of_filename("demo_foo"), None);
    }

    #[test]
    fn lookup_ignores_empty_native_result_before_json() {
        let dir = unique_dir("empty-native");
        fs::create_dir_all(&dir).unwrap();
        let result = ManpageResult {
            entries:            vec![ManpageEntry {
                switch: OwnedSwitch::Long("json".to_owned()),
                param:  None,
                desc:   "json flag".to_owned(),
            }],
            subcommands:        Vec::new(),
            positional_choices: Vec::new(),
            positionals:        Vec::new(),
            description:        String::new(),
        };
        write_result(&dir, "demo", "help", &result).unwrap();
        write_file(
            &dir.join(format!("{}.nu", filename_of_command("demo"))),
            r#"export extern "other" [
  --native
]
"#,
        )
        .unwrap();

        let found = lookup(slice::from_ref(&dir), "demo").unwrap();
        assert!(
            found
                .entries
                .iter()
                .any(|entry| matches!(&entry.switch, OwnedSwitch::Long(name) if name == "json"))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn user_cache_age_reports_recent_write_and_none_when_absent() {
        let dir = unique_dir("cache-age");
        fs::create_dir_all(&dir).unwrap();
        let result = ManpageResult {
            entries:            Vec::new(),
            subcommands:        Vec::new(),
            positional_choices: Vec::new(),
            positionals:        Vec::new(),
            description:        String::new(),
        };
        write_result(&dir, "command", "manpage", &result).unwrap();
        let age = user_cache_age(&dir, "command").expect("cache file should exist");
        assert!(age.as_secs() < 60, "age too large: {age:?}");
        assert!(user_cache_age(&dir, "nonexistent-command").is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn subcommands_of_decodes_immediate_underscored_child() {
        let dir = unique_dir("subcommands");
        fs::create_dir_all(&dir).unwrap();
        let result = ManpageResult {
            entries:            Vec::new(),
            subcommands:        Vec::new(),
            positional_choices: Vec::new(),
            positionals:        Vec::new(),
            description:        "child desc".to_owned(),
        };
        write_result(&dir, "demo foo_bar", "manpage", &result).unwrap();
        write_result(&dir, "demo foo_bar nested", "manpage", &result).unwrap();
        let subs = subcommands_of(slice::from_ref(&dir), "demo");
        assert_eq!(
            subs.iter().map(|sub| sub.name.as_str()).collect::<Vec<_>>(),
            vec!["foo_bar"]
        );
        assert_eq!(subs[0].desc, "child desc");
        fs::remove_dir_all(&dir).unwrap();
    }
}
