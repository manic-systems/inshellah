// SPDX-License-Identifier: EUPL-1.2
//! generate nushell `extern` definitions from a [`ManpageResult`].

use std::borrow::Cow;

use crate::parsers::manpage::{
    ManpageEntry, ManpageResult, ManpageSubcommand, OwnedParam, OwnedSwitch,
};
use crate::types::Positional;

pub fn nushell_type_of_param(name: &str) -> &'static str {
    match name {
        "FILE" | "file" | "PATH" | "path" | "DIR" | "dir" | "DIRECTORY" | "FILENAME"
        | "PATTERNFILE" => "path",
        "NUM" | "N" | "COUNT" | "NUMBER" | "int" | "INT" | "COLS" | "WIDTH" | "LINES" | "DEPTH"
        | "depth" => "int",
        _ => "string",
    }
}

pub fn escape_nu(s: &str) -> Cow<'_, str> {
    if !s
        .bytes()
        .any(|b| matches!(b, b'"' | b'\\' | b'\n' | b'\r' | b'\t'))
    {
        Cow::Borrowed(s)
    } else {
        let mut buf = String::with_capacity(s.len() + 4);
        for c in s.chars() {
            match c {
                '"' => buf.push_str("\\\""),
                '\\' => buf.push_str("\\\\"),
                '\n' => buf.push_str("\\n"),
                '\r' => buf.push_str("\\r"),
                '\t' => buf.push_str("\\t"),
                c => buf.push(c),
            }
        }
        Cow::Owned(buf)
    }
}

fn sanitize_token(s: &str) -> Cow<'_, str> {
    if s.chars().any(char::is_control) {
        Cow::Owned(s.chars().filter(|c| !c.is_control()).collect())
    } else {
        Cow::Borrowed(s)
    }
}

/// desc is right-padded to column 40 with a "# " prefix.
pub fn format_flag(entry: &ManpageEntry) -> String {
    let name = match &entry.switch {
        OwnedSwitch::Both(c, l) => format!("--{l}(-{c})"),
        OwnedSwitch::Long(l) => format!("--{l}"),
        OwnedSwitch::Short(c) => format!("-{c}"),
    };
    let name = sanitize_token(&name);
    let typed = match &entry.param {
        Some(OwnedParam::Mandatory(p)) | Some(OwnedParam::Optional(p)) => {
            format!(": {}", nushell_type_of_param(p))
        }
        None => String::new(),
    };
    let flag = format!("    {name}{typed}");
    if entry.desc.is_empty() {
        flag
    } else {
        let pad_len = 40usize.saturating_sub(flag.len()).max(1);
        format!("{flag}{}# {}", " ".repeat(pad_len), escape_nu(&entry.desc))
    }
}

/// hyphens become underscores, nushell identifiers can't contain hyphens.
pub fn format_positional(name: &str, p: &Positional) -> String {
    let name_underscored: String = sanitize_token(name)
        .chars()
        .map(|c| if c == '-' { '_' } else { c })
        .collect();
    let name_upper = name.to_ascii_uppercase();
    let prefix = if p.variadic { "..." } else { "" };
    let suffix = if p.optional && !p.variadic { "?" } else { "" };
    let typ = nushell_type_of_param(&name_upper);
    let typ = if typ == "string" { "glob" } else { typ };
    format!("    {prefix}{name_underscored}{suffix}: {typ}")
}

/// nushell forbids required-after-optional (promote to optional) and more than
/// one variadic (drop extras).
pub fn fixup_positionals(positionals: Vec<(String, Positional)>) -> Vec<(String, Positional)> {
    let mut seen_optional = false;
    let mut seen_variadic = false;
    let mut out = Vec::with_capacity(positionals.len());
    for (name, mut p) in positionals {
        if p.variadic {
            if seen_variadic {
                continue;
            }
            seen_variadic = true;
            seen_optional = true;
            out.push((name, p));
        } else if seen_optional {
            p.optional = true;
            out.push((name, p));
        } else {
            seen_optional = p.optional;
            out.push((name, p));
        }
    }
    out
}

pub fn module_name_of(cmd_name: &str) -> String {
    let mut s: String = cmd_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    s.push_str("-completions");
    s
}

/// unresolved subcommands get a stub block whose only content is a `# desc`
/// comment.
pub fn generate_extern(cmd_name: &str, result: &ManpageResult) -> String {
    let escaped_name = escape_nu(cmd_name);
    let positionals = fixup_positionals(result.positionals.clone());

    let mut out = String::new();
    out.push_str(&format!("export extern \"{escaped_name}\" [\n"));
    for (name, p) in &positionals {
        out.push_str(&format_positional(name, p));
        out.push('\n');
    }
    for entry in &result.entries {
        out.push_str(&format_flag(entry));
        out.push('\n');
    }
    out.push_str("]\n");

    for sc in &result.subcommands {
        out.push_str(&format!(
            "\nexport extern \"{} {}\" [  # {}\n]\n",
            escaped_name,
            escape_nu(&sc.name),
            escape_nu(&sc.desc)
        ));
    }
    out
}

/// trailing `use` brings the module's externs into scope.
pub fn generate_module(cmd_name: &str, result: &ManpageResult) -> String {
    let mod_name = module_name_of(cmd_name);
    format!(
        "module {mod_name} {{\n{}}}\n\nuse {mod_name} *\n",
        generate_extern(cmd_name, result)
    )
}

/// immediate child blocks (`cmd sub`) fold into `target_cmd`'s subcommands.
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

    let Some(matched) = blocks.iter().find(|b| b.cmd == target_cmd) else {
        return ManpageResult::default();
    };

    let prefix = format!("{target_cmd} ");
    let mut subcommands: Vec<ManpageSubcommand> = Vec::new();
    for b in &blocks {
        if let Some(suffix) = b.cmd.strip_prefix(&prefix)
            && !suffix.contains(' ')
            && !suffix.is_empty()
        {
            subcommands.push(ManpageSubcommand::new(
                suffix.to_string(),
                b.description.clone(),
            ));
        }
    }

    ManpageResult {
        entries: matched.entries.clone(),
        subcommands,
        positional_choices: Vec::new(),
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

/// always Mandatory: nushell extern syntax has no optional-with-default to
/// distinguish.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::manpage::{ManpageResult, ManpageSubcommand};

    #[test]
    fn escape_nu_escapes_line_breaking_chars() {
        assert_eq!(escape_nu("a\nb"), "a\\nb");
        assert_eq!(escape_nu("a\rb"), "a\\rb");
        assert_eq!(escape_nu("a\tb"), "a\\tb");
        assert_eq!(escape_nu("say \"hi\""), "say \\\"hi\\\"");
        assert!(matches!(escape_nu("plain"), Cow::Borrowed(_)));
    }

    #[test]
    fn flag_description_newline_does_not_break_out_of_line() {
        let entry = ManpageEntry {
            switch: OwnedSwitch::Long("verbose".into()),
            param: None,
            desc: "be loud\nrm -rf /".into(),
        };
        let line = format_flag(&entry);
        assert_eq!(
            line.lines().count(),
            1,
            "flag line must stay single-line: {line:?}"
        );
        assert!(!line.contains('\n'));
        assert!(line.contains("rm -rf /"));
    }

    #[test]
    fn subcommand_name_and_desc_newline_stays_single_line() {
        let result = ManpageResult {
            subcommands: vec![ManpageSubcommand::new(
                "stash\"]\nrm -rf /".into(),
                "danger\nous".into(),
            )],
            ..Default::default()
        };
        let out = generate_extern("git", &result);
        assert!(
            !out.contains("\"]\n"),
            "string literal not closed early: {out:?}"
        );
        assert!(out.contains("rm -rf /"));
    }

    #[test]
    fn native_nu_file_parsing_reads_flags_positionals_and_child_blocks() {
        let nu_source = r#"module completions {

  # Unofficial CLI tool
  export extern mytool [
    --help(-h)                # Print help
    --version(-V)             # Print version
  ]

  # List all items
  export extern "mytool list" [
    --raw                     # Output as JSON
    --format(-f): string      # Output format
    --help(-h)                # Print help
    name?: string             # Filter by name
  ]

}

use completions *
"#;
        let root = parse_nu_completions("mytool", nu_source);
        assert_eq!(root.entries.len(), 2, "entries: {:?}", root.entries);
        assert!(root.subcommands.iter().any(|sc| sc.name == "list"));
        assert_eq!(root.description, "Unofficial CLI tool");

        let list = parse_nu_completions("mytool list", nu_source);
        assert_eq!(list.entries.len(), 3, "list entries: {:?}", list.entries);
        assert!(
            list.entries
                .iter()
                .any(|e| matches!(&e.switch, OwnedSwitch::Both('f', long) if long == "format")),
            "list should have --format(-f): {:?}",
            list.entries
        );
        assert!(
            !list.positionals.is_empty(),
            "list should have a positional"
        );
    }
}
