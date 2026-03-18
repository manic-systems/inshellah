//! filesystem store for parsed completion data.
//!
//! write side: serialize ManpageResult to JSON, derive sanitised
//! filenames from command names ("git add" → git_add.json).
//!
//! read side: look up a command by name across the user cache + system
//! dirs, deserialize JSON or parse a .nu extern blob back into a result.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::parsers::manpage::{
    ManpageEntry, ManpageResult, ManpageSubcommand, OwnedParam, OwnedSwitch,
};
use crate::types::Positional;

/// default cache directory: $XDG_CACHE_HOME/inshellah, falling back to
/// $HOME/.cache/inshellah.
pub fn default_store_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("inshellah");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache/inshellah");
    }
    PathBuf::from(".cache/inshellah")
}

/// create directory and all parents.
pub fn ensure_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

/// derive a safe filename from a command name.
/// spaces in subcommand names ("git add") become "_" ("git_add").
/// any other non-filesystem-safe characters are also replaced.
pub fn filename_of_command(cmd: &str) -> String {
    cmd.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            ' ' => '_',
            _ => '_',
        })
        .collect()
}

/// reverse: a filename "git_add" produces command name "git add".
/// underscores are flipped to spaces unconditionally — names that
/// genuinely contained an underscore round-trip as spaces, which is
/// acceptable since the read side is only used for display.
pub fn command_of_filename(base: &str) -> String {
    base.replace('_', " ")
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn json_string(s: &str) -> String {
    format!("\"{}\"", escape_json(s))
}

fn json_switch(s: &OwnedSwitch) -> String {
    match s {
        OwnedSwitch::Short(c) => {
            format!(
                r#"{{"type":"short","char":{}}}"#,
                json_string(&c.to_string())
            )
        }
        OwnedSwitch::Long(l) => {
            format!(r#"{{"type":"long","name":{}}}"#, json_string(l))
        }
        OwnedSwitch::Both(c, l) => format!(
            r#"{{"type":"both","char":{},"name":{}}}"#,
            json_string(&c.to_string()),
            json_string(l)
        ),
    }
}

fn json_param(p: &Option<OwnedParam>) -> String {
    match p {
        None => "null".to_string(),
        Some(OwnedParam::Mandatory(n)) => {
            format!(r#"{{"kind":"mandatory","name":{}}}"#, json_string(n))
        }
        Some(OwnedParam::Optional(n)) => {
            format!(r#"{{"kind":"optional","name":{}}}"#, json_string(n))
        }
    }
}

fn json_entry(e: &ManpageEntry) -> String {
    format!(
        r#"{{"switch":{},"param":{},"desc":{}}}"#,
        json_switch(&e.switch),
        json_param(&e.param),
        json_string(&e.desc)
    )
}

fn json_subcommand(sc: &ManpageSubcommand) -> String {
    format!(
        r#"{{"name":{},"desc":{}}}"#,
        json_string(&sc.name),
        json_string(&sc.desc)
    )
}

fn json_positional(name: &str, p: &Positional) -> String {
    format!(
        r#"{{"name":{},"optional":{},"variadic":{}}}"#,
        json_string(name),
        p.optional,
        p.variadic
    )
}

fn json_list<T, F: Fn(&T) -> String>(items: &[T], f: F) -> String {
    let parts: Vec<String> = items.iter().map(f).collect();
    format!("[{}]", parts.join(","))
}

/// serialize a ManpageResult to JSON:
///   {"source":..., "description":..., "entries":[...],
///    "subcommands":[...], "positionals":[...]}
pub fn json_of_result(source: &str, result: &ManpageResult) -> String {
    let entries = json_list(&result.entries, json_entry);
    let subcommands = json_list(&result.subcommands, json_subcommand);
    let positionals_parts: Vec<String> = result
        .positionals
        .iter()
        .map(|(name, p)| json_positional(name, p))
        .collect();
    let positionals = format!("[{}]", positionals_parts.join(","));
    format!(
        r#"{{"source":{},"description":{},"entries":{},"subcommands":{},"positionals":{}}}"#,
        json_string(source),
        json_string(&result.description),
        entries,
        subcommands,
        positionals,
    )
}

pub fn write_file(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}

/// write the parsed result for `command` into `dir` as JSON.
pub fn write_result(
    dir: &Path,
    command: &str,
    source: &str,
    result: &ManpageResult,
) -> io::Result<()> {
    let path = dir.join(format!("{}.json", filename_of_command(command)));
    write_file(&path, &json_of_result(source, result))
}

/// write a native-nushell completion blob (the binary supplied its own).
pub fn write_native(dir: &Path, command: &str, data: &str) -> io::Result<()> {
    let path = dir.join(format!("{}.nu", filename_of_command(command)));
    write_file(&path, data)
}

// --- read side ---

fn read_file(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn read_json_result(path: &Path) -> Option<(String, ManpageResult)> {
    let data = read_file(path)?;
    let v = serde_json::from_str::<Value>(&data).ok()?;
    let source = v
        .get("source")
        .and_then(|x| x.as_str())
        .unwrap_or("json")
        .to_string();
    Some((source, result_from_json(&v)))
}

fn switch_from_json(v: &Value) -> Option<OwnedSwitch> {
    let t = v.get("type")?.as_str()?;
    match t {
        "short" => {
            let c = v.get("char")?.as_str()?.chars().next()?;
            Some(OwnedSwitch::Short(c))
        }
        "long" => Some(OwnedSwitch::Long(v.get("name")?.as_str()?.to_string())),
        "both" => {
            let c = v.get("char")?.as_str()?.chars().next()?;
            let n = v.get("name")?.as_str()?.to_string();
            Some(OwnedSwitch::Both(c, n))
        }
        _ => None,
    }
}

fn param_from_json(v: &Value) -> Option<OwnedParam> {
    if v.is_null() {
        return None;
    }
    let kind = v.get("kind")?.as_str()?;
    let name = v.get("name")?.as_str()?.to_string();
    Some(match kind {
        "mandatory" => OwnedParam::Mandatory(name),
        "optional" => OwnedParam::Optional(name),
        _ => return None,
    })
}

fn entry_from_json(v: &Value) -> Option<ManpageEntry> {
    let switch = switch_from_json(v.get("switch")?)?;
    let param = v.get("param").and_then(param_from_json);
    let desc = v
        .get("desc")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    Some(ManpageEntry {
        switch,
        param,
        desc,
    })
}

fn subcommand_from_json(v: &Value) -> Option<ManpageSubcommand> {
    let name = v.get("name")?.as_str()?.to_string();
    let desc = v
        .get("desc")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    Some(ManpageSubcommand { name, desc })
}

fn positional_from_json(v: &Value) -> Option<(String, Positional)> {
    let name = v.get("name")?.as_str()?.to_string();
    let optional = v.get("optional").and_then(|x| x.as_bool()).unwrap_or(false);
    let variadic = v.get("variadic").and_then(|x| x.as_bool()).unwrap_or(false);
    Some((name, Positional { optional, variadic }))
}

/// deserialize a JSON cache entry into ManpageResult.
pub fn result_from_json(v: &Value) -> ManpageResult {
    let description = v
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    let entries = v
        .get("entries")
        .and_then(|x| x.as_array())
        .map(|arr| arr.iter().filter_map(entry_from_json).collect())
        .unwrap_or_default();
    let subcommands = v
        .get("subcommands")
        .and_then(|x| x.as_array())
        .map(|arr| arr.iter().filter_map(subcommand_from_json).collect())
        .unwrap_or_default();
    let positionals = v
        .get("positionals")
        .and_then(|x| x.as_array())
        .map(|arr| arr.iter().filter_map(positional_from_json).collect())
        .unwrap_or_default();
    ManpageResult {
        entries,
        subcommands,
        positionals,
        description,
    }
}

/// parse nushell `export extern` blocks out of a .nu source file.
///
/// returns the help_result that matches `target_cmd` — its entries,
/// positionals, and any other extern blocks under it (`cmd sub`) are
/// folded into the subcommands list.
pub fn parse_nu_completions(target_cmd: &str, contents: &str) -> ManpageResult {
    let mut blocks: Vec<NuBlock> = Vec::new();
    let mut current_desc = String::new();
    let mut in_block = false;
    let mut block = NuBlock::default();

    for line in contents.split('\n') {
        let trimmed = line.trim();
        if !in_block {
            if let Some(stripped) = trimmed.strip_prefix("# ") {
                current_desc = stripped.trim().to_string();
            } else if trimmed.contains("export extern")
                && let Some(cmd) = extract_extern_name(trimmed)
            {
                in_block = true;
                block = NuBlock {
                    cmd,
                    description: std::mem::take(&mut current_desc),
                    ..Default::default()
                };
            } else {
                current_desc.clear();
            }
        } else if trimmed.starts_with(']') {
            blocks.push(std::mem::take(&mut block));
            in_block = false;
        } else {
            let (param_part, desc) = match trimmed.find('#') {
                Some(idx) => (trimmed[..idx].trim(), trimmed[idx + 1..].trim()),
                None => (trimmed, ""),
            };
            parse_nu_param_line_into(param_part, desc, &mut block);
        }
    }
    if in_block {
        blocks.push(block);
    }

    // find the block matching target_cmd
    let Some(matched) = blocks.iter().find(|b| b.cmd == target_cmd) else {
        return ManpageResult::default();
    };

    // collect immediate subcommands from other blocks ("target sub" pattern)
    let prefix = format!("{target_cmd} ");
    let mut subcommands: Vec<ManpageSubcommand> = Vec::new();
    for b in &blocks {
        if let Some(suffix) = b.cmd.strip_prefix(&prefix)
            && !suffix.contains(' ')
            && !suffix.is_empty()
        {
            subcommands.push(ManpageSubcommand {
                name: suffix.to_string(),
                desc: b.description.clone(),
            });
        }
    }

    ManpageResult {
        entries: matched.entries.clone(),
        subcommands,
        positionals: matched.positionals.clone(),
        description: matched.description.clone(),
    }
}

fn extract_extern_name(line: &str) -> Option<String> {
    let idx = line.find("export extern")?;
    let after = line[idx + "export extern".len()..].trim_start();
    if let Some(rest) = after.strip_prefix('"') {
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    } else {
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(after.len());
        if end == 0 {
            None
        } else {
            Some(after[..end].to_string())
        }
    }
}

fn parse_nu_param_line_into(param_part: &str, desc: &str, block: &mut NuBlock) {
    if param_part.len() < 2 {
        return;
    }
    if let Some(after) = param_part.strip_prefix("--") {
        // long flag: --name(-c): type or --name: type or --name
        let (name, rest) = split_at_non_name_char(after);
        if name.is_empty() {
            return;
        }
        let mut short: Option<char> = None;
        let mut rest = rest;
        if let Some(after_open) = rest.strip_prefix("(-")
            && let Some(c) = after_open.chars().next()
            && after_open[c.len_utf8()..].starts_with(')')
        {
            short = Some(c);
            rest = &after_open[c.len_utf8() + 1..];
        }
        let param = parse_type_suffix(rest);
        let switch = match short {
            Some(c) => OwnedSwitch::Both(c, name.to_string()),
            None => OwnedSwitch::Long(name.to_string()),
        };
        block.entries.push(ManpageEntry {
            switch,
            param,
            desc: desc.to_string(),
        });
    } else if param_part.starts_with('-') {
        // short flag: -c
        if let Some(c) = param_part.chars().nth(1)
            && c.is_ascii_alphanumeric()
        {
            block.entries.push(ManpageEntry {
                switch: OwnedSwitch::Short(c),
                param: None,
                desc: desc.to_string(),
            });
        }
    } else {
        // positional: name: type or name?: type or ...name: type
        let variadic = param_part.starts_with("...");
        let after_prefix = if variadic {
            &param_part[3..]
        } else {
            param_part
        };
        let optional = after_prefix.contains('?');
        let name_end = after_prefix.find([':', '?']).unwrap_or(after_prefix.len());
        let name = after_prefix[..name_end].trim();
        let name: String = name
            .chars()
            .map(|c| if c == '-' { '_' } else { c })
            .collect();
        if !name.is_empty() && !name.starts_with('-') {
            let duplicate = block
                .positionals
                .iter()
                .any(|(existing, _)| existing.eq_ignore_ascii_case(&name));
            if !duplicate {
                block.positionals.push((
                    name,
                    Positional {
                        optional: optional || variadic,
                        variadic,
                    },
                ));
            }
        }
    }
}

fn split_at_non_name_char(s: &str) -> (&str, &str) {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or(s.len());
    (&s[..end], &s[end..])
}

/// parse a `: type` suffix into an OwnedParam (always Mandatory since the
/// nushell extern syntax doesn't distinguish optional-with-default).
fn parse_type_suffix(s: &str) -> Option<OwnedParam> {
    let s = s.trim_start();
    let s = s.strip_prefix(':')?;
    let s = s.trim_start();
    let end = s
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(s.len());
    if end == 0 {
        None
    } else {
        Some(OwnedParam::Mandatory(s[..end].to_string()))
    }
}

#[derive(Default)]
struct NuBlock {
    cmd: String,
    entries: Vec<ManpageEntry>,
    positionals: Vec<(String, Positional)>,
    description: String,
}

/// look up a command's parsed result. source priority is native nushell,
/// then manpage JSON, then help JSON. parent .nu files are searched for
/// subcommand lookups because clap-generated .nu files contain all extern
/// blocks in a single file.
pub fn lookup(dirs: &[PathBuf], command: &str) -> Option<ManpageResult> {
    let base_name = filename_of_command(command);
    let parent_base = command
        .find(' ')
        .map(|i| filename_of_command(&command[..i]));

    for directory in dirs {
        let nu_path = directory.join(format!("{base_name}.nu"));
        if let Some(data) = read_file(&nu_path) {
            return Some(parse_nu_completions(command, &data));
        }
        if let Some(pb) = &parent_base {
            let parent_nu = directory.join(format!("{pb}.nu"));
            if let Some(data) = read_file(&parent_nu) {
                let r = parse_nu_completions(command, &data);
                if !r.entries.is_empty() || !r.subcommands.is_empty() || !r.positionals.is_empty() {
                    return Some(r);
                }
            }
        }
    }

    for directory in dirs {
        let json_path = directory.join(format!("{base_name}.json"));
        if let Some((source, result)) = read_json_result(&json_path)
            && source != "help"
        {
            return Some(result);
        }
    }

    for directory in dirs {
        let json_path = directory.join(format!("{base_name}.json"));
        if let Some((_, result)) = read_json_result(&json_path) {
            return Some(result);
        }
    }
    None
}

/// look up a command's raw stored data (JSON or .nu source).
pub fn lookup_raw(dirs: &[PathBuf], command: &str) -> Option<String> {
    let base_name = filename_of_command(command);
    for directory in dirs {
        let nu_path = directory.join(format!("{base_name}.nu"));
        if let Some(data) = read_file(&nu_path) {
            return Some(data);
        }
    }
    for directory in dirs {
        let json_path = directory.join(format!("{base_name}.json"));
        if let Some(data) = read_file(&json_path) {
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

/// list all indexed commands across all store directories.
/// returns a sorted, deduplicated list of command names.
pub fn all_commands(dirs: &[PathBuf]) -> Vec<String> {
    let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for directory in dirs {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && let Some(base) = chop_extension(name)
            {
                out.insert(command_of_filename(base));
            }
        }
    }
    out.into_iter().collect()
}

/// discover subcommands of a command by scanning filenames in the store
/// (e.g. for "git", finds "git_add.json", "git_log.json").
pub fn subcommands_of(dirs: &[PathBuf], command: &str) -> Vec<ManpageSubcommand> {
    let prefix = format!("{}_", filename_of_command(command));
    let mut seen: HashMap<String, ManpageSubcommand> = HashMap::new();
    for directory in dirs {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(filename) = entry.file_name().to_str().map(|s| s.to_string()) else {
                continue;
            };
            if !filename.starts_with(&prefix) {
                continue;
            }
            let is_json = filename.ends_with(".json");
            let Some(base) = chop_extension(&filename) else {
                continue;
            };
            let rest = &base[prefix.len()..];
            if rest.is_empty() || rest.contains('_') {
                continue;
            }
            if seen.contains_key(rest) {
                continue;
            }
            let desc = if is_json {
                read_file(&entry.path())
                    .and_then(|d| serde_json::from_str::<Value>(&d).ok())
                    .and_then(|v| {
                        v.get("description")
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default()
            } else {
                String::new()
            };
            seen.insert(
                rest.to_string(),
                ManpageSubcommand {
                    name: rest.to_string(),
                    desc,
                },
            );
        }
    }
    let mut out: Vec<ManpageSubcommand> = seen.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// determine how a command was indexed: "help", "manpage", "native", etc.
/// for JSON files, returns the "source" field. for .nu files, returns "native".
pub fn file_type_of(dirs: &[PathBuf], command: &str) -> Option<String> {
    let base = filename_of_command(command);
    for directory in dirs {
        let nu_path = directory.join(format!("{base}.nu"));
        if nu_path.exists() {
            return Some("native".to_string());
        }
    }
    for directory in dirs {
        let json_path = directory.join(format!("{base}.json"));
        if json_path.exists() {
            return Some(
                read_file(&json_path)
                    .and_then(|d| serde_json::from_str::<Value>(&d).ok())
                    .and_then(|v| v.get("source").and_then(|x| x.as_str()).map(String::from))
                    .unwrap_or_else(|| "json".to_string()),
            );
        }
    }
    None
}
