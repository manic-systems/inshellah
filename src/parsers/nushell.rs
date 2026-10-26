// SPDX-License-Identifier: EUPL-1.2
//! generate nushell `extern` definitions from a [`ManpageResult`].

use std::borrow::Cow;
use std::mem;

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

pub fn escape_nu(text: &str) -> Cow<'_, str> {
    if text
        .bytes()
        .any(|byte| matches!(byte, b'"' | b'\\' | b'\n' | b'\r' | b'\t'))
    {
        let mut buf = String::with_capacity(text.len() + 4);
        for ch in text.chars() {
            match ch {
                '"' => buf.push_str("\\\""),
                '\\' => buf.push_str("\\\\"),
                '\n' => buf.push_str("\\n"),
                '\r' => buf.push_str("\\r"),
                '\t' => buf.push_str("\\t"),
                other => buf.push(other),
            }
        }
        Cow::Owned(buf)
    } else {
        Cow::Borrowed(text)
    }
}

fn sanitize_token(token: &str) -> Cow<'_, str> {
    if token.chars().any(char::is_control) {
        Cow::Owned(token.chars().filter(|ch| !ch.is_control()).collect())
    } else {
        Cow::Borrowed(token)
    }
}

/// desc is right-padded to column 40 with a "# " prefix.
pub fn format_flag(entry: &ManpageEntry) -> String {
    let raw_name = match &entry.switch {
        OwnedSwitch::Both(short, long) => format!("--{long}(-{short})"),
        OwnedSwitch::Long(long) => format!("--{long}"),
        OwnedSwitch::Short(short) => format!("-{short}"),
    };
    let name = sanitize_token(&raw_name);
    let typed = match &entry.param {
        Some(OwnedParam::Mandatory(param) | OwnedParam::Optional(param)) => {
            format!(": {}", nushell_type_of_param(param))
        }
        None => String::new(),
    };
    let flag = format!("    {name}{typed}");
    if entry.desc.is_empty() {
        flag
    } else {
        let pad_len = 40_usize.saturating_sub(flag.len()).max(1);
        format!("{flag}{}# {}", " ".repeat(pad_len), escape_nu(&entry.desc))
    }
}

/// hyphens become underscores, nushell identifiers can't contain hyphens.
pub fn format_positional(name: &str, positional: &Positional) -> String {
    let name_underscored: String = sanitize_token(name)
        .chars()
        .map(|ch| if ch == '-' { '_' } else { ch })
        .collect();
    let name_upper = name.to_ascii_uppercase();
    let prefix = if positional.variadic { "..." } else { "" };
    let suffix = if positional.optional && !positional.variadic {
        "?"
    } else {
        ""
    };
    let base_type = nushell_type_of_param(&name_upper);
    let typ = if base_type == "string" {
        "glob"
    } else {
        base_type
    };
    format!("    {prefix}{name_underscored}{suffix}: {typ}")
}

/// nushell forbids required-after-optional (promote to optional) and more than
/// one variadic (drop extras).
pub fn fixup_positionals(positionals: Vec<(String, Positional)>) -> Vec<(String, Positional)> {
    let mut seen_optional = false;
    let mut seen_variadic = false;
    let mut out = Vec::with_capacity(positionals.len());
    for (name, mut positional) in positionals {
        if positional.variadic {
            if seen_variadic {
                continue;
            }
            seen_variadic = true;
            seen_optional = true;
        } else if seen_optional {
            positional.optional = true;
        } else {
            seen_optional = positional.optional;
        }
        out.push((name, positional));
    }
    out
}

pub fn module_name_of(cmd_name: &str) -> String {
    let mut module: String = cmd_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    module.push_str("-completions");
    module
}

/// unresolved subcommands get a stub block whose only content is a `# desc`
/// comment.
pub fn generate_extern(cmd_name: &str, result: &ManpageResult) -> String {
    let escaped_name = escape_nu(cmd_name);
    let positionals = fixup_positionals(result.positionals.clone());

    let mut out = String::new();
    out.push_str("export extern \"");
    out.push_str(&escaped_name);
    out.push_str("\" [\n");
    for (name, positional) in &positionals {
        out.push_str(&format_positional(name, positional));
        out.push('\n');
    }
    for entry in &result.entries {
        out.push_str(&format_flag(entry));
        out.push('\n');
    }
    out.push_str("]\n");

    for subcommand in &result.subcommands {
        out.push_str("\nexport extern \"");
        out.push_str(&escaped_name);
        out.push(' ');
        out.push_str(&escape_nu(&subcommand.name));
        out.push_str("\" [  # ");
        out.push_str(&escape_nu(&subcommand.desc));
        out.push_str("\n]\n");
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
                stripped.trim().clone_into(&mut current_desc);
            } else if trimmed.contains("export extern")
                && let Some(cmd) = extract_extern_name(trimmed)
            {
                in_block = true;
                block = NuBlock {
                    cmd,
                    description: mem::take(&mut current_desc),
                    ..Default::default()
                };
            } else {
                current_desc.clear();
            }
        } else if trimmed.starts_with(']') {
            blocks.push(mem::take(&mut block));
            in_block = false;
        } else {
            let (param_part, desc) = trimmed
                .split_once('#')
                .map_or((trimmed, ""), |(param, desc)| (param.trim(), desc.trim()));
            parse_nu_param_line_into(param_part, desc, &mut block);
        }
    }
    if in_block {
        blocks.push(block);
    }

    let Some(matched) = blocks.iter().find(|parsed| parsed.cmd == target_cmd) else {
        return ManpageResult::default();
    };

    let prefix = format!("{target_cmd} ");
    let mut subcommands: Vec<ManpageSubcommand> = Vec::new();
    for child in &blocks {
        if let Some(suffix) = child.cmd.strip_prefix(&prefix)
            && !suffix.contains(' ')
            && !suffix.is_empty()
        {
            subcommands.push(ManpageSubcommand::new(
                suffix.to_owned(),
                child.description.clone(),
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
    let (_, tail) = line.split_once("export extern")?;
    let after = tail.trim_start();
    if let Some(rest) = after.strip_prefix('"') {
        let (name, _) = rest.split_once('"')?;
        Some(name.to_owned())
    } else {
        let name = after
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
            .next()?;
        if name.is_empty() {
            None
        } else {
            Some(name.to_owned())
        }
    }
}

fn parse_nu_param_line_into(param_part: &str, desc: &str, block: &mut NuBlock) {
    if param_part.len() < 2 {
        return;
    }
    if let Some(after) = param_part.strip_prefix("--") {
        let (name, after_name) = split_at_non_name_char(after);
        if name.is_empty() {
            return;
        }
        let mut short: Option<char> = None;
        let mut rest = after_name;
        if let Some(after_open) = after_name.strip_prefix("(-") {
            let mut chars = after_open.chars();
            if let Some(ch) = chars.next()
                && let Some(tail) = chars.as_str().strip_prefix(')')
            {
                short = Some(ch);
                rest = tail;
            }
        }
        let param = parse_type_suffix(rest);
        let switch = short.map_or_else(
            || OwnedSwitch::Long(name.to_owned()),
            |ch| OwnedSwitch::Both(ch, name.to_owned()),
        );
        block.entries.push(ManpageEntry {
            switch,
            param,
            desc: desc.to_owned(),
        });
    } else if param_part.starts_with('-') {
        if let Some(ch) = param_part.chars().nth(1)
            && ch.is_ascii_alphanumeric()
        {
            block.entries.push(ManpageEntry {
                switch: OwnedSwitch::Short(ch),
                param: None,
                desc: desc.to_owned(),
            });
        }
    } else {
        let (variadic, after_prefix) = param_part
            .strip_prefix("...")
            .map_or((false, param_part), |rest| (true, rest));
        let optional = after_prefix.contains('?');
        let raw_name = after_prefix
            .split([':', '?'])
            .next()
            .unwrap_or(after_prefix)
            .trim();
        let name: String = raw_name
            .chars()
            .map(|ch| if ch == '-' { '_' } else { ch })
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

fn split_at_non_name_char(text: &str) -> (&str, &str) {
    let end = text
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .unwrap_or(text.len());
    text.split_at_checked(end).unwrap_or((text, ""))
}

/// always Mandatory: nushell extern syntax has no optional-with-default to
/// distinguish.
fn parse_type_suffix(text: &str) -> Option<OwnedParam> {
    let typed = text.trim_start().strip_prefix(':')?.trim_start();
    let type_name = typed.split(|ch: char| !ch.is_ascii_alphabetic()).next()?;
    if type_name.is_empty() {
        None
    } else {
        Some(OwnedParam::Mandatory(type_name.to_owned()))
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
            list.entries.iter().any(
                |entry| matches!(&entry.switch, OwnedSwitch::Both('f', long) if long == "format")
            ),
            "list should have --format(-f): {:?}",
            list.entries
        );
        assert!(
            !list.positionals.is_empty(),
            "list should have a positional"
        );
    }
}
