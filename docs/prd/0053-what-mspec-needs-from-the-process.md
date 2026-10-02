# PRD 0053 — What mspec needs from the process

Issue: [#145](https://github.com/ar4mirez/spinel/issues/145), first slice ·
Phase 2 · `area:infra`

## Objective

Run ruby/mspec on Spinel. After string `eval` (#38) and `require` (#39), what
stood between `mspec-run` and a spec file was the process around the program:
its arguments, its output streams, its environment, its exit status. This
slice supplies them, and ends with mspec running a spec file to its summary:

```
$ RUBY_EXE=target/release/spinel spinel run ../mspec/bin/mspec-run spec/ruby/language/and_spec.rb
spinel 0.0.1 (ruby 4.0.0) [x86_64-linux]
..........

Finished in 0.029059 seconds

1 file, 10 examples, 26 expectations, 0 failures, 0 errors, 0 tagged
```

## Baseline

Corpus 7003, 0 failed. `spinel run` took one file and no arguments.

## What landed

Each was the next thing `mspec-run` stopped on, in this order.

- **`ARGV`, `$0`, exit status, `at_exit`.** `spinel run FILE ARGS...`; the
  embedder sets the program name and arguments on the heap before boot. An
  uncaught exception is left in `$!`, and `spinel_core::exit_status` runs the
  `at_exit` blocks — last first — and answers `SystemExit#status`, 1, or 0.
- **The standard streams.** `IO` is a descriptor with `write`, `print`,
  `puts`, `printf`, `<<`, `sync` and `tty?`; `STDOUT`/`STDERR`/`STDIN` and
  `$stdout`/`$stderr`/`$stdin`. `Kernel#puts`, `print`, `p` and `warn` go
  through `$stdout` and `$stderr`, which is what lets mspec capture output.
  Reading, opening files and buffering are #41.
- **`ENV`.** Read from the process one variable at a time; a write is kept
  per heap and the whole table is built only when something enumerates it —
  building it costs milliseconds, and the spec harness touches `ENV` in every
  example's heap. Setting the process's environment would be `unsafe` in Rust
  2024, and no child process can see it until #43 starts one.
- **`RUBY_VERSION` and its neighbours**, from the same constants
  `spinel --version` prints: Ruby 4.0.0, engine `spinel`.
- **`Process`**: `pid`, `ppid`, the uids and gids, `clock_gettime` with every
  unit, and the `CLOCK_*` constants from libc. Starting and waiting for
  processes is #43.
- **`Signal.trap`** records a handler and answers the previous one exactly as
  CRuby does — measured per signal, by number so aliases share — but never
  runs it; delivery is #43.
- **`Time.now`** and `Time#-`, for mspec's timer, and nothing else of `Time`:
  `Time.new` refuses, so `core/time` stays blocked rather than answered
  wrongly (#32).
- **A backtick** compiles to a call to `` Kernel#` `` (#223's compile half),
  which refuses until #43 can run the command. mspec's platform guards
  contain backticks they never reach on Linux.
- **The instruction budget is per heap.** The harness keeps 50 million per
  evaluation; `spinel run` has none, since a program runs as long as it runs.
- **Gaps mspec hit:** `Kernel#Integer` (CRuby's strict parse, measured
  case by case), `nil.to_i`/`to_h`/`to_f`/`=~`, `String#delete_prefix` and
  `delete_suffix`, `Array#index` with a block, `Range#include?` treating an
  end that converts with `to_int` as numeric, `File.executable?` and
  friends.

## The harness

The harness sets `MSPEC_RUNNER` in each example's heap, as mspec does for the
processes it starts, so `spec_helper.rb` does not try to load mspec itself.
Two examples are now tagged as the harness's own (#145): one expects mspec's
runner in its backtrace, one reads an ivar a previous group's hook set, which
mspec's shared context carries over.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/kernel` | 384 / 1277 | **489 / 1277** |
| `core/env` | 0 / 180 | **49 / 180** |
| `core/signal` | 7 / 47 | **27 / 47** |
| `core/string` | 1209 / 1905 | **1229 / 1905** |
| `core/builtin_constants` | 0 / 27 | **17 / 27** |
| **corpus** | 7003 | **7238** |

0 failed; all passes re-run on ruby 4.0.7 agree.

## What is left of #145

mspec stops at the first refusal: a construct Spinel cannot run yet ends the
process rather than one example, and with no budget a loop Spinel gets wrong
never ends. The next slice turns a refusal into a Ruby exception mspec reports
per example, with a per-example budget, and gives mspec a formatter that
counts those as *blocked* — the harness's third column. Then `scripts/spec.sh`
moves to mspec and `spec/harness/` goes.
