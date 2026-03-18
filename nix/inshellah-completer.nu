@complete external
def --wrapped sudo [...args] {
    ^sudo ...$args
}

@complete external
def --wrapped doas [...args] {
    ^doas ...$args
}

let inshellah_nonempty = { |items|
    let result = ($items | default [] | compact)
    if ($result | is-empty) { null } else { $result }
}

let inshellah_fuzzy_score = { |needle, haystack|
    let needle = $needle | default "" | into string
    let haystack = $haystack | default "" | into string
    let needle_len = ($needle | str length)
    let haystack_len = ($haystack | str length)

    if $needle_len == 0 {
        1
    } else if $needle_len > $haystack_len {
        0
    } else if $needle == $haystack {
        1000
    } else {
        let needle_lc = $needle | str downcase
        let haystack_lc = $haystack | str downcase
        if ($haystack_lc | str starts-with $needle_lc) {
            900 + (($needle_len * 100) // $haystack_len)
        } else {
            let needle_chars = $needle_lc | split chars
            let haystack_chars = $haystack | split chars
            let haystack_lc_chars = $haystack_lc | split chars
            let scored = (
                $haystack_lc_chars
                | enumerate
                | reduce --fold {needle_idx: 0, score: 0, prev_match: -2} { |it, acc|
                    if $acc.needle_idx >= $needle_len {
                        $acc
                    } else if $it.item == ($needle_chars | get $acc.needle_idx) {
                        let idx = $it.index
                        let prev = if $idx == 0 { "" } else { $haystack_chars | get ($idx - 1) }
                        let current = $haystack_chars | get $idx
                        let boundary = (
                            ($idx == 0)
                            or ($prev == "-")
                            or ($prev == "_")
                            or (($prev =~ '^[a-z]$') and ($current =~ '^[A-Z]$'))
                        )
                        let base = if $boundary { 50 } else { 10 }
                        let consecutive = if $acc.prev_match == ($idx - 1) { 20 } else { 0 }
                        {
                            needle_idx: ($acc.needle_idx + 1)
                            score: ($acc.score + $base + $consecutive)
                            prev_match: $idx
                        }
                    } else {
                        $acc
                    }
                }
            )
            if $scored.needle_idx == $needle_len { $scored.score } else { 0 }
        }
    }
}

let inshellah_filter_candidates = { |items, prefix|
    let result = do $inshellah_nonempty $items
    if $result == null {
        null
    } else if ($prefix | is-empty) {
        $result
    } else {
        let needle = $prefix | into string
        let filtered = (
            $result
            | enumerate
            | each { |row| $row.item | insert __idx $row.index }
            | insert __score { |item| do $inshellah_fuzzy_score $needle $item.value }
            | where { |item|
                let value = ($item.value | into string)
                let desc = ($item.description? | default "" | into string | str downcase)
                let exact_command = ($value == $needle) and (($desc | str contains "subcommand") or $desc == "external command")
                ($item.__score > 0) and not $exact_command
            }
            | insert __rank { |item| 0 - $item.__score }
            | sort-by __rank __idx
            | reject __idx __score __rank
        )
        do $inshellah_nonempty $filtered
    }
}

let inshellah_static_complete = { |spans|
    try {
        let completed = (^inshellah complete ...$spans | complete)
        if $completed.exit_code != 0 {
            null
        } else {
            let parsed = (try { $completed.stdout | from json } catch { null })
            let parsed_type = ($parsed | describe)
            if $parsed == null {
                null
            } else if (($parsed_type | str starts-with "list") or ($parsed_type | str starts-with "table")) {
                do $inshellah_nonempty $parsed
            } else {
                null
            }
        }
    } catch {
        null
    }
}

let inshellah_unit_candidates = { |scope, prefix|
    try {
        ^systemctl ...$scope list-units --all --no-pager --plain --full --no-legend $"($prefix)*"
            | lines
            | each { |l|
                let parsed = $l | parse -r '(?P<unit>\S+)\s+\S+\s+\S+\s+\S+\s+(?P<desc>.*)'
                if ($parsed | length) > 0 {
                    {value: $parsed.0.unit, description: ($parsed.0.desc | str trim)}
                }
            } | compact
    } catch { null }
}

let inshellah_kubectl_scope = { |spans|
    let all_namespaces = ("-A" in $spans) or ("--all-namespaces" in $spans)
    let namespace_eq = ($spans | where { |s| $s =~ '^--namespace=' } | get 0? | default "")
    let namespace_arg = (
        $spans
        | enumerate
        | where { |it| $it.item == "-n" or $it.item == "--namespace" }
        | reverse
        | get 0?
        | default null
    )
    let namespace = if not ($namespace_eq | is-empty) {
        $namespace_eq | str replace --regex '^--namespace=' ''
    } else if $namespace_arg != null and (($namespace_arg.index + 1) < ($spans | length)) {
        $spans | get ($namespace_arg.index + 1)
    } else {
        ""
    }

    if $all_namespaces {
        {args: [--all-namespaces], all: true}
    } else if not ($namespace | is-empty) {
        {args: [-n $namespace], all: false}
    } else {
        {args: [], all: false}
    }
}

let inshellah_kubectl_names = { |kind, spans|
    if ($kind | is-empty) or ($kind | str starts-with "-") {
        null
    } else {
        let scope = do $inshellah_kubectl_scope $spans
        let columns = if $scope.all {
            "custom-columns=NAMESPACE:.metadata.namespace,NAME:.metadata.name"
        } else {
            "custom-columns=NAME:.metadata.name"
        }
        try {
            let rows = (
                ^kubectl get $kind ...$scope.args --no-headers -o $columns
                    | lines
                    | str trim
                    | where { |n| not ($n | is-empty) }
            )
            if $scope.all {
                $rows | each { |row|
                    let parts = $row | split row -r '\s+'
                    if ($parts | length) >= 2 {
                        {value: ($parts | get 1), description: $"($kind) in ($parts | get 0)"}
                    }
                } | compact
            } else {
                $rows | each { |n| {value: $n, description: $kind} }
            }
        } catch { null }
    }
}

let inshellah_git_refs = { ||
    try {
        ^git for-each-ref --format='%(refname:short)%09%(objecttype)%09%(contents:subject)' refs/heads refs/remotes refs/tags
            | lines
            | each { |l|
                let p = $l | split row "\t"
                if ($p | length) >= 3 { {value: $p.0, description: $p.2} }
            } | compact
    } catch { null }
}

let inshellah_git_branches = { ||
    try {
        ^git for-each-ref --format='%(refname:short)%09%(contents:subject)' refs/heads
            | lines
            | each { |l|
                let p = $l | split row "\t"
                if ($p | length) >= 2 { {value: $p.0, description: $p.1} }
            } | compact
    } catch { null }
}

let inshellah_git_tags = { ||
    try {
        ^git for-each-ref --format='%(refname:short)%09%(contents:subject)' refs/tags
            | lines
            | each { |l|
                let p = $l | split row "\t"
                if ($p | length) >= 2 { {value: $p.0, description: $p.1} }
            } | compact
    } catch { null }
}

let inshellah_git_remotes = { ||
    try {
        ^git remote
            | lines
            | str trim
            | where { |r| not ($r | is-empty) }
            | each { |r| {value: $r, description: "remote"} }
    } catch { null }
}

let inshellah_git_stashes = { ||
    try {
        ^git stash list
            | lines
            | each { |l|
                let m = $l | parse -r '^(?P<stash>stash@\{[0-9]+\}):\s*(?P<desc>.*)$'
                if ($m | length) > 0 { {value: $m.0.stash, description: $m.0.desc} }
            } | compact
    } catch { null }
}

let inshellah_git_status_paths = { ||
    try {
        ^git status --porcelain -uall
            | lines
            | each { |l|
                let m = $l | parse -r '^.. (?P<path>.+)$'
                if ($m | length) > 0 {
                    let raw = $m.0.path
                    let path = if ($raw | str contains " -> ") { $raw | split row " -> " | last } else { $raw }
                    {value: $path, description: "changed path"}
                }
            } | compact
    } catch { null }
}

let inshellah_git_tracked_paths = { ||
    try {
        ^git ls-files
            | lines
            | where { |p| not ($p | is-empty) }
            | each { |p| {value: $p, description: "tracked file"} }
    } catch { null }
}

let inshellah_git_submodules = { ||
    try {
        ^git config --file .gitmodules --get-regexp '^submodule\..*\.path$'
            | lines
            | each { |l|
                let p = $l | split row -r '\s+'
                if ($p | length) >= 2 { {value: $p.1, description: "submodule"} }
            } | compact
    } catch { null }
}

let inshellah_git_worktrees = { ||
    try {
        ^git worktree list --porcelain
            | lines
            | each { |l|
                let m = $l | parse -r '^worktree\s+(?P<p>.+)$'
                if ($m | length) > 0 { {value: $m.0.p, description: ""} }
            } | compact
    } catch { null }
}

let inshellah_jj_revs = { ||
    try {
        ^jj log --ignore-working-copy --no-graph -r 'all()' -T 'change_id.shortest() ++ "\t" ++ description.first_line() ++ "\n"' err> /dev/null
            | lines
            | each { |l|
                let p = $l | split row "\t"
                if ($p | length) >= 2 { {value: $p.0, description: $p.1} }
            } | compact
    } catch { null }
}

let inshellah_jj_bookmarks = { ||
    try {
        ^jj bookmark list --all-remotes -T 'name ++ "\n"' err> /dev/null
            | lines
            | str trim
            | where { |b| not ($b | is-empty) }
            | each { |b| {value: $b, description: "bookmark"} }
    } catch { null }
}

let inshellah_jj_tags = { ||
    try {
        ^jj tag list --all-remotes -T 'name ++ "\n"' err> /dev/null
            | lines
            | str trim
            | where { |t| not ($t | is-empty) }
            | each { |t| {value: $t, description: "tag"} }
    } catch { null }
}

let inshellah_jj_remotes = { ||
    try {
        ^jj git remote list err> /dev/null
            | lines
            | each { |l|
                let p = $l | str trim | split row -r '\s+'
                if ($p | length) >= 1 { {value: $p.0, description: ($p | get 1? | default "remote")} }
            } | compact
    } catch { null }
}

let inshellah_jj_ops = { ||
    try {
        ^jj op log --ignore-working-copy --no-graph -T 'id.short() ++ "\t" ++ description.first_line() ++ "\n"' err> /dev/null
            | lines
            | each { |l|
                let p = $l | split row "\t"
                if ($p | length) >= 2 { {value: $p.0, description: $p.1} }
            } | compact
    } catch { null }
}

let inshellah_jj_files = { ||
    try {
        ^jj file list --ignore-working-copy err> /dev/null
            | lines
            | str trim
            | where { |p| not ($p | is-empty) }
            | each { |p| {value: $p, description: "repo file"} }
    } catch { null }
}

let inshellah_jj_workspaces = { ||
    try {
        ^jj workspace list -T 'name ++ "\n"' err> /dev/null
            | lines
            | str trim
            | where { |w| not ($w | is-empty) }
            | each { |w| {value: $w, description: "workspace"} }
    } catch { null }
}

let inshellah_complete = { |spans|
    let completions = do $inshellah_static_complete $spans
    let span_len = ($spans | length)
    let last_span = if $span_len > 0 { $spans | last } else { "" }
    let prev_span = if $span_len >= 2 { $spans | get ($span_len - 2) } else { "" }
    let sub = if $span_len >= 2 { $spans | get 1 } else { "" }

    let additional = if ($completions == null and $span_len > 0) {
        match $spans.0 {
            "nix" => {
                if $span_len < 2 {
                    null
                } else {
                    try {
                        let nix_output = (
                            with-env { NIX_GET_COMPLETIONS: ($span_len - 1) } {
                                $spans | run-external $in
                            }
                            | split row -r '\n'
                            | str trim
                            | skip 1
                            | where { |e| not ($e | is-empty) }
                        )
                        if (($nix_output | length) < 6 and
                            $last_span =~ "[a-zA-Z][a-zA-Z0-9_-]*#[a-zA-Z][a-zA-Z0-9_-]*") {
                            with-env { NIX_ALLOW_UNFREE: "1" NIX_ALLOW_BROKEN: "1" } {
                                $nix_output | par-each { |e|
                                    try {
                                        {value: $e, description: (^nix eval --raw --impure $e --apply "f: f.meta.description" err> /dev/null)}
                                    } catch {
                                        {value: $e, description: ""}
                                    }
                                }
                            }
                        } else {
                            $nix_output | each { |e| {value: $e, description: ""} }
                        }
                    } catch { null }
                }
            }
            "systemctl" => {
                let unit_verbs = [
                    "status" "show" "cat" "help" "start" "stop" "restart" "reload" "try-restart"
                    "reload-or-restart" "reload-or-try-restart" "isolate" "kill" "reset-failed"
                    "enable" "disable" "reenable" "preset" "mask" "unmask" "is-active" "is-failed"
                    "is-enabled" "edit"
                ]
                let args = $spans | skip 1 | where { |s| not ($s | str starts-with "-") }
                let verb = $args | get 0? | default ""
                if (($verb in $unit_verbs) and $span_len >= 3) {
                    let scope = if ("--user" in $spans) { [--user] } else { [] }
                    do $inshellah_unit_candidates $scope $last_span
                } else { null }
            }
            "journalctl" => {
                if ($prev_span == "--unit" or $prev_span == "-u") {
                    let scope = if ("--user-unit" in $spans or "--user" in $spans) { [--user] } else { [] }
                    do $inshellah_unit_candidates $scope $last_span
                } else { null }
            }
            "coredumpctl" => {
                let unit_verbs = ["dump" "info" "debug" "list"]
                if (($sub in $unit_verbs) and $span_len >= 3) {
                    let units = (do $inshellah_unit_candidates [] $last_span | default [])
                    let pids = (try {
                        ^coredumpctl list --no-pager --no-legend
                            | lines
                            | each { |l|
                                let p = $l | split row -r '\s+'
                                if ($p | length) >= 5 { {value: $p.4, description: $"PID ($p.4) ($p | get 9? | default "")"} }
                            } | compact
                    } catch { [] })
                    $units | append $pids
                } else { null }
            }
            "loginctl" => {
                let user_verbs = ["user-status" "show-user" "enable-linger" "disable-linger" "kill-user" "terminate-user"]
                let session_verbs = ["session-status" "show-session" "activate" "lock-session" "unlock-session" "terminate-session" "kill-session"]
                if (($sub in $user_verbs) and $span_len >= 3) {
                    try {
                        ^loginctl list-users --no-pager --no-legend
                            | lines | each { |l|
                                let p = $l | str trim | split row -r '\s+'
                                if ($p | length) >= 2 { {value: $p.1, description: $"UID ($p.0)"} }
                            } | compact
                    } catch { null }
                } else if (($sub in $session_verbs) and $span_len >= 3) {
                    try {
                        ^loginctl list-sessions --no-pager --no-legend
                            | lines | each { |l|
                                let p = $l | str trim | split row -r '\s+'
                                if ($p | length) >= 3 { {value: $p.0, description: $"user ($p.2)"} }
                            } | compact
                    } catch { null }
                } else { null }
            }
            "machinectl" => {
                let machine_verbs = ["status" "show" "start" "login" "shell" "enable" "disable" "poweroff" "reboot" "terminate" "kill" "bind" "copy-to" "copy-from"]
                if (($sub in $machine_verbs) and $span_len >= 3) {
                    try {
                        ^machinectl list --no-pager --no-legend
                            | lines | each { |l|
                                let p = $l | str trim | split row -r '\s+'
                                if ($p | length) >= 1 { {value: $p.0, description: ($p | get 1? | default "")} }
                            } | compact
                    } catch { null }
                } else { null }
            }
            "networkctl" => {
                let link_verbs = ["status" "show" "up" "down" "renew" "forcerenew" "reconfigure" "delete"]
                if (($sub in $link_verbs) and $span_len >= 3) {
                    try {
                        ^networkctl list --no-pager --no-legend
                            | lines | each { |l|
                                let p = $l | str trim | split row -r '\s+'
                                if ($p | length) >= 4 { {value: $p.1, description: $"($p.2) ($p.3)"} }
                            } | compact
                    } catch { null }
                } else { null }
            }
            "hostnamectl" | "timedatectl" | "localectl" => {
                null
            }
            "ssh" | "scp" | "sftp" => {
                let cfg_hosts = (try {
                    open ~/.ssh/config | lines | each { |l|
                        let m = $l | parse -r '(?i)^\s*Host\s+(?P<h>.+)$'
                        if ($m | length) > 0 { $m.0.h | split row -r '\s+' } else { [] }
                    } | flatten | where { |h| not ($h | str contains '*') and not ($h | is-empty) }
                } catch { [] })
                let known = (try {
                    open ~/.ssh/known_hosts | lines | each { |l|
                        ($l | split row -r '\s+' | get 0? | default "") | split row ','
                    } | flatten | where { |h| (not ($h | is-empty)) and (not ($h | str starts-with '|')) and (not ($h | str starts-with '[')) }
                } catch { [] })
                $cfg_hosts | append $known | uniq | each { |h| {value: $h, description: ""} }
            }
            "docker" | "podman" => {
                let need_container = ["exec" "logs" "inspect" "start" "stop" "restart" "rm" "kill" "attach" "cp" "top" "wait" "pause" "unpause" "port" "commit" "diff" "export"]
                let need_image = ["run" "rmi" "tag" "push" "pull" "history" "save" "create"]
                if ($sub in $need_container) {
                    try {
                        ^($spans.0) ps -a --format '{{.Names}}\t{{.Image}}'
                            | lines | each { |l|
                                let p = $l | split row "\t"
                                if ($p | length) >= 2 { {value: $p.0, description: $p.1} }
                            } | compact
                    } catch { null }
                } else if ($sub in $need_image) {
                    try {
                        ^($spans.0) images --format '{{.Repository}}:{{.Tag}}\t{{.Size}}'
                            | lines | each { |l|
                                let p = $l | split row "\t"
                                if (($p | length) >= 2) and (not ($p.0 | str ends-with ':<none>')) {
                                    {value: $p.0, description: $p.1}
                                }
                            } | compact
                    } catch { null }
                } else { null }
            }
            "kubectl" => {
                let resource_verbs = ["get" "describe" "delete" "edit" "scale" "annotate" "label"]
                if (($sub in $resource_verbs) and $span_len >= 4) {
                    let kind = $spans | get 2? | default ""
                    do $inshellah_kubectl_names $kind $spans
                } else if (($sub == "logs" or $sub == "exec" or $sub == "port-forward") and $span_len >= 3) {
                    do $inshellah_kubectl_names "pods" $spans
                } else if ($sub == "rollout" and $span_len >= 5) {
                    let action = $spans | get 2? | default ""
                    let kind = $spans | get 3? | default ""
                    if ($action in ["history" "pause" "restart" "resume" "status" "undo"]) {
                        do $inshellah_kubectl_names $kind $spans
                    } else { null }
                } else { null }
            }
            "git" => {
                let git_verbs = [
                    "add" "bisect" "branch" "checkout" "cherry-pick" "clone" "commit" "diff"
                    "fetch" "grep" "init" "log" "merge" "mv" "pull" "push" "rebase" "reflog"
                    "remote" "reset" "restore" "revert" "rm" "show" "stash" "status"
                    "submodule" "switch" "tag" "worktree"
                ]
                let ref_verbs = ["checkout" "merge" "rebase" "log" "diff" "show" "reset" "cherry-pick" "revert" "tag" "blame" "bisect"]
                let branch_verbs = ["switch" "branch"]
                let remote_verbs = ["add" "rename" "remove" "rm" "set-head" "set-branches" "get-url" "set-url" "show" "prune" "update"]
                let stash_verbs = ["push" "save" "list" "show" "drop" "pop" "apply" "branch" "clear" "create" "store"]
                let submodule_verbs = ["add" "status" "init" "deinit" "update" "set-branch" "set-url" "summary" "foreach" "sync" "absorbgitdirs"]
                let bisect_verbs = ["start" "bad" "good" "new" "old" "terms" "skip" "next" "reset" "visualize" "view" "replay" "log" "run"]
                let git_args = $spans | skip 2 | where { |s| not ($s | is-empty) and not ($s | str starts-with "-") }
                if $span_len <= 2 {
                    $git_verbs | each { |v| {value: $v, description: "git subcommand"} }
                } else if ($sub == "worktree") {
                    let worktree_verb = $spans | get 2? | default ""
                    if $span_len <= 3 {
                        ["add" "list" "lock" "move" "prune" "remove" "repair" "unlock"] | each { |v| {value: $v, description: "worktree subcommand"} }
                    } else if ($worktree_verb in ["remove" "move" "lock" "unlock" "repair"]) {
                        do $inshellah_git_worktrees
                    } else if ($worktree_verb == "add" and $span_len >= 5) {
                        do $inshellah_git_refs
                    } else { null }
                } else if ($sub == "remote" and $span_len >= 3) {
                    let remote_verb = $spans | get 2? | default ""
                    if $span_len <= 3 {
                        $remote_verbs | each { |v| {value: $v, description: "remote subcommand"} }
                    } else if ($remote_verb in ["rename" "remove" "rm" "set-head" "set-branches" "get-url" "set-url" "show" "prune" "update"]) {
                        do $inshellah_git_remotes
                    } else { null }
                } else if (($sub in ["fetch" "push" "pull"]) and $span_len >= 3) {
                    if ($git_args | is-empty) {
                        do $inshellah_git_remotes
                    } else {
                        do $inshellah_git_refs
                    }
                } else if ($sub == "stash" and $span_len >= 3) {
                    let stash_verb = $spans | get 2? | default ""
                    if $span_len <= 3 {
                        $stash_verbs | each { |v| {value: $v, description: "stash subcommand"} }
                    } else if ($stash_verb in ["show" "drop" "pop" "apply" "store"]) {
                        do $inshellah_git_stashes
                    } else if ($stash_verb == "branch" and ($git_args | length) >= 2) {
                        do $inshellah_git_stashes
                    } else { null }
                } else if ($sub == "submodule" and $span_len >= 3) {
                    let submodule_verb = $spans | get 2? | default ""
                    if $span_len <= 3 {
                        $submodule_verbs | each { |v| {value: $v, description: "submodule subcommand"} }
                    } else if ($submodule_verb in ["status" "init" "deinit" "update" "set-branch" "set-url" "summary" "foreach" "sync"]) {
                        do $inshellah_git_submodules
                    } else { null }
                } else if ($sub == "bisect" and $span_len >= 3) {
                    let bisect_verb = $spans | get 2? | default ""
                    if $span_len <= 3 {
                        $bisect_verbs | each { |v| {value: $v, description: "bisect subcommand"} }
                    } else if ($bisect_verb in ["bad" "good" "new" "old" "skip" "reset" "start"]) {
                        do $inshellah_git_refs
                    } else { null }
                } else if ($sub == "tag" and $span_len >= 3) {
                    if (["-d" "--delete" "-v" "--verify"] | any { |f| $f in $spans }) {
                        do $inshellah_git_tags
                    } else if ($span_len >= 4) {
                        do $inshellah_git_refs
                    } else {
                        do $inshellah_git_tags
                    }
                } else if ($sub == "add" and $span_len >= 3) {
                    do $inshellah_git_status_paths
                } else if ($sub == "restore" and $span_len >= 3) {
                    if ($prev_span == "--source" or $prev_span == "-s") {
                        do $inshellah_git_refs
                    } else {
                        do $inshellah_git_status_paths
                    }
                } else if ($sub == "rm" and $span_len >= 3) {
                    do $inshellah_git_tracked_paths
                } else if ($sub == "mv" and $span_len >= 3) {
                    if ($git_args | is-empty) { do $inshellah_git_tracked_paths } else { null }
                } else if ($sub == "checkout" and $span_len >= 3) {
                    if ($prev_span in ["-b" "-B" "--orphan"]) { null } else { do $inshellah_git_refs }
                } else if ($sub == "switch" and $span_len >= 3) {
                    if ($prev_span in ["-c" "-C" "--create" "--force-create" "--orphan"]) { null } else { do $inshellah_git_branches }
                } else if (($sub in $branch_verbs) and $span_len >= 3) {
                    do $inshellah_git_branches
                } else if (($sub in $ref_verbs) and $span_len >= 3) {
                    do $inshellah_git_refs
                } else { null }
            }
            "jj" => {
                let jj_verbs = [
                    "abandon" "absorb" "bookmark" "commit" "describe" "diff" "diffedit"
                    "duplicate" "edit" "evolog" "file" "git" "interdiff" "log" "new"
                    "operation" "op" "rebase" "resolve" "restore" "revert" "show" "sparse"
                    "split" "squash" "status" "tag" "undo" "workspace" "b" "ci" "desc" "st"
                ]
                let rev_flags = [
                    "-r" "--revision" "--revisions" "--from" "--to" "-s" "--source"
                    "-d" "--destination" "--insert-after" "--insert-before" "--before"
                    "--after" "--onto" "--change"
                ]
                let rev_verbs = [
                    "abandon" "absorb" "describe" "diff" "diffedit" "duplicate" "edit"
                    "evolog" "interdiff" "log" "metaedit" "new" "parallelize" "rebase"
                    "restore" "revert" "show" "sign" "simplify-parents" "split" "squash"
                    "unsign"
                ]
                let bookmark_verbs = ["advance" "create" "delete" "forget" "list" "move" "rename" "set" "track" "untrack"]
                let jj_git_verbs = ["clone" "colocation" "export" "fetch" "import" "init" "push" "remote" "root"]
                let jj_remote_verbs = ["add" "list" "remove" "rename" "set-url"]
                let op_verbs = ["abandon" "diff" "integrate" "log" "restore" "revert" "show"]
                let file_verbs = ["annotate" "chmod" "list" "search" "show" "track" "untrack"]
                let workspace_verbs = ["add" "forget" "list" "rename" "root" "update-stale"]
                let sparse_verbs = ["edit" "list" "reset" "set"]
                let jj_args = $spans | skip 2 | where { |s| not ($s | is-empty) and not ($s | str starts-with "-") }
                if ($prev_span in $rev_flags) {
                    do $inshellah_jj_revs
                } else if ($prev_span == "--remote") {
                    do $inshellah_jj_remotes
                } else if ($prev_span == "--at-operation" or $prev_span == "--at-op") {
                    do $inshellah_jj_ops
                } else if $span_len <= 2 {
                    $jj_verbs | each { |v| {value: $v, description: "jj subcommand"} }
                } else if ($sub == "bookmark" or $sub == "b") {
                    let verb = $spans | get 2? | default ""
                    if $span_len <= 3 {
                        $bookmark_verbs | each { |v| {value: $v, description: "bookmark subcommand"} }
                    } else if ($verb in ["delete" "forget" "move" "rename" "set" "track" "untrack" "advance"]) {
                        do $inshellah_jj_bookmarks
                    } else { null }
                } else if ($sub == "tag") {
                    let verb = $spans | get 2? | default ""
                    if $span_len <= 3 {
                        ["delete" "list" "set"] | each { |v| {value: $v, description: "tag subcommand"} }
                    } else if ($verb in ["delete" "set"]) {
                        do $inshellah_jj_tags
                    } else { null }
                } else if ($sub == "git") {
                    let git_verb = $spans | get 2? | default ""
                    let remote_verb = $spans | get 3? | default ""
                    if $span_len <= 3 {
                        $jj_git_verbs | each { |v| {value: $v, description: "jj git subcommand"} }
                    } else if ($git_verb == "remote") {
                        if $span_len <= 4 {
                            $jj_remote_verbs | each { |v| {value: $v, description: "remote subcommand"} }
                        } else if ($remote_verb in ["remove" "rename" "set-url"]) {
                            do $inshellah_jj_remotes
                        } else { null }
                    } else if ($git_verb in ["fetch" "push"]) {
                        do $inshellah_jj_remotes
                    } else { null }
                } else if ($sub == "operation" or $sub == "op") {
                    let verb = $spans | get 2? | default ""
                    if $span_len <= 3 {
                        $op_verbs | each { |v| {value: $v, description: "operation subcommand"} }
                    } else if ($verb in ["abandon" "diff" "integrate" "restore" "revert" "show"]) {
                        do $inshellah_jj_ops
                    } else { null }
                } else if ($sub == "file") {
                    let verb = $spans | get 2? | default ""
                    if $span_len <= 3 {
                        $file_verbs | each { |v| {value: $v, description: "file subcommand"} }
                    } else if ($verb in ["annotate" "chmod" "list" "search" "show" "untrack"]) {
                        do $inshellah_jj_files
                    } else { null }
                } else if ($sub == "workspace") {
                    let verb = $spans | get 2? | default ""
                    if $span_len <= 3 {
                        $workspace_verbs | each { |v| {value: $v, description: "workspace subcommand"} }
                    } else if ($verb in ["forget" "update-stale"]) {
                        do $inshellah_jj_workspaces
                    } else { null }
                } else if ($sub == "sparse") {
                    if $span_len <= 3 {
                        $sparse_verbs | each { |v| {value: $v, description: "sparse subcommand"} }
                    } else { null }
                } else if ($sub in ["diff" "log"] and ($jj_args | is-empty)) {
                    do $inshellah_jj_files
                } else if ($sub in $rev_verbs and $span_len >= 3) {
                    do $inshellah_jj_revs
                } else { null }
            }
            "npm" | "pnpm" | "yarn" => {
                let wants = (
                    (($spans.0 == "yarn") and $span_len == 2)
                    or (($sub == "run" or $sub == "run-script") and $span_len == 3)
                )
                if $wants {
                    try {
                        open package.json | get scripts? | default {} | transpose name cmd
                            | each { |row| {value: $row.name, description: $row.cmd} }
                    } catch { null }
                } else { null }
            }
            "make" => {
                if $span_len <= 2 {
                    try {
                        open Makefile | lines
                            | each { |l|
                                let m = $l | parse -r '^(?P<t>[A-Za-z0-9_./-]+)\s*:'
                                if (($m | length) > 0) and (not ($m.0.t | str starts-with '.')) {
                                    {value: $m.0.t, description: ""}
                                }
                            } | compact | uniq-by value
                    } catch { null }
                } else { null }
            }
            "just" => {
                if $span_len <= 2 {
                    try {
                        ^just --list --unsorted
                            | lines | skip 1
                            | each { |l|
                                let m = $l | parse -r '^\s+(?P<t>[A-Za-z0-9_-]+)(?:\s+\S.*)?(?:\s*#\s*(?P<d>.*))?$'
                                if ($m | length) > 0 {
                                    {value: $m.0.t, description: ($m.0.d? | default "")}
                                }
                            } | compact
                    } catch { null }
                } else { null }
            }
            "cargo" => {
                let target_flags = ["--bin" "--example" "--test" "--bench"]
                if ($prev_span == "-p" or $prev_span == "--package") {
                    try {
                        ^cargo metadata --no-deps --format-version 1
                            | from json
                            | get packages
                            | each { |pkg| {value: $pkg.name, description: ($pkg.version? | default "")} }
                            | uniq-by value
                    } catch { null }
                } else if ($prev_span in $target_flags) {
                    let kind = $prev_span | str replace "--" ""
                    try {
                        ^cargo metadata --no-deps --format-version 1
                            | from json
                            | get packages
                            | each { |pkg|
                                $pkg.targets
                                    | where { |t| $kind in $t.kind }
                                    | each { |t| {value: $t.name, description: ($t.kind | str join ",")} }
                            }
                            | flatten
                            | uniq-by value
                    } catch { null }
                } else { null }
            }
            "kill" | "pkill" => {
                try {
                    ^ps -eo pid,comm --no-headers
                        | lines
                        | each { |l|
                            let parts = $l | str trim | split row -r '\s+'
                            if ($parts | length) >= 2 {
                                let pid = $parts | get 0
                                let comm = $parts | skip 1 | str join " "
                                if ($spans.0 == "kill") { {value: $pid, description: $comm} }
                                else { {value: $comm, description: $pid} }
                            }
                        } | compact
                } catch { null }
            }
            _ => { null }
        }
    } else { null }

    if $completions == null {
        do $inshellah_filter_candidates $additional $last_span
    } else {
        $completions
    }
}

$env.config.completions.external = {enable: true, max_results: 200, completer: $inshellah_complete}
