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
}
