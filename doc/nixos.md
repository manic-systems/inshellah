# nixos integration

inshellah provides a nixos module that indexes nushell completions for
every installed package at system build time, and a wrapped binary
that knows where to find the result.

## enabling

```nix
# flake.nix outputs:
{
  nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
    modules = [
      inshellah.nixosModules.default
      { programs.inshellah.enable = true; }
    ];
  };
}
```

or importing directly:

```nix
# configuration.nix
{ pkgs, ... }: {
  imports = [ ./path/to/inshellah-rs/nix/module.nix ];
  programs.inshellah.enable = true;
}
```

after rebuilding, completions are immediately available through the
autoloaded nushell shim.

## what the module does

- installs the inshellah binary, wrapped so the system completion path
  is found automatically.
- runs `inshellah index "$out"` during the system profile build,
  producing one file per command under `$out/share/inshellah/`.
- drops the full nushell external-completer shim into
  `/share/nushell/vendor/autoload/`, including sudo/doas overrides so
  elevated commands still complete through inshellah.
- emits lightweight command-name stubs for dynamic-completion backends
  that are present in the system profile, so tools like `git` and `jj`
  appear in nushell's command list while inshellah still supplies their
  argument completions lazily.
- exposes the same shim as a read-only `snippet` option for users who
  want to source or inspect it manually.

## module options

```nix
programs.inshellah = {
  enable = true;

  # the inshellah package (set automatically by the flake module)
  package = pkgs.inshellah;

  # subdirectory of the system profile holding the index files
  # default: "/share/inshellah"
  completionsPath = "/share/inshellah";

  # additional read-only completion directories to search
  extraDirs = [ "/etc/profiles/per-user/alice/share/inshellah" ];

  # commands to skip entirely during indexing
  ignoreCommands = [ "problematic-tool" ];

  # commands to skip manpage parsing for (uses --help instead)
  helpOnlyCommands = [ "nix" ];

  # per-subprocess timeout in ms during indexing (null = built-in
  # default of 200ms)
  timeoutMs = null;

  # worker-thread count for the parallel scrape
  workers = null;
};
```

## using the completer

the module installs the completer under nushell's vendor autoload path,
so no hand-written nushell config is needed for the normal NixOS case.

the read-only `snippet` option still holds the complete
external-completer config. to manage sourcing yourself instead of using
autoload, write it to a file:

```nix
# generate a config file from the snippet
environment.etc."nushell/inshellah.nu".text = config.programs.inshellah.snippet;
```

then source that file from your nushell config:

```nu
source /etc/nushell/inshellah.nu
```

or copy the snippet directly into `~/.config/nushell/config.nu`:

```nu
# (the snippet is many lines — copy it from `nix eval` of the option,
# or use the environment.etc approach above)
$env.config.completions.external = { ... }
```

the snippet provides both static lookups against the system index and
runtime fallbacks for cases the static index can't cover:

| command | dynamic source |
|---|---|
| `nix` | flake refs via `NIX_GET_COMPLETIONS`, with optional `meta.description` |
| `systemctl` / `journalctl` | unit names from `list-units` |
| `coredumpctl` | units + pids |
| `loginctl` | users / sessions |
| `machinectl` / `networkctl` | machines / links |
| `ssh` / `scp` / `sftp` | hostnames from ssh config + known_hosts |
| `docker` / `podman` | containers + image refs by subcommand |
| `kubectl` | resource names from the live cluster |
| `git` | refs + worktree paths |
| `npm` / `pnpm` / `yarn` | scripts from package.json |
| `make` / `just` | targets / recipes |
| `cargo` | workspace targets behind `--bin` / `--example` / etc. |
| `kill` / `pkill` | pid+comm pairs |

## home manager and user-level package managers

the system module only indexes packages installed system-wide. for
home-manager or per-user nix profiles, run `inshellah index` against
those prefixes separately:

```sh
# home-manager / per-user profile
inshellah index /etc/profiles/per-user/$USER

# classic nix-env profile
inshellah index ~/.nix-profile
```

this indexes into `$XDG_CACHE_HOME/inshellah`, which the completer
searches automatically. to automate via home-manager:

```nix
home.activation.inshellah-index = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
  ${pkgs.inshellah}/bin/inshellah index /etc/profiles/per-user/$USER 2>/dev/null || true
'';
```

## troubleshooting

**completions not appearing**: check that the system index exists
(`ls /run/current-system/sw/share/inshellah/`) and that the completer
is configured.

**missing completions for a specific command**: check if it's a nushell
built-in (`help commands | where name == "thecommand"`) — built-ins
are excluded.

**command name missing but arguments complete after typing it**: the
command may be installed only in a user profile. the system module can
only generate command-name stubs for binaries linked into the system
profile, though the external completer can still complete arguments
once the command word has been typed.

**stale completions after update**: the index regenerates on every
`nixos-rebuild`. if a command changed its flags, rebuild.

**build-time errors**: indexing failures are non-fatal. check
`journalctl` for the build log if completions are missing for a
specific command.
