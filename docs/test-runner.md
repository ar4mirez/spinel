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
- Built (#92), in `shims/test_runner.rb`, which `spinel test` runs on the VM after `shims/setup.rb` and the gem prelude, as `spinel run` runs a program: `lib/` and `test/` go on `$LOAD_PATH`, as Rake's TestTask puts them, and `test/test_helper.rb`, when it exists, is required by its absolute path, so a test file's own `require "test_helper"` or `require_relative` finds it loaded. `N` is the physical cores (`/sys/devices/system/cpu/*/topology` on Linux, `hw.physicalcpu` on macOS), never more than the CPUs the process may run on, and never more workers than files; `-j N` sets it. Each worker has a pipe it reads paths from and a pipe it writes events to; a worker is given its next path only after it reports the last one done, so files go out one at a time. A worker `require`s each file, and after its last one ends as a program ends, so an `at_exit` its helper registered (`minitest/autorun`) runs there; the parent ends with `exit!`, so that `at_exit` does not run again in the parent with no tests loaded.
- RSpec suites are discovered but not run yet: RSpec cannot run its examples on the VM until #835 (splatting an object whose `to_a` is Ruby). `spec/**/*_spec.rb` files are counted and reported on stderr as not run, naming #835, and the run exits 1, so a spec suite is never a silent green; `spec/` is not put on `$LOAD_PATH`, `spec/spec_helper.rb` is not required and no spec file goes to a worker. The Minitest files in the same run still run. Once #835 lands, `spec_helper.rb` is loaded once with the other helper and each worker runs the examples of its spec files through RSpec's own runner.
- Until #93, the only event is `{"event":"done","file":..,"error":..}` once a file is loaded, `error` the exception loading raised or `null`; #93's reporter writes its per-test events on the same pipe, one JSON line each, and the parent already splits what it reads into lines and parses each. Until #94, the parent prints only what went wrong, on stderr, prefixed `spinel test:`.
- A worker that ends before reporting the file it was given done — killed by a signal, or exiting with any status — is reported with that file (`worker 2 (pid 4242) was killed by SIGKILL while running test/b_test.rb`), and a new worker is forked from the parent for the files still waiting, so one crash costs one file. A worker that ends by a signal or a failing status after its last file is reported with the files it ran, and so is a file whose loading raised. Any of these, finding a spec file, or finding no test files, exits 1.
- Test files are handed out longest-first using timings recorded from the previous run in `.spinel/test_timings.json`.
- Per-worker databases: if `Rails` is defined, set `ENV["TEST_ENV_NUMBER"]` and `ActiveRecord::TestDatabases.create_and_load_schema` the way Rails' parallelization does. // ponytail: Rails only; other frameworks set `TEST_ENV_NUMBER` themselves.

`shims/test_worker.rb` (~120 lines) registers a Minitest reporter and an RSpec formatter that emit one JSON line per event: `{"event":"pass|fail|error|skip","file":..,"line":..,"name":..,"ms":..,"message":..,"backtrace":[..]}`. An `IO` that is not `sync` buffers what it writes, as CRuby's does (#41), so the shim sets `sync = true` on the pipe back to the parent, or writes each event in one `write`, and a line never straddles two flushes.

## Output

- Default: dots while running, then failures with file:line and a trimmed backtrace (gem frames folded), then a summary line. Exit code 1 on any failure.
- `--reporter json` for tools, `--reporter junit` for CI.
- Failures are re-runnable: the summary prints the exact `spinel test file:line` commands.

## Watch

`--watch` maps changed files to tests to rerun: a changed `test/foo_test.rb` reruns itself; a changed `app/models/user.rb` or `lib/user.rb` reruns `test/**/user_test.rb` and `spec/**/user_spec.rb` by name convention; anything else reruns the last failed set, or everything on Enter. // ponytail: name convention only; a require-graph from the bytecode cache could do better later.

## Not in scope

Coverage (use `simplecov`), mocking, snapshot testing, browser drivers. Spinel gets out of the way.
