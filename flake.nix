{

  inputs.nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      forAllSystems =
        f:
        lib.genAttrs (lib.systems.doubles.linux ++ lib.systems.doubles.darwin) (
          system: f (nixpkgs.legacyPackages.${system} or (import nixpkgs { inherit system; }))
        );
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            rustfmt
            rust-analyzer
            clippy
          ];
        };
      });

      packages = forAllSystems (pkgs: {
        default = pkgs.callPackage ./nix/package.nix { };
      });

      checks = forAllSystems (
        pkgs:
        let
          checkSrc = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter =
              path: type:
              let
                base = baseNameOf path;
              in
              !(type == "directory" && (base == ".git" || base == "target"));
          };
          cargoDeps = pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };
          rustInputs = with pkgs; [
            cargo
            clippy
            stdenv.cc
            rustc
          ];
          fakeInshellah = pkgs.writeShellScriptBin "inshellah" ''
            if [ -n "''${INSHELLAH_ARG_FILE:-}" ]; then
              printf '%s\n' "$@" > "$INSHELLAH_ARG_FILE"
            fi
            if [ "''${1:-}" = complete ]; then
              if [ -n "''${INSHELLAH_STATIC_FILE:-}" ] && [ -s "$INSHELLAH_STATIC_FILE" ]; then
                cat "$INSHELLAH_STATIC_FILE"
                printf '\n'
              else
                printf 'null\n'
              fi
            else
              printf 'null\n'
            fi
          '';
          fakeCompletionBackends = pkgs.symlinkJoin {
            name = "inshellah-fake-completion-backends";
            paths = [ fakeInshellah ];
          };
          rustCheckPhase = ''
            echo "running rust checks"
            rm -rf source-rust
            cp -R ${checkSrc} source-rust
            chmod -R u+w source-rust
            pushd source-rust
            export CARGO_HOME="$TMPDIR/cargo-home"
            export CARGO_TARGET_DIR="$TMPDIR/cargo-target"
            mkdir -p .cargo "$CARGO_HOME"
            cat > .cargo/config.toml <<EOF
            [source.crates-io]
            replace-with = "vendored-sources"

            [source.vendored-sources]
            directory = "${cargoDeps}"

            [net]
            offline = true
            EOF
            cargo clippy --all-targets
            cargo test --all-targets
            popd
          '';
          nushellCheckPhase = ''
            echo "running nushell shim checks"
            export PATH="${fakeCompletionBackends}/bin:$PATH"
            export INSHELLAH_STATIC_FILE="$TMPDIR/inshellah-static.json"
            : > "$INSHELLAH_STATIC_FILE"
            nu --no-config-file -c 'source ${./nix/inshellah-completer.nu}; source ${./tests/nushell-completer.nu}'

            cat > "$TMPDIR/config-load.nu" <<'EOF'
            source ${./nix/inshellah-completer.nu}

            def activate [p: path] {
              sudo nix-env --set -p /nix/var/nix/profiles/system $p
              sudo $"($p)/bin/switch-to-configuration" switch
              doas nix-env --set -p /nix/var/nix/profiles/system $p
            }
            EOF
            nu --env-config /dev/null --config "$TMPDIR/config-load.nu" -c 'print ok'
          '';
          mkShellCheck =
            name: inputs: phase:
            pkgs.runCommand name { nativeBuildInputs = inputs; } ''
              ${phase}
              touch $out
            '';
        in
        {
          # nushell is in the rust check inputs so the composed seam test
          # (tests/seam_nu.rs) runs the real binary through the real nu
          # tokenizer instead of self-skipping when nu is absent.
          rust = mkShellCheck "inshellah-rust-check" (rustInputs ++ [ pkgs.nushell ]) rustCheckPhase;
          nushell = mkShellCheck "inshellah-nushell-check" [ pkgs.nushell ] nushellCheckPhase;
          default = mkShellCheck "inshellah-check" (rustInputs ++ [ pkgs.nushell ]) ''
            ${rustCheckPhase}
            ${nushellCheckPhase}
          '';
        }
      );

      nixosModules.default = import ./nix/module.nix;
      darwinModules.default = import ./nix/module.nix;
    };
}
