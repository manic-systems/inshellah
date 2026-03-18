pub enum Switch<'a> {
    Short(char),
    Long(&'a str),
    Both(char, &'a str),
}

pub enum Param<'a> {
    Mandatory(&'a str),
    Optional(&'a str),
}

pub struct OptionEntry<'a> {
    pub switch: Switch<'a>,
    pub param: Option<Param<'a>>,
    pub desc: Vec<&'a str>,
}

pub struct Subcommand<'a> {
    pub name: &'a str,
    pub desc: &'a str,
}

#[derive(Debug, Clone)]
pub struct Positional {
    pub optional: bool,
    pub variadic: bool,
}

pub struct HelpResult<'a> {
    pub entries: Vec<OptionEntry<'a>>,
    pub subcommands: Vec<Subcommand<'a>>,
    pub positionals: Vec<(&'a str, Positional)>,
    pub desc: &'a str,
}
