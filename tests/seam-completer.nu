# composed seam: real `inshellah` binary -> fixture cache, through the external
# completer the module installs. driven by tests/seam_nu.rs, which builds the
# fixture cache (`demotool`, `othertool`) and puts the real binary on PATH.
# the only test that exercises the full production path end to end.

def fail [msg: string] { error make {msg: $msg} }

def vals [spans: list<string>] {
    let r = (inshellah-complete-spans $spans)
    if $r == null { [] } else { $r | get value }
}

def assert-has [spans: list<string>, needle: string] {
    let v = (vals $spans)
    if ($needle not-in $v) {
        fail $"($spans | to nuon): expected ($needle), got ($v | to nuon)"
    }
}

def assert-null [spans: list<string>] {
    let v = (vals $spans)
    if (not ($v | is-empty)) {
        fail $"($spans | to nuon): expected none, got ($v | to nuon)"
    }
}

assert-has [demotool al] "alpha"
assert-has [demotool ""] "beta"
assert-has [demotool ""] "gamma"
assert-has [demotool alpha r] "run"
assert-has [demotool alpha r] "reset"
# demotool has no st* sub, so this must be empty, not othertool's start/stop
assert-null [demotool st]
assert-has [othertool st] "start"
assert-has [othertool st] "stop"
# elevation wrapper transparency: rust skips sudo to the real command
assert-has [sudo demotool al] "alpha"

print "SEAM OK"
