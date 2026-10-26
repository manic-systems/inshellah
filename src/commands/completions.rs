// SPDX-License-Identifier: EUPL-1.2
//! `inshellah completions`.

use std::io::{
    self,
    BufWriter,
    Write,
};

use pound::{
    ArgSpec,
    Command,
    CommandSpec,
    Hint,
};

pub fn run(spec: &'static CommandSpec) -> io::Result<()> {
    let mut out = BufWriter::new(io::stdout().lock());
    out.write_all(b"module inshellah-completions {\n")?;
    write_extern(&mut out, &Command::root(spec))?;
    out.write_all(b"}\n\nuse inshellah-completions *\n")?;
    out.flush()
}

fn write_extern<Out>(out: &mut Out, cmd: &Command<'_>) -> io::Result<()>
where
    Out: Write,
{
    let name = cmd
        .path()
        .iter()
        .chain([&cmd.spec().name])
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    writeln!(out, "export extern \"{name}\" [")?;

    for arg in cmd.positionals() {
        let param = arg.metavar().to_lowercase().replace('-', "_");
        let shape = if arg.multi {
            format!("...{param}")
        } else if arg.required {
            param
        } else {
            format!("{param}?")
        };
        write_param(out, &format!("{shape}: {}", nu_type(arg)), arg.help)?;
    }

    for arg in cmd.options() {
        let mut param = match (arg.long, arg.short) {
            (Some(long), Some(short)) => format!("--{long}(-{short})"),
            (Some(long), None) => format!("--{long}"),
            (None, Some(short)) => format!("-{short}"),
            (None, None) => continue,
        };
        if arg.takes_value() {
            param.push_str(": ");
            param.push_str(nu_type(&arg));
        }
        write_param(out, &param, arg.help)?;
    }

    out.write_all(b"]\n\n")?;
    for sub in cmd.subcommands().filter(|sub| !sub.hidden()) {
        write_extern(out, &sub)?;
    }
    Ok(())
}

fn write_param<Out>(out: &mut Out, param: &str, help: &str) -> io::Result<()>
where
    Out: Write,
{
    if help.is_empty() {
        writeln!(out, "    {param}")
    } else {
        writeln!(out, "    {param:<32}# {help}")
    }
}

// a delimited list such as `a:b` is not one path, so nushell must not expand it
// as one
const fn nu_type(arg: &ArgSpec) -> &'static str {
    match arg.hint {
        Hint::Path if arg.delimiter.is_none() => "path",
        Hint::Path | Hint::Unknown | _ => "string",
    }
}
