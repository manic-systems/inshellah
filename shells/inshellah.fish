# inshellah completions for fish. load with
#   inshellah init fish | source

function __inshellah_complete
    set -l words (commandline -opc)
    set -l raw (commandline -ct)
    set -l current
    switch $raw
        case "'*"
            set current (string sub -s 2 -- $raw | string replace -r -- "^((?:[^\\\\]|\\\\.)*)'\$" '$1' | string replace -ra -- '\\\\([\\\\\'])' '$1')
        case '"*'
            set current (string sub -s 2 -- $raw | string replace -r -- '^((?:[^\\\\]|\\\\.)*)"$' '$1' | string replace -ra -- '\\\\(["\\\\$])' '$1')
        case '*'
            set current (string unescape -- $raw 2>/dev/null)
            or set current $raw
    end
    set -l reply (command inshellah complete --shell fish -- $words $current 2>/dev/null)
    or begin
        __fish_complete_path $raw
        return
    end
    set -q reply[1]; or set reply defer
    if test $reply[1] != defer
        printf '%s\n' $reply[2..-1]
    end
    if test $reply[1] != values
        __fish_complete_path $raw
    end
end

# fish 4 embeds its own completions and only reaches them after every
# fish_complete_path dir, so a stub loads them itself, and fish 3 ships them as
# files. a command without them may still have completions a config registered
# at runtime, as `jj util completion fish | source` does, which takeover drops.
function __inshellah_native -a cmd
    if test -n "$INSHELLAH_TAKEOVER"; and not builtin -q -- $cmd; and not functions -q -- $cmd
        complete -e -c $cmd
        return 1
    end
    set -l native (status get-file completions/$cmd.fish 2>/dev/null)
    and string join \n -- $native | source
    and return 0
    test -f $__fish_data_dir/completions/$cmd.fish
    and source $__fish_data_dir/completions/$cmd.fish
    and return 0
    test -z "$INSHELLAH_TAKEOVER"; and complete -c $cmd | string length -q
end

function __inshellah_stubs --on-variable PATH
    command inshellah fish-stubs >/dev/null 2>&1
end

# fish's own manpage-generated completions are the ones inshellah improves on,
# so fallback slots the stubs in just ahead of them
function __inshellah_place --on-variable INSHELLAH_TAKEOVER
    set -q __inshellah_stub_dir[1]; or return
    set -l rest
    for dir in $fish_complete_path
        test "$dir" = "$__inshellah_stub_dir"; or set -a rest $dir
    end
    if test -n "$INSHELLAH_TAKEOVER"
        set -g fish_complete_path $__inshellah_stub_dir $rest
        return
    end
    set -l before $rest
    set -l after
    set -l generated (string match -e generated_completions -- $rest)
    if set -q generated[1]
        set -l at (contains -i -- $generated[1] $rest)
        set before
        test $at -gt 1; and set before $rest[1..(math $at - 1)]
        set after $rest[$at..-1]
    end
    set -g fish_complete_path $before $__inshellah_stub_dir $after
end

set -g __inshellah_stub_dir (command inshellah fish-stubs 2>/dev/null)
and __inshellah_place
