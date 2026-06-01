use inshellah::parsers::help::help_parser;

#[test]
fn parser_recovers_past_no_bracket_long_form() {
    // git clone -h produces lines like `--[no-]progress` that switch_parser
    // can't parse. previously the help parser got stuck on these because
    // skip_non_option_line refused to skip option-looking lines. now it falls
    // through to skip, letting the parser continue to the next real entry.
    let text = r#"usage: git clone [<options>] [--] <repo> [<dir>]

    -v, --[no-]verbose    be more verbose
    -q, --[no-]quiet      be more quiet
    --[no-]progress       force progress reporting
    --[no-]reject-shallow don't clone shallow repository
    -n, --no-checkout     don't create a checkout
    --checkout            opposite of --no-checkout
    -s, --[no-]shared     setup as shared repository
"#;
    let (_, r) = help_parser(text).expect("parse");
    // before the fix: only 2 entries (-v, -q) before the parser got stuck.
    // after: -v, -q, -n/--no-checkout, --checkout, -s, plus any others.
    assert!(
        r.entries.len() >= 4,
        "expected ≥4 entries, got {}",
        r.entries.len()
    );
    assert!(
        r.entries.iter().any(|e| {
            matches!(
                &e.switch,
                inshellah::parsers::manpage::OwnedSwitch::Both('v', l) if *l == "verbose"
            )
        }),
        "expected -v/--verbose from --[no-]verbose, got {:?}",
        r.entries.len()
    );
}

#[test]
fn parser_keeps_negatable_params() {
    let text = r#"usage: git clone [<options>] [--] <repo> [<dir>]

    -j, --[no-]jobs <n>   number of submodules cloned in parallel
    --[no-]recurse-submodules[=<pathspec>]
                          initialize submodules in the clone
    --[no-]reject-shallow don't clone shallow repository
"#;
    let (_, r) = help_parser(text).expect("parse");
    let jobs = r
        .entries
        .iter()
        .find(|e| matches!(&e.switch, inshellah::parsers::manpage::OwnedSwitch::Both('j', l) if *l == "jobs"))
        .expect("jobs entry");
    assert!(matches!(
        &jobs.param,
        Some(inshellah::parsers::manpage::OwnedParam::Mandatory(p)) if p == "n"
    ));

    let recurse = r
        .entries
        .iter()
        .find(|e| matches!(&e.switch, inshellah::parsers::manpage::OwnedSwitch::Long(l) if *l == "recurse-submodules"))
        .expect("recurse-submodules entry");
    assert!(matches!(
        &recurse.param,
        Some(inshellah::parsers::manpage::OwnedParam::Optional(p)) if p == "pathspec"
    ));

    let reject = r
        .entries
        .iter()
        .find(|e| matches!(&e.switch, inshellah::parsers::manpage::OwnedSwitch::Long(l) if *l == "reject-shallow"))
        .expect("reject-shallow entry");
    assert!(
        reject.param.is_none(),
        "reject-shallow should not parse prose as a param"
    );
}
