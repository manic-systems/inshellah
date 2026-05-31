def fail [msg: string] {
    error make {msg: $msg}
}

def assert-eq [actual expected msg: string] {
    if $actual != $expected {
        fail $"($msg): expected ($expected | to nuon), got ($actual | to nuon)"
    }
}

# The dynamic dispatch (git/jj/systemctl/kubectl/nix/etc.) is now driven
# from the Rust binary and covered by the `dynamic_complete` integration
# test suite under tests/. What remains here is the shim contract: the
# closure must read `^inshellah complete`'s JSON, pass through valid
# results, and fall back to null on anything malformed or empty.

let completer = $env.config.completions.external.completer

'[{"value":"--demo","description":"from the static cache"}]' | save --force $env.INSHELLAH_STATIC_FILE
let pass_through = do $completer [demo ""]
assert-eq ($pass_through | get 0.value) "--demo" "shim returns the binary's JSON unchanged"

let commandline_pass_through = inshellah-complete-commandline "demo " 5
assert-eq ($commandline_pass_through | get 0.value) "--demo" "commandline adapter returns the binary's JSON unchanged"

let arg_file = (mktemp)
with-env { INSHELLAH_ARG_FILE: $arg_file } {
    let _ = inshellah-complete-commandline 'demo "quoted arg"  --flag ' 26
}
let parsed_args = (open $arg_file | lines)
assert-eq $parsed_args [complete demo "quoted arg" --flag ""] "commandline adapter uses nu ast tokenization"

# Regression: stub-extern commands parse with a shape_internalcall head, not
# shape_external. The tokenizer must keep that head, else the command name is
# stripped and `jj git a` gets completed as if it were `git a`. The fake
# backend echoes its args, so this asserts the real spans — unlike the
# --ide-complete check, whose backend ignores args entirely.
extern "jj" [...args: string@inshellah-complete-commandline]
let stub_arg_file = (mktemp)
with-env { INSHELLAH_ARG_FILE: $stub_arg_file } {
    let _ = inshellah-complete-commandline 'jj git a' 8
}
let stub_args = (open $stub_arg_file | lines)
assert-eq $stub_args [complete jj git a] "stub-extern command head (shape_internalcall) survives tokenization"

"[]" | save --force $env.INSHELLAH_STATIC_FILE
let empty_list = do $completer [demo ""]
assert-eq $empty_list null "empty list collapses to null so nu's file completer can take over"

"null" | save --force $env.INSHELLAH_STATIC_FILE
let null_payload = do $completer [demo ""]
assert-eq $null_payload null "literal null payload collapses to null"

"not-json{" | save --force $env.INSHELLAH_STATIC_FILE
let malformed = do $completer [demo ""]
assert-eq $malformed null "malformed JSON falls back to null without erroring"

"" | save --force $env.INSHELLAH_STATIC_FILE

# Sanity: sudo/doas wrappers must accept arbitrary positional tails so
# the @complete external decorators don't error on partially-typed
# command-line state.
def _assert_elevation_wrappers_accept_command_tails [p: path] {
    sudo nix-env --set -p /nix/var/nix/profiles/system $p
    doas nix-env --set -p /nix/var/nix/profiles/system $p
}
