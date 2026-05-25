# inshellah

nushell completions engine. indexes completions from manpages, native
generators, and `--help` output, then serves them to nushell's external
completer.

see `doc/` for details:

- [building and installing](doc/building.md) — cargo, nix, post-install setup
- [nushell integration](doc/nushell-integration.md) — setup, the pipeline, the completer
- [nixos / nix-darwin module](doc/nixos.md) — automatic build-time indexing + module options
- [runtime completions](doc/runtime-completions.md) — on-the-fly caching via the completer

## license

EUPL-1.2. see [LICENSE](LICENSE)
