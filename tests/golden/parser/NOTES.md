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

## Known-pinned bugs (these goldens will change *deliberately* in later phases)

- **`getent` — type confusion (Phase 4 target).** `passwd`/`group`/`hosts` are
  positional *database choices* mined from DESCRIPTION prose, but they are
  currently filed under `subcommands` (because the model has no positional-
  choices channel). When Phase 4 introduces `CompletionKind`, these move out of
  `subcommands` and this golden is re-blessed to reflect it.
- **`widget` — empty COMMANDS (Phase 5 target).** A `.SH COMMANDS` section in
  `.TP`/`.B name` tagged-list form yields **zero** subcommands today — the
  strategy/section extractors don't cover this layout, and the failure is
  silent (empty, indistinguishable from "no subcommands"). Phase 5's principled
  selection should surface `create`/`list`/`remove`; this golden will then be
  re-blessed to show them.
