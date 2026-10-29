# PRD 0094 — One OS thread per Ruby thread

Issue: [#328](https://github.com/ar4mirez/spinel/issues/328) · Phase 3 ·
`area:runtime`

## Objective

PRD 0089 gave Spinel threads that take turns on their Ractor's one OS
thread. Every wait the scheduler knew about overlapped, but a call that
blocks in the kernel with no descriptor to wait on stopped every thread: an
`open(2)` of a FIFO whose other end another thread was about to open never
returned, and a slow `getaddrinfo` froze the rest. `Thread#native_thread_id`
answered the process id for all of them. The scheduler half of #328 (PR
#360) left this, the OS threads, as what was still to do.

## Design

**Each thread runs on an OS thread of its own, and they take turns with the
heap under the per-Ractor lock** `docs/engine.md` describes. The lock lives
in the heap's thread table: which thread's OS thread holds the heap, a
condition variable per thread to wait for its turn on, and a pipe for
waking a scheduler waiting in `poll(2)`. `Thread.new` starts the OS thread
at once, as `pthread_create` does in CRuby, and it waits for its first turn.

**A switch is what it was, with a hand-over in the middle.** The
interpreter never recurses on the Rust stack for a Ruby call, so the
instruction loop moved out of `eval_in` into `run`, and a thread's OS thread
runs `run` over its own vectors. A switch parks the running thread's
vectors, fiber bookkeeping and specials in its entry, picks the next thread
as before, hands that thread's OS thread the heap and waits to be handed it
back. The woken thread takes its own state back and goes on in its own loop.
Every thread also has a root stack of its own, parked with it: the handles
a loop and its primitives hold are indexes into it, and another thread's
loop truncating a shared one would have left them dangling. The collector
traces parked root stacks with the parked vectors.

**A blocking call is made `outside` the lock.** `threads::outside(call)`
parks the thread as if it waited, hands the heap to a thread able to run, or
else to a waiting one to keep the scheduler going, or else to nobody, and
makes the call. Coming back, it takes the heap if nobody has it, or gets in
line and writes a byte to the wake pipe, and the holder puts it back in line
at its next turn check or wakes from `poll`. A thread outside counts as one
that will come back, so it never makes a deadlock. `open(2)` and
`getaddrinfo` are made there now. A call made outside must not touch the
heap, so the primitive reads its arguments first and builds objects from
the answer afterwards.

**A thread's end is its loop's end.** `__thread_exit__` is gone. The root
fiber of a thread running out of frames ends `run`, and the OS thread marks
the thread dead and hands the heap on. A refusal nothing in the thread
catches is left for the main thread, which meets it where it waits: from
the switch it waits in, or at its next turn check if it was outside the
lock. This is the same place `abort_to_main` put it.

**The program's end kills the threads, after `at_exit`.** Threads are no
longer dropped at the end of each evaluation. `Kernel.__exit__` runs the
`at_exit` blocks with every thread alive, as CRuby does; once an uncaught
error is reported, `Kernel.__cleanup__` has `Thread.__terminate_all__` kill
the rest and wait for them, and then the finalizers run. A kill is a
`throw` of the `Thread`, and it now goes out of every fiber the thread is
in, down to the `catch` at the thread's bottom.
So the `ensure` clauses of the fiber a thread is in run, and a suspended
fiber's do not.

**Unsafe code.** The heap goes from one OS thread to another as a raw
pointer, through the lock's mutex. The holder hands over the pointer of
its own scope, so the heap escapes into the lock before the wait and comes
back out of it after. A spawned thread's outermost scope is never dropped,
because dropping would truncate a root stack that by then belongs to
another thread. Its roots are let go by hand before the hand-over.

## Not done

- Only `open(2)` and `getaddrinfo` go outside the lock. Every other
  blocking primitive is one `outside` call away. A blocking FFI call is the
  next one, once `Spinel::FFI` declares `blocking: true` (#105).
- A call made outside cannot be interrupted. `Thread#raise` and
  `Thread#kill` are met when it returns, so a program that ends with a
  thread stuck in one leaves that thread to the process's end.
  CRuby's unblocking function signals the waiting OS thread, and that is
  the upgrade.
- A thread an embedder never ends, or one the program's end did not kill,
  keeps its OS thread waiting for a turn that never comes, until the
  process ends.
- `fork` from a thread other than the main one: in the child, the end of
  that thread ends the process, with `at_exit` unrun. CRuby makes it the
  child's main thread.

## Check

`core/thread` 382 to 384 (`native_thread_id`, and `kill` with a fiber
active), `core/exception` 241 to 243 (`top_level`'s threads and fibers at
the program's end), `core/file` 876 to 877 (`File.open` on a FIFO). Corpus
28144 to 28149, and no example that passed fails. `os_threads.rb` in
`crates/spinel-cli/tests/threads.rs` checks distinct native ids, a FIFO open
that leaves another thread running, and a sleeping thread killed at the
end, `ensure` and all, after `at_exit`.
