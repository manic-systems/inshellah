{ pkgs }:

let
  inherit (pkgs) lib;
  fakeInshellah = pkgs.writeShellScriptBin "inshellah" ''
    command="$1"
    shift
    case "$command" in
      index)
        profile="$1"
        shift
        indexDir=
        ignoreList=
        while [ "$#" -gt 0 ]; do
          case "$1" in
            --dir)
              indexDir="$2"
              shift 2
              ;;
            --ignore)
              ignoreList="$2"
              shift 2
              ;;
            --help-only|--timeout-ms|--workers)
              shift 2
              ;;
            *)
              exit 64
              ;;
          esac
        done
        test -n "$indexDir"
        mkdir -p "$indexDir"
        if [ -n "$ignoreList" ]; then
          while read -r ignored; do
            printf '%s\n' "$ignored" > "$indexDir/command-ignored-$ignored"
          done < "$ignoreList"
        fi
        for binary in "$profile"/bin/*; do
          test -e "$binary" || test -L "$binary" || continue
          name=$(basename "$binary")
          printf '%s\n' "$profile" > "$indexDir/command-$name"
        done
        ;;
      merge)
        profile="$1"
        shift
        indexDir=
        sources=
        while [ "$#" -gt 0 ]; do
          case "$1" in
            --dir)
              indexDir="$2"
              shift 2
              ;;
            --sources)
              sources="$2"
              shift 2
              ;;
            --ignore|--help-only|--timeout-ms|--workers)
              shift 2
              ;;
            *)
              exit 65
              ;;
          esac
        done
        test -n "$profile"
        test -n "$indexDir"
        test -n "$sources"
        mkdir -p "$indexDir"
        while read -r prefix packageIndex; do
          test -n "$prefix"
          test -d "$packageIndex"
          for marker in "$packageIndex"/command-*; do
            test -e "$marker" || continue
            destination="$indexDir/$(basename "$marker")"
            rm -f "$destination"
            cp "$marker" "$destination"
          done
        done < "$sources"
        printf 'merged\n' > "$indexDir/merge-ran"
        ;;
      complete)
        test -n "''${INSHELLAH_RECORD_DIR:-}"
        mkdir -p "$INSHELLAH_RECORD_DIR"
        printf '%s\n' "''${INSHELLAH_DIR:-}" > "$INSHELLAH_RECORD_DIR/dirs"
        printf '%s\n' "$@" > "$INSHELLAH_RECORD_DIR/args"
        ;;
      *)
        exit 66
        ;;
    esac
  '';
  fakeNushell = pkgs.writeShellScriptBin "nu" ''
    exit 0
  '';
  profilePackage = pkgs.writeShellScriptBin "profile-tool" ''
    exit 0
  '';
  extraPackage = pkgs.writeShellScriptBin "extra-tool" ''
    exit 0
  '';
  manOnlyPackage = pkgs.runCommand "man-only" { } ''
    mkdir -p $out/share/man/man1
    printf '.TH MAN-ONLY 1\n.SH NAME\nman-only \\- documented but not installed\n' > $out/share/man/man1/man-only.1
  '';
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
  evalIndexModule =
    {
      systemPackages ? [ ],
      extraScrapePackages ? [ ],
      ignoreCommands ? [ ],
    }:
    lib.evalModules {
      specialArgs = { inherit pkgs; };
      modules = [
        moduleHarness
        ../module.nix
        {
          environment = { inherit systemPackages; };
          programs.inshellah = {
            enable = true;
            package = fakeInshellah;
            nushellPackage = fakeNushell;
            inherit extraScrapePackages ignoreCommands;
          };
        }
      ];
    };
  profileSetup =
    (evalIndexModule { systemPackages = [ profilePackage ]; }).config.environment.extraSetup;
  mixedSetup =
    (evalIndexModule {
      systemPackages = [ profilePackage ];
      extraScrapePackages = [ extraPackage ];
    }).config.environment.extraSetup;
  ignoreSetup =
    (evalIndexModule {
      systemPackages = [ profilePackage ];
      ignoreCommands = [
        "hidden-tool"
        "other-tool"
      ];
    }).config.environment.extraSetup;
  manOnlySetup =
    (evalIndexModule { systemPackages = [ manOnlyPackage ]; }).config.environment.extraSetup;
  emptyEvaluation = evalIndexModule { };
  wrapperPackage = lib.findFirst (
    pkg: pkg.outPath != fakeInshellah.outPath && lib.getName pkg == "inshellah"
  ) (throw "generated inshellah wrapper not found") emptyEvaluation.config.environment.systemPackages;
in
pkgs.runCommand "inshellah-module-indexing-check" { } ''
  checkOutput="$out"

  (
    export out="$TMPDIR/bin-only-profile"
    mkdir -p "$out/bin"
    ln -s ${profilePackage}/bin/profile-tool "$out/bin/profile-tool"
    ${profileSetup}
    test -f "$out/share/inshellah/merge-ran"
    test -f "$out/share/inshellah/command-profile-tool"
    test -f "$out/share/nushell/vendor/autoload/inshellah.nu"
    test ! -e "$out/share/inshellah/command-extra-tool"
  )

  (
    export out="$TMPDIR/mixed-profile"
    mkdir -p "$out/bin"
    ln -s ${profilePackage}/bin/profile-tool "$out/bin/profile-tool"
    ${mixedSetup}
    test -f "$out/share/inshellah/merge-ran"
    test -f "$out/share/inshellah/command-profile-tool"
    test -f "$out/share/inshellah/command-extra-tool"
  )

  (
    export out="$TMPDIR/ignore-profile"
    mkdir -p "$out/bin"
    ln -s ${profilePackage}/bin/profile-tool "$out/bin/profile-tool"
    ${ignoreSetup}
    test -f "$out/share/inshellah/command-ignored-hidden-tool"
    test -f "$out/share/inshellah/command-ignored-other-tool"
  )

  (
    export out="$TMPDIR/man-only-profile"
    mkdir -p "$out/bin"
    ${manOnlySetup}
    # quoted on purpose, an unmatched bin/* glob used to index a command named *
    test ! -e "$out/share/inshellah/command-*"
  )

  export INSHELLAH_RECORD_DIR="$TMPDIR/wrapper-record"
  export XDG_CACHE_HOME="$TMPDIR/cache"
  ${wrapperPackage}/bin/inshellah complete --target-option target
  printf '%s\n' --target-option target > "$TMPDIR/expected-wrapper-args"
  cmp "$TMPDIR/expected-wrapper-args" "$INSHELLAH_RECORD_DIR/args"
  test "$(cat "$INSHELLAH_RECORD_DIR/dirs")" = "$TMPDIR/cache/inshellah:/run/current-system/sw/share/inshellah"

  INSHELLAH_DIR=/custom ${wrapperPackage}/bin/inshellah complete
  test "$(cat "$INSHELLAH_RECORD_DIR/dirs")" = /custom

  touch "$checkOutput"
''
