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
        let stub = bin.join("demotool");
        fs::write(&stub, "#!/bin/sh\nexit 0\n").expect("write stub executable");
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod");

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
comp() {
    COMP_LINE=$1
    COMP_POINT=${#1}
    read -ra COMP_WORDS <<<"$1"
    [[ $1 == *' ' ]] && COMP_WORDS+=("")
    COMP_CWORD=$((${#COMP_WORDS[@]} - 1))
    COMPREPLY=()
    _inshellah_complete demotool "${COMP_WORDS[COMP_CWORD]}" "${COMP_WORDS[COMP_CWORD - 1]}"
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
"#,
    );
}

const ZSH: &str = r#"
# nixos /etc/zshenv resets PATH even under -f
export PATH=$SEAM_PATH
source "$SHIM/inshellah.zsh"
typeset -A compstate
compadd() {
    local -a _vals _displays
    while (($#)); do
        case $1 in
            -a) _vals=("${(@P)2}"); shift 2 ;;
            -d) _displays=("${(@P)2}"); shift 2 ;;
            -V) shift 2 ;;
            *) shift ;;
        esac
    done
    print -rn -- "${(j:|:)_vals}"
    (($#_displays)) && print -rn -- " {${(j:|:)_displays}}"
    return 0
}
_description() { eval "$3=()" }
_files() { print -rn -- ' +files' }
_default() { print -rn -- 'default' }
_prev() { print -rn -- 'prev' }
comp() {
    compstate[context]=command
    words=("${(@z)1}")
    [[ $1 == *' ' ]] && words+=('')
    CURRENT=$#words
    PREFIX=$words[-1]
    print -rn -- "$1 => "
    _inshellah
    print
}
comp 'demotool al'
comp 'demotool alpha r'
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
",
    );
}

const FISH: &str = r#"
source $SHIM/inshellah.fish
for line in 'demotool al' 'demotool alpha r' 'demotool --' 'demotool --color=' 'demotool has' 'demotool "al' 'demotool has\\ sp'
    echo "$line =>" (complete -C $line | string join '|')
end
set -g INSHELLAH_TAKEOVER 1
test "$fish_complete_path[1]" = "$__inshellah_stub_dir"; and echo "takeover => stubs first"
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
takeover => stubs first
",
    );
    assert!(
        fixture
            .dir
            .path()
            .join("inshellah/fish/demotool.fish")
            .exists(),
        "fish-stubs wrote no stub for demotool"
    );
}
