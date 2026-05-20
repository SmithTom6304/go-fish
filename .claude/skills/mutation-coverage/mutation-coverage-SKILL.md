---
name: mutation-coverage
description: Run cargo-mutants on the go-fish workspace, then triage the surviving mutants into a ranked report of high-leverage test gaps. Use when the user asks to find weak test coverage, run mutation testing, check what their tests miss, improve mutation score, or asks about cargo-mutants. STOPS at triage — does not write tests unless explicitly asked in a follow-up.
---

# Mutation coverage triage

This skill runs `cargo-mutants` on the workspace and produces a ranked triage of surviving mutants. It deliberately stops at the report — the user has opted into a workflow where they pick which gaps to address and re-prompt for fixes.

## 1. Check prerequisite

```bash
cargo mutants --version
```

If the command isn't found, tell the user:

> cargo-mutants isn't installed. Install it with `cargo install cargo-mutants --locked` (takes a minute or two), then re-run.

Stop. Do not attempt to install it for them — it's a global tool install and they should opt into it.

## 2. Pick scope

Use `AskUserQuestion` to choose the run scope. Default options:

- **Changed files vs main** — `cargo mutants --in-diff origin/main`. Fastest, best for "did the PR I'm about to open leave any gaps?"
- **A specific package** — `cargo mutants --package <name>`. Packages: `go-fish`, `go-fish-game-server`, `go-fish-tui-client`. (`go-fish-web` is excluded in `.cargo/mutants.toml` — pure types.)
- **A specific file** — `cargo mutants --file <path>`. Best when iterating on one module.
- **Whole workspace** — `cargo mutants`. Warn that this is slow (likely 30+ minutes on this codebase) and should usually be a scheduled job, not an interactive one.

If the user has just made edits in a particular file or package and asks for triage, infer that scope without asking and confirm in one sentence.

## 3. Run

Always set `PROPTEST_CASES=20` for the run — the workspace has proptests configured for up to 10,000 cases (see `go-fish/tests/complete_games.rs`), which would make every mutant test minutes long. The `.cargo/mutants.toml` already enables `--all-features` (needed for the `bots`-gated integration tests) and uses nextest if installed.

Example invocation:

```bash
PROPTEST_CASES=20 cargo mutants --in-diff origin/main
```

The run streams progress; let it complete. Output lands in `mutants.out/` at the workspace root. Tell the user how long the run took.

If `mutants.out/` doesn't already appear in `.gitignore`, suggest adding it (don't add it without asking — they may want to track historical artefacts).

## 4. Read the outcomes

Parse `mutants.out/outcomes.json`. The interesting entries are those where the outcome is `MissedMutant` (also called "missed" in the text reports) — these are mutants that survived all tests. Also note `Timeout` entries (test took too long — may need timeout bump) and skip `Unviable` (didn't compile, not a coverage gap).

For each missed mutant, the JSON includes:
- The source file and line range
- A human description of the mutation (e.g. "replace + with - in `Hand::receive_hook`")
- The diff applied

Read each mutant's source context (file + ~10 lines around the mutation) before classifying. Don't triage from the description alone — the context is usually what tells you whether a mutant is genuine or equivalent.

## 5. Triage

Classify each surviving mutant into one of:

- **Genuine gap** — the mutation changes observable behaviour and no test catches it. These are the ones worth pinning with tests.
- **Equivalent mutant** — the mutation produces semantically identical behaviour (e.g. `x * 1` ↔ `x`, dead branches, redundant clamps). Note these so the user can suppress them with `#[mutants::skip]` or an `exclude_re` entry, but they don't need new tests.
- **Unreachable** — the mutation is in code that can never run given existing invariants. Often suggests the code itself is dead, not a coverage gap.
- **Not worth testing** — logging, telemetry, error message wording, debug-only paths. Cost of a pinning test exceeds the value.

Then rank by impact, weighted toward the core engine and server logic:

1. `go-fish/src/lib.rs`, `go-fish/src/bots.rs` — core invariants. Highest priority.
2. `go-fish-game-server/src/lobby.rs`, `go-fish-game-server/src/connection.rs` — protocol correctness.
3. `go-fish-tui-client/src/state.rs`, `network.rs` — client logic.
4. `go-fish-tui-client/src/ui.rs`, `input.rs` — UI behaviour. Often hard to test meaningfully.

## 6. Present the report

Render the triage as a markdown table with columns: file:line, mutation, classification, suggested test (one line, only for "genuine gap" rows). Group by file, sort genuine gaps before equivalents/unreachable.

Lead with a one-paragraph summary: total mutants tested, how many survived, how many you classed as genuine gaps, and which file or area has the highest concentration. End with the explicit next step:

> To address any of these, re-prompt with something like "add tests for the missed mutants in `go-fish/src/bots.rs`" and I'll write them and re-run cargo-mutants on the affected file to confirm each one is killed.

## 7. Do NOT write tests in this skill

The user has explicitly opted into stop-at-triage. Even if the gaps look obvious, do not edit any source or test files as part of this skill's execution. The follow-up is a separate request.

## Reference: useful cargo-mutants flags

- `--in-diff <ref>` — only mutate lines changed vs `<ref>`. Use `origin/main` by default.
- `--package <name>` / `--file <path>` — scope to a package or file.
- `--list` — list mutants that would be tested without running them. Useful for estimating run time.
- `--shard k/N` — split work across N runs (use the kth). For CI parallelism, not interactive use.
- `--baseline=skip` — skip the unmutated baseline run if you've just run `cargo test` and know it passes. Saves a couple of minutes.
- `--json` — JSON to stdout in addition to `mutants.out/`.

## Reference: known noise on this codebase

Things to expect as surviving mutants that are almost always not worth pinning:

- Default values in `Config` structs in `go-fish-game-server/src/main.rs` and `go-fish-tui-client/src/main.rs` — excluded via `main.rs` glob.
- `tracing` macro arguments (log message strings, levels) — typically classified "not worth testing".
- ratatui widget render code in `go-fish-tui-client/src/ui.rs` — without snapshot tests, most mutations here are unkillable. Consider recommending the user adopt `insta` snapshot testing for widgets if these dominate the report.
