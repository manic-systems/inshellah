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

## cache management

```sh
# list cached commands
inshellah dump

# view stored data for a command
inshellah query docker

# clear cache
rm -rf ~/.cache/inshellah/

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
