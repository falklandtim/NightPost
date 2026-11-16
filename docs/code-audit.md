# Code audit

How we check that NightPost's code stays readable, documented and safe, and what each audit found. The rules being checked are in [AGENTS.md](../AGENTS.md).

## When

- Before installing a new build on the other machines, when a batch of features has landed.
- At least once a month.
- After any large change: a new thread, process or synced record, or a file split.

## The process

1. **Run the numbers:** `scripts/audit.sh` (about 2 minutes; `--quick` skips the build and takes seconds). It runs the tests, clippy and rustdoc, then counts the things below. Compare its summary with the last audit's line in the log at the bottom.
2. **Tests:** all pass. Look at which files have no tests (`grep -L '#\[test\]'`) and whether anything new should have had one.
3. **Warnings:** clippy and rustdoc report no more than last time. New warnings get fixed, not allowed.
4. **Documentation:**
   - Every file has a `//!` summary. The script lists any that don't.
   - The share of public items with a `///` doc comment goes up, never down. Work through the worst files the script lists.
   - Read a few comments in what changed since the last audit: do they say *why*?
5. **Size:** no new file over 1,500 lines and no new function over 100. Anything already over that was touched since the last audit should have shrunk.
6. **Safety:**
   - Every `unsafe` has a `// SAFETY:` comment.
   - The count of `unwrap`/`expect`/`panic` outside tests doesn't rise. Each one left is on a lock, a thread spawn, or a value that can't fail, with the reason in its message.
7. **README:** the script lists the commits since the last audit. Each user-visible one is described in the README, and nothing in the README is out of date (check "Not yet implemented" and "Known issues" especially).
8. **Architecture:** if a thread, process, module, table or synced record was added, `docs/architecture.md` says so.
9. **Mac and Linux:** each new feature works on both, or the README says it's Mac-only.
10. **Dependencies and platforms** ([AGENTS.md](../AGENTS.md) section 6): no security advisories and no future-incompatibility warnings (both in the script's output; install `cargo-audit` once with `cargo install cargo-audit`). Run `cargo update`, the full checks, the Linux build and `scripts/ios-rust.sh`, and commit it as "Update dependencies". Note any major versions waiting, whether a newer stable Rust is out, and any new macOS, iOS, Xcode or Fedora release that needs trying.
11. **Record it:** add a dated entry to the log below: the summary numbers, what was fixed, and what's left. The script uses the newest date to list commits for step 7.

## Grades

| Grade | Means |
|---|---|
| A | Meets the rule everywhere |
| B | Meets it in most places; gaps are listed |
| C | Gaps that slow down anyone new to the code |
| D | Missing |

## Log

### 2026-11-11 (first audit, before fixes)

Commit `0d870ce`. 23,675 lines of Rust in two crates.

| Check | Result | Grade |
|---|---|---|
| Tests | 137 passed, 0 failed (1 ignored) | B: none for `web_host.rs`, `prefs.rs`, `credentials.rs`, `onboarding.rs`; IMAP code tested on the demo account only |
| Clippy warnings | 4 | A− |
| Rustdoc warnings | 4 | B |
| File summaries | 35 of 35 | A−: the biggest (`app.rs`, `models.rs`, `prefs.rs`) have only one line |
| Public items documented | 329 of 567 (58%) | B−: worst `web_host.rs` 36/48 missing, `store.rs` 32/80, `models.rs` 22/41, `theme.rs` 19/32 |
| Comments explain why | Yes, in the parts read | A− |
| Files over 1,500 lines | 2 (`app.rs` 6,353; `store.rs` 1,549) | C+ |
| Functions over 100 lines | 21 (largest `thread_list` 481, MCP `call` 304) | C+ |
| unsafe without SAFETY | 4 of 13 (`contacts.rs` ×3, `web_host.rs` ×1) | B |
| unwrap/expect/panic outside tests | 39, mostly locks and thread spawns in `worker.rs` | B |
| TODO/FIXME | 0 | A |
| README | Stale: said pins are local, HTML email on Linux and draft autosave not done; folder add/delete, cross-account moves, row tints and pin sync not mentioned | B |
| Developer docs | No architecture document, no house rules | C |

Fixes planned: update the README; add `AGENTS.md` and `docs/architecture.md`; doc comments for `store.rs`, `models.rs` and `web_host.rs`; the missing SAFETY comments; the clippy and rustdoc warnings; split `app.rs` into modules.

### 2026-11-11 (after fixes)

Same day, after the fixes. Tests also run on Fedora (Linux): 137 passed, no compiler warnings.

| Check | Before | After | Grade |
|---|---|---|---|
| Tests | 137 passed | 137 passed (Mac and Linux) | B (gaps unchanged) |
| Clippy warnings | 4 | 0 | A |
| Rustdoc warnings | 4 | 0 | A |
| Public items documented | 329/567 (58%) | 385/533 (72%) | B |
| Files over 1,500 lines | 2 | 0 | A |
| Functions over 100 lines | 21 | 21 (`thread_list` 481 → 188) | C+ |
| unsafe without SAFETY | 4 of 13 | 0 of 13 | A |
| unwrap/expect/panic outside tests | 39 | 39 | B |
| README | stale | current | A |
| Developer docs | none | `AGENTS.md`, `CLAUDE.md`, `docs/architecture.md`, this file | A− |

What changed:

- **README:** pins sync, HTML email on Linux, folder add/delete, cross-account moves, row tints, account arrows; "Not yet implemented" corrected; a "For developers" section.
- **New docs:** `AGENTS.md` (house rules; `CLAUDE.md` points to it), `docs/architecture.md`, this file, and `scripts/audit.sh`.
- **Doc comments:** for `store`, `models` and `web_host`'s public API. `web_host`'s per-platform functions are now `pub(super)`, which is what they always were in effect. That, and the helpers moved out of `app.rs`, is why the count of public items fell from 567 to 533.
- **`app.rs` (6,353 lines) is now `app/`:** `mod.rs` (state and start-up), `sidebar`, `list`, `reader`, `toolbar`, `sheets`, `actions`, `input` and `ui_tests`. Nothing over 1,000 lines.
- **`thread_list`:** split into `empty_list`, `advance_swipe`, `row_look`, `paint_swipe_under`, `paint_row`, `quick_actions`, `row_menu` and `apply_row_pick`. The right-click menu now returns one `RowPick` instead of setting eight variables.
- **`store.rs` is now `store/`:** `mod.rs`, plus `categories.rs` (tabs and rules) and `pins.rs`.
- **SAFETY comments:** on the three Contacts blocks and one Wayland block.
- **Warnings:** the four clippy and four rustdoc warnings, and a Linux-only dead-code warning.

Still to do, in order:

1. **Long functions:** 21 still over 100 lines, `thread_list` (188) among them. Worst: MCP `call` (304; one arm per tool, which could each be a function), `windows` (243), `sidebar` (233), `run_action` (215). Shrink them when working there.
2. **Missing doc comments:** 148 public items, mostly in `theme.rs`, `mail_view.rs`, `style.rs`, `composer.rs` and `prefs.rs`.
3. **unwrap/expect:** 39 left. Most are lock and thread-spawn `expect`s in `worker.rs`; `snooze.rs` (7) and `attachments.rs` (6) need a look.
4. **Tests:** none yet for `web_host.rs`, `prefs.rs`, `credentials.rs` or `onboarding.rs`.
5. **The unused `outbox` table:** drop it or use it (asks first: it's a schema change).
