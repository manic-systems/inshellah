#[test]
fn nix_wrapper_keeps_user_dir_overridable() {
    let module = include_str!("../nix/module.nix");

    assert!(
        module.contains("has_dir=0") && module.contains("[ \"$arg\" = \"--dir\" ]"),
        "wrapper must detect user-provided --dir"
    );
    assert!(
        module.contains("if [ \"$has_dir\" = 1 ]; then\n                exec ${cfg.package}/bin/inshellah \"$@\""),
        "wrapper must pass explicit user args through unchanged"
    );
    assert!(
        !module.contains(
            "complete|query|dump|purge)\n              exec ${cfg.package}/bin/inshellah \"$@\" --dir"
        ),
        "wrapper must not blindly append a non-overridable --dir"
    );
}

#[test]
fn nix_module_installs_nushell_vendor_autoload() {
    let module = include_str!("../nix/module.nix");

    assert!(
        module.contains("share/nushell/vendor/autoload/inshellah.nu"),
        "nushell loads profile autoload files from vendor/autoload"
    );
    assert!(
        !module.contains("environment.pathsToLink = [\n      \"/share/nushell/vendor/autoload\""),
        "module must not link every package-provided vendor autoload tree"
    );
    assert!(
        !module.contains("share/nushell/autoload/inshellah.nu"),
        "legacy non-vendor autoload path is not loaded by nushell"
    );
}

#[test]
fn nix_module_uses_configurable_nushell_package_for_indexing() {
    let module = include_str!("../nix/module.nix");

    assert!(
        module.contains("nushellPackage = lib.mkOption"),
        "module should expose the nushell package used for index-time discovery"
    );
    assert!(
        module.contains("default = pkgs.nushell"),
        "module should default index-time discovery to pkgs.nushell"
    );
    assert!(
        module.contains("PATH=\"${cfg.nushellPackage}/bin:$PATH\""),
        "index command should run with the configured nushell package on PATH"
    );
    assert!(
        !module.contains("2>/dev/null || true"),
        "index-time discovery failures should fail the build, not be hidden"
    );
}
