# inshellah completions for zsh. load with
#   eval "$(inshellah init zsh)"

_inshellah_takeover() {
    emulate -L zsh
    setopt extendedglob
    [[ ${(L)INSHELLAH_TAKEOVER-} == [[:space:]]#(1|true|yes|on)[[:space:]]# ]]
}

_inshellah_pass() {
    emulate -L zsh
    [[ -n ${_inshellah_prev_first-} ]] || return 1
    unset _compskip
    $_inshellah_prev_first "$@"
}

_inshellah() {
    emulate -L zsh
    local first=
    if [[ $service == -first- ]]; then
        _inshellah_memo_key=
        _inshellah_takeover || { _inshellah_pass "$@"; return }
        [[ $compstate[context] == command ]] && ((CURRENT > 1)) || { _inshellah_pass "$@"; return }
        first=1
    else
        [[ $compstate[context] == command ]] && ((CURRENT > 1)) || return 1
    fi
    local cmd=${(Q)words[1]}
    local -a reply=(defer)
    if ! (($+builtins[$cmd] || $+functions[$cmd] || ${reswords[(I)$cmd]})); then
        local key=${(pj:\0:)words[1,CURRENT-1]}$'\0'$PREFIX
        if [[ -z $first && $_inshellah_memo_key == $key ]]; then
            reply=("${(@)_inshellah_memo}")
        else
            reply=("${(@f)$(command inshellah complete --shell zsh -- "${(@Q)words[1,CURRENT-1]}" "${(Q)PREFIX}" 2>/dev/null)}")
        fi
        if [[ -n $first ]]; then
            typeset -g _inshellah_memo_key=$key
            typeset -ga _inshellah_memo=("${(@)reply}")
        else
            _inshellah_memo_key=
        fi
    fi
    local mode=${reply[1]:-defer}
    if [[ $mode == defer ]]; then
        [[ -n $first ]] && { _inshellah_pass "$@"; return }
        _default "$@"
        return
    fi
    [[ -n $first ]] && _compskip=all

    local -a values descs expl
    local entry
    local width=0
    for entry in "${(@)reply[2,-1]}"; do
        values+=("${entry%%$'\t'*}")
        descs+=("${entry#*$'\t'}")
        ((${(m)#values[-1]} > width)) && width=${(m)#values[-1]}
    done
    _description -V inshellah expl inshellah
    local ret=1
    local -a dirs dirdisplays plain plaindisplays
    local i line
    local hasdescs=
    [[ -n ${(j::)descs} ]] && hasdescs=1
    for ((i = 1; i <= $#values; i++)); do
        line=
        if [[ -n $hasdescs ]]; then
            line="${(mr:width:)values[i]}  -- $descs[i]"
            if ((COLUMNS > 8 && ${(m)#line} >= COLUMNS)); then
                line=${line[1,COLUMNS-4]}
                while ((${(m)#line} > COLUMNS - 4)); do line=${line[1,-2]}; done
                line+=...
            fi
        fi
        if [[ $values[i] == */ ]]; then
            dirs+=("$values[i]")
            dirdisplays+=("$line")
        else
            plain+=("$values[i]")
            plaindisplays+=("$line")
        fi
    done
    if [[ -n $hasdescs ]]; then
        (($#plain)) && compadd "${expl[@]}" -l -d plaindisplays -a plain && ret=0
        (($#dirs)) && compadd "${expl[@]}" -S '' -l -d dirdisplays -a dirs && ret=0
    else
        (($#plain)) && compadd "${expl[@]}" -a plain && ret=0
        (($#dirs)) && compadd "${expl[@]}" -S '' -a dirs && ret=0
    fi
    if [[ $mode == values+files ]]; then
        _files && ret=0
    elif ((ret && $#values)); then
        [[ -n $first ]] && { _inshellah_pass "$@"; return }
        _default "$@"
        return
    fi
    return ret
}

# a compinit that runs after the first prompt, as deferred plugin managers do,
# resets both contexts, so every prompt and keystroke checks the registration. under
# takeover `-default-` still catches the command behind nohup, env and the like.
_inshellah_register() {
    emulate -L zsh
    (($+functions[compdef])) || return 0
    [[ $_comps[-default-] == _inshellah ]] || compdef _inshellah -default-
    if [[ $_comps[-first-] != _inshellah ]]; then
        _inshellah_prev_first=$_comps[-first-]
        compdef _inshellah -first-
    fi
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd _inshellah_register
if [[ -o interactive ]] && zmodload zsh/zle 2>/dev/null; then
    autoload -Uz add-zle-hook-widget
    add-zle-hook-widget line-pre-redraw _inshellah_register
fi
