# PRD 0054 — Blocked, under mspec

Issue: [#145](https://github.com/ar4mirez/spinel/issues/145), second slice ·
Phase 2 · `area:infra`

## Objective

mspec ran a spec file on Spinel after PRD 0053, but stopped at the first
construct Spinel could not run: a refusal ended the process, not the example.
`spec/harness` had a third column for that — *blocked* — and the failure
column meant one thing because of it. This slice gives mspec the same three
outcomes, and runs the whole corpus under mspec for the first time.

## Design

**A refusal boundary.** `__refusal_boundary__(budget) { ... }` runs a block
with an instruction budget. A refusal inside it — something the VM declines to
guess at, a body the compiler could not lower, or running past the budget —
ends the block and answers the reason as a String. It travels as a `return`
aimed at the boundary's frame, so every `ensure` on the way out runs (mspec's
output matchers restore `$stdout` in one), and no `rescue`, not even
`rescue Exception`, sees it. A fiber is its own stack: one with no boundary
on it ends where it refused, and its resumer's boundary takes the refusal. The
loop keeps the live boundaries in a list it refreshes when the frame count
changes, so the per-instruction cost is one comparison.

**Uncompilable bodies refuse when they run.** The compiler used to refuse a
whole file for one construct it could not lower. A block or method body that
fails now compiles to `Insn::Refuse`, which raises
`Error::NotCompiled` only when executed, so the rest of the file runs. This is
the granularity the harness had when it compiled each example on its own, and
it is right for programs too: an unsupported construct in a method nobody
calls no longer stops the program.

**`spec/spinel.mspec` and `spec/spinel/runner.rb`.** The config maps
`spec/tags/` onto `spec/ruby/`, excludes `fails` tags, and installs
`SpinelFormatter`. The runner wraps `MSpec.protect` — every example, hook and
file load — in a boundary (a 50 million instruction budget per example, none
for a file load). An example is *passed*, *failed* (an unmet expectation) or
*blocked* (a refusal, or any other exception — the harness's rule). The
report lists failures, files that stopped loading and fixtures that stopped
part way, then the blocked ranking. `SPINEL_SPEC_LIST=1` prints one line per
example; `SPINEL_SPEC_TRACE=1` names each file as it loads, and `=2` each
example.

**The harness's leniencies, kept and reported.** A fixture that raises part
way keeps what it defined, and the heap is marked partial so `defined?`
refuses rather than answering `nil`. A spec file's own top-level `require` of
a library Spinel cannot load (`stringio`, `date`) is skipped, as the harness
skipped it. Both appear in the report.

**ruby/mspec is a submodule at `spec/mspec`**, pinned as a gitlink. Locally it
can be any checkout; CI fetches it with `spec/ruby`.

## Found on the way

- `Array#==` on two self-containing arrays overflowed the Rust stack.
- `Integer#**` refuses a result past 16G bits with CRuby's `ArgumentError`,
  and `Integer#to_s` with a base is a primitive: building a bignum's digits in
  Ruby divides it once per digit.
- `Dir.glob`/`Dir[]`, `Dir.children`/`entries`, and a read-only `File.open`
  (the contents and a position; writing refuses, #41); `$/`.
- `RbConfig::CONFIG` and `SIZEOF`, which CRuby has loaded before a program
  starts, and `require` of a feature it preloads answers false.
- `Enumerable#sum` compensates Float error, as `Array#sum` did.

## Results

Under `spec/harness` the corpus goes from 7239 to 7335 passing, 0 failed —
the compile fallback lets examples run whose file had one construct the
compiler refused.

Under mspec, the whole corpus, one process per directory:

| runner | examples | passed | failed | blocked |
|---|---:|---:|---:|---:|
| `spec/harness` | 25624 | 7335 | 0 | 16272 |
| mspec | 25517 | **13423** | 351 | 11743 |

mspec runs what the harness could not see — shared groups, examples built at
run time, `it_behaves_like` with its argument. The 351 failures are
disagreements the harness never reached: mostly warnings Spinel does not emit,
and matchers the harness could not run.

## Next

Resolve those failures — fix or tag each — then move `scripts/spec.sh`,
`spec-status.sh`, `verify-passes.rb` and CI to mspec and delete
`spec/harness/`.
