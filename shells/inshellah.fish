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
    set -q now[1]; and printf '%s\n' $now[-1..1] | string match -v -r -- '\(__inshellah_(complete\)|retry )'
end

function __inshellah_extras -a cmd
    set -l orig __inshellah_orig_(string escape --style=var -- $cmd)
    __inshellah_after $cmd $$orig
end

# what a command has registered beyond the given lines, which must be in the
# order it registered them
function __inshellah_after -a cmd
    set -l kept $argv[2..-1]
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
        and not contains -- $cmd $__inshellah_user
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

# whether a completion file sits in a dir ahead of the stub dir. fish
# autoloads such a file at the first tab, and sourcing it by hand as well
# would run it twice.
function __inshellah_user_file -a cmd
    for dir in $fish_complete_path
        test "$dir" = "$__inshellah_stub_dir"; and return 1
        not string match -q '*generated_completions' -- $dir; and test -f $dir/$cmd.fish; and return 0
    end
    return 1
end

# the first completion file fish would have loaded had the stub dir not been
# in the way. fish 4 embeds its own completions and reaches them after every
# fish_complete_path dir, fish 3 ships them as files.
function __inshellah_source_native -a cmd
    set -l behind false
    for dir in $fish_complete_path
        if test "$dir" = "$__inshellah_stub_dir"
            set behind true
            continue
        end
        if $behind; and not string match -q '*generated_completions' -- $dir; and test -f $dir/$cmd.fish
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
# ctrl-c aborts the whole call chain, so a load marks its command pending
# first and clears that last. what a cut short load leaves behind is the
# pre-load completions, kept so the next attempt can start from them, and a
# stand-in completion that finishes the load at the next tab.
function __inshellah_load -a cmd adopt
    set -l orig __inshellah_orig_(string escape --style=var -- $cmd)
    set -l pre __inshellah_pre_(string escape --style=var -- $cmd)
    set -l add __inshellah_add_(string escape --style=var -- $cmd)
    if __inshellah_user_file $cmd
        contains -- $cmd $__inshellah_user; or set -ga __inshellah_user $cmd
        contains -- $cmd $__inshellah_loaded; or set -ga __inshellah_loaded $cmd
        return
    end
    contains -- $cmd $__inshellah_pending; or set -ga __inshellah_pending $cmd
    if not set -q $pre
        if contains -- $cmd $__inshellah_loaded
            set -g $pre kept $$orig (__inshellah_extras $cmd)
        else if test "$adopt" = adopt
            set -g $pre adopt (__inshellah_snapshot $cmd)
        else
            set -g $pre fresh (__inshellah_snapshot $cmd)
        end
        set -l saved $$pre
        if test $saved[1] = kept
            complete -e -c $cmd
            printf '%s\n' $saved[2..-1] | source
        end
    else
        set -l saved $$pre
        set -l kept
        if complete -c $cmd | string match -q -- '*(__inshellah_retry *'
            set -l now (__inshellah_snapshot $cmd)
            for line in $saved[2..-1]
                contains -- $line $now; and set -a kept $line
            end
        end
        if test (count $kept) -ne (count $saved[2..-1])
            set -g $pre $saved[1] $kept
            if test $saved[1] = kept
                set -l left
                for line in $$orig
                    contains -- $line $kept; and set -a left $line
                end
                set -g $orig $left
            end
            set saved $$pre
        end
        for line in (__inshellah_after $cmd $saved[2..-1])
            contains -- $line $$add; or set -ga $add $line
        end
        complete -e -c $cmd
        printf '%s\n' $saved[2..-1] | source
    end
    __inshellah_hook $cmd
    set -l at (contains -i -- $cmd $__inshellah_taken)
    set -q at[1]; and set -e __inshellah_taken[$at]
    set -l saved $$pre
    if test $saved[1] != kept
        test $saved[1] = adopt; or __inshellah_source_native $cmd
        set -l now (__inshellah_snapshot $cmd)
        set -g $pre kept $now
        set -g $orig $now
    end
    set saved $$pre
    set -q $orig; or set -g $orig $saved[2..-1]
    complete -e -c $cmd
    printf '%s\n' $saved[2..-1] | source
    for line in $$add
        contains -- $line $saved[2..-1]; or echo $line | source
    end
    if test (count $$orig) -gt 0
        __inshellah_apply $cmd
    else
        __inshellah_install $cmd
    end
    contains -- $cmd $__inshellah_loaded; or set -ga __inshellah_loaded $cmd
    set -e $pre $add
    set -e __inshellah_pending[(contains -i -- $cmd $__inshellah_pending)]
end

function __inshellah_hook -a cmd
    complete -c $cmd -k -f -a "(__inshellah_retry $(string escape --style=var -- $cmd))"
end

function __inshellah_retry -a name
    set -l cmd (string unescape --style=var -- $name)
    __inshellah_load $cmd
    contains -- $cmd $__inshellah_pending; or complete -C "$(commandline -cp)"
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
# assigned here, since any assignment makes fish drop everything it autoloaded.
# names wait in a queue so that a ctrl-c partway through loses none of them
function __inshellah_refresh
    set -l out (command inshellah fish-stubs 2>/dev/null)
    __inshellah_path_mark
    set -q out[2]; and set -ga __inshellah_queue $out[2..-1]
    __inshellah_drain
end

function __inshellah_drain
    while set -q __inshellah_queue[1]
        set -l name $__inshellah_queue[1]
        if not contains -- $name $__inshellah_loaded $__inshellah_pending
            set -l have (complete -c $name)
            if set -q have[1]
                __inshellah_load $name adopt
            else
                __inshellah_load $name
            end
        end
        set -e __inshellah_queue[1]
    end
end

function __inshellah_stubs --on-variable PATH
    __inshellah_refresh
end

# runs before each prompt, so it picks up a command installed into a dir that
# was already on PATH. a load that ctrl-c cut short is not retried here, since a
# slow completion file would hold up every prompt. it only gets its stand-in
# completion back in case the cut came before it was registered
function __inshellah_prompt --on-event fish_prompt
    for cmd in $__inshellah_pending
        complete -c $cmd | string match -q -- '*(__inshellah_retry *'; or __inshellah_hook $cmd
    end
    if test "$(__inshellah_path_stamp)" != "$__inshellah_path_seen"
        __inshellah_refresh
    else
        __inshellah_drain
    end
end

function __inshellah_retake --on-variable INSHELLAH_TAKEOVER
    for cmd in $__inshellah_loaded
        contains -- $cmd $__inshellah_pending; or __inshellah_apply $cmd
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
