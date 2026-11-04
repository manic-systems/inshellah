# inshellah completions for zsh. load with
#   eval "$(inshellah init zsh)"

_inshellah_takeover() {
    setopt localoptions extendedglob
    [[ ${(L)INSHELLAH_TAKEOVER-} == [[:space:]]#(1|true|yes|on)[[:space:]]# ]]
}

_inshellah_pass() {
    [[ -n ${_inshellah_prev_first-} ]] || return 1
    unset _compskip
    $_inshellah_prev_first "$@"
}

_inshellah() {
    [[ $compstate[context] == command ]] && ((CURRENT > 1)) || return 1
    local first=
    if [[ $service == -first- ]]; then
        _inshellah_takeover || { _inshellah_pass "$@"; return }
        first=1
    fi
    local cmd=${(Q)words[1]}
    local -a reply=(defer)
    if ! (($+builtins[$cmd] || $+functions[$cmd] || ${reswords[(I)$cmd]})); then
        reply=("${(@f)$(command inshellah complete --shell zsh -- "${(@Q)words[1,CURRENT-1]}" "${(Q)PREFIX}" 2>/dev/null)}")
    fi
    local mode=${reply[1]:-defer}
    if [[ $mode == defer ]]; then
        [[ -n $first ]] && { _inshellah_pass "$@"; return }
        _default "$@"
        return
    fi
    [[ -n $first ]] && _compskip=all

    local -a values descs displays expl
    local entry
    local width=0
    for entry in "${(@)reply[2,-1]}"; do
        values+=("${entry%%$'\t'*}")
        descs+=("${entry#*$'\t'}")
        ((${#values[-1]} > width)) && width=${#values[-1]}
    done
    _description -V inshellah expl inshellah
    local ret=1
    if [[ -n ${(j::)descs} ]]; then
        local i line
        for ((i = 1; i <= $#values; i++)); do
            line="${(r:width:)values[i]}  -- $descs[i]"
            ((COLUMNS > 8 && ${#line} >= COLUMNS)) && line="${line[1,COLUMNS-4]}..."
            displays+=("$line")
        done
        compadd "${expl[@]}" -l -d displays -a values && ret=0
    else
        compadd "${expl[@]}" -a values && ret=0
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
# resets both contexts, so every prompt checks the registration. under
# takeover `-default-` still catches the command behind nohup, env and the like.
_inshellah_register() {
    (($+functions[compdef])) || return 0
    [[ $_comps[-default-] == _inshellah ]] || compdef _inshellah -default-
    if [[ $_comps[-first-] != _inshellah ]]; then
        _inshellah_prev_first=$_comps[-first-]
        compdef _inshellah -first-
    fi
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd _inshellah_register
