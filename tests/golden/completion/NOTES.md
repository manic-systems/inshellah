# Completion goldens

Snapshots of `inshellah complete` output for a hermetic fixture, captured by
`tests/golden_completion.rs`. They pin the **current** behavior of the
resolver/completer so the Phase 1-4 refactor can be proven behavior-preserving.

Re-bless after an *intentional* behavior change:

```
INSHELLAH_BLESS=1 cargo test --test golden_completion
```

then review the diff before committing.

## What each case pins

- `subs_*` — first-level subcommand completion and the depth-guard surface
  (partial token -> fuzzy-ranked subcommands; no match -> `null`).
- `flags_*` — flag completion gated on the `-` trigger, including the
  `(aka --long)` short/long pairing and `<PARAM>` rendering.
- `sub_build_*` — a cached subcommand's own flags; empty token -> `null` handoff.
- `resolve_extra_flags` — on-the-fly resolution of an *uncached* subcommand via
  the binary's `--help`.
- `system_only_subs` — read from the system dir (user/system `--dir` precedence).
- `sudo_passthrough` — elevation-wrapper transparency (`sudo tool b` == `tool b`).

## Known-pinned quirks

None flagged at capture time — all current outputs look correct. If a later
phase intends to *fix* something here, that is a deliberate re-bless, not a
silent golden change.
