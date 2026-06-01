# Composed seam assertions: real nu tokenizer -> real `inshellah` binary -> fixture cache.
#
# Driven by tests/seam_nu.rs, which builds the matching fixture cache (commands
# `demotool` and `othertool`) and puts the real binary on PATH. This is the only
# test that exercises the FULL production path end to end, and it is the guard
# for the stub-extern regression class: a command declared via
#   extern "<cmd>" [...args: string@inshellah-complete-commandline]
# parses with a shape_internalcall head, and if the tokenizer drops that head
# the command name never reaches the binary (`demotool al` -> `inshellah
# complete al` -> null). A fake backend cannot catch this; the real binary can.

def fail [msg: string] { error make {msg: $msg} }

def vals [line: string] {
    let r = (inshellah-complete-commandline $line ($line | str length))
    if $r == null { [] } else { $r | get value }
}

def assert-has [line: string, needle: string] {
    let v = (vals $line)
    if ($needle not-in $v) {
        fail $"($line): expected candidate ($needle), got ($v | to nuon)"
    }
}

def assert-null [line: string] {
    let v = (vals $line)
    if (not ($v | is-empty)) {
        fail $"($line): expected no candidates, got ($v | to nuon)"
    }
}

# stub externs exactly as the nix module installs them
extern "demotool" [...args: string@inshellah-complete-commandline]
extern "othertool" [...args: string@inshellah-complete-commandline]

# 1. command head survives tokenization (the regression): demotool al -> alpha
assert-has "demotool al" "alpha"
# 2. empty token lists all first-level subcommands
assert-has "demotool " "beta"
assert-has "demotool " "gamma"
# 3. depth-2 completion routed through the tokenizer
assert-has "demotool alpha r" "run"
assert-has "demotool alpha r" "reset"
# 4. the command head is actually USED, not just present: demotool has no `st*`
#    subcommand, so this must be empty — NOT othertool's start/stop.
assert-null "demotool st"
# 5. othertool resolves to its own subs (proves per-command dispatch)
assert-has "othertool st" "start"
assert-has "othertool st" "stop"
# 6. elevation wrapper transparency through the tokenizer
assert-has "sudo demotool al" "alpha"

print "SEAM OK"
