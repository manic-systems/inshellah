# inshellah completions for zsh. load with
#   eval "$(inshellah init zsh)"

_inshellah_takeover() {
    emulate -L zsh
    setopt extendedglob
    [[ ${(L)INSHELLAH_TAKEOVER-} == [[:space:]]#(1|true|yes|on)[[:space:]]# ]]
}

_inshellah_pass() {
    [[ -n ${_inshellah_prev_first-} ]] || return 1
    unset _compskip
    $_inshellah_prev_first "$@"
}

_inshellah() {
    emulate -L zsh
    (($+_comp_options)) && setopt ${_comp_options[@]}
    local first= again=
    ((${_matcher_num-1} > 1 || ${_completer_num-1} > 1)) && again=1
    if [[ $service == -first- ]]; then
        [[ -n $again ]] || _inshellah_memo_key=
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
        if [[ $_inshellah_memo_key == $key && (-z $first || -n $again) ]]; then
            reply=("${(@)_inshellah_memo}")
        else
            reply=("${(@f)$(command inshellah complete --shell zsh -- "${(@Q)words[1,CURRENT-1]}" "${(Q)PREFIX}" 2>/dev/null)}")
        fi
        if [[ -n $first ]]; then
            typeset -g _inshellah_memo_key=$key
            typeset -ga _inshellah_memo=("${(@)reply}")
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
    local -a v_plain d_plain v_dirs d_dirs v_tplain d_tplain v_tdirs d_tdirs
    local i line kind tilde
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
        kind=plain
        [[ $values[i] == */ ]] && kind=dirs
        if [[ $values[i] == \~* && -z $compstate[quote] ]]; then
            tilde=${values[i]%%/*}
            kind=t$kind
            values[i]=$tilde${(q)values[i]#$tilde}
        fi
        eval "v_$kind+=(\"\$values[i]\")" "d_$kind+=(\"\$line\")"
    done
    local -a flags vals
    for kind in plain dirs tplain tdirs; do
        flags=()
        [[ $kind == *dirs ]] && flags+=(-S "")
        [[ $kind == t* ]] && flags+=(-Q)
        [[ -n $hasdescs ]] && flags+=(-l -d d_$kind)
        vals=("${(@P)${:-v_$kind}}")
        (($#vals)) && compadd "${expl[@]}" "${flags[@]}" -a v_$kind && ret=0
    done
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
