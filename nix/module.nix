# NixOS / nix-darwin module: automatic shell completion indexing
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
# (environment.{variables,systemPackages,extraSetup} and
# programs.{bash,zsh,fish}.interactiveShellInit), so the
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
  completerSnippet = ../shells/inshellah.nu;
  nushellOnly = lib.all (shell: shell == "nushell") cfg.shells;
  defaultPackage = pkgs.callPackage ./package.nix { };
  wrapperFor =
    buildPkgs: package:
    let
      dirPaths = lib.concatStringsSep ":" (
        [ "/run/current-system/sw${cfg.completionsPath}" ] ++ cfg.extraDirs
      );
    in
    buildPkgs.writeShellScriptBin "inshellah" ''
      export INSHELLAH_DIR="''${INSHELLAH_DIR:-''${XDG_CACHE_HOME:-$HOME/.cache}/inshellah:${dirPaths}}"
      exec ${package}/bin/inshellah "$@"
    '';
  wrapped = wrapperFor pkgs cfg.package;
in
{
  options.programs.inshellah = {
    enable = lib.mkEnableOption "shell completion indexing via inshellah";

    shells = lib.mkOption {
      type = lib.types.nonEmptyListOf (
        lib.types.enum [
          "nushell"
          "bash"
          "zsh"
          "fish"
        ]
      );
      default = [ "nushell" ];
      example = [
        "nushell"
        "zsh"
      ];
      description = ''
        shells to hook inshellah into. nushell gets its external completer
        installed into its vendor autoload dir, bash, zsh and fish get their
        hook added to `programs.<shell>.interactiveShellInit`. an index for
        nushell alone skips binaries named like nushell builtins, any other
        shell runs those binaries so they are indexed too.
      '';
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
      description = "package to use for indexing completions";
    };

    nushellPackage = lib.mkOption {
      type = lib.types.package;
      default = pkgs.nushell;
      defaultText = lib.literalExpression "pkgs.nushell";
      description = ''
        nushell package to use at index time for discovering native nushell
        commands that should not be indexed as external completions. only
        used when `shells` is nushell alone.
      '';
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
        list of command names to skip manpage parsing for, along with
        their subcommands, using --help scraping instead
      '';
    };

    extraScrapePackages = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ ];
      example = lib.literalExpression "[ pkgs.git pkgs.clang ]";
      description = ''
        additional packages to scrape for completions alongside the system
        profile. each package is indexed on its own and merged in. only commands
        under its bin/ get completions, its share/man/ pages enrich them.

        useful on macOS, where the active developer toolchain (git, clang,
        …) lives outside the nix system profile behind /usr/bin shims:
        install the nix equivalents and list them here so their completions
        get indexed reproducibly, rather than probing the host toolchain.
      '';
    };

    timeoutMs = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.unsigned;
      default = null;
      example = 200;
      description = ''
        per-subprocess timeout in milliseconds. when null the binary's
        compiled-in default is used (currently 1200ms).
      '';
    };

    dynamicTimeoutMs = lib.mkOption {
      type = lib.types.ints.unsigned;
      default = 5000;
      example = 2000;
      description = ''
        timeout in milliseconds for live dynamic completions in every shell.
        this bounds runtime calls such as nix, kubectl, and systemctl.
        set to 0 to disable the runtime timeout.
      '';
    };

    dynamicLimit = lib.mkOption {
      type = lib.types.ints.unsigned;
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
      type = lib.types.ints.unsigned;
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
      type = lib.types.ints.unsigned;
      default = 604800;
      example = 86400;
      description = ''
        rescrape user-cached completion sets older than N seconds; 0 disables
        time-based rescraping. exported as INSHELLAH_CACHE_TTL_SECS.
      '';
    };

    completeTimeoutMs = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.unsigned;
      default = null;
      example = 400;
      description = ''
        per-subprocess timeout in milliseconds for the on-the-fly --help
        resolution the completer performs for uncached commands. distinct
        from `timeoutMs` (build-time indexing) and `dynamicTimeoutMs` (live
        providers). null uses the binary's compiled
        default (currently 1200ms). exported as INSHELLAH_TIMEOUT_MS.
      '';
    };

    workers = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      example = 8;
      description = ''
        positive worker thread count for the parallel scrape pool. when
        null, `std::thread::available_parallelism` is used.
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

    environment.systemPackages = [
      (lib.hiPrio wrapped)
      cfg.package
    ];
    environment.extraSetup =
      let
        destDir = "$out${cfg.completionsPath}";
        ignoreFile = pkgs.writeText "inshellah-ignore" (
          lib.concatMapStrings (name: "${name}\n") cfg.ignoreCommands
        );
        ignoreFlag = lib.optionalString (cfg.ignoreCommands != [ ]) " --ignore ${ignoreFile}";
        helpOnlyFile = pkgs.writeText "inshellah-help-only" (
          lib.concatMapStrings (name: "${name}\n") cfg.helpOnlyCommands
        );
        helpOnlyFlag = lib.optionalString (cfg.helpOnlyCommands != [ ]) " --help-only ${helpOnlyFile}";
        timeoutFlag = lib.optionalString (cfg.timeoutMs != null) " --timeout-ms ${toString cfg.timeoutMs}";
        workersFlag = lib.optionalString (cfg.workers != null) " --workers ${toString cfg.workers}";
        # the indexer only tells nushell alone from any other mix, so a canonical
        # flag keeps adding or reordering shells from rebuilding every index.
        shellFlag = " --shell ${if nushellOnly then "nushell" else "bash"}";
        indexFlags = "${helpOnlyFlag}${timeoutFlag}${workersFlag}${shellFlag}";
        flags = "${ignoreFlag}${indexFlags}";
        installedOutputs =
          pkg:
          let
            main = pkg.outputName or "out";
            wanted = lib.unique ((pkg.meta.outputsToInstall or [ main ]) ++ [ "man" ]);
          in
          map (o: if o == main then pkg else pkg.${o}) (
            lib.filter (o: lib.elem o (pkg.outputs or [ main ])) wanted
          );
        inherit
          (import ./tools.nix {
            inherit pkgs cfg defaultPackage;
            wrapper = {
              package = wrapped;
              forBuild = wrapperFor;
            };
          })
          mergeTools
          indexToolsFor
          ;
        inshellah = "${mergeTools.inshellah}/bin/inshellah";
        # some tools put their own bin/ on PATH before exec'ing `man` for
        # `--help`, so `man` sits beside them as in the profile.
        mkIndex =
          name: tools: prefixes:
          derivation {
            name = "inshellah-index-${name}";
            inherit (tools) system;
            builder = "${tools.bash}/bin/bash";
            PATH = lib.makeBinPath [ tools.coreutils ];
            args = [
              "-ec"
              ''
                shopt -s nullglob
                mkdir -p $out profile/bin profile/share/man
                for prefix in ${lib.escapeShellArgs prefixes}; do
                  if [ -d "$prefix/bin" ]; then
                    for bin in "$prefix"/bin/*; do
                      [ -e "profile/bin/$(basename "$bin")" ] || ln -s "$bin" profile/bin/
                    done
                  fi
                  if [ -d "$prefix/share/man" ]; then
                    cp -rsn "$prefix/share/man/." profile/share/man/
                    chmod -R u+w profile/share/man
                  fi
                done
                [ -n "$(ls -A profile/bin)" ] || exit 0

                touch ignore
                ${lib.optionalString (cfg.ignoreCommands != [ ]) "cat ${ignoreFile} > ignore"}
                for helper in ${tools.man}/bin/*; do
                  name=$(basename "$helper")
                  if [ ! -e "profile/bin/$name" ]; then
                    ln -s "$helper" profile/bin/
                    echo "$name" >> ignore
                  fi
                done

                ${lib.optionalString nushellOnly "PATH=\"${tools.nushell}/bin:$PATH\" "}${tools.inshellah}/bin/inshellah index "$PWD/profile" --dir $out --ignore ignore${indexFlags}
              ''
            ];
          };
        # a cross host scrapes each package's build-platform twin natively.
        indexFor =
          pkg:
          let
            plan = indexToolsFor pkg;
            index = mkIndex pkg.name plan.tools (installedOutputs plan.scrape);
          in
          lib.optionalString (plan != null) (
            lib.concatMapStrings (prefix: "${prefix} ${index}\n") (installedOutputs pkg)
          );
        sourcesFile = pkgs.writeText "inshellah-sources" (
          lib.concatMapStrings indexFor (
            lib.filter lib.isDerivation (config.environment.systemPackages ++ cfg.extraScrapePackages)
          )
        );
        snippetFile = pkgs.writeText "inshellah-completer.nu" cfg.snippet;
      in
      ''
        mkdir -p ${destDir}

        ${lib.optionalString nushellOnly "PATH=\"${mergeTools.nushell}/bin:$PATH\" "}${inshellah} merge "$out" --dir ${destDir} --sources ${sourcesFile}${flags}

        find ${destDir} -maxdepth 1 -empty -delete

      ''
      + lib.optionalString (lib.elem "nushell" cfg.shells) ''

        # no per-command stubs: an extern/def makes nu complete its own (empty)
        # declared flags for a leading `-`; the external completer handles all.
        mkdir -p $out/share/nushell/vendor/autoload
        install -m 0644 ${snippetFile} $out/share/nushell/vendor/autoload/inshellah.nu
      '';

    warnings = map (
      shell:
      "programs.inshellah.shells lists ${shell}, but programs.${shell}.enable is off, so its hook is never loaded"
    ) (lib.filter (shell: shell != "nushell" && !(config.programs.${shell}.enable or true)) cfg.shells);

    programs.bash.interactiveShellInit = lib.mkIf (lib.elem "bash" cfg.shells) (
      lib.mkAfter (builtins.readFile ../shells/inshellah.bash)
    );
    programs.zsh.interactiveShellInit = lib.mkIf (lib.elem "zsh" cfg.shells) (
      builtins.readFile ../shells/inshellah.zsh
    );
    programs.fish.interactiveShellInit = lib.mkIf (lib.elem "fish" cfg.shells) (
      builtins.readFile ../shells/inshellah.fish
    );
  };
}
