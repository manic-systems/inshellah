use inshellah::parsers::{
    help::help_parser,
    manpage::{
        OwnedParam,
        OwnedSwitch,
    },
};

#[test]
fn parser_recovers_past_no_bracket_long_form() {
    // git clone -h produces lines like `--[no-]progress` that switch_parser
    // can't parse. previously the help parser got stuck on these because
    // skip_non_option_line refused to skip option-looking lines. now it falls
    // through to skip, letting the parser continue to the next real entry.
    let text = "usage: git clone [<options>] [--] <repo> [<dir>]

    -v, --[no-]verbose    be more verbose
    -q, --[no-]quiet      be more quiet
    --[no-]progress       force progress reporting
    --[no-]reject-shallow don't clone shallow repository
    -n, --no-checkout     don't create a checkout
    --checkout            opposite of --no-checkout
    -s, --[no-]shared     setup as shared repository
";
    let (_, result) = help_parser(text).expect("parse");
    // before the fix: only 2 entries (-v, -q) before the parser got stuck.
    // after: -v, -q, -n/--no-checkout, --checkout, -s, plus any others.
    assert!(
        result.entries.len() >= 4,
        "expected ≥4 entries, got {}",
        result.entries.len()
    );
    assert!(
        result.entries.iter().any(|entry| {
            matches!(
                &entry.switch,
                OwnedSwitch::Both('v', long) if *long == "verbose"
            )
        }),
        "expected -v/--verbose from --[no-]verbose, got {:?}",
        result.entries.len()
    );
}

#[test]
fn parser_keeps_negatable_params() {
    let text = "usage: git clone [<options>] [--] <repo> [<dir>]

    -j, --[no-]jobs <n>   number of submodules cloned in parallel
    --[no-]recurse-submodules[=<pathspec>]
                          initialize submodules in the clone
    --[no-]reject-shallow don't clone shallow repository
";
    let (_, result) = help_parser(text).expect("parse");
    let jobs = result
        .entries
        .iter()
        .find(|entry| matches!(&entry.switch, OwnedSwitch::Both('j', long) if *long == "jobs"))
        .expect("jobs entry");
    assert!(matches!(
        &jobs.param,
        Some(OwnedParam::Mandatory(param)) if param == "n"
    ));

    let recurse = result
        .entries
        .iter()
        .find(|entry| matches!(&entry.switch, OwnedSwitch::Long(long) if *long == "recurse-submodules"))
        .expect("recurse-submodules entry");
    assert!(matches!(
        &recurse.param,
        Some(OwnedParam::Optional(param)) if param == "pathspec"
    ));

    let reject = result
        .entries
        .iter()
        .find(|entry| matches!(&entry.switch, OwnedSwitch::Long(long) if *long == "reject-shallow"))
        .expect("reject-shallow entry");
    assert!(
        reject.param.is_none(),
        "reject-shallow should not parse prose as a param"
    );
}
