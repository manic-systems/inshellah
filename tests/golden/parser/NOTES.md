# Parser goldens

Compact snapshots of the parser layer over `tests/fixtures/parser/*`, captured
by `tests/golden_parser.rs`. They pin **current** parse behavior so the Phase 4
(model) and Phase 5 (parser dedup / strategy selection) refactors can be proven
behavior-preserving. `.txt` = `--help` text; `.1` = groff manpage.

Re-bless after an intentional change:

```
INSHELLAH_BLESS=1 cargo test --test golden_parser
```

## What each case pins

- `cargo_help` — comma-aliased subcommands (`build, b`) keep the canonical
  name (regression already fixed); option parsing; the `COMMAND` positional.
- `getent` — prose-mined positional value choices (`passwd`/`group`/`hosts`)
  land in the dedicated `positional_choices` channel, NOT `subcommands`
  (Phase 4 fix). The completer still offers them in the argument slot, but
  they never flow into the real-child paths (recursion, supplement, extern
  stubs).

## Known-pinned bugs (these goldens will change *deliberately* in later phases)

- **`widget` — empty COMMANDS (Phase 5 target).** A `.SH COMMANDS` section in
  `.TP`/`.B name` tagged-list form yields **zero** subcommands today — the
  strategy/section extractors don't cover this layout, and the failure is
  silent (empty, indistinguishable from "no subcommands"). Phase 5's principled
  selection should surface `create`/`list`/`remove`; this golden will then be
  re-blessed to show them.
