{ pkgs }:

let
  inherit (pkgs) lib;
  moduleHarness =
    { lib, ... }:
    {
      options.environment = {
        variables = lib.mkOption {
          type = lib.types.attrsOf lib.types.str;
          default = { };
        };
        systemPackages = lib.mkOption {
          type = lib.types.listOf lib.types.package;
          default = [ ];
        };
        extraSetup = lib.mkOption {
          type = lib.types.lines;
          default = "";
        };
      };
    };
  evalModule =
    values:
    lib.evalModules {
      specialArgs = { inherit pkgs; };
      modules = [
        moduleHarness
        ../module.nix
        {
          programs.inshellah = {
            enable = true;
          }
          // values;
        }
      ];
    };
  accepts =
    case:
    let
      name = builtins.head (builtins.attrNames case);
      evaluated = (evalModule case).config.programs.inshellah.${name};
      config = (evalModule case).config;
      forced = builtins.seq (map (pkg: pkg.drvPath) config.environment.systemPackages) (
        builtins.stringLength config.environment.extraSetup
      );
    in
    (builtins.tryEval (builtins.seq forced evaluated)).success;
  validCases = [
    { workers = 1; }
    { timeoutMs = 0; }
    { dynamicTimeoutMs = 0; }
    { dynamicLimit = 0; }
    { maxCompletions = 0; }
    { cacheTtlSecs = 0; }
    { completeTimeoutMs = 0; }
  ];
  invalidCases = [
    { workers = 0; }
    { workers = -1; }
    { timeoutMs = -1; }
    { dynamicTimeoutMs = -1; }
    { dynamicLimit = -1; }
    { maxCompletions = -1; }
    { cacheTtlSecs = -1; }
    { completeTimeoutMs = -1; }
  ];
in
assert lib.all accepts validCases;
assert lib.all (case: !accepts case) invalidCases;
pkgs.runCommand "inshellah-module-options-check" { } ''
  touch "$out"
''
