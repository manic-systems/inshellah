// SPDX-License-Identifier: EUPL-1.2
//! generate nushell `extern` definitions from parsed help data.
//!
//! this module is the code generation backend. it takes a [`ManpageResult`]
//! (from the help or manpage parsers) and produces nushell source that defines
//! `extern` declarations — nushell's mechanism for teaching the shell about
//! external commands' flags and subcommands so it can offer completions.
//!
//! key responsibilities:
//!   - deduplicating flag entries (same flag from multiple help sources)
//!   - mapping parameter names to nushell types (path, int, string)
//!   - formatting flags in nushell syntax: --flag(-f): type  # description
//!   - handling positional arguments with nushell's ordering constraints
//!   - escaping special characters for nushell string literals

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::OnceLock;

use crate::parsers::manpage::{ManpageEntry, ManpageResult, OwnedParam, OwnedSwitch};
use crate::types::Positional;

/// nushell built-in commands and keywords — we must never generate `extern`
/// definitions for these because it would shadow nushell's own implementations.
/// maintained manually and should be updated with new nushell releases.
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

/// returns true if the given command name collides with a nushell built-in.
pub fn is_nushell_builtin(cmd: &str) -> bool {
    builtin_set().contains(cmd)
}

/// map parameter names to nushell types.
/// nushell's `extern` declarations use typed parameters, so we infer the type
/// from the parameter name. file/path-related names become "path" (enables
/// path completion), numeric names become "int", everything else is "string".
pub fn nushell_type_of_param(name: &str) -> &'static str {
    match name {
        "FILE" | "file" | "PATH" | "path" | "DIR" | "dir" | "DIRECTORY" | "FILENAME"
        | "PATTERNFILE" => "path",
        "NUM" | "N" | "COUNT" | "NUMBER" | "int" | "INT" | "COLS" | "WIDTH" | "LINES" | "DEPTH"
        | "depth" => "int",
        _ => "string",
    }
}

/// escape a string for use inside nushell double-quoted string literals.
/// only double quotes and backslashes need escaping in nushell's syntax.
pub fn escape_nu(s: &str) -> Cow<'_, str> {
    if !s.contains('"') && !s.contains('\\') {
        Cow::Borrowed(s)
    } else {
        let mut buf = String::with_capacity(s.len() + 4);
        for c in s.chars() {
            match c {
                '"' => buf.push_str("\\\""),
                '\\' => buf.push_str("\\\\"),
                c => buf.push(c),
            }
        }
        Cow::Owned(buf)
    }
}

/// format a single flag entry as a nushell `extern` parameter line.
/// output examples:
///   "    --verbose(-v)                       # increase verbosity"
///   "    --output(-o): path                  # write output to file"
///   "    -n: int                             # number of results"
///
/// the description is right-padded to column 40 with a "# " comment prefix.
pub fn format_flag(entry: &ManpageEntry) -> String {
    let name = match &entry.switch {
        OwnedSwitch::Both(c, l) => format!("--{l}(-{c})"),
        OwnedSwitch::Long(l) => format!("--{l}"),
        OwnedSwitch::Short(c) => format!("-{c}"),
    };
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
        format!("{flag}{}# {}", " ".repeat(pad_len), entry.desc)
    }
}

/// format a positional argument as a nushell `extern` parameter line.
/// nushell syntax: "...name: type" for variadic, "name?: type" for optional.
/// hyphens in names are converted to underscores since nushell identifiers
/// cannot contain hyphens.
pub fn format_positional(name: &str, p: &Positional) -> String {
    let name_underscored: String = name
        .chars()
        .map(|c| if c == '-' { '_' } else { c })
        .collect();
    let prefix = if p.variadic { "..." } else { "" };
    let suffix = if p.optional && !p.variadic { "?" } else { "" };
    let typ = match nushell_type_of_param(&name.to_ascii_uppercase()) {
        "string" => "glob",
        other => other,
    };
    format!("    {prefix}{name_underscored}{suffix}: {typ}")
}

/// enforce nushell's positional argument ordering rules:
///   1. no required positional may follow an optional one
///   2. at most one variadic ("rest") parameter is allowed
///
/// if a required positional appears after an optional one, it is silently
/// promoted to optional. duplicate variadic params are dropped.
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

/// derive a nushell `module` name from a command name.
/// replaces non-alphanumeric characters with hyphens and appends "-completions".
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

/// generate the full nushell `extern` block for a command.
///
/// produces output like:
///   export extern "git add" [
///     ...pathspec?: path
///     --verbose(-v)              # be verbose
///     --dry-run(-n)              # dry run
///   ]
///
/// subcommands that weren't resolved into their own full definitions get
/// stub `extern` blocks with just a comment containing their description:
///   export extern "git stash" [  # stash changes
///   ]
pub fn generate_extern(cmd_name: &str, result: &ManpageResult) -> String {
    // entries arrive deduped from the parser pipeline (`parse_manpage_lines`
    // and `From<&HelpResult>` both run `manpage::dedup_entries`), so we can
    // emit them directly here.
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

/// generate a complete nushell `module` wrapping the `extern`.
/// output: "module git-completions { ... }\n\nuse git-completions *\n"
/// the `use` at the end makes the `extern` immediately available in scope.
pub fn generate_module(cmd_name: &str, result: &ManpageResult) -> String {
    let mod_name = module_name_of(cmd_name);
    format!(
        "module {mod_name} {{\n{}}}\n\nuse {mod_name} *\n",
        generate_extern(cmd_name, result)
    )
}
