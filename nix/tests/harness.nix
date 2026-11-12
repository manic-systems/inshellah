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
  options.warnings = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = [ ];
  };
  options.programs = lib.genAttrs [ "bash" "zsh" "fish" ] (_: {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = false;
    };
    interactiveShellInit = lib.mkOption {
      type = lib.types.lines;
      default = "";
    };
  });
}
