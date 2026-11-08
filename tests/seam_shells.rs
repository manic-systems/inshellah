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
