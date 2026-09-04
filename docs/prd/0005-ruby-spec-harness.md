# PRD 0005 — ruby/spec submodule and the `spec/harness/` Rust runner

Tracks [#5](https://github.com/ar4mirez/spinel/issues/5). Milestone: Phase 0: skeleton. `P0`, `size:M`, `area:infra`.

## Objective

Put ruby/spec in the tree, and give the project a way to count it. Compatibility is the definition of done for every engine slice from phase 1 onward, so the corpus and the thing that counts it have to exist before the first slice that claims a number.

The counter is `spec/harness/`, a Rust binary. It is temporary by design: mspec is Ruby, and the moment Spinel runs enough Ruby to run mspec, `spec/harness/` is deleted and `spec/ruby/spec_helper.rb` takes over. That is the phase 2 milestone.

What it can do today is bounded by one fact: there is no VM. Nothing can execute a line of Ruby, so no example can pass or fail. What is left is everything a spec file's *syntax tree* says — which examples exist, what they are called, and which guards stand between them and an interpreter — and that turns out to be the whole reporting skeleton.

## Non-goals

- **Matchers.** The issue names `should ==` and `should_raise`. Neither can mean anything without an evaluator: `should ==` compares two values, and there are no values. Pattern-matching them out of the tree would produce a readiness metric nothing could falsify. They land in phase 1 with the interpreter that gives them meaning. See A1 for the second half of this, which is that `should_raise` is not a thing ruby/spec has.
- **`spec/tags/`.** The skip mechanism `docs/engine.md` describes. Every example is blocked today, so there is nothing a tag could usefully exclude, and inventing a file format before knowing whether it must match mspec's is a guess with a maintenance bill. Lands with phase 1, the first slice that can have a spec worth skipping.
- **`bench/spec-status.md`.** The per-directory progress table CI publishes. It wants a pass column that is not always zero.
- **Expanding shared specs and `eval`.** 219 files build their examples through `it_behaves_like` or an `eval` string (A3). Expanding those means resolving `require_relative` and deciding which string arguments are Ruby — a heuristic, on a tool with a scheduled deletion date. mspec does it correctly for free in phase 2.
- **Running mspec.** Phase 2.

## Users

| User | Needs from this slice |
|---|---|
| Every engine slice from phase 1 on | `scripts/spec.sh <dir>` before and after, and a delta to put in the PR |
| CI | A check that fails when the corpus goes missing or a spec file stops parsing |
| Anyone reading progress | Counts that cannot be inflated by a harness that guesses |

## Requirements

### R1 — `spec/ruby` is a submodule pinned to the target language branch

`.gitmodules` records `branch = master`. ruby/spec keeps `master` on the newest language version and cuts a branch (`2.6`, and so on) only when a version goes out of support; 4.0 is what README.md commits to, so `master` is the branch that matches. The submodule is pinned at `620a912d`, which is the same commit `.github/workflows/ci.yml` was already cloning at — the pin did not move, it moved into the tree.

### R2 — The harness reports, and cannot report a pass it did not earn

Every example gets one of five columns: `passed`, `failed`, `blocked`, `skipped`, or it is a file that could not be parsed. `blocked` is the whole point of the design. An example that cannot be executed is not a pass and not a failure, and folding it into either would make the pass count — which is the project's only progress measure — a lie on the very first slice that produces one.

Today the report reads `0 passed · 0 failed · N blocked`, and phase 1 moves examples out of `blocked` one directory at a time.

### R3 — Guards are evaluated, or they skip

`ruby_version_is` and `platform_is`/`platform_is_not` are evaluated against the target: `LANGUAGE_VERSION` from `spinel-vm`, and the host OS under Ruby's name for it (`darwin`, not `macos`).

Every other guard — `guard -> { }`, `with_feature`, `ruby_bug`, `not_supported_on`, `quarantine!`, and the `platform_is wordsize: 64` form — takes an argument only a VM can evaluate. Those skip with the reason attached. **They are never assumed true.** A guard silently assumed true is an example reported on without being run, which is the same failure as a fake pass, one level down.

The split across the corpus, from `spec-harness --list`:

| Skipped because | Examples |
|---|---|
| `ruby_version_is` excluded it from Ruby 4.0 | 723 |
| `it "..."` with no block — mspec's pending marker | 332 |
| `platform_is`/`platform_is_not` excluded it on this host | 309 |
| A guard the harness cannot evaluate | 490 |

1,364 of the 1,854 skips are real decisions. The remaining 490 — 1.9% of the corpus — are the harness declining to guess, and they resolve for free when mspec takes over.

### R4 — A guard that excludes examples still counts them

An excluded `platform_is :windows` block is walked anyway, and its examples are reported skipped. A total that shrank when you changed laptops would be a useless progress bar.

### R5 — `scripts/spec.sh` takes corpus-relative paths

`scripts/spec.sh core/array`, the spelling `CLAUDE.md` already used. Paths resolve against `spec/ruby` first, so the argument reads the way ruby/spec's own directories do; a path that exists as given is used as given. No argument runs the whole corpus, which is mspec's default too. An uninitialised submodule is caught by name, with the `git submodule update --init` that fixes it, because otherwise it is indistinguishable from a corpus with no specs in it.

### R6 — A file that yields no examples is named, not counted as zero

219 spec files parse cleanly and produce nothing (A3). Reporting `0 examples` and stopping would read as a broken checkout. The report names them and says why in one line.

### R7 — CI asserts the corpus is actually there

The `spec` job runs `scripts/spec.sh language/if_spec.rb` and `scripts/spec.sh language`, and fails if `language/` reports fewer than 2,000 examples. Without a floor, an unfetched submodule reports zero examples, finds no failures, and passes — the same hole `stdlib/`'s test closed in [#4](https://github.com/ar4mirez/spinel/issues/4).

### R8 — The sweep reads the submodule, and the clone is gone

`.github/workflows/ci.yml` loses the `Fetch ruby/spec` step and the `RUBY_SPEC_SHA` pin; the `sweep` job reads `spec/ruby/` from the tree, as [#4](https://github.com/ar4mirez/spinel/issues/4) said it would once this slice landed. Nothing in CI is cloned at a floating pin any more.

## Definition of done

- [x] `spec/ruby` submodule pinned to the target language branch — `master`, at `620a912d`
- [x] `scripts/spec.sh` runs a directory and reports counts
- [x] Harness runs `language/if_spec.rb` and reports, even though everything is blocked
- [x] Integration test that shells out to the built binary, per `CLAUDE.md`
- [x] CI asserts the definition of done, with a floor that an empty corpus cannot pass
- [x] `docs/roadmap.md`, `docs/engine.md`, `docs/architecture.md` corrected in the same PR

## Corpus result

```
$ scripts/spec.sh language/if_spec.rb
spec/ruby/language/if_spec.rb · 52 examples · 0 passed · 0 failed · 52 blocked · 0 skipped · 0.0s
blocked: this build has no VM. Running Ruby lands in phase 1: https://github.com/ar4mirez/spinel/milestones

$ scripts/spec.sh language
80 files · 2735 examples · 0 passed · 0 failed · 2691 blocked · 44 skipped · 0.0s

$ scripts/spec.sh
3835 files · 25624 examples · 0 passed · 0 failed · 23770 blocked · 1854 skipped · 0.3s
```

Zero of the 3,835 `*_spec.rb` files failed to parse, which is an independent confirmation of [#3](https://github.com/ar4mirez/spinel/issues/3)'s lowering over a corpus the sweep only ever checked for unhandled nodes.

The 52 in `if_spec.rb` was cross-checked against `grep -c` on the file. Across `language/`, the harness and a grep for `it` lines agree on every file but one, and that one is A3.

## Tasks

| | Task | Check |
|---|---|---|
| T1 | `spec/ruby` submodule on `master` | `.gitmodules` records the branch; pin equals CI's old SHA |
| T2 | `spec/harness/` crate, workspace member outside `crates/` | `cargo build --workspace` |
| T3 | `describe`/`it` discovery out of `spinel_ast` | 10 unit tests |
| T4 | `ruby_version_is`, `platform_is`, `platform_is_not` evaluated; every other guard skips | unit tests for both directions and for an undecidable guard |
| T5 | Report with a `blocked` column and a reason | `nothing_passes_while_there_is_no_vm` |
| T6 | Files yielding no examples named rather than counted as zero | `a_file_that_yields_no_examples_is_named_rather_than_counted_as_zero` |
| T7 | `scripts/spec.sh` with corpus-relative paths | runs a file, a directory, and the whole corpus; names an uninitialised submodule |
| T8 | `spec/harness/tests/harness.rs` | 10 tests, 8 of them offline on temporary fixtures |
| T9 | CI: submodule checkout, `spec` job with a floor, clone deleted, no-globals grep widened | six jobs |
| T10 | Correct `roadmap.md`, `engine.md`, `architecture.md`, `README.md` | `should_raise` gone; no doc claims a clone that no longer exists |

## What the audit caught

**A1 — `should_raise` does not exist in ruby/spec.** The issue and `docs/roadmap.md` both named it as a matcher the harness must understand. It appears nowhere in the corpus except inside a fixture's filename. mspec spells it `-> { }.should.raise(ArgumentError)` — 527 uses in `language/` alone — and `raise_error`, the RSpec spelling, appears zero times there. Corrected in `docs/roadmap.md`; had matchers been built this slice, they would have been built against a matcher that does not exist.

**A2 — Matchers cannot be falsified before the VM.** Recognising `x.should == y` in a syntax tree is easy and would have produced a "N examples are ready for phase 1" figure. Nothing could check that figure, because nothing can run the examples. Deferred to phase 1 rather than shipped as an unfalsifiable metric; `docs/roadmap.md` now says so.

**A3 — 219 spec files produce no examples, and silence would have hidden it.** 213 delegate entirely through `it_behaves_like`, 5 build examples inside an `eval` string, and one — `core/dir/fileno_spec.rb` — wraps its examples in a runtime `if`. All three need a VM. The first version of the harness reported these as `0 examples` and said nothing, so `scripts/spec.sh core/binding` looked like a broken checkout. The report now names them and gives the reason. The runtime-`if` case also falsified a `ponytail:` comment in `discover.rs` that claimed nothing in ruby/spec does that; the comment now names the file.

**A4 — Version equality disagreed with version ordering.** `Version` derived `PartialEq` over the segment list while implementing `Ord` with zero-padding, so `"3.5" >= "3.5.0"` was true and `"3.5" == "3.5.0"` was false. `ruby_version_is "3.5"` on Ruby 3.5.0 was right only because the guard uses `>=`; an inclusive range end would have been wrong. Caught by a unit test written before the bug was suspected, and fixed by defining equality through `Ord`.

**A5 — Examples built in a loop were invisible.** The walk returned early on any call with a receiver, on the grounds that the DSL is bare calls. That is true of `describe` and `it`, and false of `[1, 2].each do ... it ... end`, which is how several specs generate examples. Caught by a unit test; the walk now descends into a receiver call's block without treating it as DSL.

**A6 — The report was unreadable through `scripts/spec.sh`.** The script resolves `core/array` to an absolute path, so every line began with ninety columns of `/Users/...` before the first count. Paths are now printed relative to the working directory, which also makes them pasteable back into a command. A single-file run printed its counts twice, once per file and once as a summary; the summary now names the file instead.

**A7 — CI's no-globals grep only ever looked at `crates/`.** `spec/harness/` is Rust in the workspace and the rule in `CLAUDE.md` applies to it. Widened.

### Verified, no change needed

- **Every `*_spec.rb` in the corpus parses.** 3,835 files, zero errors, 0.3s.
- **The submodule pin equals the SHA CI was already cloning** (`620a912d`), so this slice moved a pin into the tree without changing which commit the project builds against.
- **The example count is independently reproducible.** `--list` prints 25,624 lines with a tab, matching the summary exactly, so the counter and the enumerator do not disagree.
- **`master` is the right branch.** ruby/spec's version branches (`2.6`) exist for versions that left support; `master` tracks the newest, which is the one README.md targets.

## Open decisions for the owner

1. **Submodule rather than a vendored copy.** The issue asked for a submodule and this delivers one, which is the opposite choice from `stdlib/` in [#4](https://github.com/ar4mirez/spinel/issues/4). The difference is that ruby/spec is a corpus we read and never edit, while `stdlib/` ships inside the binary. The cost is 21 MB and a `--init` step that a fresh clone will forget at least once; `scripts/spec.sh` and the harness both name it when that happens. Say the word and it becomes a vendored tree with a drift check, like `stdlib/`.
2. **`blocked` as a fifth column.** It is honest and it disappears in phase 2. If the preference is that a blocked example be a failure, CI goes red for all of phase 1 and stays red until `language/` is finished.

## Follow-ups

- Matchers, and the evaluation seam in `spec/harness/src/main.rs` marked `ponytail:`, land with the interpreter in phase 1 ([#7](https://github.com/ar4mirez/spinel/issues/7) onward).
- `spec/tags/` and its reader, phase 1 — the first slice that can have a spec worth skipping.
- `bench/spec-status.md`, once a pass column exists.
- `spec/harness/` is deleted at the end of phase 2, when mspec runs on Spinel.
