# PRD 0001 — Cargo workspace skeleton, CI, and `spinel --version`

Tracks [#1](https://github.com/ar4mirez/spinel/issues/1). Milestone: Phase 0: skeleton. `P0`, `size:S`, `area:infra`.

## Objective

Turn an empty repo of design docs into a repo that compiles. After this slice a
contributor can clone, run `cargo build --release`, and get a `spinel` binary that
reports its own version. Every later slice fills in a crate that already exists here.

This slice ships **no Ruby semantics**. No parser, no VM, no gems. The value is that
the skeleton in `architecture.md` becomes real and CI starts guarding it.

## Non-goals

- Any subcommand from `docs/cli.md` (`run`, `install`, `test`, ...). Each is its own issue.
- Depending on `ruby-prism` yet. Issue #3 adds it when it lowers to `spinel_ast`.
- A correct RubyGems platform string (`arm64-darwin24`). Needs host-triple detection; issue #3+.
- Windows. `README.md` lists it as a non-goal for first releases.

## Users

| User | Needs from this slice |
|---|---|
| Contributor | `cargo build` works; obvious where a new subsystem's code goes |
| CI | One command per check, fails loudly, runs on both tier-1 targets |
| End user | `spinel --version` answers "what do I have installed, and what Ruby does it speak" |

## Requirements

### R1 — Workspace with the crates from `architecture.md`

Seven members, dependency edges exactly as the doc's table states:

| crate | depends on |
|---|---|
| `spinel-ast` | — |
| `spinel-parse` | ast |
| `spinel-vm` | ast |
| `spinel-core` | vm |
| `spinel-ext` | vm |
| `spinel-jit` | vm |
| `spinel-cli` | all of the above; produces the `spinel` binary |

A virtual manifest at the root owns shared version, edition, and lints. Crates are
empty on purpose: a placeholder type per crate, no speculative API.

### R2 — `spinel --version`

```
spinel 0.0.1 (ruby 4.0.0) [arm64-darwin]
```

Three facts on one line: engine version, the Ruby **language** version this build
implements, and the host platform. Modelled on `ruby -v`, which every Ruby user can
already read. `RUBY_ENGINE_VERSION` / `RUBY_VERSION` / `RUBY_PLATFORM` in `docs/cli.md`
are later fed from the same constants, so the CLI and the VM cannot drift apart.

### R3 — `spinel --help` and bare `spinel`

Bare `spinel` prints help and exits non-zero, so a typo never looks like success.
Help states plainly that subcommands are not implemented yet rather than pretending.

### R4 — CI on macOS arm64 and Linux x64

One workflow, matrix over `macos-14` (arm64) and `ubuntu-latest` (x64):
build release, run tests, and separately check `fmt` and `clippy -D warnings`.

### R5 — Integration test that drives the built binary

Per `CLAUDE.md`, tooling done means a test shelling out to the built binary. Covers
`--version`, `--help`, exit codes, and the error paths.

Landed in `crates/spinel-cli/tests/`, **not** a top-level `tests/` package — see
"What the audit caught" below. No `fixtures/` yet: nothing in this build reads a file,
so the first fixture arrives with `spinel run` in phase 1.

## Definition of done

- [x] `cargo build --release` produces `target/release/spinel` (774 KB)
- [x] `spinel --version` prints `spinel 0.0.1 (ruby 4.0.0) [arm64-darwin]`
- [x] `cargo test` passes: 6 integration + 2 unit
- [x] `cargo fmt --check` and `cargo clippy -D warnings` clean
- [x] CI green on macOS arm64 and Linux x64
- [x] `docs/architecture.md` matches what was built

## Open decisions for the owner

1. **Version string format.** R2 is a judgement call; `spinel 0.0.1` alone would also
   satisfy the issue. Flagged because it becomes `RUBY_DESCRIPTION` later.
2. **Starting version `0.0.1`.** Repo is pre-code; nothing executes Ruby yet.

## Tasks

| # | Task | Proves |
|---|---|---|
| T1 | Root virtual manifest: shared version, edition 2024, MSRV, workspace lints | `cargo metadata` lists 8 members |
| T2 | Scaffold the six library crates with the dependency edges from R1 | `cargo build` |
| T3 | `spinel-cli`: `[[bin]] name = "spinel"`, clap parser, version + help | `spinel --version` |
| T4 | `tests/` package: fixture + integration test over the built binary | `cargo test` |
| T5 | `.github/workflows/ci.yml`: build/test matrix + fmt + clippy | green CI |
| T6 | CLI UX audit: version/help/error output, exit codes, `--help` on a bad flag | audit table below |
| T7 | Reconcile `docs/architecture.md` with what shipped | doc diff |


## What the audit caught

Everything below was found by running the binary as a user would, not by reading the
diff. Each was fixed in this PR.

### Correctness

**A1 — The integration test was asserting against a stale binary.** *(the real find)*
The suite first lived in a top-level `tests/` package, per the repo layout in
`architecture.md`. Cargo sets `CARGO_BIN_EXE_*`, and guarantees the binary is rebuilt
before the test runs, **only for tests inside the binary's own package**. A separate
package has no dependency edge to `spinel-cli`, so `cargo test` ran the suite against
whatever `target/debug/spinel` happened to be lying around. It was caught only because
a deliberate CLI change failed a test that should have passed.

A test that silently checks the wrong artifact is worse than no test: it reports green
while the thing it guards is broken. Every tooling slice after this one would have
inherited it.

Fixed by moving the suite into `crates/spinel-cli/tests/` and using
`env!("CARGO_BIN_EXE_spinel")`. Verified by deleting `target/debug/spinel` and running
`cargo test` alone: Cargo rebuilds it first. `architecture.md` and `CLAUDE.md` were
corrected in this PR, per the repo's own "fix the doc in the same slice" rule.

### Clarity

| | Found | Fixed to |
|---|---|---|
| C1 | `spinel app.rb` → `error: unexpected argument 'app.rb' found`. The single likeliest first thing a Ruby developer types, answered with a parser complaint. | `spinel: cannot run \`app.rb\` — this build has no VM yet.` plus when it will work and where to watch. |
| C2 | `Usage: spinel` — told the reader nothing, despite options existing. | `Usage: spinel [OPTIONS]` |
| C3 | `-v` worked but was invisible in `--help`, so Ruby users could not discover the spelling they already use. | Shown as `[alias: -v]` |
| C4 | The `--version` help text carried its own rationale ("because that is how `ruby` spells it") into user-facing output. | Rationale moved to a code comment; help reads `Print version and exit`. |
| C5 | Help said "No subcommands are available", which is narrower than the truth. | "This build does not run Ruby yet." |

### Verified, no change needed

- `--version` goes to stdout, errors to stderr, so both pipe correctly.
- Exit codes: `0` success, `2` usage error. A bare `spinel` is never a silent success.
- Typos get a suggestion: `--verison` → `tip: a similar argument exists: '--version'`.
- No ANSI escapes when piped or under `NO_COLOR=1`.
- `-v`, `-V`, `--version` produce byte-identical output.

### Performance

Startup is the number that matters for a Ruby competitor, and the one that will only
get harder to hold as the VM lands. Recorded now as the baseline.

| | median, 50 runs |
|---|---|
| `spinel --version` | **3.1 ms** |
| `ruby --version` (system 3.x, arm64) | 8.7 ms |

Binary: 774 KB. Target from `architecture.md` is under 30 MB with the embedded assets.

## Follow-ups filed

None blocking. Two things this slice deliberately did not do, both already covered by
existing issues: the Prism dependency (#3) and a real RubyGems platform string, which
needs host-triple detection and is marked with a `ponytail:` comment in
`spinel-vm/src/lib.rs`.
