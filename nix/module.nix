# NixOS module: automatic nushell completion indexing
#
# Indexes completions using three strategies in priority order:
#   1. Native completion generators (e.g. CMD completions nushell)
#   2. Manpage parsing
#   3. --help output parsing
#
# Produces a directory of .json/.nu files at build time.
# The `complete` command reads from this directory as a system overlay.
#
# Usage:
#   { pkgs, ... }: {
#     imports = [ ./path/to/inshellah-rs/nix/module.nix ];
#     programs.inshellah.enable = true;
#   }

{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.inshellah;
  completerSnippet = ./inshellah-completer.nu;
  dynamicStubCommands = [
    "systemctl"
    "journalctl"
    "coredumpctl"
    "loginctl"
    "machinectl"
    "networkctl"
    "hostnamectl"
    "timedatectl"
    "localectl"
    "ssh"
    "scp"
    "sftp"
    "docker"
    "podman"
    "kubectl"
    "git"
    "jj"
    "npm"
    "pnpm"
    "yarn"
    "make"
    "just"
    "cargo"
    "pkill"
  ];
  dynamicStubCommandArgs = lib.escapeShellArgs dynamicStubCommands;
in
{
  options.programs.inshellah = {
    enable = lib.mkEnableOption "nushell completion indexing via inshellah";

    package = lib.mkOption {
      type = lib.types.package;
      description = "package to use for indexing completions";
    };

    completionsPath = lib.mkOption {
      type = lib.types.str;
      default = "/share/inshellah";
      description = ''
        subdirectory within the system profile where completion files
        are placed. used as --dir for the completer.
      '';
    };

    extraDirs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "/etc/profiles/per-user/alice/share/inshellah" ];
      description = ''
        additional read-only completion directories to search.
        these are appended (colon-separated) to the --dir path
        alongside the system completions path.
      '';
    };

    ignoreCommands = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "problematic-tool" ];
      description = ''
        list of command names to skip during completion indexing
      '';
    };

    helpOnlyCommands = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "nix" ];
      description = ''
        list of command names to skip manpage parsing for,
        using --help scraping instead
      '';
    };

    timeoutMs = lib.mkOption {
      type = lib.types.nullOr lib.types.int;
      default = null;
      example = 200;
      description = ''
        per-subprocess timeout in milliseconds. when null the binary's
        compiled-in default is used (currently 200ms).
      '';
    };

    workers = lib.mkOption {
      type = lib.types.nullOr lib.types.int;
      default = null;
      example = 8;
      description = ''
        worker thread count for the parallel scrape pool. when null,
        `std::thread::available_parallelism` is used.
      '';
    };

    snippet = lib.mkOption {
      type = lib.types.str;
      readOnly = true;
      default = builtins.readFile completerSnippet;
      description = ''
        nushell external completer snippet installed by the module.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages =
      let
        systemDir = "/run/current-system/sw${cfg.completionsPath}";
        dirPaths = lib.concatStringsSep ":" ([ systemDir ] ++ cfg.extraDirs);
        wrapped = pkgs.writeShellScriptBin "inshellah" ''
          case "''${1:-}" in
            complete|query|dump)
              exec ${cfg.package}/bin/inshellah "$@" --dir "''${XDG_CACHE_HOME:-$HOME/.cache}/inshellah:${dirPaths}"
              ;;
            *)
              exec ${cfg.package}/bin/inshellah "$@"
              ;;
          esac
        '';
      in
      [
        (lib.hiPrio wrapped)
        cfg.package
      ];
    environment.pathsToLink = [
      "/share/nushell/autoload"
      "/share/nushell/vendor/autoload"
    ];
    environment.extraSetup =
      let
        inshellah = "${cfg.package}/bin/inshellah";
        destDir = "$out${cfg.completionsPath}";
        ignoreFile = pkgs.writeText "inshellah-ignore" (lib.concatStringsSep "\n" cfg.ignoreCommands);
        ignoreFlag = lib.optionalString (cfg.ignoreCommands != [ ]) " --ignore ${ignoreFile}";
        helpOnlyFile = pkgs.writeText "inshellah-help-only" (
          lib.concatStringsSep "\n" cfg.helpOnlyCommands
        );
        helpOnlyFlag = lib.optionalString (cfg.helpOnlyCommands != [ ]) " --help-only ${helpOnlyFile}";
        timeoutFlag = lib.optionalString (cfg.timeoutMs != null) " --timeout-ms ${toString cfg.timeoutMs}";
        workersFlag = lib.optionalString (cfg.workers != null) " --workers ${toString cfg.workers}";
        snippetFile = pkgs.writeText "inshellah-completer.nu" cfg.snippet;
      in
      ''
        mkdir -p ${destDir}

        if [ -d "$out/bin" ] && [ -d "$out/share/man" ]; then
          ${inshellah} index "$out" --dir ${destDir}${ignoreFlag}${helpOnlyFlag}${timeoutFlag}${workersFlag} \
            2>/dev/null || true
        fi

        find ${destDir} -maxdepth 1 -empty -delete

        # Install the full nushell completer plus sudo/doas wrapped commands.
        # Nushell otherwise hardcodes sudo/doas to bypass external completers.
        mkdir -p $out/share/nushell/vendor/autoload
        cp ${snippetFile} $out/share/nushell/vendor/autoload/inshellah.nu

        # Register command names for dynamic backends that are actually present
        # in the linked profile. The externs keep Nu's command list aware of
        # these commands while the external completer still supplies arguments.
        stubFile=$out/share/nushell/vendor/autoload/inshellah-command-stubs.nu
        : > "$stubFile"
        for cmd in ${dynamicStubCommandArgs}; do
          if [ -x "$out/bin/$cmd" ]; then
            printf '@complete external\nextern "%s" [...args]\n\n' "$cmd" >> "$stubFile"
          fi
        done
        if [ ! -s "$stubFile" ]; then
          rm -f "$stubFile"
        fi
      '';
  };
}
