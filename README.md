# inshellah

shell completions engine for nushell, bash, zsh and fish. it indexes
completions from manpages, native generators, and `--help` output, then
hands them off to whichever shell you are in.

see `doc/` for details:

- [building and installing](doc/building.md) — cargo, nix, post-install setup
- [nushell integration](doc/nushell-integration.md) — setup, the pipeline, the completer
- [bash, zsh and fish](doc/shells.md) — setup, fallback and takeover, the line protocol
- [nixos / nix-darwin module](doc/nixos.md) — automatic build-time indexing + module options
- [runtime completions](doc/runtime-completions.md) — on-the-fly caching via the completer
- [changelog](CHANGELOG.md) — breaking changes between releases

## license

EUPL-1.2. see [LICENSE](LICENSE)
