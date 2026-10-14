# PRD 0079 — `Process`: children, waiting and signals

Issue: [#43](https://github.com/ar4mirez/spinel/issues/43) · Phase 3 ·
`area:runtime`

## Objective

`IO.popen` was the largest blocker in the corpus (532 examples), with
`Process.spawn` (80) and the process refusals (67) behind it. Most of it was
not about processes at all: mspec's `ruby_exe` runs every spec that needs a
fresh interpreter through `IO.popen`, so `command_line/` and every "in a new
process" example stopped there. This slice starts, waits for and signals
children, and with #62's flags that lets `ruby_exe` start Spinel.

## Design

**One primitive starts a child.** `__sys_spawn__` is `fork(2)` and
`execve(2)` as CRuby's `rb_fork_async_signal_safe` does them: everything the
child needs — the C strings of the path, `argv` and the environment, each
descriptor it gets — is made before the fork, and the child calls only
async-signal-safe functions. Sources are first copied above every descriptor
named, so making one cannot overwrite the source of another. A failure in
the child is its errno, written down a close-on-exec pipe that a successful
`execve` closes; the parent then reaps the child and answers its pid with
the errno, so `$?` is the exit 127 CRuby reports. Rust starts every program
with `SIGPIPE` ignored, which a child would inherit across `execve`, so the
child gets the default back. `exec` is the same set-up without the fork.

**What a command is, is Ruby.** `Process::Spawn` in `core/process.rb` splits
`spawn`'s arguments into environment, command and options; decides when a
String needs `/bin/sh` (CRuby's meta characters, reserved words, builtins
and assignments); searches `PATH` before the fork, so an unknown program is
`Errno::ENOENT` in the parent; builds the whole environment from `ENV`; and
turns every redirection — Integers, `:in`/`:out`/`:err`, IOs, Arrays of
them, paths, `[path, mode, perm]`, `:close`, `[:child, fd]` — into
`[child, parent]` pairs. Options are checked as `rb_execarg_addopt` checks
them. `spawn`, `system` (with `exception:`), backticks, `exec`, `fork` with
and without a block, `_fork`, `exit!`, `daemon` and `IO.popen` are built on
it.

**`IO.popen` is an IO with a pid.** `"r"`, `"w"`, and `"r+"` as a duplex IO
whose writes go to a second descriptor; `close_write` and `close_read` close
one side each. Closing waits for the child and sets `$?`. A command Array
may carry its own environment and options, and options may follow it as a
positional Hash. `popen("-")` refuses.

**Waiting and status.** `Process.wait`, `wait2`, `waitall`, `Status.wait`,
and `Process::Status` over the raw status word with POSIX's macros, worded
as `pst_message_status` words them. `$?` and `$$` are kept by the runtime
and read-only to a program.

**Signals, as far as they can go.** `Process.kill` takes numbers, names and
`-` for a group. A signal this process sends itself is sent when the
operating system acts on it alone (`KILL`, `STOP`, anything at
`SYSTEM_DEFAULT`); one Ruby would act on — trapped, or one CRuby turns into
an exception — is refused until handlers run (#44). An uncaught
`SignalException` ends the process by its signal, as CRuby's does.

**The rest of `Process`.** Process groups and sessions, `getrlimit` and
`setrlimit` by name or number, priorities, `times` as `Process::Tms`,
`clock_getres` including CRuby's emulated clocks, the id setters and
`Process::UID`, `GID` and `Sys`, `groups` and `maxgroups`.

**On the way.** `send` takes a String name. A program that does not parse
still runs the `at_exit` handlers `-r` registered, and the uncaught error is
printed after them, as CRuby does, so an `exit!` in one prints nothing.
`spinel some_word` is a script unless the word is a typo of a subcommand.

## Not in this slice

`popen("-")`. `Process.detach` waits on #45's Thread. `:rlimit_*` spawn
options are accepted and not applied. A child given a closed standard
descriptor finds `/dev/null` there, because Rust's runtime opens one before
`main`; tagged.

## Results

| | passed | failed | blocked |
|---|---:|---:|---:|
| corpus before | 22,043 | 0 | 9,114 |
| this slice | **22,687** | **0** | 8,306 |

`core/` goes from 81.0% to **83.2%**; `core/process` from 83 to 384 of
430 passing, `core/kernel` from 1,874 to 1,947.

Compared example by example with `main`, none that passed fails. One that
passed is blocked now: `Signal.trap`'s "raises an exception for a
non-callable at the point of use" passed because `Process.kill` did not exist
and its `NoMethodError` was the one the example expects. Now `kill` refuses a
signal to this process that a handler would answer (#44).

**Tags.** About 220 examples that were blocked at `IO.popen` now run in a
fresh Spinel and disagree. Each is tagged with its owner: #314, filed for
the ones that need a whole program — `RUBYOPT`, `RUBYLIB`, `-S`, `-K`/`-U`
and the other flags #62 parses and does not yet act on, `END`/`BEGIN`,
`DATA`, `TOPLEVEL_BINDING`, top-level error output, per-process hash seeds —
and #36 (`ObjectSpace`, finalizers), #41 (`ARGF`), #44 (signals), #45
(`Thread`), #29, #38, #47 and #249 for the rest. Twenty-six tags that now
pass are deleted, some of them stale before this slice, and the C-API tag
whose file now loads with no examples.
