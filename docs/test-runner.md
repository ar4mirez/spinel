# Test runner

Goal: `spinel test` runs a project's Minitest or RSpec suite in parallel with no configuration, and `spinel test --watch` reruns affected files on save.

## Scope

Spinel schedules, parallelizes, and reports. Minitest and RSpec still define what a test is and what passes. No new assertion library, no new DSL. Prerequisite: both frameworks run on Spinel's VM (phase 3 milestone).

## Discovery

- `test/**/*_test.rb` → Minitest. `spec/**/*_spec.rb` → RSpec. Both present: run both.
- `spinel test path/to/file_test.rb:42` runs one test by line (Minitest: resolve line → method via `spinel_ast`; RSpec: pass `file:line` through).
- `-n /pattern/` filters by name.

## Execution

```
spinel test ──fork N workers──▶ each worker: Spinel VM, -r shims/test_worker.rb
                                    │ reads file paths from a pipe, one at a time
                                    │ runs them, streams JSON events back
                                ◀───┘
              aggregates, prints, exits non-zero on failure
```

- `N` = physical cores by default (`-j`). Workers are forked *after* loading `test_helper.rb`/`spec_helper.rb` once in a parent Spinel process, so Rails boot is paid once, not N times. This is the same trick `parallel_tests` and Rails' own parallelization use; Spinel just does it without the gem. `fork` is a phase 3 primitive with CRuby's rule: only from the main Ractor while no other Ractor exists.
- Test files are handed out longest-first using timings recorded from the previous run in `.spinel/test_timings.json`.
- Per-worker databases: if `Rails` is defined, set `ENV["TEST_ENV_NUMBER"]` and `ActiveRecord::TestDatabases.create_and_load_schema` the way Rails' parallelization does. // ponytail: Rails only; other frameworks set `TEST_ENV_NUMBER` themselves.

`shims/test_worker.rb` (~120 lines) registers a Minitest reporter and an RSpec formatter that emit one JSON line per event: `{"event":"pass|fail|error|skip","file":..,"line":..,"name":..,"ms":..,"message":..,"backtrace":[..]}`.

## Output

- Default: dots while running, then failures with file:line and a trimmed backtrace (gem frames folded), then a summary line. Exit code 1 on any failure.
- `--reporter json` for tools, `--reporter junit` for CI.
- Failures are re-runnable: the summary prints the exact `spinel test file:line` commands.

## Watch

`--watch` maps changed files to tests to rerun: a changed `test/foo_test.rb` reruns itself; a changed `app/models/user.rb` or `lib/user.rb` reruns `test/**/user_test.rb` and `spec/**/user_spec.rb` by name convention; anything else reruns the last failed set, or everything on Enter. // ponytail: name convention only; a require-graph from the bytecode cache could do better later.

## Not in scope

Coverage (use `simplecov`), mocking, snapshot testing, browser drivers. Spinel gets out of the way.
