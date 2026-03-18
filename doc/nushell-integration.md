# using inshellah completions in nushell

inshellah indexes completions for the commands in your `$PATH` and
serves them to nushell's external completer. indexed data is stored as
`.json` and `.nu` files that the `complete` command reads at
tab-completion time.

## quick start

index completions from a system prefix:

```sh
# from a prefix containing bin/ and share/man/
inshellah index /usr

# multiple prefixes
inshellah index /usr /usr/local

# custom directory
inshellah index /usr --dir ~/my-completions
```

then wire up the completer in `~/.config/nushell/config.nu`:

```nu
$env.config.completions.external = {
    enable: true
    completer: {|spans|
        inshellah complete ...$spans
        | from json
    }
}
```

that's it. tab-completion now works for every command indexed.

## commands

```
inshellah index PREFIX... [--dir PATH] [--ignore FILE] [--help-only FILE]
                          [--workers N] [--timeout-ms N]
    index completions into a directory of json/nu files.
    PREFIX is a directory containing bin/ and share/man/.
    default dir: $XDG_CACHE_HOME/inshellah
    --ignore FILE     skip listed commands entirely
    --help-only FILE  skip manpages for listed commands, use --help instead
    --workers N       worker-thread count
    --timeout-ms N    per-subprocess timeout in ms (default: 200)

inshellah complete CMD [ARGS...] [--dir PATH[:PATH...]] [--timeout-ms N]
    nushell custom completer. outputs JSON completion candidates.
    falls back to on-the-fly --help resolution if a command isn't
    indexed yet — the result is cached and subsequent presses are
    instant.
    --dir takes colon-separated paths. the first path is the writable
    user cache; additional paths are read-only system directories.

inshellah query CMD [--dir PATH[:PATH...]]
    print stored completion data for CMD.

inshellah dump [--dir PATH[:PATH...]]
    list indexed commands.

inshellah manpage FILE
    parse a manpage and emit a nushell extern block.

inshellah manpage-dir DIR
    batch-process manpages under DIR (man1 and man8 sections).
```

## what gets handled

- **sources**: native nushell completion generators (clap/cobra tools
  that can emit completions themselves), manpages in section 1 and 8,
  `--help` and `-h` output.
- **groff styles**: gnu `.TP` (coreutils, help2man), `.IP` (curl,
  hand-written), `.PP`+`.RS`/`.RE` (git, docbook), nix3 bullet
  (`nix run`, `nix build`), mdoc (BSD), plus a deroff fallback.
- **subcommand naming**: `git-commit.1` produces `git commit`, not
  `git-commit`. clap-style per-subcommand manpages get one file each.
- **synopsis-only flags**: flags declared in a manpage SYNOPSIS but
  missing from the body (e.g. nix-env's `--profile`, most of sed's
  interface) are picked up too.
- **elevation wrappers**: `sudo`, `doas`, `pkexec`, `su`, `run0` are
  stripped before lookup, including when the real target is given as
  an absolute path.
- **exclusions**: nushell built-ins (ls, cd, mv, etc.) are skipped —
  nushell serves its own completions for those.

## extern blocks (manpage / manpage-dir)

```nu
export extern "rg" [
    --regexp(-e): string            # a pattern to search for
    --file(-f): path                # search for patterns from the given file
    --count(-c)                     # only show the count of matching lines
    --color: string                 # controls when to use color
    --max-depth: int                # limit the depth of directory traversal
]
```

these are produced by `inshellah manpage` / `inshellah manpage-dir` and
can be source'd directly in your nushell config if you prefer that to
the json completer flow.

## native completions and file completion

when a tool ships its own nushell completion generator (clap, cobra, etc.),
inshellah caches its output verbatim as a `.nu` file under the autoload
dir. nushell loads the `extern` declarations and uses its built-in
completer for that command — the external completer (inshellah's `complete`
subcommand) is only consulted as a fallback.

at the `extern` layer, positional/flag types drive what nushell offers:

- `: path` triggers nushell's built-in file/path completion for that slot.
- `: string@my_completer` runs a user-defined closure.
- bare `: string` / `: int` provides no candidates of its own.

so when a native `.nu` declares `--file: path`, you'll see file completions
intermixed with whatever else is in scope. that's intrinsic to the type,
not something inshellah injects.

a few things worth knowing:

- nushell ≤ 0.69 had a bug
  ([#6407](https://github.com/nushell/nushell/issues/6407)) where file
  completion superseded the external completer when the prefix was empty
  or matched a real path. upgrade if you see this.
- [PR #14781](https://github.com/nushell/nushell/pull/14781) tightened the
  contract: an external completer that returns a non-null list now
  suppresses file fallback; only an explicit `null` opts back in. inshellah
  already follows this — `null` for "hand off to nu", `[...]` to override.
- if you want different ranking, the relevant settings are
  `$env.config.completions.{algorithm, sort, partial, case_sensitive}`.
  none of them disables file completion for `: path` parameters — that
  behavior is tied to the type itself.

if a particular native completion bothers you, the workaround is to drop
that one `.nu` file from the autoload directory. nushell falls back to the
external completer for unknown commands, and inshellah's `complete`
subcommand returns candidates directly as JSON — bypassing the `extern`
type layer entirely, so no `: path` slot triggers nu's built-in file
completer.

## nixos

`programs.inshellah.enable = true` will index at system build time and
ship a richer completer with runtime fallbacks (live cluster queries,
git/ssh/docker/k8s lookups, etc.). see [nixos.md](nixos.md).
