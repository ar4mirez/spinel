# PRD 0089 — `Thread` and `Mutex`

Issue: [#45](https://github.com/ar4mirez/spinel/issues/45) · Phase 3 ·
`area:runtime`

## Objective

Threads within a Ractor, taking turns under its lock, with the lock let go
around every wait so that IO overlaps: `Thread`, `Mutex`,
`ConditionVariable`, `Queue`, `SizedQueue` and `ThreadGroup` as CRuby 4.0.6
has them. Until now `Thread.new` refused, and with it every spec, library and
gem that starts one: `Timeout`, `Process.detach`, `Open3`, a server's accept
loop.

## Design

**A thread is a saved interpreter state, as a fiber is.** The interpreter
never recurses on the Rust stack for a Ruby call, so a fiber is a second pair
of its two vectors — the value stack and the frames — and switching fibers is
swapping which pair the loop runs. A thread is the same pair plus fiber
bookkeeping of its own: which of its fibers runs, its root fiber, the root's
vectors while a fiber was transferred to. `crates/spinel-vm/src/threads.rs`
keeps a table of them beside the fibers, traced by the collector, and
switches by swapping all of it, along with `$~`, `$!`, `$_` and `$?`, which
the VM keeps one of per heap and Ruby keeps one of per thread.

**The scheduler is the VM's; what a thread means is Ruby's.** A thread waits
through one primitive, `__thread_block__`, saying for what: a deadline, any
of a set of descriptors, or a wakeup from another thread. When no thread can
run, the scheduler `poll(2)`s for the first of them; when every thread waits
for ever on another, it wakes the main thread to raise CRuby's `fatal`
"No live threads left. Deadlock?" — a `sleep` with nothing to end it but a
signal does not count, as in CRuby. The interpreter loop preempts the
running thread every 10 ms, checked on the same compare it already makes for
the instruction budget, so a thread that never waits does not starve the
rest — but not part-way through the core library, whose methods are Ruby
here where CRuby's are C and run whole under the lock: `Array#reject!` is
not interleaved with another thread's `<<`, while a block it calls can be.
`Thread#raise` and `#kill` leave an exception, or a throw tagged with
the thread, for the target to meet when it next runs.

`core/thread.rb` is everything above that, in Ruby: `join` and `value`,
`status` down to "aborting", fiber-local `Thread#[]`, thread variables,
`report_on_exception` and `abort_on_exception`, `handle_interrupt`'s masks,
an `exit` from any thread ending the program in the main one, a `Mutex` held
by a fiber, and the queues. Every wait in the core library goes through the
scheduler: `sleep`, `join`, `Queue#pop`, `Mutex#lock`, `IO.select` and
`IO#wait`, a read or write on a descriptor that is not ready, `Process.wait`
(polling `WNOHANG` while another thread runs), and a `require` another thread
is part-way through.

`ponytail:` the threads of a Ractor take turns on its one OS thread — the
lock `docs/engine.md` describes, held for all of them by one OS thread — so a
blocking call no thread scheduled around (`getaddrinfo`, `open(2)` on a FIFO,
a blocking FFI call) stops them all. One OS thread per Ruby thread, handing
the heap over under the lock, is the upgrade, and keeps the table and the
switch; it is #328, with the rest this leaves.

## Results

Checked by `crates/spinel-cli/tests/threads.rs`: two threads each reading a
child's output that takes 1 s finish together in about 1 s, and a busy
thread is preempted.

`core/thread`, `core/mutex`, `core/conditionvariable`, `core/queue` and
`core/sizedqueue` go from all blocked to **503** of 551 passing, 0 failing;
what is left is #328, signals (#44), and the frames of primitives (#29).
Corpus 25,251 to **25,874** passing, 0 failed: compared example by example
with `main`, 585 blocked examples pass — the thread directories,
`library/socket`'s servers, `Timeout`, `Open3`, `Process.detach` — and 7
failing ones, and none that passed fails. Every pass agrees with ruby
4.0.6.

Threads also surfaced fixes made here: an anonymous exception class's
message is its `inspect`, `IO.select` refuses a NaN or non-numeric timeout,
a fiber cannot be run from another thread, a new thread starts with its
creator's fiber storage, `fork` leaves only the forking thread in the
child, and a refusal boundary belongs to its thread. `File.open` on a FIFO
with the other end opened by another thread is tagged for #328: `open(2)`
blocks every thread.
