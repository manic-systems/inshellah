# runtime completion resolution

`inshellah complete` uses the static index first. if an uncached command
or subcommand is needed, it runs `--help` (or `-h`), caches the result in
the user directory, and returns completions immediately. if the static
index reaches a value slot or leaf argument it cannot answer, inshellah
asks a live provider. if no static or live provider has candidates, it
prints `null` so nushell can use its normal file completion.

## how it works

typing `docker compose up --<TAB>`:

1. nushell calls `inshellah complete docker compose up --`
2. inshellah looks up the longest matching prefix in the index
3. if found, it fuzzy-matches indexed flags and subcommands against the
   partial input
4. if a value slot or leaf argument remains, it asks the command's live
   provider, if one exists
5. if no indexed prefix is found, it locates the binary in `$PATH`, runs `--help`,
   recursively resolves subcommands, caches the results in the user
   directory (`$XDG_CACHE_HOME/inshellah`), and returns completions

all subsequent static completions for that command are served from cache.
live providers stay live because their values can change between keypresses.

elevation wrappers (`sudo`, `doas`, `pkexec`, `su`, `run0`) are
stripped before lookup: `sudo docker compose up --` resolves against
`docker`, not `sudo`. absolute paths after the wrapper are recognised
too.

## setup

```nu
# ~/.config/nushell/config.nu
source /path/to/inshellah-completer.nu
```

the bundled shim wraps `inshellah complete` defensively: malformed JSON,
`null`, and empty candidate lists return `null`, which lets nushell fall
back to its normal file completion. a raw `| from json` example is shorter
but can throw errors while tab-completing.

with the nixos module, no extra config is needed beyond enabling the
module — the wrapper has the system paths baked in.

to manually specify system dirs, use colon-separated `--dir`:

```nu
$env.config.completions.external = {
    enable: true
    completer: {|spans|
        let completed = (inshellah complete ...$spans --dir $"($env.XDG_CACHE_HOME)/inshellah:/run/current-system/sw/share/inshellah" | complete)
        if $completed.exit_code == 0 {
            try { $completed.stdout | from json } catch { null }
        } else {
            null
        }
    }
}
```

paths after the first in `--dir` are read-only system dirs.

## configuration

the `complete` path reads a few behavioural knobs from the environment.
each has a compiled-in default that reproduces the original behaviour, so
an unconfigured install is unchanged. on nixos these are set for you by
the module options (see [nixos.md](nixos.md)); elsewhere, export them in
your shell before nushell starts.

| variable | default | effect |
|---|---|---|
| `INSHELLAH_FLAG_TRIGGERS` | `-` | characters that surface flag completions when a partial token begins with one of them. set to `-+` to also trigger on `+`; whitespace is ignored. an empty value disables prefix-triggered flags (leaving only `INSHELLAH_FLAG_ON_EMPTY`). |
| `INSHELLAH_FLAG_ON_EMPTY` | `0` | when truthy (`1`/`true`/`yes`/`on`), also surface flags on an empty token — i.e. right after a space — alongside subcommands. otherwise an empty token hands off to file/dynamic completion. |
| `INSHELLAH_DYNAMIC_TIMEOUT_MS` | `5000` | wall-clock budget in milliseconds shared by live provider subprocesses for one completion request. `0` disables this runtime timeout. on timeout the provider returns no candidates and inshellah prints `null` if nothing else can answer. |
| `INSHELLAH_DYNAMIC_LIMIT` | `200` | cap passed to live providers that support native limits, such as `git for-each-ref --count`, `jj log -n`, and `docker ps --last`. `0` omits those provider-specific limit flags; providers without native caps ignore it. |
| `INSHELLAH_TIMEOUT_MS` | `1200` | per-subprocess timeout for on-the-fly `--help` resolution of uncached commands and subcommands. it also bounds the current `adb` value provider. an explicit `--timeout-ms` flag overrides it. this is separate from `INSHELLAH_DYNAMIC_TIMEOUT_MS`. |
| `INSHELLAH_MAX_COMPLETIONS` | `0` | cap on candidates returned by indexed/static matching, and nushell's `max_results` when sourcing the bundled snippet. `0` imposes no inshellah cap; nushell's own default of 200 still applies. |

### flag triggering

by default flags are offered only once a token begins with `-`
(`git commit --<TAB>`). two overrides are available:

- **other trigger characters** — `INSHELLAH_FLAG_TRIGGERS="-+"` makes a
  leading `+` surface flags too. for non-dash triggers the typed text
  after the trigger is matched against the bare flag name, so `+ver`
  completes to `--verbose`. the emitted value keeps the tool's real
  dashed flag.
- **flags after a space** — `INSHELLAH_FLAG_ON_EMPTY=1` lists flags
  immediately after a space, mixed in with subcommands, before any
  character is typed.

## cache management

```sh
# list cached commands
inshellah dump

# view stored data for a command
inshellah query docker

# clear the on-the-fly user cache (.json/.nu files; system dirs untouched)
inshellah purge

# re-index from a prefix
inshellah index /usr --dir ~/.cache/inshellah
```

## when to use this vs build-time indexing

the nixos module (`programs.inshellah.enable = true`) handles system
packages at build time. runtime resolution covers:

- commands installed outside the system profile (cargo, pip, npm, go)
- subcommand completions at arbitrary depth
- systems without the nixos module

for upfront indexing on non-nixos systems:

```sh
inshellah index /usr /usr/local
```

## live providers

live providers are narrow value completers. they run only after the
static index has handled command structure and either reaches a known
value slot or has no leaf candidates to offer. unsupported commands,
empty provider output, parse failures, and provider timeouts all hand off
with `null`.

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
| `zig` | build.zig steps |
| `kill` / `pkill` | pid and command pairs |

## macOS developer toolchain

`/usr/bin/git`, `/usr/bin/clang`, and friends are `xcrun` shims whose real
binaries and manpages live under the active developer dir (`xcode-select
-p` — Command Line Tools or full Xcode), outside the usual prefixes. to
index those, point `index` at the real prefix explicitly — either the
developer dir or, preferably, the nix equivalents:

```sh
# the active developer toolchain
inshellah index --prefix "$(xcode-select -p)/usr"

# or nix-provided tools, kept reproducible
inshellah index /run/current-system/sw --prefix /nix/store/…-git:/nix/store/…-clang
```

`--prefix` takes a colon-separated list of extra prefixes, scraped
alongside the positional ones. the nix module exposes this as
`programs.inshellah.extraScrapePackages` (see [nixos.md](nixos.md)).
