## unreleased

### breaking

#### cli

- `index` and `merge` fail when an `--ignore` or `--help-only` file cannot be read, instead of treating it as an empty list. make sure the list files exist. `5b48f8a`
- `manpage` fails on a missing, unreadable or non-file input, and `manpage-dir` fails on a missing or unreadable path or one that is not a directory. both used to print nothing and exit 0. `5b48f8a`
- an empty entry in `INSHELLAH_DIR`, a colon-separated `--dir`, `index --prefix` or `diff --dir`, as in `:/x`, a trailing `:` or a set but empty `INSHELLAH_DIR=`, fails with an error naming the option, and so does an empty positional `index` prefix, `diff --scan` value, `index --dir` or `merge --dir`. empty entries used to resolve to the working directory. `complete` and `explain` skip empty entries instead. `5b48f8a`
- `index` and `diff --scan` fail on a prefix that does not exist or whose `bin/` cannot be listed, instead of reporting zero commands. a prefix without `bin/` stays valid. a home-manager activation that indexes `/etc/profiles/per-user/$USER` without `useUserPackages` now fails, index `${config.home.profileDirectory}` instead. `5b48f8a`
- a command counts as executable only when the current user may run it, for `PATH` lookup, `index` bin scans and explicit paths given to `complete`. `index` now skips binaries such as root-only 0700 tools that it used to scrape. empty and relative `PATH` entries are skipped, also for every command inshellah runs, so a binary planted in the working directory is never picked up. `dd88075`
- `--help-only` entries also apply to every subcommand of the listed command, so `git` keeps `git add` on `--help` data too. `8fdc9c1`
- `merge` fails on an unreadable profile, source package or source index instead of skipping it, and a command belongs to the first source package its profile symlink chain passes through. `caa277f`
- commands run for dynamic completion have no controlling terminal, and every command inshellah runs gets `SSH_ASKPASS_REQUIRE=never`, so a dynamic completion that reaches an ssh host needing a password or host key confirmation fails at once, where it used to hang until the dynamic timeout. `325e898` `5b48f8a`
- `dynamicLimit` / `INSHELLAH_DYNAMIC_LIMIT` only bounds the unfiltered listing. once a prefix is typed, providers query without it so matches past the limit are found, which costs more on very large ref or container lists. `22c6922`
- user cache entries written by another inshellah build, including every 0.2.0 cache, are dropped and resolved again on first use. caches built with `inshellah index` into the user dir go the same way, so rerun `index` on those prefixes after updating. `7090b28`

#### nixos module

- `dynamicTimeoutMs`, `dynamicLimit`, `maxCompletions`, `cacheTtlSecs` and `completeTimeoutMs` reject negative values at evaluation. negative values used to be ignored or replaced by the default at runtime. a negative `timeoutMs` or `workers = 0` already failed the build and now fails at evaluation. `ee0111c`
- `helpOnlyCommands` follows the `--help-only` change above and covers the subcommands of each listed command. `8fdc9c1`
- when cross compiling, a scraped package, `nushellPackage` or `package` is only replaced by its spliced build-platform twin, or by the stock build package when it is the unmodified stock package. an override is no longer matched to the stock build package by name and version. a scraped package without a twin is skipped with a warning naming it, and a `nushellPackage` or `package` without one fails evaluation, where both used to run with the host-platform tools. `1524d5f`
