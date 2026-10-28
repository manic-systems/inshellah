{ pkgs }:

let
  inherit (pkgs) lib;
  fakeInshellah = pkgs.writeShellScriptBin "inshellah" ''
    command="$1"
    profile="$2"
    shift
    if [ "$command" = complete ]; then
      mkdir -p "$INSHELLAH_RECORD_DIR"
      printf '%s\n' "''${INSHELLAH_DIR:-}" > "$INSHELLAH_RECORD_DIR/dirs"
      printf '%s\n' "$@" > "$INSHELLAH_RECORD_DIR/args"
      exit 0
    fi
    shift
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --dir) indexDir="$2" ;;
        --ignore) ignoreList="$2" ;;
        --sources) sources="$2" ;;
      esac
      shift 2
    done
    mkdir -p "$indexDir"
    if [ "$command" = index ]; then
      while read -r ignored; do
        echo "$ignored" > "$indexDir/command-ignored-$ignored"
      done < "$ignoreList"
      for binary in "$profile"/bin/*; do
        echo "$binary" > "$indexDir/command-$(basename "$binary")"
      done
    else
      while read -r prefix packageIndex; do
        cp -r --no-preserve=mode "$packageIndex/." "$indexDir/"
      done < "$sources"
      echo merged > "$indexDir/merge-ran"
    fi
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
    mkdir -p $out/bin $out/share/man/man1
    printf '.TH MAN-ONLY 1\n.SH NAME\nman-only \\- documented but not installed\n' > $out/share/man/man1/man-only.1
  '';
  evalIndexModule =
    {
      systemPackages ? [ ],
      extraScrapePackages ? [ ],
      ignoreCommands ? [ ],
    }:
    lib.evalModules {
      specialArgs = { inherit pkgs; };
      modules = [
        ./harness.nix
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
    export out="$TMPDIR/mixed-profile"
    mkdir -p "$out/bin"
    ln -s ${profilePackage}/bin/profile-tool "$out/bin/profile-tool"
    ${mixedSetup}
    test -f "$out/share/inshellah/merge-ran"
    test -f "$out/share/inshellah/command-profile-tool"
    test -f "$out/share/inshellah/command-extra-tool"
    test -f "$out/share/nushell/vendor/autoload/inshellah.nu"
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
    mkdir -p "$out"
    ${manOnlySetup}
    test -e "$out/share/inshellah/merge-ran"
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
