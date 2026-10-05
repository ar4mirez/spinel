# PRD 0055 — mspec replaces the harness

Issue: [#145](https://github.com/ar4mirez/spinel/issues/145), last slice ·
Phase 2 · `area:infra`

## Objective

PRD 0054 ran the whole corpus under mspec on Spinel for the first time and
found 351 examples where the two disagree that `spec/harness` had never
reached. This slice resolves every one of them, moves every script and CI
job from the harness to mspec, and deletes `spec/harness/`.

## Design

**`scripts/spec.sh` runs mspec, in parallel.** The spec files under the
given paths are split into processes of 16 (`SPEC_CHUNK`), run one per core
(`SPEC_JOBS`). Each runs `spec/mspec/bin/mspec-run -B spec/spinel.mspec` on
Spinel with `SPINEL_SPEC_LIST=1`, so its formatter prints one record per
outcome rather than a report. A chunk of files and not a directory, because
a few directories (`core/io`, `core/string`, `core/range`) took ten minutes
each and set the wall time on their own: by directory the corpus took over
fifteen minutes on 8 cores, by chunk seven and a half. A process that stops
without reporting is named, with the end of its stderr, and fails the run.

**`scripts/spec-report.sh` merges the records** into the same report one
process prints (`spec/spinel/report.rb`), or into `bench/spec-status.md` with
`--table`. It is awk. The first version was Ruby run on Spinel, so that
`spec.sh` needed no other Ruby; merging thirty thousand records took over six
minutes of CPU, because `String#lines` is quadratic and each record costs
about a millisecond of hash and regexp work. The merge is only counting, so
it is written in the tool that does counting fast. The two report printers
are kept in step by hand; the format is a dozen lines.

**Tags are checked where they are read.** The harness enforced
`spec/tags/README.md`'s rules — a reason, no parenthesis in it, only `fails`,
an example that exists. Each mspec process now checks the tag files of its
own spec files against its own records and reports a `tag` record per
problem, which fails the run. `spec.sh --tagged` runs only tagged examples;
one that passes is a tag to delete.

**`scripts/verify-passes.rb` runs the real thing on both sides.** Before,
it sliced each passing example out of its file and replayed it under a
four-line mspec shim, which had to learn every matcher the harness did. Now
`spec.sh --list` names Spinel's passes and the processes the files ran in,
and CRuby runs mspec over the same processes, in the same order, through
`spec/verify.mspec`, which filters to Spinel's passes and prints a verdict
per example. Same processes matters: `SpecEvaluate.desc` is global, an
`evaluate` example's description is built from it, and a different grouping
gave CRuby a different name for the same example. A pass Ruby never ran is
reported, except under `ruby_bug`: that guard runs its examples everywhere
but on a CRuby with the bug, so on Spinel and not on the checker. mspec's
`report_on` mode still evaluates such a block and registers its examples as
guarded, which is how they are named and counted rather than reported.

**CI.** The `ruby/spec harness` job is `ruby/spec on mspec`: the same
`language/` floors and zero-failure ceiling. The status job is unchanged in
shape and now holds the whole corpus to zero failures. The passes job
verifies every pass in the corpus.

## Found on the way

- A repeated `_` parameter was bound to the wrong slot when Prism's scope
  listed a later parameter in the position the repeat needed: `|_, _, d|`
  left `d` nil. The formatter's own `|_, file, _, detail|` found it.
- A `describe` body that raised or refused while mspec collected its
  examples — a `guard` whose lambda calls a missing method — dropped the
  whole group from every count, silently. The formatter now blocks the
  examples it collected and reports the group as stopped while loading. A
  tag naming an example past such a stop cannot be checked for existence,
  so in a file that stopped only its format is.
- `Kernel#dup` did not call `initialize_dup`/`initialize_copy` (#201's other
  half). mspec's `ContextState` clears its cached hook lists there, so a
  shared group copied a second time ran the first copy's `before :all`.
- `Hash#to_hash` was missing, so `ENV.replace` in an `ensure` raised and left
  `ENV` cleared for every later file in the process.
- `Array#each_index`, `map`, `select`, `reject` and their aliases, and
  `Enumerator::Product#each`, returned enumerators without a size.
- Among the fixes for the 351: `Symbol#inspect` for operators and special
  globals, the `format` corner cases (precision 0, `#` with negative octal,
  `%{name}` with a precision), paragraph-mode `each_line`, float `%` with
  `fmod`, `Process` constants and `exit!`/`abort`, `Range#cover?` with a
  Range, `Enumerable`/`Hash` enumerator sizes, `Hash#inspect` in 3.4's
  format, `attr` with the legacy boolean, `Array#join` with `$,`, magic
  comments by CRuby's rule (the first line, or the second after `#!`).

## Results

The whole corpus, 3835 files:

| runner | examples | passed | failed | blocked | skipped |
|---|---:|---:|---:|---:|---:|
| `spec/harness` | 25624 | 7335 | 0 | 16272 | 2017 |
| mspec, PRD 0054 | 25517 | 13423 | 351 | 11743 | — |
| mspec, this slice | 30902 | **14186** | **0** | 16414 | 302 |

`examples` counts skipped ones too, as the harness did. The counts are CI's:
a machine without IPv6 skips two `library/socket` examples behind
`SocketSpecs.ipv6_available?`, which is why `bench/spec-status.md` is
generated on CI's Linux. The harness skipped
2017 examples it could not expand (`it_behaves_like`, `eval`, runtime `if`);
mspec runs them all. Blocked rose by the groups that used to vanish while
loading, 307 examples. Of the 351, most were fixed and the rest tagged: tags
go from 156 to 306, half of the new ones warnings Spinel does not print yet
(#268). Every pass is re-run on CRuby 4.0 by the passes job, and all agree;
one more sits under a `ruby_bug` guard CRuby 4.0 skips.

## Next

#145 is done. What the harness needed from the VM and nothing else does —
`compile::flattened_expression`, `body`, `declared_locals` and the statement
at a time evaluation they serve — can go in a cleanup slice.
