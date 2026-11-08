// SPDX-License-Identifier: EUPL-1.2
//! the bash, zsh and fish shims driving the real binary against a fixture
//! cache, the counterpart of `seam_nu.rs`. each test skips when its shell is
//! not on PATH. zsh's `compadd` only works inside a completion widget, so its
//! test stubs the compsys builtins the shim calls and checks what reaches
//! them.
#![cfg(test)]

#[path = "common/exec.rs"] mod exec;
#[path = "common/temp_dir.rs"] mod temp_dir;

use std::{
    env,
    ffi::OsString,
    fs,
    path::Path,
    process::Command,
};

use inshellah::store::{
    result_from_json,
    write_result,
};
use serde_json::{
    Value,
    json,
};
use temp_dir::TempDir;

struct Fixture {
    dir:  TempDir,
    path: OsString,
}

fn spec(entries: &Value, subcommands: &[(&str, &str)]) -> Value {
    let subs: Vec<Value> = subcommands
        .iter()
        .map(|(name, desc)| json!({"name": name, "desc": desc, "aliases": []}))
        .collect();
    json!({
        "source": "help",
        "description": "",
        "entries": entries,
        "subcommands": subs,
        "positional_choices": [],
        "positionals": [],
    })
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = TempDir::new(name);
        let cache = dir.path().join("inshellah");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).expect("bin dir");
        for tool in ["demotool", "twotool", "fwrap", "dirtool", "pendtool"] {
            exec::write_executable_file(&bin.join(tool), "#!/bin/sh\nexit 0\n");
        }

        let demotool = spec(
            &json!([
                {
                    "switch": {"type": "long", "name": "color"},
                    "param": {"kind": "mandatory", "name": "WHEN", "choices": [
                        {"value": "always", "desc": ""},
                        {"value": "never", "desc": ""},
                    ]},
                    "desc": "paint the output",
                },
                {"switch": {"type": "long", "name": "verbose"}, "param": null, "desc": "say more"},
            ]),
            &[
                ("alpha", "first"),
                ("beta", "second"),
                ("has space", "quoted"),
            ],
        );
        let alpha = spec(&json!([]), &[("run", "run it"), ("reset", "reset it")]);
        let dirtool = spec(
            &json!([{
                "switch": {"type": "long", "name": "out"},
                "param": {"kind": "mandatory", "name": "FILE", "choices": []},
                "desc": "write here",
            }]),
            &[],
        );
        for (command, value) in [
            ("demotool", &demotool),
            ("demotool alpha", &alpha),
            ("twotool", &demotool),
            ("dirtool", &dirtool),
        ] {
            write_result(&cache, command, "help", &result_from_json(value)).expect("cache entry");
        }

        let built = Path::new(env!("CARGO_BIN_EXE_inshellah"))
            .parent()
            .expect("binary dir");
        let mut path = env::join_paths([bin.as_path(), built]).expect("join PATH");
        if let Some(old) = env::var_os("PATH") {
            path.push(":");
            path.push(old);
        }
        Self { dir, path }
    }

    /// runs `script` under `shell`, `None` when the shell is missing.
    fn run(&self, shell: &str, args: &[&str], script: &str) -> Option<String> {
        let output = Command::new(shell)
            .args(args)
            .arg(script)
            .env("PATH", &self.path)
            .env("SEAM_PATH", &self.path)
            .env(
                "LC_ALL",
                if cfg!(target_os = "macos") {
                    "en_US.UTF-8"
                } else {
                    "C.UTF-8"
                },
            )
            .env("HOME", self.dir.path())
            .env("XDG_CACHE_HOME", self.dir.path())
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env("XDG_DATA_HOME", self.dir.path().join("data"))
            .env("SHIM", Path::new(env!("CARGO_MANIFEST_DIR")).join("shells"))
            .env_remove("INSHELLAH_DIR")
            .env_remove("INSHELLAH_TAKEOVER")
            .env_remove("INSHELLAH_FLAG_TRIGGERS")
            .env_remove("INSHELLAH_FLAG_ON_EMPTY")
            .env_remove("INSHELLAH_MAX_COMPLETIONS")
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(
            output.status.success(),
            "{shell} failed\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Some(stdout)
    }
}

fn expect(got: &str, want: &str) {
    assert_eq!(got.trim_end(), want.trim_end(), "\n--- got ---\n{got}");
}

const BASH: &str = r#"
set -eu
source "$SHIM/inshellah.bash"
chk() { [[ $1 == "$2" ]] || echo "FAIL want <$1> got <$2> for $3"; }
flat() { local IFS='|'; printf %s "${COMPREPLY[*]-}"; }
words() {
    COMP_LINE=$1 COMP_POINT=${#1}
    read -ra COMP_WORDS <<<"$1"
    [[ $1 == *' ' ]] && COMP_WORDS+=("")
    COMP_CWORD=$((${#COMP_WORDS[@]} - 1))
    COMPREPLY=()
}
comp_as() {
    words "$3"
    _inshellah_complete "$2" "${COMP_WORDS[COMP_CWORD]}" "${COMP_WORDS[COMP_CWORD - 1]}"
    chk "$1" "$(flat)" "$3"
}
comp() { comp_as "$1" demotool "$2"; }
comp 'alpha' 'demotool al'
comp 'run|reset' 'demotool alpha r'
comp 'beta' 'FOO=1 demotool be'
comp 'has\ space' 'demotool has'
comp 'has space' 'demotool "has'
comp '' 'demotool > out'
comp_as 'x$(demotool --color=always |x$(demotool --color=always  ' echo 'echo "x$(demotool --color=al'
comp_as '"$(demotool "alpha"' echo 'echo "$(demotool "al'
comp_as 'alpha' echo 'echo `demotool al'
comp_as 'alpha' command 'command demotool al'
comp_as '' builtin 'builtin demotool al'
takeover() {
    local state=off
    INSHELLAH_TAKEOVER=$2 _inshellah_takeover && state=on
    chk "$1" "$state" "takeover '$2'"
}
takeover on 1
takeover on ' Yes '
takeover off off
takeover off maybe
_comp_command() { COMPREPLY=(bc); }
_inshellah_wrap_unwrappers
_inshellah_wrap_unwrappers
unwrap() {
    words "$2"
    _comp_command nohup "${COMP_WORDS[COMP_CWORD]}" x
    chk "$1" "$(flat)" "unwrap $2"
}
unwrap bc 'nohup demotool al'
unwrap bc 'nohup nosuchtool al'
INSHELLAH_TAKEOVER=1 unwrap alpha 'nohup demotool al'
INSHELLAH_TAKEOVER=1 unwrap bc 'nohup nosuchtool al'
_comp_load() { complete -W lazy -- "${@: -1}"; }
_inshellah_wrap_loaders
_inshellah_wrap_loaders
complete -W mine 'a b'
complete -W mine mytool
_comp_load -- lazytool
_inshellah_registered=1
INSHELLAH_TAKEOVER=1 _inshellah_register_at_prompt
registered() { complete -p -- "$1" >/dev/null 2>&1 && echo kept || echo dropped; }
chk dropped "$(registered lazytool)" lazytool
chk kept "$(registered mytool)" mytool
chk kept "$(registered 'a b')" 'a b'
_comp_complete_longopt() { COMPREPLY=(longopt); [[ ${COMPREPLY-} == *= ]] && :; }
_inshellah_wrap_longopt
_inshellah_wrap_longopt
longopt() {
    words "$3"
    local rc=0
    _comp_complete_longopt diff "${COMP_WORDS[COMP_CWORD]}" x || rc=$?
    chk "$1 rc=$2" "$(flat) rc=$rc" "longopt $3"
}
longopt longopt 1 'diff --he'
longopt alpha 0 'diff <(demotool al'
longopt 'run|reset' 0 'diff `demotool alpha r'
longopt longopt 1 'diff <(true) --ig'
longopt alpha 0 'diff <(true) <(demotool al'
row() {
    local line=${4%\~}
    [[ $4 == *'~' ]] && line+=' '
    local got
    if [[ $1 == nested ]]; then
        COMP_LINE=$line COMP_POINT=${#line}
        got=closed
        _inshellah_nested && got=open
    else
        words "$line"
        got=asks
        _inshellah_shell_owns "$3" && got=owns
    fi
    chk "$2" "$got" "$4"
}
while IFS='|' read -r kind want arg line; do
    row "$kind" "$want" "$arg" "$line"
done <<'TABLE'
nested|closed||a <(b) c
nested|open||a $(b) $(c
nested|closed||a $(echo ")") --x
nested|open||a $(echo ")" --x
nested|open||"$(a
nested|closed||a "$(b)" c
nested|open||a <(b $(c) d
nested|open||a `b` `c
nested|closed||a `b` c
nested|open||a $(echo '(
nested|closed||a $((1+2)) c
nested|closed||a "<(b
owns|owns|builtin|builtin ki
owns|owns|builtin|builtin kill
owns|asks|builtin|builtin kill -
owns|asks|builtin|builtin builtin kill -s ~
owns|owns|builtin|builtin cd ~
owns|asks|builtin|echo $(builtin kill -s T
owns|asks|builtin|echo "$(builtin kill -s T
owns|asks|builtin|echo `builtin kill -s ~
owns|asks|builtin|echo $(true; builtin command kill -s T
owns|owns|builtin|echo $(builtin cd ~
owns|asks|builtin|echo $(time -p builtin kill -s T
owns|asks|builtin|if builtin kill -s T
owns|asks|builtin|x=1 builtin kill -s T
owns|asks|builtin|{ builtin kill -s T
owns|asks|kill|echo $(kill -s T
owns|asks|builtin|builtin command kill -s T
owns|asks|builtin|builtin exec kill -
owns|owns|builtin|builtin command cd ~
TABLE
echo done
"#;

#[test]
fn bash_shim_seam() {
    let fixture = Fixture::new("inshellah-seam-bash");
    if fixture
        .run("bash", &["-c"], "type -t complete || true")
        .as_deref()
        .map(str::trim)
        != Some("builtin")
    {
        eprintln!("skipping: bash not found or built without readline");
        return;
    }
    let Some(out) = fixture.run("bash", &["-c"], BASH) else {
        eprintln!("skipping: bash not found");
        return;
    };
    expect(&out, "done");
}

const ZSH: &str = r#"
# nixos /etc/zshenv resets PATH even under -f
export PATH=$SEAM_PATH
source "$SHIM/inshellah.zsh"
typeset -A compstate
compadd() {
    emulate -L zsh
    local -a _vals _displays
    local _suffix=
    while (($#)); do
        case $1 in
            -a) _vals=("${(@P)2}"); shift 2 ;;
            -d) _displays=("${(@P)2}"); shift 2 ;;
            -V) shift 2 ;;
            -S) _suffix=" S[$2]"; shift 2 ;;
            *) shift ;;
        esac
    done
    print -rn -- "${(j:|:)_vals}$_suffix"
    (($#_displays)) && print -rn -- " {${(j:|:)_displays}}"
    return 0
}
_description() { eval "$3=()" }
_files() { print -rn -- ' +files' }
_comp_options=(nullglob)
_default() { local g; g=(/nonexistent*); print -rn -- 'default' }
_prev() { local g; g=(/nonexistent*); print -rn -- 'prev' }
setup() {
    emulate -L zsh
    words=("${(@z)1}")
    [[ $1 == *' ' ]] && words+=('')
    compstate[context]=${ctx:-command}
    CURRENT=$#words
    PREFIX=$words[-1]
}
run() {
    _inshellah_reset
    setup "$1"
    _inshellah >"$HOME/out"
    got=$(<"$HOME/out")
}
comp() { run "$2"; [[ $got == "$1" ]] || print -r -- "FAIL $2 want <$1> got <$got>" }
kindof() {
    run "$2"
    local kind=pids
    [[ $got == *'-- TERM'[\|\}]* ]] && kind=signals
    [[ $kind == "$1" ]] || print -r -- "FAIL $2 want $1 got $kind"
}
memos() { emulate -L zsh; local -a k=(${(k)_inshellah_memo}); [[ $#k == "$1" ]] || print -r -- "FAIL memo want $1 got $#k" }
first() { INSHELLAH_TAKEOVER=1 service=-first- "$@" }
comp 'alpha {alpha  -- first}' 'demotool al'
COLUMNS=12 comp 'run|reset {run    -...|reset  -...}' 'demotool alpha r'
comp '--color=always|--color=never' 'demotool --color='
comp 'has space {has space  -- quoted}' 'demotool has'
comp 'alpha|beta|has space {alpha      -- first|beta       -- second|has space  -- quoted}' 'demotool --color always '
comp 'default' 'pwd -'
_inshellah_prev_default=_prev comp 'prev' 'pwd -'
first comp '' 'demotool alpha run '
service=-first- comp '' 'demotool al'
INSHELLAH_TAKEOVER=" On " service=-first- comp 'alpha {alpha  -- first}' 'demotool al'
_inshellah_prev_first=_prev service=-first- comp 'prev' 'demotool al'
ctx=redirect INSHELLAH_TAKEOVER=1 _inshellah_prev_first=_prev service=-first- comp 'prev' 'cat < pd'
ctx=redirect service=-first- comp '' 'cat < pd'
ctx=redirect service=-default- comp '' 'cat < pd'
mkdir -p $HOME/pdir/sub
cd $HOME
comp '--out=pdir/ S[] {--out=pdir/  -- directory}' 'dirtool --out=pd'
comp '--out=pdir/sub/ S[] {--out=pdir/sub/  -- directory}' 'dirtool --out=pdir/'
setopt ksharrays nomatch kshglob shwordsplit nounset
comp 'run|reset {run    -- run it|reset  -- reset it}' 'demotool alpha r'
comp '--out=pdir/ S[] {--out=pdir/  -- directory}' 'dirtool --out=pd'
_inshellah_prev_first=_prev first comp 'prev' 'command exec demotool al'
first comp 'alpha {alpha  -- first}' 'exec -cl -a name demotool al'
first comp 'alpha {alpha  -- first}' 'noglob command -p -- demotool al'
_inshellah_prev_first=_prev first comp 'prev' 'command -v demotool al'
_inshellah_prev_first=_prev first comp 'prev' 'builtin demotool al'
first comp 'alpha {alpha  -- first}' 'demotool al'
memos 1
_inshellah_reset
memos 0
typeset -A _comps
_comps[-default-]=_prev
compdef() { _comps[$2]=$1 }
_inshellah_register
_inshellah_register
got="${_comps[-first-]} ${_comps[-default-]} ${(j:,:)compprefuncs} $_inshellah_prev_default"
[[ $got == '_inshellah _inshellah _inshellah_reset _prev' ]] || print -r -- "FAIL registered $got"
print done
"#;

#[test]
fn zsh_shim_seam() {
    let fixture = Fixture::new("inshellah-seam-zsh");
    let Some(out) = fixture.run("zsh", &["-f", "-c"], ZSH) else {
        eprintln!("skipping: zsh not found");
        return;
    };
    expect(&out, "done");
}

#[test]
fn zsh_shim_survives_strict_options() {
    let fixture = Fixture::new("inshellah-seam-zsh-strict");
    for (strict, errexit) in [
        ("set -e", "on"),
        ("set -eu", "on"),
        ("setopt err_return", "off"),
    ] {
        let script = format!(
            r#"
export PATH=$SEAM_PATH
{strict}
typeset -A _comps
compdef() {{ _comps[$2]=$1 }}
source "$SHIM/inshellah.zsh"
before=$options[errexit]
_inshellah_register
print -r -- "survived $before $options[errexit] $_comps[-first-]"
"#
        );
        let Some(out) = fixture.run("zsh", &["-f", "-i", "-c"], &script) else {
            eprintln!("skipping: zsh not found");
            return;
        };
        expect(&out, &format!("survived {errexit} {errexit} _inshellah"));
    }
}

const FISH: &str = r#"
source $SHIM/inshellah.fish
function chk
    test "$argv[2]" = "$argv[3..-1]"; or echo "FAIL $argv[1] want <$argv[2]> got <$argv[3..-1]>"
end
function show
    chk $argv[1] "$(printf %b $argv[3])" (complete -C $argv[2] | string join "|")
end
show al 'demotool al' 'alpha\tfirst'
show alpha-r 'demotool alpha r' 'run\trun it|reset\treset it'
show flags 'demotool --' '--color\tpaint the output <WHEN>|--verbose\tsay more'
show value 'demotool --color=' '--color=always|--color=never'
show space 'demotool has' 'has space\tquoted'
show quote 'demotool "al' 'alpha\tfirst'
show escaped 'demotool has\\ sp' 'has space\tquoted'
complete -c twotool -f -a nativechoice
complete -c fwrap -x -a "(__fish_complete_subcommand)"
show "fallback keeps fish own" "twotool " nativechoice
set -g INSHELLAH_TAKEOVER 1
show "takeover reaches loaded" "twotool al" 'alpha\tfirst'
show "takeover wrapper" "fwrap demotool al" 'alpha\tfirst'
show "takeover defers to fish" "twotool alpha run " nativechoice
set -l before (complete -c twotool | count)
source $SHIM/inshellah.fish
show "resourced keeps fish own" "twotool alpha run " nativechoice
chk "resourced keeps completions" same (test (complete -c twotool | count) -eq $before; and echo same; or echo changed)
chk "xargs like" wraps (__inshellah_wraps "complete --exclusive xargs -a '(__fish_complete_subcommand)'"; and echo wraps; or echo plain)
complete -c pendtool -f -a mine
set -g __inshellah_pre_pendtool fresh (__inshellah_snapshot pendtool)
set -g __inshellah_pending pendtool
complete -c pendtool -f -a half
__inshellah_hook pendtool
complete -C "pendtool " >/dev/null
chk "interrupted load" "taken kept 0" (contains pendtool $__inshellah_taken; and echo taken; or echo lost) (string match -q "*half*" (complete -c pendtool); and echo kept; or echo dropped) (count $__inshellah_pending)
complete -c pendhook -f -a mine
set -g __inshellah_pending pendhook
__inshellah_prompt
chk "prompt restores hook" "hooked 1" (string match -q '*__inshellah_retry*' (complete -c pendhook | string collect); and echo hooked; or echo bare) (count $__inshellah_pending)
set -e __inshellah_pending
set -l late $PATH[1]/latetool
printf '#!/bin/sh\n' > $late
chmod +x $late
__inshellah_prompt
chk "late binary" loaded (contains latetool $__inshellah_loaded; and echo loaded; or echo missed)
mkdir -p $HOME/slowvend/vendor_completions.d
printf 'complete -c slowcmd -f -a own\n' > $HOME/slowvend/vendor_completions.d/slowcmd.fish
set -g fish_complete_path $fish_complete_path $HOME/slowvend/vendor_completions.d
__inshellah_place
set -g __inshellah_pending slowcmd
__inshellah_hook slowcmd
for round in 1 2
    complete -C "slowcmd " >/dev/null
    chk "retried load $round" "1 0 own" (count $__inshellah_orig_slowcmd) (count $__inshellah_pending) (string match -q '*__inshellah_complete*' "$__inshellah_orig_slowcmd"; and echo stub; or echo own)
end
printf 'complete -c slowerase -f -a own\n' > $HOME/slowvend/vendor_completions.d/slowerase.fish
complete -c slowerase -f -a confd
set -g __inshellah_pre_slowerase fresh (__inshellah_snapshot slowerase)
set -g __inshellah_pending slowerase
__inshellah_hook slowerase
complete -c slowerase -e
__inshellah_prompt
complete -C "slowerase " >/dev/null
chk "erased while pending" "gone 1 0" (string match -q '*confd*' "$__inshellah_orig_slowerase"; and echo back; or echo gone) (count $__inshellah_orig_slowerase) (count $__inshellah_pending)
printf 'complete -c slowthree -f -a own\nreturn 130\n' > $HOME/slowvend/vendor_completions.d/slowthree.fish
set -l slowthree $PATH[1]/slowthree
printf '#!/bin/sh\n' > $slowthree
chmod +x $slowthree
__inshellah_load slowthree
chk "status 130 alone" "0 loaded" (count $__inshellah_pending) (contains slowthree $__inshellah_loaded; and echo loaded; or echo pending)
complete -C "manynew1 " >/dev/null
set -l manydir $HOME/manybin
mkdir -p $manydir
for i in (seq 40)
    printf '#!/bin/sh\n' > $manydir/manynew$i
    chmod +x $manydir/manynew$i
end
set -l path_before $fish_complete_path
set -l have_before (complete -c twotool | count)
set -p PATH $manydir
chk "many new names" "path same kept loaded loaded" (test "$path_before" = "$fish_complete_path"; and echo path same; or echo path changed) (test (complete -c twotool | count) -eq $have_before; and echo kept; or echo dropped) (test (complete -c manynew1 | count) -gt 0; and echo loaded; or echo missed) (test (complete -c manynew40 | count) -gt 0; and echo loaded; or echo missed)
functions -c __inshellah_refresh __seam_refresh
function __inshellah_refresh
    set -g refreshes (math 0$refreshes + 1)
    __seam_refresh
end
function localpath
    set -l PATH $HOME $PATH
end
localpath
set -l local_runs (math 0$refreshes + 0)
set -a PATH $HOME
chk "local PATH" "0 1" $local_runs (math 0$refreshes + 0)
echo done
"#;

const FISH_USER_DIR: &str = r#"
set -g fish_complete_path $HOME/usercomp $fish_complete_path
mkdir -p $HOME/usercomp
echo "complete -c demotool -f -a userchoice" > $HOME/usercomp/demotool.fish
set -gx INSHELLAH_TAKEOVER yes
source $SHIM/inshellah.fish
echo "user file =>" (complete -C "demotool " | string join "|")
set -l twice $PATH[1]/usertwice
printf '#!/bin/sh\n' > $twice
chmod +x $twice
printf 'set -g usertwice_runs (math 0$usertwice_runs + 1)\ncomplete -c usertwice -f -a userchoice\n' > $HOME/usercomp/usertwice.fish
__inshellah_prompt
complete -C "usertwice " >/dev/null
echo "user file once =>" $usertwice_runs (complete -c usertwice | count)
"#;

#[test]
fn fish_shim_seam() {
    let fixture = Fixture::new("inshellah-seam-fish");
    let Some(out) = fixture.run("fish", &["--no-config", "-c"], FISH) else {
        eprintln!("skipping: fish not found");
        return;
    };
    expect(&out, "done");
    let user = fixture
        .run("fish", &["--no-config", "-c"], FISH_USER_DIR)
        .expect("fish ran once already");
    expect(&user, "user file => userchoice\nuser file once => 1 1");
    assert!(
        fixture
            .dir
            .path()
            .join("inshellah/fish/demotool.fish")
            .exists(),
        "fish-stubs wrote no stub for demotool"
    );
}
