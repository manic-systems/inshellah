{ pkgs }:

let
  inherit (pkgs) lib;
  crossPkgs =
    if pkgs.stdenv.hostPlatform.isAarch64 then
      pkgs.pkgsCross.gnu64
    else
      pkgs.pkgsCross.aarch64-multiplatform;
  defaultPackage = crossPkgs.callPackage ../package.nix { };
  extraSetupFor =
    programs:
    (lib.evalModules {
      specialArgs.pkgs = crossPkgs;
      modules = [
        ./harness.nix
        ../module.nix
        {
          programs.inshellah = {
            enable = true;
          }
          // programs;
        }
      ];
    }).config.environment.extraSetup;
  indexToolsFor =
    cfg:
    (import ../tools.nix {
      pkgs = crossPkgs;
      cfg = {
        package = defaultPackage;
        nushellPackage = crossPkgs.nushell;
      }
      // cfg;
      inherit defaultPackage;
    }).indexToolsFor;
  has =
    needle: hay:
    lib.hasInfix (builtins.unsafeDiscardStringContext needle) (builtins.unsafeDiscardStringContext hay);

  buildNu = crossPkgs.buildPackages.nushell;
  buildInshellah = crossPkgs.buildPackages.callPackage ../package.nix { };
  buildHello = crossPkgs.buildPackages.hello;
  override = old: {
    env = (old.env or { }) // {
      INSHELLAH_TEST_OVERRIDE = "1";
    };
  };
  overriddenNu = crossPkgs.nushell.overrideAttrs override;
  overriddenHello = crossPkgs.hello.overrideAttrs override;

  mergeOnBuild =
    extra: nu: inshellah:
    let
      setup = extraSetupFor extra;
    in
    has "${nu}/bin:$PATH" setup && has "${inshellah}/bin/inshellah merge" setup;
  indexOnBuild =
    cfg: nu: inshellah:
    let
      index = indexToolsFor cfg crossPkgs.hello;
    in
    index.tools.nushell == nu && index.tools.inshellah == inshellah;
  indexUsesBuildPlatform =
    pkg:
    let
      tools = (indexToolsFor { } pkg).tools;
    in
    tools.bash == crossPkgs.buildPackages.bash
    && tools.coreutils == crossPkgs.buildPackages.coreutils
    && tools.man == crossPkgs.buildPackages.man
    && tools.system == crossPkgs.stdenv.buildPlatform.system;
  throws = value: !(builtins.tryEval value).success;
  checks = {
    "stock packages merge with the build twins" = mergeOnBuild { } buildNu buildInshellah;
    "stock packages index with the build twins and build platform tools" =
      indexOnBuild { } buildNu buildInshellah
      && (indexToolsFor { } crossPkgs.hello).scrape == buildHello
      && indexUsesBuildPlatform crossPkgs.hello;
    "the default package indexes with its build twin" =
      (indexToolsFor { } defaultPackage).scrape == buildInshellah;
    "a priority wrapped stock package resolves to its build twin" =
      (indexToolsFor { } (lib.hiPrio crossPkgs.hello)).scrape == buildHello;
    "an overridden scraped package is skipped" = indexToolsFor { } overriddenHello == null;
    "an overridden nushellPackage fails evaluation" = throws (extraSetupFor {
      nushellPackage = overriddenNu;
    });
    "an overridden package fails evaluation" = throws (extraSetupFor {
      package = overriddenHello;
    });
  };
  failed = lib.attrNames (lib.filterAttrs (_: ok: !ok) checks);
in
if failed != [ ] then
  throw "cross module checks failed: ${lib.concatStringsSep ", " failed}"
else
  pkgs.runCommand "inshellah-module-cross-check" { } "touch $out"
