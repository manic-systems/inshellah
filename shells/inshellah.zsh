# inshellah completions for zsh. load with
#   eval "$(inshellah init zsh)"

_inshellah_takeover() {
    emulate -L zsh
    setopt extendedglob
    [[ ${(L)INSHELLAH_TAKEOVER-} == [[:space:]]#(1|true|yes|on)[[:space:]]# ]]
}

typeset -gA _inshellah_memo

_inshellah_reset() {
    _inshellah_memo=()
}

_inshellah_pass() {
    [[ -n ${_inshellah_prev_first-} ]] || return 1
    unset _compskip
    $_inshellah_prev_first "$@"
}

_inshellah_unwrap() {
    emulate -L zsh
    local -i i=1
    local w o viabuiltin=
    while ((i < CURRENT)); do
        w=${(Q)words[i]}
        case $w in
            noglob|nocorrect|-) ((i++)) ;;
            builtin) viabuiltin=1; ((i++)) ;;
            command|exec)
                pre=$i
                viabuiltin=
                ((i++))
                while ((i < CURRENT)); do
                    o=${(Q)words[i]}
                    [[ $o == -?* ]] || break
                    ((i++))
                    [[ $o == -- ]] && break
                    if [[ $w == command ]]; then
                        [[ $o == -*[vV]* ]] && return 1
                    elif [[ $o == -*a ]]; then
                        ((i++))
                    fi
                done
                ((i < CURRENT)) && case ${(Q)words[i]} in
                    builtin|exec) return 1 ;;
                esac
                ;;
            *) break ;;
        esac
    done
    ((i < CURRENT)) || return 1
    [[ -z $viabuiltin || ${(Q)words[i]} == kill ]] || return 1
    REPLY=$i
}

_inshellah() {
    emulate -L zsh
    (($+_comp_options)) && setopt ${_comp_options[@]}
    local first=
    local -i at=1 pre=0 start
    if [[ $service == -first- ]]; then
        _inshellah_takeover || { _inshellah_pass "$@"; return }
        [[ $compstate[context] == command ]] && ((CURRENT > 1)) || { _inshellah_pass "$@"; return }
        first=1
        _inshellah_unwrap || { _inshellah_pass "$@"; return }
        at=$REPLY
    else
        [[ $compstate[context] == command ]] && ((CURRENT > 1)) || return 1
    fi
    local cmd=${(Q)words[at]}
    start=$at
    [[ $cmd == kill ]] && ((pre)) && start=$pre
    local -a reply=(defer)
    local skip=
    if ((${reswords[(I)$cmd]} || $+functions[$cmd])); then
        skip=1
    elif (($+builtins[$cmd])); then
        [[ -n $first && $cmd == kill ]] || skip=1
    fi
    if [[ -z $skip ]]; then
        local key=${(pj:\x01:)words[start,CURRENT-1]}$'\x01'$PREFIX
        if (($+_inshellah_memo[(e)$key])); then
            reply=("${(@f)_inshellah_memo[(e)$key]}")
        else
            reply=("${(@f)$(command inshellah complete --shell zsh -- "${(@Q)words[start,CURRENT-1]}" "${(Q)PREFIX}" 2>/dev/null)}")
            _inshellah_memo[(e)$key]=${(F)reply}
        fi
    fi
    local mode=${reply[1]:-defer}
    if [[ $mode == defer ]]; then
        [[ -n $first ]] && { _inshellah_pass "$@"; return }
        ${_inshellah_prev_default:-_default} "$@"
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
        if [[ $values[i] == \~* ]]; then
            [[ -n $compstate[quote] ]] && continue
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
        ${_inshellah_prev_default:-_default} "$@"
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
    (($compprefuncs[(Ie)_inshellah_reset])) || compprefuncs+=(_inshellah_reset)
    if [[ $_comps[-default-] != _inshellah ]]; then
        _inshellah_prev_default=$_comps[-default-]
        compdef _inshellah -default-
    fi
    if [[ $_comps[-first-] != _inshellah ]]; then
        _inshellah_prev_first=$_comps[-first-]
        compdef _inshellah -first-
    fi
    return 0
}

() {
    emulate -L zsh
    autoload -Uz add-zsh-hook
    add-zsh-hook precmd _inshellah_register
    if [[ -o interactive ]] && zmodload zsh/zle 2>/dev/null; then
        autoload -Uz add-zle-hook-widget
        add-zle-hook-widget line-pre-redraw _inshellah_register
    fi
    return 0
}
