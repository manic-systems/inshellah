# nixos / nix-darwin integration

inshellah provides a module that indexes nushell completions for every
installed package at system build time, and a wrapped binary that knows
where to find the result. the same module body backs both NixOS and
nix-darwin — on Linux it scrapes ELF binaries, on macOS Mach-O ones,
selected automatically by the inshellah build's target platform.

## enabling (NixOS)

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

## enabling (nix-darwin)

```nix
# flake.nix outputs:
{
  darwinConfigurations.mymac = nix-darwin.lib.darwinSystem {
    modules = [
      inshellah.darwinModules.default
      { programs.inshellah.enable = true; }
    ];
  };
}
```

the options and behaviour are identical to the NixOS module — it reads
the same `programs.inshellah.*` settings and writes the same completion
index and nushell shim under the system profile (`/run/current-system/sw`,
which nix-darwin also uses).

on macOS, the tools you reach through `/usr/bin` (`git`, `clang`, …) are
`xcrun` shims whose real binaries and manpages live under the active
developer dir, outside the nix system profile — so they aren't indexed by
default. rather than probe the host toolchain impurely, list the nix
equivalents you want completed in `extraScrapePackages`; the module rolls
their store paths into the build-time scrape (`inshellah index … --prefix
…`). this applies on NixOS too, for any package whose completions you want
indexed without putting it on the system path.

after rebuilding, completions are immediately available through the
autoloaded nushell shim.

## what the module does

- installs the inshellah binary, wrapped so the system completion path
  is found automatically. when you pass `--dir` explicitly, the wrapper
  leaves your value unchanged.
- runs `inshellah index "$out"` during the system profile build,
  producing one file per command under `$out/share/inshellah/`.
- drops the full nushell external-completer shim into
  `/share/nushell/autoload/`, including sudo/doas overrides so elevated
  commands still complete through inshellah.
- emits lightweight command-name stubs for live-provider commands
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

  # extra packages to scrape alongside the system profile; each store path
  # is passed to `inshellah index --prefix`. handy on macOS for the nix
  # equivalents of /usr/bin shim tools (git, clang, …)
  extraScrapePackages = [ pkgs.git pkgs.clang ];

  # per-subprocess timeout in ms during indexing (null = built-in
  # default of 200ms)
  timeoutMs = null;

  # wall-clock timeout in ms for live providers at tab-completion time.
  # set to 0 to disable this runtime timeout
  dynamicTimeoutMs = 5000;

  # result cap requested from live providers that support native limits.
  # set to 0 to omit provider-specific limit flags
  dynamicLimit = 200;

  # characters that trigger flag completions when a partial token begins
  # with one of them. default "-"; e.g. "-+" also triggers on "+"
  flagTriggers = "-";

  # also surface flags on an empty token (right after a space), mixed in
  # with subcommands. default false
  flagOnEmpty = false;

  # cap on indexed/static candidates returned and nushell's max_results.
  # 0 = no inshellah cap
  # (nushell's built-in default of 200 still applies)
  maxCompletions = 0;

  # per-subprocess timeout (ms) for the completer's on-the-fly --help
  # resolution of uncached commands. null = built-in default of 200ms.
  # distinct from timeoutMs (indexing) and dynamicTimeoutMs (live shim)
  completeTimeoutMs = null;

  # worker-thread count for the parallel scrape
  workers = null;
};
```

### flag-triggering behaviour

`flagTriggers` and `flagOnEmpty` control when option/flag completions are
offered. by default flags appear only after a leading `-`. add characters
to `flagTriggers` (e.g. `"-+"`) to trigger on them as well - for a
non-dash trigger the text after it is matched against the bare flag name,
so `+ver` completes to `--verbose`. set `flagOnEmpty = true` to list flags
immediately after a space, alongside subcommands. these map to the
`INSHELLAH_FLAG_TRIGGERS` / `INSHELLAH_FLAG_ON_EMPTY` environment variables
(see [runtime-completions.md](runtime-completions.md)).

## using the completer

the module installs the completer under nushell's autoload path, so no
hand-written nushell config is needed for the normal NixOS case. package
`vendor/autoload` trees are not linked by the module; generated and native
completion data stays behind `inshellah complete` so priority is consistent.

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

the snippet calls `inshellah complete`, which uses the static system index
first. live providers run only for value slots and leaf arguments the
index cannot answer. if the static index and live provider both have no
candidates, or if a provider times out, the completer returns `null` so
nushell can fall back to normal file completion.

the provider timeout defaults to 5s and is controlled by
`programs.inshellah.dynamicTimeoutMs` / `INSHELLAH_DYNAMIC_TIMEOUT_MS`.
set it to 0 to disable the runtime timeout. providers with native result
caps use `programs.inshellah.dynamicLimit` / `INSHELLAH_DYNAMIC_LIMIT`,
defaulting to 200. set it to 0 to omit provider-specific limit flags;
providers without native caps ignore it.

`programs.inshellah.completeTimeoutMs` / `INSHELLAH_TIMEOUT_MS` is
separate: it is the per-subprocess timeout for on-the-fly `--help`
resolution of uncached commands, and also bounds the current `adb` value
provider. `programs.inshellah.maxCompletions` /
`INSHELLAH_MAX_COMPLETIONS` caps indexed/static candidates and the
snippet's nushell `max_results`.

| command | provider |
|---|---|
| `nix` | `NIX_GET_COMPLETIONS`, with optional `meta.description` lookup |
| `systemctl` / `journalctl` | systemd unit names |
| `coredumpctl` | units and pids |
| `loginctl` | users and sessions |
| `machinectl` / `networkctl` | machines and links |
| `ssh` / `scp` / `sftp` | ssh config and known_hosts names |
| `adb` | device selectors and installed package names |
| `docker` / `podman` | containers and image refs by subcommand |
| `kubectl` | resource names from the selected cluster and namespace |
| `git` | refs and worktree paths |
| `jj` | revisions, operations, bookmarks, remotes, files, and workspaces |
| `npm` / `pnpm` / `yarn` | package.json scripts |
| `make` / `just` | targets and recipes |
| `cargo` | workspace targets for `--bin`, `--example`, and related slots |
| `kill` / `pkill` | pid and command pairs |

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
