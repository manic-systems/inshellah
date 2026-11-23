# inshellah

nushell completions engine. indexes completions from manpages, native
generators, and `--help` output, then serves them to nushell's external
completer.

see `doc/` for details:

- [building and installing](doc/building.md) — cargo, nix, post-install setup
- [nushell integration](doc/nushell-integration.md) — setup, the pipeline, the completer
- [nixos / nix-darwin module](doc/nixos.md) — automatic build-time indexing + module options
- [runtime completions](doc/runtime-completions.md) — on-the-fly caching via the completer
- [changelog](CHANGELOG.md) — breaking changes between releases

## shell completions

the nix package installs completions for bash, zsh, fish and nushell. to set
completions up by hand, source the script inshellah prints.

| shell  | setup                                                              |
|--------|--------------------------------------------------------------------|
| bash   | `source <(COMPLETE=bash inshellah)`                                |
| zsh    | `COMPLETE=zsh inshellah > ~/.zfunc/_inshellah`, with it in `fpath` |
| fish   | `COMPLETE=fish inshellah \| source`                                |
| nu     | `COMPLETE=nu inshellah \| save inshellah.nu`, then `source` it     |
| elvish | `eval (env COMPLETE=elvish inshellah \| slurp)`                    |

## license

EUPL-1.2. see [LICENSE](LICENSE)
