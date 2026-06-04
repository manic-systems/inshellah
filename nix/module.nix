# NixOS / nix-darwin module: automatic nushell completion indexing
#
# Indexes completions using three strategies in priority order:
#   1. Native completion generators (e.g. CMD completions nushell)
#   2. Manpage parsing
#   3. --help output parsing
#
# Produces a directory of .json/.nu files at build time.
# The `complete` command reads from this directory as a system overlay.
#
# This module body only uses options shared by NixOS and nix-darwin
# (environment.{variables,systemPackages,extraSetup}), so the
# same file backs both flake outputs. On macOS the indexer scrapes Mach-O
# binaries; on Linux, ELF — selected by the inshellah build's target os.
#
# Usage (NixOS):
#   { pkgs, ... }: {
#     imports = [ ./path/to/inshellah-rs/nix/module.nix ];
#     programs.inshellah.enable = true;
#   }
# Usage (nix-darwin): identical — import the same file (or the flake's
#   darwinModules.default) and set programs.inshellah.enable = true.

{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.inshellah;
  completerSnippet = ./inshellah-completer.nu;
  defaultPackage = pkgs.callPackage ./package.nix { };
in
{
  options.programs.inshellah = {
    enable = lib.mkEnableOption "nushell completion indexing via inshellah";

    package = lib.mkOption {
      type = lib.types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
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

    extraScrapePackages = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ ];
      example = lib.literalExpression "[ pkgs.git pkgs.clang ]";
      description = ''
        additional packages to scrape for completions alongside the system
        profile. each package's store path is passed to `inshellah index`
        via `--prefix`, so it must contain bin/ and/or share/man/.

        useful on macOS, where the active developer toolchain (git, clang,
        …) lives outside the nix system profile behind /usr/bin shims:
        install the nix equivalents and list them here so their completions
        get indexed reproducibly, rather than probing the host toolchain.
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

    dynamicTimeoutMs = lib.mkOption {
      type = lib.types.int;
      default = 5000;
      example = 2000;
      description = ''
        timeout in milliseconds for live dynamic completions in the nushell
        shim. this bounds runtime calls such as nix, jj, kubectl, and systemctl.
        set to 0 to disable the runtime timeout.
      '';
    };

    dynamicLimit = lib.mkOption {
      type = lib.types.int;
      default = 200;
      example = 100;
      description = ''
        maximum number of results requested from live dynamic completion
        providers when they expose a native result limit. set to 0 to omit
        native result-limit flags.
      '';
    };

    flagTriggers = lib.mkOption {
      type = lib.types.str;
      default = "-";
      example = "-+";
      description = ''
        characters that trigger flag (option) completions when a partial
        token begins with one of them. the default "-" reproduces the
        original behaviour where only a leading dash surfaces flags. each
        character is taken literally; whitespace is ignored. exported as
        INSHELLAH_FLAG_TRIGGERS.
      '';
    };

    flagOnEmpty = lib.mkOption {
      type = lib.types.bool;
      default = false;
      example = true;
      description = ''
        also surface flag completions when nothing has been typed yet —
        i.e. right after a space/tab — alongside subcommands. when false
        (the default) an empty token hands off to file/dynamic completion.
        exported as INSHELLAH_FLAG_ON_EMPTY.
      '';
    };

    maxCompletions = lib.mkOption {
      type = lib.types.int;
      default = 0;
      example = 100;
      description = ''
        upper bound on the number of static completion candidates returned,
        and the nushell `max_results` shown. 0 means no inshellah-imposed
        cap (nushell's built-in default of 200 still applies). exported as
        INSHELLAH_MAX_COMPLETIONS.
      '';
    };

    cacheTtlSecs = lib.mkOption {
      type = lib.types.int;
      default = 604800;
      example = 86400;
      description = ''
        rescrape user-cached completion sets older than N seconds; 0 disables
        time-based rescraping. exported as INSHELLAH_CACHE_TTL_SECS.
      '';
    };

    completeTimeoutMs = lib.mkOption {
      type = lib.types.nullOr lib.types.int;
      default = null;
      example = 400;
      description = ''
        per-subprocess timeout in milliseconds for the on-the-fly --help
        resolution the completer performs for uncached commands. distinct
        from `timeoutMs` (build-time indexing) and `dynamicTimeoutMs` (the
        nushell shim's live providers). null uses the binary's compiled
        default (currently 200ms). exported as INSHELLAH_TIMEOUT_MS.
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
    environment.variables.INSHELLAH_DYNAMIC_TIMEOUT_MS = toString cfg.dynamicTimeoutMs;
    environment.variables.INSHELLAH_DYNAMIC_LIMIT = toString cfg.dynamicLimit;
    environment.variables.INSHELLAH_FLAG_TRIGGERS = cfg.flagTriggers;
    environment.variables.INSHELLAH_FLAG_ON_EMPTY = if cfg.flagOnEmpty then "1" else "0";
    environment.variables.INSHELLAH_MAX_COMPLETIONS = toString cfg.maxCompletions;
    environment.variables.INSHELLAH_CACHE_TTL_SECS = toString cfg.cacheTtlSecs;
    environment.variables.INSHELLAH_TIMEOUT_MS = lib.mkIf (cfg.completeTimeoutMs != null) (
      toString cfg.completeTimeoutMs
    );

    environment.systemPackages =
      let
        systemDir = "/run/current-system/sw${cfg.completionsPath}";
        dirPaths = lib.concatStringsSep ":" ([ systemDir ] ++ cfg.extraDirs);
        wrapped = pkgs.writeShellScriptBin "inshellah" ''
          case "''${1:-}" in
            complete|query|dump|purge)
              has_dir=0
              for arg in "$@"; do
                if [ "$arg" = "--dir" ]; then
                  has_dir=1
                  break
                fi
              done
              if [ "$has_dir" = 1 ]; then
                exec ${cfg.package}/bin/inshellah "$@"
              else
                exec ${cfg.package}/bin/inshellah "$@" --dir "''${XDG_CACHE_HOME:-$HOME/.cache}/inshellah:${dirPaths}"
              fi
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
        # roll the explicit extra packages up into a single colon-separated
        # --prefix so they're scraped alongside the system profile.
        prefixFlag = lib.optionalString (cfg.extraScrapePackages != [ ]) (
          " --prefix " + lib.concatStringsSep ":" (map toString cfg.extraScrapePackages)
        );
        snippetFile = pkgs.writeText "inshellah-completer.nu" cfg.snippet;
      in
      ''
        mkdir -p ${destDir}

        if [ -d "$out/bin" ] && [ -d "$out/share/man" ]; then
          ${inshellah} index "$out" --dir ${destDir}${ignoreFlag}${helpOnlyFlag}${prefixFlag}${timeoutFlag}${workersFlag} \
            2>/dev/null || true
        fi

        find ${destDir} -maxdepth 1 -empty -delete

        # no per-command stubs: an extern/def makes nu complete its own (empty)
        # declared flags for a leading `-`; the external completer handles all.
        mkdir -p $out/share/nushell/vendor/autoload
        install -m 0644 ${snippetFile} $out/share/nushell/vendor/autoload/inshellah.nu
      '';
  };
}
