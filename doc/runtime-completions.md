# runtime completion resolution

when a command isn't in the static index yet, `inshellah complete`
runs `--help` (or `-h`) on the binary, caches the result in the user
directory, and returns completions immediately. tab-completion just
works for tools installed outside the indexed prefixes — via cargo,
pip, npm, go, etc.

## how it works

typing `docker compose up --<TAB>`:

1. nushell calls `inshellah complete docker compose up --`
2. inshellah looks up the longest matching prefix in the index
3. if found, it fuzzy-matches flags and subcommands against the partial input
4. if not found, it locates the binary in `$PATH`, runs `--help`,
   recursively resolves subcommands, caches the results in the user
   directory (`$XDG_CACHE_HOME/inshellah`), and returns completions

all subsequent completions for that command are served from cache.

elevation wrappers (`sudo`, `doas`, `pkexec`, `su`, `run0`) are
stripped before lookup: `sudo docker compose up --` resolves against
`docker`, not `sudo`. absolute paths after the wrapper are recognised
too.

## setup

```nu
# ~/.config/nushell/config.nu
$env.config.completions.external = {
    enable: true
    completer: {|spans|
        inshellah complete ...$spans
        | from json
    }
}
```

with the nixos module, no extra config is needed beyond enabling the
module — the wrapper has the system paths baked in.

to manually specify system dirs, use colon-separated `--dir`:

```nu
$env.config.completions.external = {
    enable: true
    completer: {|spans|
        inshellah complete ...$spans --dir $"($env.XDG_CACHE_HOME)/inshellah:/run/current-system/sw/share/inshellah"
        | from json
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
| `INSHELLAH_MAX_COMPLETIONS` | `0` | cap on the number of candidates returned (and nushell's `max_results` when sourcing the bundled snippet). `0` imposes no inshellah cap; nushell's own default of 200 still applies. |
| `INSHELLAH_TIMEOUT_MS` | `200` | per-subprocess timeout for the on-the-fly `--help` resolution. an explicit `--timeout-ms` flag overrides it. |

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
