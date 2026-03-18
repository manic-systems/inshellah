{

  inputs.nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      forAllSystems =
        f: nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed (sys: f nixpkgs.legacyPackages.${sys});
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
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "inshellah";
          version = "0.1.1";
          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;
          meta = {
            description = "nushell completion indexer";
            mainProgram = "inshellah";
          };
        };
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
          fakeNix = pkgs.writeShellScriptBin "nix" ''
            if [ "''${1:-}" = eval ]; then
              printf 'raw package description\n'
            else
              printf 'header\nbuild\nflake#pkg\n'
            fi
          '';
          fakeSystemctl = pkgs.writeShellScriptBin "systemctl" ''
            case "$*" in
              *"g*"*)
                printf 'greetd.service loaded active running Greeter\n'
                ;;
              *)
                printf 'demo.service loaded active running Demo Unit\n'
                ;;
            esac
          '';
          fakeKubectl = pkgs.writeShellScriptBin "kubectl" ''
            printf '%s\n' "$*" > "$KUBECTL_ARGS_FILE"
            if [ "''${1:-}" = get ] && [ "''${2:-}" = deployment ]; then
              printf 'deploy-a\n'
            elif [ "''${1:-}" = get ]; then
              printf 'pod-a\n'
            fi
          '';
          fakeCargo = pkgs.writeShellScriptBin "cargo" ''
            cat <<'JSON'
            {"packages":[{"name":"app-lib","version":"0.1.0","targets":[{"name":"app-lib","kind":["lib"]},{"name":"app-cli","kind":["bin"]},{"name":"app-integration","kind":["test"]}]},{"name":"helper-lib","version":"0.2.0","targets":[{"name":"helper-lib","kind":["lib"]}]}]}
            JSON
          '';
          fakeGit = pkgs.writeShellScriptBin "git" ''
            case "''${1:-}" in
              remote)
                printf 'origin\nupstream\n'
                ;;
              for-each-ref)
                case "$*" in
                  *"refs/heads refs/remotes refs/tags"*)
                    printf 'main\tcommit\tMain branch\norigin/main\tcommit\tRemote main\nv1.0\tcommit\tRelease 1\n'
                    ;;
                  *"refs/heads"*)
                    printf 'main\tMain branch\nfeature\tFeature branch\n'
                    ;;
                  *"refs/tags"*)
                    printf 'v1.0\tRelease 1\nv2.0\tRelease 2\n'
                    ;;
                esac
                ;;
              stash)
                if [ "''${2:-}" = list ]; then
                  printf 'stash@{0}: WIP on main: demo stash\n'
                fi
                ;;
              status)
                printf ' M src/main.rs\n?? new-file.txt\nR  old.txt -> renamed.txt\n'
                ;;
              ls-files)
                printf 'src/main.rs\nREADME.md\n'
                ;;
              config)
                printf 'submodule.demo.path deps/demo\n'
                ;;
              worktree)
                if [ "''${2:-}" = list ]; then
                  printf 'worktree /repo/linked\n'
                fi
                ;;
            esac
          '';
          fakeJj = pkgs.writeShellScriptBin "jj" ''
            case "''${1:-}" in
              log)
                printf 'k\tworking change\nm\tmain change\n'
                ;;
              bookmark)
                if [ "''${2:-}" = list ]; then
                  printf 'main\nfeature\norigin/main\n'
                fi
                ;;
              tag)
                if [ "''${2:-}" = list ]; then
                  printf 'v1.0\nv2.0\n'
                fi
                ;;
              git)
                if [ "''${2:-}" = remote ] && [ "''${3:-}" = list ]; then
                  printf 'origin https://example.com/repo.git\nupstream https://example.com/upstream.git\n'
                fi
                ;;
              op|operation)
                if [ "''${2:-}" = log ]; then
                  printf 'abc123\tcheckout working copy\n'
                fi
                ;;
              file)
                if [ "''${2:-}" = list ]; then
                  printf 'src/main.rs\nREADME.md\n'
                fi
                ;;
              workspace)
                if [ "''${2:-}" = list ]; then
                  printf 'default\nlinked\n'
                fi
                ;;
            esac
          '';
          fakeCompletionBackends = pkgs.symlinkJoin {
            name = "inshellah-fake-completion-backends";
            paths = [
              fakeInshellah
              fakeNix
              fakeSystemctl
              fakeKubectl
              fakeCargo
              fakeGit
              fakeJj
            ];
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
            export KUBECTL_ARGS_FILE="$TMPDIR/kubectl.args"
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
          rust = mkShellCheck "inshellah-rust-check" rustInputs rustCheckPhase;
          nushell = mkShellCheck "inshellah-nushell-check" [ pkgs.nushell ] nushellCheckPhase;
          default = mkShellCheck "inshellah-check" (rustInputs ++ [ pkgs.nushell ]) ''
            ${rustCheckPhase}
            ${nushellCheckPhase}
          '';
        }
      );

      nixosModules.default =
        { pkgs, ... }:
        {
          imports = [ ./nix/module.nix ];
          programs.inshellah.package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        };
    };
}
