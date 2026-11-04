# inshellah completions for fish. load with
#   inshellah init fish | source

function __inshellah_takeover
    string trim -- "$INSHELLAH_TAKEOVER" | string lower | string match -rq "^(1|true|yes|on)\$"
end

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
    or set reply defer
    set -q reply[1]; or set reply defer
    if test $reply[1] != defer
        printf '%s\n' $reply[2..-1]
    else if set -q words[1]; and contains -- $words[1] $__inshellah_taken
        set -l own (__inshellah_fish_own $words[1])
        if set -q own[1]
            printf '%s\n' $own
            return
        end
    end
    if test $reply[1] != values
        __fish_complete_path $raw
    end
end

# fish's own answer for a command takeover replaced, found by putting its
# completions back for the length of one lookup
function __inshellah_fish_own -a cmd
    set -l orig __inshellah_orig_(string escape --style=var -- $cmd)
    set -l extras (__inshellah_extras $cmd)
    complete -e -c $cmd
    printf '%s\n' $$orig $extras | source
    complete -C "$(commandline -cp)"
    complete -e -c $cmd
    __inshellah_install $cmd
    set -q extras[1]; and printf '%s\n' $extras | source
end

# fish lists a command's completions newest first, and -k cares about the
# order they were registered in
function __inshellah_snapshot -a cmd
    set -l now (complete -c $cmd)
    set -q now[1]; and printf '%s\n' $now[-1..1]
end

function __inshellah_extras -a cmd
    set -l orig __inshellah_orig_(string escape --style=var -- $cmd)
    for line in (__inshellah_snapshot $cmd)
        string match -q -- '*(__inshellah_complete)*' $line; and continue
        contains -- $line $$orig; or printf '%s\n' $line
    end
end

function __inshellah_install -a cmd
    complete -c $cmd -k -f -a '(__inshellah_complete)'
end

# a line that completes another command's arguments, as opposed to the git
# subcommands that merely run one
function __inshellah_wraps
    for line in $argv
        string match -rq -- '__fish_complete_\w*subcommand|__fish_complete_command' $line
        and not string match -rq -- 'using_command|seen_subcommand_from|number_of_cmd_args|\btest\b' $line
        and return 0
    end
    return 1
end

# takeover replaces a command's completions with the stub's, remembering them
# so turning takeover off puts them back. wrappers like sudo keep fish's own
# since they hand the wrapped command back to the completion engine.
function __inshellah_apply -a cmd
    set -l orig __inshellah_orig_(string escape --style=var -- $cmd)
    set -l at (contains -i -- $cmd $__inshellah_taken)
    if __inshellah_takeover
        and not set -q at[1]
        and test (count $$orig) -gt 0
        and not builtin -q -- $cmd
        and not functions -q -- $cmd
        and not __inshellah_wraps $$orig
        set -l extras (__inshellah_extras $cmd)
        complete -e -c $cmd
        __inshellah_install $cmd
        set -q extras[1]; and printf '%s\n' $extras | source
        set -ga __inshellah_taken $cmd
    else if not __inshellah_takeover; and set -q at[1]
        set -l extras (__inshellah_extras $cmd)
        complete -e -c $cmd
        printf '%s\n' $$orig $extras | source
        set -e __inshellah_taken[$at]
    end
end

# the first completion file fish would have loaded had the stub dir not been
# in the way. fish 4 embeds its own completions and reaches them after every
# fish_complete_path dir, fish 3 ships them as files.
function __inshellah_source_native -a cmd
    set -l seen false
    for dir in $fish_complete_path
        if test "$dir" = "$__inshellah_stub_dir"
            set seen true
        else if $seen; and not string match -q '*generated_completions' -- $dir; and test -f $dir/$cmd.fish
            source $dir/$cmd.fish
            return
        end
    end
    set -l native (status get-file completions/$cmd.fish 2>/dev/null)
    if set -q native[1]
        string join \n -- $native | source
    else if test -f $__fish_data_dir/completions/$cmd.fish
        source $__fish_data_dir/completions/$cmd.fish
    end
end

# a command without any completion gets the stub's. changing fish_complete_path
# makes fish forget what every stub registered, and the stub then runs again,
# so a second run puts the remembered completions back before applying takeover.
function __inshellah_load -a cmd
    set -l orig __inshellah_orig_(string escape --style=var -- $cmd)
    if contains -- $cmd $__inshellah_loaded
        set -l extras (__inshellah_extras $cmd)
        complete -e -c $cmd
        printf '%s\n' $$orig $extras | source
        set -l at (contains -i -- $cmd $__inshellah_taken)
        set -q at[1]; and set -e __inshellah_taken[$at]
    else
        set -ga __inshellah_loaded $cmd
        __inshellah_source_native $cmd
        set -g $orig (__inshellah_snapshot $cmd)
    end
    if test (count $$orig) -gt 0
        __inshellah_apply $cmd
    else
        __inshellah_install $cmd
    end
end

function __inshellah_stubs --on-variable PATH
    command inshellah fish-stubs >/dev/null 2>&1
end

function __inshellah_retake --on-variable INSHELLAH_TAKEOVER
    for cmd in $__inshellah_loaded
        __inshellah_apply $cmd
    end
end

# the stubs go ahead of everything shipped or vendored, which they replace,
# but behind dirs the user put first, so a user's own completion file still wins
function __inshellah_place
    set -l rest
    for dir in $fish_complete_path
        test "$dir" = "$__inshellah_stub_dir"; or set -a rest $dir
    end
    set -l before $rest
    set -l after
    for at in (seq (count $rest))
        if string match -rq -- '/vendor_completions\.d$|/generated_completions$' $rest[$at]
            or test "$rest[$at]" = "$__fish_data_dir/completions"
            test $at -gt 1; and set before $rest[1..(math $at - 1)]
            test $at -eq 1; and set before
            set after $rest[$at..-1]
            break
        end
    end
    set -l path $before $__inshellah_stub_dir $after
    test "$path" = "$fish_complete_path"; or set -g fish_complete_path $path
end

set -g __inshellah_stub_dir (command inshellah fish-stubs 2>/dev/null)
and __inshellah_place
