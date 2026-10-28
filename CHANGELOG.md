## unreleased

### breaking

#### cli

- `index` and `merge` fail when an `--ignore` or `--help-only` file cannot be read, instead of treating it as an empty list. make sure the list files exist. `2c33f68`
- `manpage` fails on a missing, unreadable or non-file input, and `manpage-dir` fails on a missing or unreadable directory. both used to print nothing and exit 0. `2c33f68`
- empty entries in `INSHELLAH_DIR` or a colon-separated `--dir`, as in `:/x` or a trailing `:`, are ignored. they used to resolve to the working directory, so a cache could land wherever the shell happened to be. `2c33f68`
- command lookup on `PATH` requires execute permission for the current user, not just any execute bit in the file mode. `64afdf6`
- `merge` fails on an unreadable profile, source package or source index instead of skipping it, and resolves command ownership through symlinks. `bd00112`

#### nixos module

- `timeoutMs`, `dynamicTimeoutMs`, `dynamicLimit`, `maxCompletions`, `cacheTtlSecs` and `completeTimeoutMs` reject negative values, and `workers` must be at least 1. fix any config that set a negative number or `workers = 0`. `cfa6028`
