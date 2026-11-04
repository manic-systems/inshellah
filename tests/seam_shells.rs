// SPDX-License-Identifier: EUPL-1.2
//! the bash, zsh and fish shims driving the real binary against a fixture
//! cache, the counterpart of `seam_nu.rs`. each test skips when its shell is
//! not on PATH. zsh's `compadd` only works inside a completion widget, so its
//! test stubs the compsys builtins the shim calls and checks what reaches
//! them.
#![cfg(test)]

#[path = "common/temp_dir.rs"] mod temp_dir;

use std::{
    env,
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt as _,
    path::Path,
    process::Command,
};

use inshellah::store::{
    result_from_json,
    write_result,
};
use serde_json::json;
use temp_dir::TempDir;

struct Fixture {
    dir:  TempDir,
    path: OsString,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = TempDir::new(name);
        let cache = dir.path().join("inshellah");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).expect("bin dir");
        for tool in [
            "demotool", "twotool", "fwrap", "ordtool", "dirtool", "widetool", "bigtool", "pendtool",
        ] {
            let stub = bin.join(tool);
            fs::write(&stub, "#!/bin/sh\nexit 0\n").expect("write stub executable");
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod");
        }

        let demotool = json!({
            "source": "help",
            "description": "",
            "entries": [
                {
                    "switch": {"type": "long", "name": "color"},
                    "param": {"kind": "mandatory", "name": "WHEN", "choices": [
                        {"value": "always", "desc": ""},
                        {"value": "never", "desc": ""},
                    ]},
                    "desc": "paint the output",
                },
                {"switch": {"type": "long", "name": "verbose"}, "param": null, "desc": "say more"},
            ],
            "subcommands": [
                {"name": "alpha", "desc": "first", "aliases": []},
                {"name": "beta", "desc": "second", "aliases": []},
                {"name": "has space", "desc": "quoted", "aliases": []},
            ],
            "positional_choices": [],
            "positionals": [],
        });
        let alpha = json!({
            "source": "help",
            "description": "",
            "entries": [],
            "subcommands": [
                {"name": "run", "desc": "run it", "aliases": []},
                {"name": "reset", "desc": "reset it", "aliases": []},
            ],
            "positional_choices": [],
            "positionals": [],
        });
        write_result(&cache, "demotool", "help", &result_from_json(&demotool)).expect("demotool");
        write_result(&cache, "demotool alpha", "help", &result_from_json(&alpha)).expect("alpha");
        write_result(&cache, "twotool", "help", &result_from_json(&demotool)).expect("twotool");

        let dirtool = json!({
            "source": "help",
            "description": "",
            "entries": [
                {
                    "switch": {"type": "long", "name": "out"},
                    "param": {"kind": "mandatory", "name": "FILE", "choices": []},
                    "desc": "write here",
                },
            ],
            "subcommands": [],
            "positional_choices": [],
            "positionals": [],
        });
        write_result(&cache, "dirtool", "help", &result_from_json(&dirtool)).expect("dirtool");
        let widetool = json!({
            "source": "help",
            "description": "",
            "entries": [],
            "subcommands": [
                {"name": "alpha", "desc": "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{8aac}\u{660e}\u{6587}\u{306f}\u{3068}\u{3066}\u{3082}\u{9577}\u{3044}", "aliases": []},
                {"name": "beta", "desc": "short", "aliases": []},
            ],
            "positional_choices": [],
            "positionals": [],
        });
        write_result(&cache, "widetool", "help", &result_from_json(&widetool)).expect("widetool");

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
set -e
source "$SHIM/inshellah.bash"
comp() { comp_as demotool "$1"; }
comp_as() {
    set -- "$2" "$1"
    COMP_LINE=$1
    COMP_POINT=${#1}
    read -ra COMP_WORDS <<<"$1"
    [[ $1 == *' ' ]] && COMP_WORDS+=("")
    COMP_CWORD=$((${#COMP_WORDS[@]} - 1))
    COMPREPLY=()
    _inshellah_complete "$2" "${COMP_WORDS[COMP_CWORD]}" "${COMP_WORDS[COMP_CWORD - 1]}"
    printf '%s|' "$1" "${COMPREPLY[@]}"
    echo
}
comp 'demotool al'
comp 'demotool alpha r'
comp 'demotool --color='
comp 'demotool --color=al'
comp 'FOO=1 demotool be'
comp 'demotool has'
comp 'demotool "has'
comp 'demotool > out'
comp 'demotool a"l"p'
comp 'time demotool be'
comp 'demotool alpha run '
comp_as time 'time demotool be'
comp_as if 'if demotool al'
comp_as echo 'echo "$(demotool al'
comp_as echo 'echo "x$(demotool --color=al'
comp_as echo 'echo "$(demotool "al'
comp_as echo 'echo `demotool al'
comp_as command 'command demotool al'
comp_as exec 'exec demotool al'
comp_as echo 'echo "$(command demotool al'
for value in 1 true ' Yes ' ON 0 off '' maybe; do
    INSHELLAH_TAKEOVER=$value _inshellah_takeover && state=on || state=off
    echo "takeover '$value' => $state"
done
_comp_command() { COMPREPLY=(bc); }
_inshellah_wrap_unwrappers
_inshellah_wrap_unwrappers
unwrap() {
    COMP_LINE=$1 COMP_POINT=${#1}
    read -ra COMP_WORDS <<<"$1"
    COMP_CWORD=$((${#COMP_WORDS[@]} - 1))
    COMPREPLY=()
    _comp_command nohup "${COMP_WORDS[COMP_CWORD]}" x
    printf '%s|' "unwrap $1" "${COMPREPLY[@]}"
    echo
}
unwrap 'nohup demotool al'
unwrap 'nohup nosuchtool al'
INSHELLAH_TAKEOVER=1 unwrap 'nohup demotool al'
INSHELLAH_TAKEOVER=1 unwrap 'nohup nosuchtool al'
"#;

#[test]
fn bash_shim_seam() {
    let fixture = Fixture::new("inshellah-seam-bash");
    let Some(out) = fixture.run("bash", &["-c"], BASH) else {
        eprintln!("skipping: bash not found");
        return;
    };
    expect(
        &out,
        r#"demotool al|alpha|
demotool alpha r|run|reset|
demotool --color=|always|never|
demotool --color=al|always|
FOO=1 demotool be|beta|
demotool has|has\ space|
demotool "has|has space|
demotool > out|
demotool a"l"p|alpha|
time demotool be|beta|
demotool alpha run |
time demotool be|beta|
if demotool al|alpha|
echo "$(demotool al|$(demotool alpha |$(demotool alpha  |
echo "x$(demotool --color=al|x$(demotool --color=always |x$(demotool --color=always  |
echo "$(demotool "al|"$(demotool "alpha"|
echo `demotool al|alpha|
command demotool al|alpha|
exec demotool al|alpha|
echo "$(command demotool al|$(command demotool alpha |$(command demotool alpha  |
takeover '1' => on
takeover 'true' => on
takeover ' Yes ' => on
takeover 'ON' => on
takeover '0' => off
takeover 'off' => off
takeover '' => off
takeover 'maybe' => off
unwrap nohup demotool al|bc|
unwrap nohup nosuchtool al|bc|
unwrap nohup demotool al|alpha|
unwrap nohup nosuchtool al|bc|
"#,
    );
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
    compstate[context]=${ctx:-command}
    words=("${(@z)1}")
    [[ $1 == *' ' ]] && words+=('')
    CURRENT=$#words
    PREFIX=$words[-1]
}
comp() {
    setup "$1"
    print -rn -- "$1 => "
    _inshellah
    print
}
comp 'demotool al'
comp 'demotool alpha r'
COLUMNS=12 comp 'demotool alpha r'
comp 'demotool --'
comp 'demotool --color='
comp 'demotool has'
comp 'demotool --color always '
comp 'demotool alpha run '
comp 'pwd -'
INSHELLAH_TAKEOVER=1 service=-first- comp 'demotool alpha run '
service=-first- comp 'demotool al'
INSHELLAH_TAKEOVER=0 service=-first- comp 'demotool al'
INSHELLAH_TAKEOVER=" On " service=-first- comp 'demotool al'
INSHELLAH_TAKEOVER=on _inshellah_prev_first=_prev service=-first- comp 'demotool alpha run '
_inshellah_prev_first=_prev service=-first- comp 'demotool al'
ctx=redirect INSHELLAH_TAKEOVER=1 _inshellah_prev_first=_prev service=-first- comp 'cat < pd'
ctx=redirect _inshellah_prev_first=_prev service=-first- comp 'cat < pd'
ctx=redirect service=-first- comp 'cat < pd'
ctx=redirect service=-default- comp 'cat < pd'
mkdir -p $HOME/pdir/sub
cd $HOME
comp 'dirtool --out=pd'
comp 'dirtool --out=pdir/'
COLUMNS=24 comp 'widetool '
setopt ksharrays nomatch kshglob shwordsplit nounset
comp 'demotool alpha r'
comp 'demotool alpha run '
_inshellah_prev_first=_prev service=-first- comp 'demotool al'
comp 'dirtool --out=pd'
service=-first- INSHELLAH_TAKEOVER=1 comp 'demotool al'
typeset -A _comps
compdef() { _comps[$2]=$1 }
_inshellah_register
print -r -- "registered ${_comps[-first-]} ${_comps[-default-]}"
"#;

#[test]
fn zsh_shim_seam() {
    let fixture = Fixture::new("inshellah-seam-zsh");
    let Some(out) = fixture.run("zsh", &["-f", "-c"], ZSH) else {
        eprintln!("skipping: zsh not found");
        return;
    };
    expect(
        &out,
        "demotool al => alpha {alpha  -- first}
demotool alpha r => run|reset {run    -- run it|reset  -- reset it}
demotool alpha r => run|reset {run    -...|reset  -...}
demotool -- => --color|--verbose {--color    -- paint the output <WHEN>|--verbose  -- say more}
demotool --color= => --color=always|--color=never
demotool has => has space {has space  -- quoted}
demotool --color always  => alpha|beta|has space {alpha      -- first|beta       -- second|has \
         space  -- quoted}
demotool alpha run  => default
pwd - => default
demotool alpha run  => 
demotool al => 
demotool al => 
demotool al => alpha {alpha  -- first}
demotool alpha run  => prev
demotool al => prev
cat < pd => prev
cat < pd => prev
cat < pd => 
cat < pd => 
dirtool --out=pd => --out=pdir/ S[] {--out=pdir/  -- directory}
dirtool --out=pdir/ => --out=pdir/sub/ S[] {--out=pdir/sub/  -- directory}
widetool  => alpha|beta {alpha  -- \u{65e5}\u{672c}\u{8a9e}\u{306e}\u{8aac}...|beta   -- short}
demotool alpha r => run|reset {run    -- run it|reset  -- reset it}
demotool alpha run  => default
demotool al => prev
dirtool --out=pd => --out=pdir/ S[] {--out=pdir/  -- directory}
demotool al => alpha {alpha  -- first}
registered _inshellah _inshellah
",
    );
}

const FISH: &str = r#"
source $SHIM/inshellah.fish
for line in 'demotool al' 'demotool alpha r' 'demotool --' 'demotool --color=' 'demotool has' 'demotool "al' 'demotool has\\ sp'
    echo "$line =>" (complete -C $line | string join '|')
end
function show
    echo "$argv[1] =>" (complete -C $argv[2] | string join "|")
end
complete -c twotool -f -a nativechoice
complete -c fwrap -x -a "(__fish_complete_subcommand)"
show "fallback keeps fish own" "twotool "
set -g INSHELLAH_TAKEOVER 1
show "takeover reaches loaded" "twotool al"
show "takeover wrapper" "fwrap demotool al"
show "takeover defers to fish" "twotool alpha run "
set -g INSHELLAH_TAKEOVER 0
show "zero is off" "twotool "
set -g INSHELLAH_TAKEOVER " TRUE "
show "trimmed and case folded" "twotool al"
set -e INSHELLAH_TAKEOVER
show "unset is off" "twotool "
complete -c ordtool -k -f -a zz3
complete -c ordtool -k -f -a aa1
complete -c ordtool -k -f -a mm2
show "order vanilla" "ordtool "
set -g INSHELLAH_TAKEOVER 1
show "order under takeover" "ordtool "
complete -c ordtool -k -f -a RT2
show "runtime spec after defer" "ordtool "
set -g INSHELLAH_TAKEOVER 0
show "runtime spec after flip" "ordtool "
set -g INSHELLAH_TAKEOVER 1
set -l before (complete -c twotool | count)
source $SHIM/inshellah.fish
show "resourced keeps fish own" "twotool alpha run "
echo "resourced keeps completions =>" (test (complete -c twotool | count) -eq $before; and echo same; or echo changed)
echo "ssh like =>" (__inshellah_wraps "complete -c ssh -n 'test (__fish_number_of_cmd_args_wo_opts) -ge 2' -xa '(__fish_complete_subcommand --fcs-skip=2)'"; and echo wraps; or echo plain)
echo "xargs like =>" (__inshellah_wraps "complete --exclusive xargs -a '(__fish_complete_subcommand)'"; and echo wraps; or echo plain)
echo "sudo like =>" (__inshellah_wraps "complete --exclusive sudo -a '(__fish_complete_sudo_subcommand)' -n 'not __fish_seen_argument -s e'"; and echo wraps; or echo plain)
for i in (seq 3000)
    complete -c bigtool -l opt$i
end
set -g INSHELLAH_TAKEOVER 1
set -l began (date +%s%N)
complete -C "bigtool --opt1" >/dev/null
set -l took (math "("(date +%s%N)" - $began)/1000000")
echo "big first load =>" (test $took -lt 3000; and echo fast; or echo slow $took)
echo "big taken =>" (contains bigtool $__inshellah_taken; and echo yes; or echo no)
complete -c pendtool -f -a mine
set -g __inshellah_pre_pendtool (__inshellah_snapshot pendtool)
set -g __inshellah_pending pendtool
complete -c pendtool -f -a half
__inshellah_resume
echo "interrupted load =>" (contains pendtool $__inshellah_taken; and echo taken; or echo lost) (string match -q '*half*' (complete -c pendtool); and echo stale; or echo clean) (count $__inshellah_pending)
set -l late $PATH[1]/latetool
printf '#!/bin/sh\n' > $late
chmod +x $late
__inshellah_prompt
echo "late binary =>" (contains latetool $__inshellah_loaded; and echo loaded; or echo missed)
"#;

const FISH_USER_DIR: &str = r#"
set -g fish_complete_path $HOME/usercomp $fish_complete_path
mkdir -p $HOME/usercomp
echo "complete -c demotool -f -a userchoice" > $HOME/usercomp/demotool.fish
set -gx INSHELLAH_TAKEOVER yes
source $SHIM/inshellah.fish
echo "user file =>" (complete -C "demotool " | string join "|")
"#;

#[test]
fn fish_shim_seam() {
    let fixture = Fixture::new("inshellah-seam-fish");
    let Some(out) = fixture.run("fish", &["--no-config", "-c"], FISH) else {
        eprintln!("skipping: fish not found");
        return;
    };
    expect(
        &out,
        "demotool al => alpha\tfirst
demotool alpha r => run\trun it|reset\treset it
demotool -- => --color\tpaint the output <WHEN>|--verbose\tsay more
demotool --color= => --color=always|--color=never
demotool has => has space\tquoted
demotool \"al => alpha\tfirst
demotool has\\ sp => has space\tquoted
fallback keeps fish own => nativechoice
takeover reaches loaded => alpha\tfirst
takeover wrapper => alpha\tfirst
takeover defers to fish => nativechoice
zero is off => nativechoice
trimmed and case folded => alpha\tfirst
unset is off => nativechoice
order vanilla => mm2|aa1|zz3
order under takeover => mm2|aa1|zz3
runtime spec after defer => RT2|mm2|aa1|zz3
runtime spec after flip => RT2|mm2|aa1|zz3
resourced keeps fish own => nativechoice
resourced keeps completions => same
ssh like => plain
xargs like => wraps
sudo like => wraps
big first load => fast
big taken => yes
interrupted load => taken clean 0
late binary => loaded
",
    );
    let user = fixture
        .run("fish", &["--no-config", "-c"], FISH_USER_DIR)
        .expect("fish ran once already");
    expect(&user, "user file => userchoice");
    assert!(
        fixture
            .dir
            .path()
            .join("inshellah/fish/demotool.fish")
            .exists(),
        "fish-stubs wrote no stub for demotool"
    );
}
