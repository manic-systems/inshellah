{
  pkgs,
  cfg,
  defaultPackage,
  wrapper ? null,
}:

let
  inherit (pkgs) lib;
  crossCompiling = !pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform;
  hostTools = {
    inherit (pkgs) bash coreutils man;
    inherit (pkgs.stdenv.hostPlatform) system;
    nushell = cfg.nushellPackage;
    inshellah = cfg.package;
  };
  # aliases warn once instantiated, long after tryEval has passed them
  # as a twin, so skip them by name
  aliases = import (pkgs.path + "/pkgs/top-level/aliases.nix") lib pkgs pkgs;
  # setPrio and friends rebuild a package through overrideAttrs, which drops
  # __spliced but leaves the drvPath alone. a real override changes the drvPath.
  sameAsStock =
    pkg: pname:
    let
      stock = builtins.tryEval (
        let
          candidate = pkgs.${pname};
        in
        if lib.isDerivation candidate && candidate.drvPath == pkg.drvPath then candidate else null
      );
    in
    !(aliases ? ${pname}) && pkgs ? ${pname} && stock.success && stock.value != null;
  stockBuildTwin =
    pkg:
    let
      pname = pkg.pname or null;
      twin = builtins.tryEval (
        let
          candidate = pkgs.buildPackages.${pname};
        in
        if lib.isDerivation candidate then candidate else null
      );
    in
    if pname != null && sameAsStock pkg pname && twin.success then twin.value else null;
  buildTwin = pkg: if !crossCompiling then null else pkg.__spliced.buildHost or (stockBuildTwin pkg);
  buildInshellah =
    if !crossCompiling then
      null
    else if cfg.package == defaultPackage then
      pkgs.buildPackages.callPackage ./package.nix { }
    else
      buildTwin cfg.package;
  buildNushell = buildTwin cfg.nushellPackage;
  noTwin =
    what: pkg:
    throw "programs.inshellah: ${what} ${pkg.name} has no build-platform twin to run while cross compiling, use the unmodified nixpkgs package";
  buildTools = {
    inherit (pkgs.buildPackages) bash coreutils man;
    inherit (pkgs.stdenv.buildPlatform) system;
    nushell = if buildNushell != null then buildNushell else noTwin "nushellPackage" cfg.nushellPackage;
    inshellah = if buildInshellah != null then buildInshellah else noTwin "package" cfg.package;
  };
  twinOf =
    pkg:
    if pkg.drvPath == cfg.package.drvPath then
      buildInshellah
    else if wrapper != null && buildInshellah != null && pkg.drvPath == wrapper.package.drvPath then
      wrapper.forBuild pkgs.buildPackages buildInshellah
    else
      buildTwin pkg;
in
{
  mergeTools = if crossCompiling then buildTools else hostTools;
  indexToolsFor =
    pkg:
    let
      twin = twinOf pkg;
    in
    if !crossCompiling then
      {
        tools = hostTools;
        scrape = pkg;
      }
    else if twin != null then
      {
        tools = buildTools;
        scrape = twin;
      }
    else
      lib.warn "programs.inshellah: ${pkg.name} has no build-platform twin to run while cross compiling, so it is not indexed" null;
}
