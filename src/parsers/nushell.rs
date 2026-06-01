// SPDX-License-Identifier: EUPL-1.2
//! generate nushell `extern` definitions from a [`ManpageResult`].

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::OnceLock;

use crate::parsers::manpage::{ManpageEntry, ManpageResult, OwnedParam, OwnedSwitch};
use crate::types::Positional;

/// emitting `extern` for these shadows nushell builtins. update on new releases.
pub const NUSHELL_BUILTINS: &[&str] = &[
    "alias",
    "all",
    "ansi",
    "any",
    "append",
    "ast",
    "attr",
    "bits",
    "break",
    "bytes",
    "cal",
    "cd",
    "char",
    "chunk-by",
    "chunks",
    "clear",
    "collect",
    "columns",
    "commandline",
    "compact",
    "complete",
    "config",
    "const",
    "continue",
    "cp",
    "date",
    "debug",
    "decode",
    "def",
    "default",
    "describe",
    "detect",
    "do",
    "drop",
    "du",
    "each",
    "echo",
    "encode",
    "enumerate",
    "error",
    "every",
    "exec",
    "exit",
    "explain",
    "explore",
    "export",
    "export-env",
    "extern",
    "fill",
    "filter",
    "find",
    "first",
    "flatten",
    "for",
    "format",
    "from",
    "generate",
    "get",
    "glob",
    "grid",
    "group-by",
    "hash",
    "headers",
    "help",
    "hide",
    "hide-env",
    "histogram",
    "history",
    "http",
    "if",
    "ignore",
    "input",
    "insert",
    "inspect",
    "interleave",
    "into",
    "is-admin",
    "is-empty",
    "is-not-empty",
    "is-terminal",
    "items",
    "job",
    "join",
    "keybindings",
    "kill",
    "last",
    "length",
    "let",
    "let-env",
    "lines",
    "load-env",
    "loop",
    "ls",
    "match",
    "math",
    "merge",
    "metadata",
    "mkdir",
    "mktemp",
    "module",
    "move",
    "mut",
    "mv",
    "nu-check",
    "nu-highlight",
    "open",
    "overlay",
    "panic",
    "par-each",
    "parse",
    "path",
    "plugin",
    "port",
    "prepend",
    "print",
    "ps",
    "query",
    "random",
    "reduce",
    "reject",
    "rename",
    "return",
    "reverse",
    "rm",
    "roll",
    "rotate",
    "run-external",
    "save",
    "schema",
    "scope",
    "select",
    "seq",
    "shuffle",
    "skip",
    "sleep",
    "slice",
    "sort",
    "sort-by",
    "source",
    "source-env",
    "split",
    "start",
    "stor",
    "str",
    "sys",
    "table",
    "take",
    "tee",
    "term",
    "timeit",
    "to",
    "touch",
    "transpose",
    "try",
    "tutor",
    "ulimit",
    "umask",
    "uname",
    "uniq",
    "uniq-by",
    "unlet",
    "update",
    "upsert",
    "url",
    "use",
    "values",
    "version",
    "view",
    "watch",
    "where",
    "which",
    "while",
    "whoami",
    "window",
    "with-env",
    "wrap",
    "zip",
];

fn builtin_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| NUSHELL_BUILTINS.iter().copied().collect())
}

pub fn is_nushell_builtin(cmd: &str) -> bool {
    builtin_set().contains(cmd)
}

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
            subcommands: vec![ManpageSubcommand {
                name: "stash\"]\nrm -rf /".into(),
                desc: "danger\nous".into(),
            }],
            ..Default::default()
        };
        let out = generate_extern("git", &result);
        assert!(
            !out.contains("\"]\n"),
            "string literal not closed early: {out:?}"
        );
        assert!(out.contains("rm -rf /"));
    }
}
