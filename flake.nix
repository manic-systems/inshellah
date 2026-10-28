{

  outputs =
    { self, ... }@args:
    let
      inputs = import ./.tack { overrides = args.tackOverrides or { }; };
      inherit (inputs) fenix nixpkgs;
      inherit (nixpkgs) lib;
      forAllSystems =
        f:
        lib.genAttrs (lib.systems.doubles.linux ++ lib.systems.doubles.darwin) (
          system: f (nixpkgs.legacyPackages.${system} or (import nixpkgs { inherit system; }))
        );
      nightlyRustfmt =
        pkgs: fenix.packages.${pkgs.stdenv.hostPlatform.system}.latest.rustfmt or pkgs.rustfmt;
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            (nightlyRustfmt pkgs)
            rust-analyzer
            clippy
            tombi
          ];
        };

        # `nix develop .#fmt` formats on entry
        fmt = pkgs.mkShellNoCC {
          packages = [
            pkgs.cargo
            (nightlyRustfmt pkgs)
            pkgs.tombi
            pkgs.nixfmt
          ];
          shellHook = ''
            cargo fmt
            tombi format
            find . -name '*.nix' -not -path './target/*' -exec nixfmt {} +
          '';
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
            cargo clippy --all-targets -- -D warnings
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
            pkgs.runCommand name
              {
                nativeBuildInputs = inputs;
                buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];
              }
              ''
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
          module-indexing = import ./nix/tests/module-indexing.nix { inherit pkgs; };
          fmt =
            pkgs.runCommand "inshellah-fmt-check"
              {
                nativeBuildInputs = [
                  pkgs.cargo
                  (nightlyRustfmt pkgs)
                  pkgs.tombi
                  pkgs.nixfmt
                ];
                src = checkSrc;
              }
              ''
                cp -r $src ./tree
                chmod -R +w ./tree
                cd ./tree
                cargo fmt -- --check
                tombi format --offline --check
                find . -name '*.nix' -exec nixfmt --check {} +
                touch $out
              '';
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
