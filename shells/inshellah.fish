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
    set -q now[1]; and printf '%s\n' $now[-1..1] | string match -v -- '*(__inshellah_complete)*'
end

function __inshellah_extras -a cmd
    set -l orig __inshellah_orig_(string escape --style=var -- $cmd)
    set -l kept $$orig
    set -l left (count $kept)
    set -l now (__inshellah_snapshot $cmd)
    if test (count $now) -ge $left; and begin; test $left -eq 0; or test "$now[1..$left]" = "$kept"; end
        set -q now[(math $left + 1)]; and printf '%s\n' $now[(math $left + 1)..-1]
        return
    end
    set -l at 1
    for line in $now
        if test $at -le $left; and test "$line" = "$kept[$at]"
            set at (math $at + 1)
        else
            printf '%s\n' $line
        end
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
# in the way, so a file in a dir ahead of the stub dir wins as it does when
# fish autoloads. fish 4 embeds its own completions and reaches them after
# every fish_complete_path dir, fish 3 ships them as files.
function __inshellah_source_native -a cmd
    for dir in $fish_complete_path
        test "$dir" = "$__inshellah_stub_dir"; and continue
        if not string match -q '*generated_completions' -- $dir; and test -f $dir/$cmd.fish
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

# a command without any completion gets the stub's. a second run for a loaded
# command puts the remembered completions back before applying takeover.
# a ctrl-c resume runs inside the load it cut short, which must not write afterwards.
# fish runs no handler for a second ctrl-c while the first resume is going, so a
# load that a SIGINT cut short stays pending for the next prompt to finish
function __inshellah_load -a cmd adopt
    set -g __inshellah_loading $cmd
    __inshellah_load_run $cmd $adopt
    set -e __inshellah_loading
end

function __inshellah_load_run -a cmd adopt
    set -q __inshellah_gen; or set -g __inshellah_gen 0
    set -q __inshellah_sigint; or set -g __inshellah_sigint 0
    set -l sigint $__inshellah_sigint
    set -l gen (math $__inshellah_gen + 1)
    set -g __inshellah_gen $gen
    set -l orig __inshellah_orig_(string escape --style=var -- $cmd)
    set -l pre __inshellah_pre_(string escape --style=var -- $cmd)
    if contains -- $cmd $__inshellah_loaded
        set -l extras (__inshellah_extras $cmd)
        complete -e -c $cmd
        printf '%s\n' $$orig $extras | source
        set -l at (contains -i -- $cmd $__inshellah_taken)
        set -q at[1]; and set -e __inshellah_taken[$at]
    else
        if contains -- $cmd $__inshellah_pending
            complete -e -c $cmd
            printf '%s\n' $$pre | source
        else
            set -ga __inshellah_pending $cmd
            set -g $pre (__inshellah_snapshot $cmd)
        end
        test "$adopt" = adopt; or __inshellah_source_native $cmd
        set -l sourced $status
        test $sourced -ne 130; or test $sigint -eq $__inshellah_sigint; or return
        test $__inshellah_gen = $gen; or return
        set -g $orig (__inshellah_snapshot $cmd)
    end
    if test (count $$orig) -gt 0
        __inshellah_apply $cmd
    else
        __inshellah_install $cmd
    end
    contains -- $cmd $__inshellah_loaded; or set -ga __inshellah_loaded $cmd
    set -l pending (contains -i -- $cmd $__inshellah_pending)
    set -q pending[1]; and set -e __inshellah_pending[$pending]
    set -e $pre
end

function __inshellah_path_stamp
    path mtime -- $PATH | string join ,
end

# a dir changed within the last second or two can change again without its
# mtime moving, so it is not recorded and the next prompt looks again
function __inshellah_path_mark
    set -g __inshellah_path_seen (__inshellah_path_stamp)
    for age in (path mtime --relative -- $PATH)
        test $age -lt 2; and set -g __inshellah_path_seen
    end
    true
end

# fish forgets a name it found no completion file for until its autoload cache
# expires, so new commands are loaded by hand. fish_complete_path is never
# assigned here, since any assignment makes fish drop everything it autoloaded
function __inshellah_refresh
    set -l out (command inshellah fish-stubs 2>/dev/null)
    __inshellah_path_mark
    set -q out[2]; or return
    for name in $out[2..-1]
        set -l have (complete -c $name)
        if set -q have[1]
            __inshellah_load $name adopt
        else
            __inshellah_load $name
        end
    end
end

function __inshellah_stubs --on-variable PATH
    __inshellah_refresh
end

# runs before each prompt, so it picks up a command installed into a dir that
# was already on PATH and finishes a load that ctrl-c cut short. a ctrl-c at the
# prompt or during these loads skips the next run, so pressing it never waits on one.
function __inshellah_prompt --on-event fish_prompt
    if set -q __inshellah_quiet
        set -e __inshellah_quiet
        return
    end
    set -g __inshellah_at_prompt
    set -l sigint $__inshellah_sigint
    for cmd in $__inshellah_pending
        test "$sigint" = "$__inshellah_sigint"; or break
        __inshellah_load $cmd
    end
    test "$sigint" = "$__inshellah_sigint"; and test "$(__inshellah_path_stamp)" != "$__inshellah_path_seen"; and __inshellah_refresh
    set -e __inshellah_at_prompt
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

set -l out (command inshellah fish-stubs 2>/dev/null)
and set -g __inshellah_stub_dir $out[1]
and __inshellah_path_mark
and __inshellah_place

function __inshellah_resume --on-signal SIGINT
    set -q __inshellah_sigint; or set -g __inshellah_sigint 0
    set -g __inshellah_sigint (math $__inshellah_sigint + 1)
    if set -q __inshellah_at_prompt; or not set -q __inshellah_loading
        set -g __inshellah_quiet
        return
    end
    for cmd in $__inshellah_pending
        __inshellah_load $cmd
    end
end
