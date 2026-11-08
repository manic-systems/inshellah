# bash, zsh and fish

bash, zsh and fish can all use the same completion index and live providers
that back nushell. each shell sources a small hook from `inshellah init` that
calls `inshellah complete --shell SHELL` whenever you hit tab.

## setup

```sh
# ~/.bashrc
eval "$(inshellah init bash)"

# ~/.zshrc, anywhere, the hook registers itself after compinit has run
eval "$(inshellah init zsh)"
```

```fish
# ~/.config/fish/config.fish
inshellah init fish | source
```

on nixos or nix-darwin you can list your shells in
`programs.inshellah.shells` instead, see [nixos.md](nixos.md). each one you
list also needs `programs.<shell>.enable` turned on. if a shell is set up
through home-manager alone, put the `inshellah init` line in its rc file as
shown above.

takeover is on when `INSHELLAH_TAKEOVER` is `1`, `true`, `yes` or `on`, in any
case and ignoring surrounding spaces. anything else, including `0`, leaves it
off. the variable gets checked when you press tab, and bash and fish also
check it when the hook first loads. if you flip it later in a running shell,
fish and zsh notice right away, in both directions, and bash catches up at the
next prompt.

if you build an index by hand, pass `--shell` with the shells you want it to
serve. without it, `index` assumes nushell alone and runs `nu` to find out
which builtins to skip, which will fail if nushell is not installed.

```sh
inshellah index /usr --shell bash,zsh
```

in bash, the hook re-claims `complete -D` when the first prompt draws, so load
order relative to bash-completion does not matter. wrapped commands like
`sudo` and `nohup` that bash-completion unwraps still get handed through to
inshellah. under takeover, bash-completion's `_comp_command` and `_comp_root_command`, which it registers up front for `nohup`, `time`, `nice`, `xargs` and the like, ask inshellah about the whole line first and only fall back to the inner command's bash-completion spec when inshellah defers. with takeover off they are left alone. the catch-all spec also carries `-o bashdefault`, so a line the hook defers still gets bash's own command, variable and substitution completion, and under takeover the borrowed bash-completion completer keeps the `-o nospace`, `filenames`, `noquote` and `nosort` options of its spec so a finished word gets one space, not two.

bash has to be 4.1 or newer. macos still ships 3.2 at `/bin/bash`, so grab a
newer bash from nix or homebrew there. on older bash versions the hook stays
out of the way and does nothing.

## fallback and takeover

out of the box, inshellah stays in the background and only steps in when the
shell has no completion of its own for a command, since hand-written
bash-completion, zsh and fish completions usually beat scraped ones. if you
want inshellah to answer first so you get its live providers for git, nix,
systemd and the rest, set `INSHELLAH_TAKEOVER=1` before the hook loads. takeover is on only for `1`, `true`, `yes` or `on`, in any case and ignoring surrounding whitespace, and anything else leaves it off.

| shell | fallback | takeover |
|---|---|---|
| bash | sits on `complete -D` and lets bash-completion's loader try first | answers first, falls back to bash-completion's completer when it comes up empty, and drops the specs bash-completion's own loader registers later at the next prompt and leaves every spec you register yourself alone. anything bash-completion registers before the first prompt still wins |
| zsh | hooks into `-default-`, and calls the `-default-` completer you had before whenever inshellah steps aside | also hooks into `-first-` and cuts off other completers once it has an answer. a `-first-` completer you had before is kept and called whenever inshellah steps aside, including while takeover is off and in contexts such as redirections and parameter names |
| fish | answers only for commands fish has no completion for | answers first, falls back to fish's own completion for that command when it comes up empty, and replaces completions your config registers at runtime. fish's builtins, functions and wrappers like `sudo`, `env` and `xargs` keep fish's own completion, which hands the wrapped command back to inshellah. a command whose first argument is its own, like `ssh` with its host, is not a wrapper and gets taken over. completions you register with `complete -c` after a command loaded survive takeover being turned on or off |

in zsh, the hook checks its registration at every prompt and again as you type, so a `compinit` that a plugin manager defers until after the first prompt does not lose it. pressing tab before any key or prompt follows that deferred `compinit` still reaches the stock completers.

in zsh, long descriptions are cut to fit the terminal width with a trailing `...`, so every candidate stays on one line. wide characters such as cjk and emoji are measured by display width.

in zsh, directory candidates such as the value of `--output=pdir/` keep the cursor right after the slash with no space appended, so you can keep typing the path. the hook sets its own shell options for the functions it runs, so options like `ksharrays` in your config do not break it. the completer itself runs with the same options compsys gives every completion function, so a glob that matches nothing inside `_files` or your own `-first-` completer stays quiet. candidates that start with `~` are inserted unquoted so the shell still expands them. under takeover every answer is remembered for the same words, so `-default-` behind a wrapper like `nohup` and every matcher-list or completer pass reuse it, and a single tab press runs `inshellah complete` once per distinct command line. `command`, `exec`, `noglob`, `nocorrect` and the `-` precommand are looked through under takeover, with their options such as `command -p` and `exec -a name` skipped, so `command git sta` reaches inshellah as `git sta`. `command -v` and `command -V`, `builtin` in front of anything but `kill` or `command`, and `builtin` or `exec` behind `command` or `exec` are left to zsh. behind `command` and `exec`, `kill` reaches inshellah with that precommand still in front, so it is answered as the binary and not as the builtin. with takeover off, zsh's own completion handles all of them. the memory is dropped at the start of each completion attempt through `compprefuncs`, so a branch created between two tab presses shows up on the second. inside quotes, as in `git -C "~/my`, candidates that start with `~` are left to zsh's own completion because the shell does not expand a tilde there.

in bash and zsh, `kill` is a builtin that inshellah normally leaves to the shell, and `command kill` counts as `kill` too. bash runs the builtin behind `command` and `builtin`, zsh runs the builtin only behind `builtin`, and `exec`, `sudo`, `env` and the like run the binary in both. under
takeover, or when bash-completion has no spec for it, inshellah's own `kill`
provider answers instead. that provider offers the builtin's own flags
(`-s`, `-n`, `-l` and `-L`, plus `-q` in zsh) and not the binary's, completes signal
names after them, also attached as in `-sTERM`, and for `-SIGTERM` style words,
and lists pids otherwise. after `--` no flags are offered. zsh's `-n` takes only
numbers, so there it lists signal numbers and bash lists names. the signal
table follows the os inshellah was built for, with `STKFLT`, `PWR` and the
`RTMIN` to `RTMAX` range on linux, and `EMT` and `INFO` on macos. a signal word is not offered once a pid has been typed, as the builtin takes none there. in bash, `builtin kill` and `builtin command kill` reach inshellah with or without bash-completion and in either mode, also behind leading words such as `if`, `!`, `time -p` or `x=1`, and inside an open `$(`, backticks or `<(` where the nested command is what counts. bash-completion's `--help` scraper, which it registers up front for commands such as `cat` and `diff`, is bypassed there, so `cat <(kill -s T` completes the nested command. only a substitution still open at the cursor counts, so `diff <(true) --ig` keeps bash-completion's flags. the hook's completers also run safely under `set -e` and `set -u` in the rc, bash-completion's longopt completer returns 1 after most flags and no hook function lets that end the shell. `builtin exec kill` completes the binary's flags and `builtin command git` completes git's verbs, while `builtin` in front of anything else stays with bash and lists builtin names as before. other eager bash-completion specs on the outer command of an open `<(` still run first. a lone capital such as `pkill -T` offers signals unless pkill has a flag spelled exactly so.
the binary gets signal names after `-s`, `--signal`, `-l` and `--list`, and
pkill gets them after `--signal` and `-SIG` words, including capitalised ones such as `-TE` that a flag letter would otherwise claim. pkill and pgrep get user and group
names after `-u`, `-U`, `-G` and their long forms, also attached as in `-uroot`, every user and group `getent` lists whatever its id, as bash-completion does, or `dscl` on macos. pgrep has no signals. fish keeps its own `kill`, a
function that hands the work to the binary, so fish lists pids after `-l` as it
does without inshellah, while nushell's own `kill` takes a signal number and keeps getting
only pids. reserved words that start a command, like `time`,
`if`, `while`, `!` and `{`, never stop inshellah from completing the command
after them, and neither does a `$(` or backtick substitution opened inside
double quotes. builtins that run another command, `command` and `exec`, hand
the command behind them to inshellah as well. file candidates are not offered
inside a substitution. a line that begins with
an assignment, as in `x="$(git sw`, is still completed by bash as a command
name, so the hook never sees it.

readline would close an open quote after a lone match, which inside a `"$(` is
the substitution's quote and not the match's. bash only avoids that in its own
substitution completer, so the hook hands readline two matches that share the
wanted text and the line is left open, at the price of the same text listed
twice if you press tab again. a quote opened by the command inside a substitution, as in `"$(git checkout "fea`, is closed by inshellah along with the candidate. a directory that exists is completed without
closing its quote and without a space, as are file names that stand alone,
which readline quotes itself and shows without escapes in menus. file names
listed next to provider values, and flag paths such as `--output=`, are escaped
by inshellah and show their backslashes in menus, and menus inside a `"$(` show
the text from the start of the quoted word, because bash lists exactly what it
inserts.

whenever inshellah comes up empty for a cursor position, the shell drops back
to its normal file completion.

## fish stubs

fish does not have a single catch-all completer hook. instead it autoloads
`CMD.fish` from the first directory on `fish_complete_path` that has one. to
work with that, the hook runs `inshellah fish-stubs`, which drops a one-line
stub for every command on `PATH` into the `fish` directory inside the user
cache and prints that directory path. the hook re-runs it whenever the global `PATH`
changes (a local `PATH` that a completer sets for itself does not count), so entering a `nix shell` picks up the new commands automatically. it
also checks the modification times of the `PATH` dirs before each prompt, which
is how a binary installed into a dir already on `PATH`, with `nix profile
install` or `cargo install`, gets completions at the next prompt.
the stub dir goes ahead of the vendor and shipped completion dirs and behind
any dir you put first on `fish_complete_path`, so a completion file of your own
such as `~/.config/fish/completions/CMD.fish` wins even under takeover. a stub
loads fish's own completion for its command and, unless takeover is turned on,
steps aside when there is one or your config registered one with `complete -c`.
turning takeover on or off puts fish's completions away or back for commands
that already loaded, in the order fish had them, which matters for `complete -k`.
sourcing your config again leaves `fish_complete_path` alone when the stub dir is
already in place, so completions that fish loaded stay loaded.
inshellah installs no signal handler, so ctrl-c does exactly what it does without
it and aborts whatever is running, a load included. a load marks its command pending
before it sources anything, keeps fish's completions from before the load, and
clears the mark as its very last step. a load that ctrl-c cut short is still pending
and has left a stand-in completion on the command. the next tab on that command
puts the kept completions back, loads again from scratch and then answers the tab.
completions registered between the cut and the retry are kept, except a line identical to one the load registers itself, which is not repeated. a `complete -e` between the cut and the retry stays erased, so the retry only puts back kept completions that are still registered, and none at all when the stand-in itself was erased. the completion file still loads as normal.
after a cut, fish itself sometimes skips command substitutions in completions on the next tabs, as plain fish does with a slow `-a (...)`, so finishing an interrupted load can take a second tab.
another ctrl-c during that retry changes nothing, so no number of them loses a
load. the prompt does not retry pending loads, since a slow completion file would
hold up every prompt. it only puts the stand-in back when the cut came before it
was registered. a completion file that ends in status 130 by itself is just a
finished load.
new binaries are loaded by hand whatever their number, since assigning
`fish_complete_path` at runtime makes fish drop every completion it autoloaded.
the by-hand load looks for a completion file in `fish_complete_path` order just as
autoloading does, so a file of your own ahead of the stub dir wins for a new binary too.
the load leaves such a file to fish, which autoloads it at the first tab, so it runs once,
and takeover leaves that command alone as it does for one present at startup.
a user file wins when it exists at load time. one created after inshellah loaded the command merges with inshellah's completions until the next shell, as in plain fish, since checking for new files at every tab would cost more than it saves.
names of new binaries wait in a queue, so a ctrl-c partway through loading them
leaves the rest for the next prompt.

## the line protocol

`complete --shell bash|zsh|fish` prints a short header line followed by one
`value<TAB>description` line per candidate. the header is `values` on a normal
match, `values+files` when the shell should offer file completions alongside
the candidates, or `defer` when inshellah wants the shell to handle the
position on its own.

bash hands over the raw command line with `--line` because `COMP_WORDS`
already splits tokens on `COMP_WORDBREAKS` characters like `=` and `:`.
inshellah does its own split and formats each returned value the way readline
expects to insert it, stripping the prefix up to the last wordbreak and
escaping shell characters. the wordbreaks `@` and `$` stay in the replaced word,
as they do in readline, so `autovt@` completes the part after the `@` and keeps it.
for bash, the header line also tacks on a tab and
the command it ended up completing, since a cursor inside `$(` can be
completing a different command than the outer one bash passed to the hook.

```sh
inshellah complete --shell bash --line 'git log --pretty=fu'
inshellah complete --shell zsh -- git log --pretty=fu
```

## flag values attached with `=`

none of the shells split a word like `--output=pa` at the `=`, so the word
inshellah sees is the whole thing. when the flag takes a path (a `FILE`, `PATH`
or `DIR` placeholder) and the value is attached, inshellah lists the matching
files and directories itself and keeps the `--output=` lead on every value.
directories end in `/`. the answer is a plain `values` list, so the shell does
not need to add its own file completion, and the lead is what lets it replace
the word that was typed. a value given as a separate word, `--output pa`, is
left to the shell's own file completion as before.

`values+files` is only used where the position may really be a path. a git
remote, branch or subcommand slot answers plain `values`, while `git checkout`
and `git diff` keep files beside their refs.

a value that starts with `~` or `$` is not listed. no shell expands either one
inside an attached `--output=` value, so a listed path would resolve against
the working directory while the shell shows it as the home directory. the
position is deferred to the shell instead. in bash a candidate that begins
with `~` or `~user` keeps its tilde unescaped so the shell expands it.

`git fetch`, `git pull` and `git push` take a path or url in place of a remote
name, as in `git pull ../other`, so their first argument answers
`values+files` with the remote names beside the shell's files. only the
arguments after it, which are refs, stay plain `values`.

after a bare `--`, `git stash` takes paths, so the stash verbs are no longer
offered and the position is deferred to the shell.

live providers treat an attached value like a separate one. `journalctl
--unit=dbus-b` lists units the way `-u dbus-b` does, and `git restore
--source=ma` lists refs, each value keeping its `--unit=` lead.

`nix` says itself whether a position takes paths, so `nix flake` and `nix
store` answer plain `values` with the subcommands, while a position that takes
installables keeps the shell's files beside nix's candidates.

the value of a directory flag, such as `git -C`, `git --git-dir`, `make -C`,
`just -d` and `npm --prefix`, answers `values` with only the directories, so
the shell does not add plain files beside them. when no directory matches, the
position is deferred to the shell. a typed `~` is expanded for a separate value
and left alone for an attached one.
