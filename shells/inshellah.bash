# inshellah completions for bash 4.1 or newer. load with
#   eval "$(inshellah init bash)"

# bash names the outer command in $1, but inside a `$(` still open at the
# cursor the command being typed is another one. only inshellah's lexer can
# tell which, so a line holding an opener is decided after it answers.
_inshellah_nested() {
    local line=${COMP_LINE:0:COMP_POINT}
    [[ $line == *'$('* || $line == *'<('* || $line == *'>('* || $line == *'`'* ]]
}

# the values parse_bool accepts in the config, so every hook reads the
# variable alike
_inshellah_takeover() {
    local value=${INSHELLAH_TAKEOVER-}
    value=${value#"${value%%[![:space:]]*}"}
    value=${value%"${value##*[![:space:]]}"}
    case ${value,,} in
        1 | true | yes | on) return 0 ;;
    esac
    return 1
}

_inshellah_shell_owns() {
    case $(type -t -- "$1") in
        builtin)
            [[ $1 == command || $1 == exec ]] && return 1
            [[ $1 == kill ]] && { _inshellah_takeover || ! complete -p kill >/dev/null 2>&1; } && return 1
            return 0
            ;;
        keyword | function) return 0 ;;
    esac
    return 1
}

_inshellah_load() {
    if declare -F _comp_load >/dev/null; then
        _comp_load -- "$1"
    elif declare -F __load_completion >/dev/null; then
        __load_completion "$1"
    else
        return 1
    fi
}

# fails when the shell should complete on its own. sets _inshellah_command to
# the command inshellah completed
_inshellah_answer() {
    _inshellah_command=$1
    local nested=
    _inshellah_nested && nested=1
    [[ $(type -t -- "$1") == keyword ]] && nested=1
    [[ -z $nested ]] && _inshellah_shell_owns "$1" && return 1

    local -a reply
    mapfile -t reply < <(command inshellah complete --shell bash \
        --line "${COMP_LINE:0:COMP_POINT}" ${COMP_WORDBREAKS+"--wordbreaks=$COMP_WORDBREAKS"} 2>/dev/null)
    local header=${reply[0]-defer}
    local mode=${header%%$'\t'*}
    [[ $header == *$'\t'* ]] && _inshellah_command=${header#*$'\t'}
    COMPREPLY=()
    [[ -n $nested ]] && _inshellah_shell_owns "$_inshellah_command" && return 1
    [[ $mode == defer ]] && return 1

    compopt +o default +o nospace 2>/dev/null || true
    compopt -o nosort 2>/dev/null || true
    local entry
    for entry in "${reply[@]:1}"; do
        COMPREPLY+=("${entry%%$'\t'*}")
    done
    if [[ $mode == values+files ]]; then
        _inshellah_add_files
    fi
    [[ ${#COMPREPLY[@]} -eq 1 ]] && _inshellah_single
    return 0
}

# readline closes an open quote and adds a space after a lone match, which is
# wrong for a directory and for a `"$(` whose quote is not the match's. bash
# skips the quote for a directory it can stat, but for a substitution only in
# its own completer, so there two matches that share the wanted text make
# readline insert it and nothing more.
_inshellah_single() {
    local only=${COMPREPLY[0]} pad=" " lead=
    local opener='(^|[^\\])(\$\(|<\(|>\(|`)'
    [[ $only =~ $opener || $only == " "* ]] && lead=1
    if [[ $only == *[=/] ]]; then
        pad=
        compopt -o nospace 2>/dev/null || true
    fi
    if [[ -z $lead ]]; then
        [[ $only == */ ]] && { compopt -o filenames -o noquote 2>/dev/null || true; }
    elif _inshellah_open_quote; then
        compopt -o nospace 2>/dev/null || true
        COMPREPLY=("$only$pad" "$only$pad ")
    fi
}

# succeeds when readline counts a quote as open at the cursor, going by quote
# characters alone as it does
_inshellah_open_quote() {
    local line=${COMP_LINE:0:COMP_POINT} quote= ch i
    for ((i = 0; i < ${#line}; i++)); do
        ch=${line:i:1}
        if [[ -n $quote && $ch == "$quote" ]]; then
            quote=
        elif [[ $quote == "'" ]]; then
            :
        elif [[ $ch == "\\" ]]; then
            ((i++))
        elif [[ -z $quote && ($ch == '"' || $ch == "'") ]]; then
            quote=$ch
        fi
    done
    [[ -n $quote ]]
}

# files alone are left to readline to quote and mark, which also keeps menus
# showing plain names. next to provider values the names are escaped here, a
# provider may already list a directory the file listing repeats, so a file
# replaces its bare twin instead of doubling it
_inshellah_add_files() {
    local cur=${COMP_WORDS[COMP_CWORD]} quoted= file slash i
    if [[ ${#COMPREPLY[@]} -eq 0 ]]; then
        mapfile -t COMPREPLY < <(compgen -f -- "$cur")
        compopt -o filenames 2>/dev/null || true
        return 0
    fi
    [[ $cur == [\"\']* ]] && quoted=1
    local -A seen=()
    for i in "${!COMPREPLY[@]}"; do
        seen[${COMPREPLY[i]}]=$i
    done
    while IFS= read -r file; do
        slash=
        [[ -d $file ]] && slash=/
        [[ -n $quoted ]] || printf -v file '%q' "$file"
        if [[ -n ${seen[$file]+set} ]]; then
            COMPREPLY[seen[$file]]=$file$slash
        else
            COMPREPLY+=("$file$slash")
        fi
    done < <(compgen -f -- "$cur")
}

# under takeover, runs bash-completion's completer for $1 once without leaving
# its spec behind, which would stop `complete -D` reaching the command again
_inshellah_borrow() {
    local spec fn
    _inshellah_load "$1" || return 1
    spec=$(complete -p -- "$1" 2>/dev/null) || return 1
    complete -r -- "$1"
    [[ $spec == *' -F '* ]] || return 1
    fn=${spec#* -F }
    fn=${fn%% *}
    for opt in nospace filenames noquote nosort; do
        [[ $spec == *" -o $opt"* ]] && { compopt -o "$opt" 2>/dev/null || true; }
    done
    "$fn" "$@" || true
}

_inshellah_complete() {
    local takeover=
    _inshellah_takeover && takeover=1
    if [[ -z $takeover ]] && ! _inshellah_nested; then
        _inshellah_load "$1" && return 124
    fi
    _inshellah_answer "$@" && return 0
    if [[ $_inshellah_command == "$1" ]]; then
        if [[ -n $takeover ]]; then
            _inshellah_borrow "$@" && return 0
        elif _inshellah_nested; then
            _inshellah_load "$1" && return 124
        fi
    fi
    if [[ -n ${_inshellah_prev_default-} ]]; then
        "$_inshellah_prev_default" "$@" || return $?
    fi
    return 0
}

# bash-completion completes the command behind sudo, nohup and the like with
# its minimal completer, which never reaches `complete -D`
_inshellah_wrap_minimal() {
    local name orig
    for name in _comp_complete_minimal _minimal; do
        declare -F "$name" >/dev/null || continue
        if [[ $(declare -f "$name") != *_inshellah_answer* ]]; then
            _inshellah_wraps=$((${_inshellah_wraps:-0} + 1))
            orig=_inshellah_orig$_inshellah_wraps
            eval "$orig$(declare -f "$name")"
            eval "$name() { _inshellah_answer \"\$@\" || $orig$name \"\$@\"; }"
        fi
        return 0
    done
}

# another `complete -D` handler, such as fzf's `**` trigger, still gets the
# lines inshellah defers. bash-completion's loader is called directly instead.
_inshellah_register() {
    local prev
    prev=$(complete -p -D 2>/dev/null) || true
    if [[ $prev == *' -F '* ]]; then
        prev=${prev#* -F }
        prev=${prev%% *}
        case $prev in
            _inshellah_complete | _comp_complete_load | _completion_loader) ;;
            *) _inshellah_prev_default=$prev ;;
        esac
    fi
    complete -D -o bashdefault -o default -F _inshellah_complete
    _inshellah_wrap_minimal
}

# bash-completion may load after this hook, nix-darwin sources it last, so
# registration is repeated once at the first prompt. under takeover, specs
# bash-completion loads later, as it does for the command behind `time` or
# `nohup`, are dropped again so `complete -D` keeps answering.
_inshellah_register_at_prompt() {
    if [[ -z ${_inshellah_registered-} ]]; then
        _inshellah_registered=1
        _inshellah_register
        _inshellah_eager=$(complete -p 2>/dev/null) || true
        return 0
    fi
    _inshellah_takeover || return 0
    local spec
    while IFS= read -r spec; do
        [[ ${spec##* } == -* || $'\n'$_inshellah_eager$'\n' == *$'\n'"$spec"$'\n'* ]] ||
            complete -r -- "${spec##* }"
    done < <(complete -p 2>/dev/null)
}

if [[ $(type -t complete) == builtin ]] &&
    ((BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 1))); then
    _inshellah_register
    [[ -n ${_inshellah_eager+set} ]] || unset _inshellah_registered
    if [[ ${PROMPT_COMMAND[*]-} == *_inshellah_register_at_prompt* ]]; then
        true
    elif [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]] &&
        ((BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1))); then
        PROMPT_COMMAND+=(_inshellah_register_at_prompt)
    else
        PROMPT_COMMAND=${PROMPT_COMMAND:+$PROMPT_COMMAND$'\n'}_inshellah_register_at_prompt
    fi
fi
