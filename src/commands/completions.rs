// SPDX-License-Identifier: EUPL-1.2
//! `inshellah completions`.

use std::fmt::Write as _;

use pound::{ArgSpec, Command, CommandSpec, Hint};

pub fn run(spec: &'static CommandSpec) {
    let mut out = String::from("module inshellah-completions {\n");
    write_extern(&mut out, &Command::root(spec));
    out.push_str("}\n\nuse inshellah-completions *\n");
    print!("{out}");
}

fn write_extern(out: &mut String, cmd: &Command<'_>) {
    let name = cmd
        .path()
        .iter()
        .chain([&cmd.spec().name])
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    writeln!(out, "export extern \"{name}\" [").expect("write to String");

    for arg in cmd.positionals() {
        let param = arg.metavar().to_lowercase().replace('-', "_");
        let shape = if arg.multi {
            format!("...{param}")
        } else if arg.required {
            param
        } else {
            format!("{param}?")
        };
        write_param(out, &format!("{shape}: {}", nu_type(arg)), arg.help);
    }

    for arg in cmd.options() {
        let mut param = match (arg.long, arg.short) {
            (Some(long), Some(short)) => format!("--{long}(-{short})"),
            (Some(long), None) => format!("--{long}"),
            (None, Some(short)) => format!("-{short}"),
            (None, None) => continue,
        };
        if arg.takes_value() {
            write!(param, ": {}", nu_type(&arg)).expect("write to String");
        }
        write_param(out, &param, arg.help);
    }

    out.push_str("]\n\n");
    for sub in cmd.subcommands().filter(|sub| !sub.hidden()) {
        write_extern(out, &sub);
    }
}

fn write_param(out: &mut String, param: &str, help: &str) {
    if help.is_empty() {
        writeln!(out, "    {param}").expect("write to String");
    } else {
        writeln!(out, "    {param:<32}# {help}").expect("write to String");
    }
}

// a delimited list such as `a:b` is not one path, so nushell must not expand it as one
fn nu_type(arg: &ArgSpec) -> &'static str {
    match arg.hint {
        Hint::Path if arg.delimiter.is_none() => "path",
        _ => "string",
    }
}
