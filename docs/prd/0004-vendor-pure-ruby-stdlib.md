# PRD 0004 — Vendor the pure-Ruby stdlib under `stdlib/`

Tracks [#4](https://github.com/ar4mirez/spinel/issues/4). Milestone: Phase 0: skeleton. `P1`, `size:S`, `area:infra`, `area:stdlib`.

## Objective

Put Ruby's pure-Ruby standard library in the tree, at a pin, with a check that
proves it is still a copy and not a fork.

Two things need it. The one that pays off later: `architecture.md` ships
`stdlib/**/*.rb` inside the binary and extracts it to `~/.spinel/stdlib/<ver>/`,
so `require "erb"` has something to find. The one that pays off now: the `sweep`
CI job from [#3](https://github.com/ar4mirez/spinel/issues/3) clones this corpus
on every run to prove the lowering handles real Ruby. After this slice it reads
the tree instead, and the clone and its pin are deleted.

The slice ships **no Spinel code**. Nothing requires, loads, or executes any of
these files; `$LOAD_PATH` is phase 3.

## Non-goals

- **A `git subtree`.** The issue title asks for one. See R3 — it is the wrong
  tool here, and the roadmap and `architecture.md` have been corrected.
- **Deciding what ships in the binary.** `bundler/` and `rubygems/` are 4.2 MB
  of the 6.6 MB and Spinel replaces both. Filtering them out of the *vendored
  tree* would cost the sweep its corpus and buy nothing today; filtering them out
  of the *embedded asset* is a real question for the slice that builds it. Open
  decision 2.
- **`require`, `$LOAD_PATH`, the extraction step.** Phase 3.
- **Patching upstream.** Nothing here is edited. If a file ever must be, R4 says
  where the exception goes.
- **Tracking upstream automatically.** Bumping the tag is a human decision, made
  by editing one line and re-running one script.

## Users

| User | Needs from this slice |
|---|---|
| CI's `sweep` job | 600-odd files of real Ruby in the tree, so the lowering's check stops depending on a clone |
| The binary packager (phase 3/4) | A directory to zstd into the release binary |
| `require` (phase 3) | A `$LOAD_PATH` root, so `require "net/http"` resolves to `stdlib/net/http.rb` |
| A reviewer | Confidence that a 6.6 MB directory nobody reads is still upstream's bytes |
| Anyone bumping Ruby | One line to edit, one command to run, a diff that is only upstream's changes |

## Requirements

### R1 — `stdlib/` is a `$LOAD_PATH` root

Upstream `lib/` is flattened to the root of `stdlib/`, not nested under
`stdlib/lib/`. `require "erb"` will look for `stdlib/erb.rb`, and
`require "net/http"` for `stdlib/net/http.rb`, with no path arithmetic in
between. Two entries are ours, both non-`.rb` so they cannot shadow a feature:
`UPSTREAM` and `LICENSE/`.

### R2 — Upstream's licenses travel with the code

`stdlib/LICENSE/` holds `COPYING`, `COPYING.ja`, `BSDL`, and `LEGAL`. The first
three are Ruby's dual license. `LEGAL` is the one that is easy to drop and
should not be: `lib/` mixes licenses, and `LEGAL` is upstream's per-file record
of which file is under what.

### R3 — A pinned copy, not a subtree

The issue asks for a `git subtree`. It is the wrong tool:

- `git subtree add` copies a *whole repository*. We want one directory out of
  ruby/ruby; the rest is C.
- `git subtree split -P lib` can synthesise a lib-only history, but only from a
  full clone of ruby/ruby, and the result shares no history with upstream — so
  `git subtree pull`, the one thing a subtree buys, would never merge cleanly.

`scripts/vendor-stdlib.sh` does a blobless depth-1 sparse fetch of the pinned
tag and writes the tree. It runs in about four seconds. `stdlib/UPSTREAM`
records the repository, tag, commit, and how to bump it. The pin is `v4.0.6`,
the latest 4.0 tag, matching the language version README.md commits to.

### R4 — CI fails on drift

`scripts/vendor-stdlib.sh --check` re-fetches the tag, stages what the tree
*should* be, and diffs. The `stdlib drift` job runs it on every PR.

Both modes stage the same tree, so the check tests exactly what the vendor step
writes; a bug in the staging cannot make the check pass and the vendoring wrong.
Any difference fails. That is stricter than the "unexplained drift" the issue
asked for, and needs no allowlist to stay honest — an allowlist that is empty is
a mechanism with no user, and the day a patch is genuinely needed, the diff is
the place to add it with the reason next to it.

`.gitattributes` marks `stdlib/**` as `-text`. Without it a contributor with
`core.autocrlf` set would rewrite every line ending on checkout and fail this
job for a reason that has nothing to do with their change.

### R5 — The sweep reads the tree, and the clone is deleted

`.github/workflows/ci.yml` loses the `Fetch ruby/ruby lib` step, the
`Sweep the pure-Ruby stdlib` step, and the `RUBY_SRC_SHA` pin. The existing
`Sweep vendored corpora` step already covers `stdlib/` now that it exists, as
[#3](https://github.com/ar4mirez/spinel/issues/3) planned. The ruby/spec clone
stays until [#5](https://github.com/ar4mirez/spinel/issues/5) vendors it.

## Definition of done

- [x] `stdlib/` present and populated — 776 files, 626 of them `.rb`, 6.6 MB
- [x] Upstream license files preserved — `stdlib/LICENSE/{COPYING,COPYING.ja,BSDL,LEGAL}`
- [x] CI job diffs `stdlib/` against the upstream tag and fails on drift — `stdlib drift`
- [x] The sweep's cloned stdlib corpus is deleted and the vendored tree replaces it
- [x] Integration test under `crates/spinel-cli/tests/`, per `CLAUDE.md`
- [x] `docs/architecture.md` and `docs/roadmap.md` corrected in the same PR
- [x] Backlog corrected where the vendored tree falsified it — #4 retitled, #48 retriaged, #47 and #5 handed the facts

## Corpus result

```
$ ./target/release/spinel parse stdlib
626 files · 0 unhandled · 0 syntax errors · 0.1s
```

The baseline to match, from [#3](https://github.com/ar4mirez/spinel/issues/3)'s
clone of ruby/ruby `lib/` at `5efd4ad`, was 589 files, 0 unhandled, 0 syntax
errors. The vendored tree is 37 files larger — a different ref, not a different
sweep — and just as clean. No lowering change was needed.

## Tasks

| | Task | Check |
|---|---|---|
| T1 | `scripts/vendor-stdlib.sh`, vendor and `--check` modes | runs in 4 s; `--check` is green on a clean tree and red on a touched one |
| T2 | Vendor `lib/` at `v4.0.6`, flattened | 626 `.rb` files under `stdlib/` |
| T3 | Copy the four license files | `stdlib/LICENSE/` |
| T4 | `stdlib/UPSTREAM` with repo, tag, commit, bump instructions | matches the script's pin |
| T5 | `.gitattributes`: `stdlib/** -text` | no EOL normalisation on checkout |
| T6 | CI: add `stdlib drift`, delete the clone and `RUBY_SRC_SHA` | five jobs, sweep reads the tree |
| T7 | `crates/spinel-cli/tests/stdlib.rs` | 4 tests, offline |
| T8 | Correct `architecture.md` and `roadmap.md` | "git subtree" gone from both |

## What the audit caught

**A1 — `set.rb` is not in Ruby 4.0's `lib/`, and three places assumed it was.**
Found when a deliberately tampered `stdlib/set.rb` made the drift check report
the file as *added*. Upstream ships `set.c` and no `lib/set.rb`: `Set` is a core
class now. That falsified, in order of blast radius:

- `docs/roadmap.md`, which listed `set` among the stdlib extracted on first run
  in phase 3 — fixed here.
- `docs/architecture.md`, which listed it among the vendored files — fixed here.
- [#48](https://github.com/ar4mirez/spinel/issues/48), *"stdlib: `set`"*, whose
  definition of done was `library/set/` passing. ruby/spec at the pinned
  `620a912` has `core/set/` (58 entries) and no `library/set/`, so the slice was
  unachievable as written. Retriaged to *"core: `Set`"*, phase 2, `area:core-lib`.

`the_stdlib_is_vendored` asserts the file's absence, so an upstream bump that
reintroduces it is read rather than absorbed.

**A2 — The drift check needed the network, so nothing guarded the tree
locally.** A contributor bumping `RUBY_TAG` without re-running the script would
have had a green `cargo test` and a red CI. `the_recorded_pin_matches_the_script`
compares the script's pin against `stdlib/UPSTREAM` offline and catches it.

**A3 — A sweep over an empty directory is a passing sweep.**
`every_vendored_file_lowers_to_spinel_ast` asserts the file count as well as the
error counts, so deleting `stdlib/` cannot turn the test green.

**A4 — `core.autocrlf` would have failed the drift job for the wrong reason.**
Fixed by `.gitattributes` before it could happen; see R4.

### Verified, no change needed

- **Nothing in `stdlib/` is hidden from git.** `git ls-files --others --ignored`
  over the tree is empty; `/target` and `.spinel/` do not match anything in it.
- **No hazardous paths.** No symlinks, no executable bits, no filename with a
  character Windows rejects — so the tree stays checkout-safe even though
  Windows is a non-goal for first releases.
- **No CRLF in the vendored bytes**, so the `-text` attribute is a guard rather
  than a correction.
- **The tree is 776 files** — 771 from upstream `lib/`, plus `UPSTREAM` and the
  four licenses. Nothing was dropped or added.

## Open decisions for the owner

1. **Pinned copy instead of `git subtree`.** R3 is a decision, taken because a
   subtree cannot express "one directory of another repository" without giving
   up the history sharing that makes it a subtree. Say the word and it becomes a
   real subtree of a synthesised lib-only branch; the cost is a full clone of
   ruby/ruby at bump time and a history that still will not merge.
2. **`bundler/` and `rubygems/` are 4.2 MB of the 6.6.** Vendored verbatim here
   because filtering costs the sweep its corpus and the binary is not built yet.
   The question belongs to the slice that embeds the asset — Spinel replaces both
   tools, so shipping their Ruby inside a 30 MB binary is probably wrong — and is
   now on [#47](https://github.com/ar4mirez/spinel/issues/47) with the numbers,
   including the 145 non-`.rb` files `architecture.md`'s `stdlib/**/*.rb` already
   excludes. Filtering belongs in the packager, not in the vendored tree, so
   `stdlib/` stays byte-exact and the drift check stays allowlist-free.
3. **Bump policy.** Today: edit `RUBY_TAG`, run the script, commit the tree in
   its own commit. No automation, because a language-version bump is a decision
   and not a chore.

## Follow-ups

- Delete the ruby/spec clone from the `sweep` job when
  [#5](https://github.com/ar4mirez/spinel/issues/5) vendors `spec/ruby/`. The
  job already sweeps it when present.
- The extraction step and `$LOAD_PATH` wiring, phase 3
  ([#47](https://github.com/ar4mirez/spinel/issues/47)), which also carries open
  decision 2.
- `Set` as a core class, phase 2
  ([#48](https://github.com/ar4mirez/spinel/issues/48)), retriaged by A1.
