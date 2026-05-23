def fail [msg: string] {
    error make {msg: $msg}
}

def assert-eq [actual expected msg: string] {
    if $actual != $expected {
        fail $"($msg): expected ($expected | to nuon), got ($actual | to nuon)"
    }
}

def assert-contains [items needle msg: string] {
    if not ($needle in $items) {
        fail $"($msg): expected ($items | to nuon) to contain ($needle | to nuon)"
    }
}

def values [items] {
    $items | default [] | get value
}

let completer = $env.config.completions.external.completer

def _assert_elevation_wrappers_accept_command_tails [p: path] {
    sudo nix-env --set -p /nix/var/nix/profiles/system $p
    doas nix-env --set -p /nix/var/nix/profiles/system $p
}

'[{"value":"--static","description":"from static cache"}]' | save --force $env.INSHELLAH_STATIC_FILE
let static_result = do $completer [demo ""]
assert-eq ($static_result | get 0.value) "--static" "static completion pass-through"
'[{"value":"--server","description":"from static cache"},{"value":"--preserve","description":"from static cache"}]' | save --force $env.INSHELLAH_STATIC_FILE
let static_fuzzy_result = do $completer [demo ser]
assert-eq (values $static_fuzzy_result) ['--server' '--preserve'] "static fuzzy completions are not refiltered by shim"

"{" | save --force $env.INSHELLAH_STATIC_FILE
let bad_static_result = do $completer [demo ""]
assert-eq $bad_static_result null "bad static JSON falls back cleanly"
"" | save --force $env.INSHELLAH_STATIC_FILE

assert-eq (do $completer [nix]) null "nix completion ignores too-short spans"
let nix_commands = do $completer [nix ""]
assert-eq ($nix_commands | get 0.value) "build" "nix command completion uses NIX_GET_COMPLETIONS"
let nix_pkg = do $completer [nix "flake#pkg"]
assert-eq ($nix_pkg | get 0.description) "raw package description" "nix descriptions are raw strings"
let nix_slow = do $completer [nix slow ""]
assert-eq $nix_slow null "slow dynamic completions time out"

let systemctl_empty = do $completer [systemctl daemon-reload ""]
assert-eq $systemctl_empty null "systemctl does not offer units for non-unit verbs"
let systemctl_units = do $completer [systemctl status ""]
assert-eq ($systemctl_units | get 0.value) "demo.service" "systemctl offers units for unit verbs"
let systemctl_prefixed_units = do $completer [systemctl start g]
assert-eq ($systemctl_prefixed_units | get 0.value) "greetd.service" "systemctl unit completions accept typed prefixes"

let kubectl_pods = do $completer [kubectl get pods -n prod ""]
assert-eq ($kubectl_pods | get 0.value) "pod-a" "kubectl resource names complete"
assert-eq (open $env.KUBECTL_ARGS_FILE | str contains "-n prod") true "kubectl preserves namespace flags"
let kubectl_rollout = do $completer [kubectl rollout status deployment ""]
assert-eq ($kubectl_rollout | get 0.description) "deployment" "kubectl rollout uses resource kind, not action"

let cargo_packages = do $completer [cargo test -p ""]
assert-eq (values $cargo_packages) [app-lib helper-lib] "cargo -p completes packages"
let cargo_bins = do $completer [cargo run --bin ""]
assert-eq (values $cargo_bins) [app-cli] "cargo --bin completes only bin targets"

"[]" | save --force $env.INSHELLAH_STATIC_FILE
let git_top = do $completer [git ""]
assert-contains (values $git_top) "remote" "git top-level completes common commands"
assert-contains (values $git_top) "stash" "git top-level includes stash"
let git_push = do $completer [git push ""]
assert-eq (values $git_push) [origin upstream] "empty static completions fall through to git remotes"
let git_remote_verbs = do $completer [git remote ""]
assert-eq (values $git_remote_verbs) [add rename remove rm set-head set-branches get-url set-url show prune update] "git remote completes subcommands"
let git_remote_filtered = do $completer [git remote sho]
assert-eq (values $git_remote_filtered) [show] "git remote subcommands filter by typed prefix"
let git_remote_fuzzy = do $completer [git remote shw]
assert-eq (values $git_remote_fuzzy) [show] "git remote subcommands use fuzzy filtering"
let git_remote_exact = do $completer [git remote show]
assert-eq $git_remote_exact null "exact dynamic completion disappears"
let git_remote_show = do $completer [git remote show ""]
assert-eq (values $git_remote_show) [origin upstream] "git remote show completes remote names"
let git_fetch = do $completer [git fetch ""]
assert-eq (values $git_fetch) [origin upstream] "git fetch completes remotes"
let git_fetch_ref = do $completer [git fetch origin ""]
assert-contains (values $git_fetch_ref) "main" "git fetch after remote completes refs"
let git_branch_delete = do $completer [git branch -d ""]
assert-eq (values $git_branch_delete) [main feature] "git branch delete completes local branches"
let git_tag_delete = do $completer [git tag -d ""]
assert-eq (values $git_tag_delete) [v1.0 v2.0] "git tag delete completes tags"
let git_stash_apply = do $completer [git stash apply ""]
assert-eq (values $git_stash_apply) ['stash@{0}'] "git stash apply completes stashes"
let git_submodule_update = do $completer [git submodule update ""]
assert-eq (values $git_submodule_update) [deps/demo] "git submodule update completes submodule paths"
let git_bisect = do $completer [git bisect ""]
assert-contains (values $git_bisect) "good" "git bisect completes subcommands"
let git_bisect_good = do $completer [git bisect good ""]
assert-contains (values $git_bisect_good) "main" "git bisect good completes refs"
let git_add_paths = do $completer [git add ""]
assert-eq (values $git_add_paths) [src/main.rs new-file.txt renamed.txt] "git add completes changed paths"
let git_rm_paths = do $completer [git rm ""]
assert-eq (values $git_rm_paths) [src/main.rs README.md] "git rm completes tracked paths"
"" | save --force $env.INSHELLAH_STATIC_FILE
let git_worktree_add = do $completer [git worktree add ""]
assert-eq $git_worktree_add null "git worktree add first argument falls back to files"
let git_worktree_remove = do $completer [git worktree remove ""]
assert-eq ($git_worktree_remove | get 0.value) "/repo/linked" "git worktree remove completes existing worktrees"

"[]" | save --force $env.INSHELLAH_STATIC_FILE
let jj_top = do $completer [jj ""]
assert-contains (values $jj_top) "bookmark" "jj top-level completes common commands"
assert-contains (values $jj_top) "git" "jj top-level includes git command"
let jj_bookmarks = do $completer [jj bookmark delete ""]
assert-eq (values $jj_bookmarks) [main feature origin/main] "jj bookmark delete completes bookmarks"
let jj_tags = do $completer [jj tag delete ""]
assert-eq (values $jj_tags) [v1.0 v2.0] "jj tag delete completes tags"
let jj_git_fetch = do $completer [jj git fetch ""]
assert-eq (values $jj_git_fetch) [origin upstream] "jj git fetch completes remotes"
let jj_git_remote_verbs = do $completer [jj git remote ""]
assert-eq (values $jj_git_remote_verbs) [add list remove rename set-url] "jj git remote completes subcommands"
let jj_git_remote_remove = do $completer [jj git remote remove ""]
assert-eq (values $jj_git_remote_remove) [origin upstream] "jj git remote remove completes remotes"
let jj_revs = do $completer [jj rebase -d ""]
assert-eq (values $jj_revs) [k m] "jj revision flags complete revisions"
let jj_ops = do $completer [jj op restore ""]
assert-eq (values $jj_ops) [abc123] "jj op restore completes operations"
let jj_files = do $completer [jj file show ""]
assert-eq (values $jj_files) [src/main.rs README.md] "jj file show completes repo files"
let jj_workspaces = do $completer [jj workspace forget ""]
assert-eq (values $jj_workspaces) [default linked] "jj workspace forget completes workspaces"
"" | save --force $env.INSHELLAH_STATIC_FILE
