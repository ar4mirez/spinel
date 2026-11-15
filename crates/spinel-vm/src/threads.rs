//! Threads (#45, #328): the VM half, under `interp`.
//!
//! A thread is what a fiber is — the interpreter's two vectors, its value
//! stack and its frames — plus the fiber bookkeeping of its own: which fiber
//! of the thread runs, its root fiber's object, and the root's vectors while a
//! fiber was transferred to. Every heap has one table of them, beside its
//! fibers, and the collector traces both.
//!
//! Each thread runs on an OS thread of its own, and they take turns with the
//! heap under the per-Ractor lock `docs/engine.md` describes (#328): the OS
//! thread holding it runs its thread's loop (`interp::run`), and every other
//! waits for its turn. A switch parks the running thread here — its vectors,
//! its root stack, its fiber bookkeeping and specials — picks the next, hands
//! it the heap and waits to be handed it back; the next takes its own back
//! from here and goes on in its own loop. So a switch is what it was when the
//! threads took turns on one OS thread, and the scheduler is the same; what
//! the OS threads add is a call that blocks in the kernel — `flock`, `fsync`,
//! `open(2)` of a FIFO, a read or write of a blocking pipe or terminal,
//! `getaddrinfo` — made [`outside`] the lock, its thread parked as if it
//! waited while the others run.
//!
//! Such a call is interrupted as CRuby's blocking region is, by an unblocking
//! function ([`outside_interruptibly`]): its OS thread is recorded in the
//! lock, and a kill or raise for it, a signal for the main thread, a `close`
//! of the descriptor it uses and the program's end send that OS thread
//! [`UNBLOCK`], installed without `SA_RESTART`, so the call answers EINTR.
//! The thread comes back, gets in line, and meets what was asked of it
//! (`Thread.__check_interrupts__`), then makes the call again if nothing
//! raised. The signal is sent again a [`RESEND`] apart until it comes back,
//! as CRuby's timer thread sends it, since the first may arrive before the
//! call enters the kernel.
//!
//! What a thread *does* — `join`, `Mutex`, `Queue`, `Thread#raise`'s
//! exception, `report_on_exception` — is `core/thread.rb`. This file is the
//! scheduler under it: who runs, who waits for what, and the switches. A
//! thread that waits says what for, a deadline or a descriptor becoming
//! ready, or nothing (another thread's `wakeup`); when no thread can run the
//! scheduler waits in `poll(2)` for the first of them, or for a thread back
//! from outside the lock; and when every thread waits on another for ever,
//! the main thread is woken to raise the deadlock. The interpreter loop
//! preempts the running thread every time slice, so a thread that never
//! waits does not starve the others.
//!
//! ponytail: `getaddrinfo` is made outside the lock with no unblocking
//! function — glibc's resolver restarts what EINTR cuts short — so a kill or
//! raise for a thread in it is met when it returns, where CRuby 3.3 resolves
//! on a thread of its own it can leave. A signal for the main thread while
//! every thread is outside the lock is met when one comes back: nothing is
//! running the scheduler to send [`UNBLOCK`].

use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::{
    Call, Context, Error, HandleScope, Heap, Pending, Stack, Target, Unwind, Value, adopt_roots,
    frame_roots, push_proc_frame,
};
use crate::shared::signals::UNBLOCK;

/// How long a thread runs before the loop lets another have a turn.
const SLICE: Duration = Duration::from_millis(10);

/// How long a thread asked to stop outside the lock is given before it is
/// sent [`UNBLOCK`] again: CRuby's timer thread's quantum for its `ubf_list`.
const RESEND: Duration = Duration::from_millis(10);

/// Instructions between the loop's looks at the clock.
pub(super) const SLICE_INSNS: u64 = 4096;

/// The stack each thread's OS thread is made with: the main thread's usual
/// size, since a primitive that recurses — `inspect`, `Marshal` — runs on it.
const OS_STACK: usize = 8 << 20;

pub(super) enum ThreadState {
    /// Running now: its vectors are the loop's.
    Running,
    /// Able to run, its vectors here; `push` is what the call it stopped in
    /// answers when it goes on, or nothing when it was preempted between
    /// instructions.
    Ready {
        context: Context,
        push: Option<Value>,
    },
    /// Waiting, its vectors here.
    Blocked {
        context: Context,
        deadline: Option<Instant>,
        fds: Vec<(libc::c_int, libc::c_short)>,
        /// Whether waiting for ever here counts toward a deadlock: `join`,
        /// `Queue#pop` and `Mutex#lock` do; `sleep` does not, as in CRuby,
        /// since a signal could end it.
        deadlockable: bool,
    },
    /// In a call made [`outside`] the lock, its vectors here, until the call
    /// returns and it gets back in line; `unblockable` when an unblocking
    /// function can end the call early ([`outside_interruptibly`]).
    Outside { context: Context, unblockable: bool },
    /// Finished.
    Dead,
}

/// A thread's own fiber bookkeeping, kept here while another thread runs.
#[derive(Default)]
pub(super) struct FiberWorld {
    pub(super) current: Option<usize>,
    pub(super) root: Option<Value>,
    pub(super) root_parked: Option<Context>,
}

pub(super) struct ThreadEntry {
    /// The Ruby `Thread`; nil for the main thread until Ruby names it.
    pub(super) object: Value,
    pub(super) state: ThreadState,
    pub(super) fibers: FiberWorld,
    /// Raises and kills for it, the first met when it next runs and each of
    /// the rest the time after: a kill that follows a raise waits for the
    /// raise to be met, as CRuby's queue of interrupts does. Each is a frame,
    /// built in the thread that asked, that `core/thread.rb` runs here — so a
    /// raise is asked for `#exception` again in this thread and takes this
    /// thread's backtrace, where it was interrupted, as CRuby's does (#328).
    pub(super) pending: VecDeque<Call>,
    /// Its `$~`, `$!`, `$_` and `$?` while another thread runs: one slot
    /// each per heap in the VM, one each per thread in Ruby.
    pub(super) specials: [Value; 4],
    /// Its root stack while another thread runs: what the handles its loop
    /// and its primitives hold are indexes into.
    pub(super) roots: Vec<Value>,
    /// Its OS thread, as the kernel names it (`gettid(2)` on Linux).
    pub(super) native_id: i64,
}

/// Every thread a heap has made. Empty until the first one is: the main
/// thread is entry 0, made then.
#[derive(Default)]
pub(crate) struct ThreadTable {
    pub(super) entries: Vec<ThreadEntry>,
    pub(super) current: usize,
    /// The main thread's entry: 0, but in a child forked from another
    /// thread, the one that forked, as CRuby's `rb_thread_atfork` makes it.
    pub(super) main: usize,
    pub(super) slice_started: Option<Instant>,
    /// Whether the running thread owes another a turn it could not give
    /// because it was in the core library at the turn check (#728): the loop
    /// checks again at the call or return that leaves the core library.
    pub(super) owed: bool,
    /// Whether the running thread was last entered out of a wait, woken:
    /// an interrupt met there is met blocking.
    pub(super) woken: bool,
    /// Whether a trap handler runs: signals that wait go on waiting until it
    /// returns, as CRuby's `TRAP_INTERRUPT_MASK` keeps them (#44).
    pub(super) trapping: bool,
    /// Which OS thread has the heap; made with the main thread's entry.
    lock: Option<Arc<Lock>>,
    /// A refusal a thread met with nothing in it to catch it: the thread
    /// ended, and the main thread meets the refusal where it waits, which
    /// is the example that started the thread.
    refusal: Option<Error>,
}

impl ThreadTable {
    pub(super) fn each_root(&self, f: &mut impl FnMut(Value)) {
        for entry in &self.entries {
            f(entry.object);
            match &entry.state {
                ThreadState::Ready { context: c, push } => {
                    context_roots(c, f);
                    if let Some(value) = push {
                        f(*value);
                    }
                }
                ThreadState::Blocked { context: c, .. }
                | ThreadState::Outside { context: c, .. } => {
                    context_roots(c, f);
                }
                ThreadState::Running | ThreadState::Dead => {}
            }
            if let Some(root) = entry.fibers.root {
                f(root);
            }
            if let Some(parked) = &entry.fibers.root_parked {
                context_roots(parked, f);
            }
            for frame in &entry.pending {
                frame_roots(frame, f);
            }
            entry.specials.iter().copied().for_each(&mut *f);
            entry.roots.iter().copied().for_each(&mut *f);
        }
    }

    /// Whether a thread other than the running one is alive.
    pub(super) fn others(&self) -> bool {
        self.entries
            .iter()
            .enumerate()
            .any(|(i, e)| i != self.current && !matches!(e.state, ThreadState::Dead))
    }
}

fn context_roots(context: &Context, f: &mut impl FnMut(Value)) {
    context.stack.iter().copied().for_each(&mut *f);
    for frame in &context.frames {
        frame_roots(frame, f);
    }
}

/// The per-Ractor lock: which thread's OS thread may use the heap.
struct Lock {
    held: Mutex<Held>,
    /// A pipe a thread back from [`outside`] writes a byte to, so that a
    /// scheduler waiting in `poll(2)` for something else wakes to put it
    /// back in line.
    wake: [libc::c_int; 2],
    /// Told when a thread leaves a call made [`outside_interruptibly`], for
    /// a `close` waiting for the ones using its descriptor ([`close_fd`]).
    left: Condvar,
}

struct Held {
    /// The thread whose OS thread has the heap; none while every thread that
    /// could have it is outside the lock.
    holder: Option<usize>,
    /// The heap, as its last holder handed it on.
    heap: HeapPtr,
    /// What each thread's OS thread waits on for its turn, by index.
    turns: Vec<Arc<Condvar>>,
    /// Threads back from outside the lock, to be put back in line.
    back: Vec<usize>,
    /// Each thread's call made [`outside_interruptibly`], while it is in one:
    /// what its unblocking function needs. By index, beside `turns`.
    blocking: Vec<Option<Blocking>>,
}

/// A call made outside the lock that an unblocking function can end, as
/// CRuby keeps a thread's `unblock` while it is in a blocking region.
struct Blocking {
    /// The OS thread making it, which [`UNBLOCK`] is sent to.
    os: OsThread,
    /// The descriptor it uses, for a `close` in another thread.
    fd: Option<libc::c_int>,
    /// Whether it has been asked to stop: looked at once before the call,
    /// and the signal is sent again while it stays.
    asked: bool,
    /// When the signal last went.
    sent: Instant,
}

/// A `pthread_t`, which is a pointer on macOS.
struct OsThread(libc::pthread_t);

// SAFETY: a `pthread_t` names an OS thread to the C library from any thread;
// nothing is reached through it but `pthread_kill`.
unsafe impl Send for OsThread {}

impl Held {
    /// Ask thread `index`, if it is in a call made outside the lock that an
    /// unblocking function can end, to stop: [`UNBLOCK`] to its OS thread,
    /// unless it went less than a [`RESEND`] ago.
    fn unblock(&mut self, index: usize) {
        if let Some(call) = self.blocking.get_mut(index).and_then(Option::as_mut)
            && !(call.asked && call.sent.elapsed() < RESEND)
        {
            call.asked = true;
            call.sent = Instant::now();
            // SAFETY: the OS thread is alive — it empties this slot, under
            // this mutex, before it can end — and `pthread_kill` reads
            // nothing else. `UNBLOCK`'s handler was installed with the lock.
            unsafe { libc::pthread_kill(call.os.0, UNBLOCK) };
        }
    }

    /// Send [`UNBLOCK`] again to each thread asked to stop that is still in
    /// its call; whether any is.
    fn unblock_again(&mut self) -> bool {
        let mut any = false;
        for index in 0..self.blocking.len() {
            if self.blocking[index].as_ref().is_some_and(|call| call.asked) {
                any = true;
                self.unblock(index);
            }
        }
        any
    }

    /// Whether a thread but `me` is in a call outside the lock using `fd`;
    /// each is asked to stop.
    fn unblock_users(&mut self, me: usize, fd: libc::c_int) -> bool {
        let mut any = false;
        for index in 0..self.blocking.len() {
            if index != me
                && self.blocking[index]
                    .as_ref()
                    .is_some_and(|call| call.fd == Some(fd))
            {
                any = true;
                self.unblock(index);
            }
        }
        any
    }
}

struct HeapPtr(*mut Heap);

// SAFETY: the heap goes from one OS thread to another only through `Held`,
// under its mutex, and only the OS thread `holder` names uses it.
unsafe impl Send for HeapPtr {}

impl Lock {
    fn new(heap: *mut Heap, holder: usize, threads: usize) -> Lock {
        let mut wake = [-1; 2];
        // SAFETY: `wake` is the two descriptors `pipe` fills in; `fcntl` only
        // sets flags on them.
        unsafe {
            if libc::pipe(wake.as_mut_ptr()) == 0 {
                for fd in wake {
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                    libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
                }
            }
        }
        // Before any thread could be sent it.
        crate::shared::signals::install_unblock();
        Lock {
            held: Mutex::new(Held {
                holder: Some(holder),
                heap: HeapPtr(heap),
                turns: (0..threads).map(|_| Arc::new(Condvar::new())).collect(),
                back: Vec::new(),
                blocking: (0..threads).map(|_| None).collect(),
            }),
            wake,
            left: Condvar::new(),
        }
    }

    fn held(&self) -> MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hand the heap to `to`, or to nobody.
    fn give(&self, heap: *mut Heap, to: Option<usize>) {
        let mut held = self.held();
        held.heap = HeapPtr(heap);
        held.holder = to;
        if let Some(to) = to {
            held.turns[to].notify_one();
        }
    }

    /// Wait for `me`'s turn with the heap; the heap, once it is.
    fn wait(&self, me: usize) -> *mut Heap {
        Self::wait_in(self.held(), me)
    }

    fn wait_in(mut held: MutexGuard<'_, Held>, me: usize) -> *mut Heap {
        let turn = Arc::clone(&held.turns[me]);
        while held.holder != Some(me) {
            held = turn.wait(held).unwrap_or_else(PoisonError::into_inner);
        }
        held.heap.0
    }

    /// Back from outside: take the heap if nobody has it, or get in line
    /// and wait for a turn.
    fn come_back(&self, me: usize) -> *mut Heap {
        let mut held = self.held();
        if held.holder.is_none() {
            held.holder = Some(me);
            return held.heap.0;
        }
        held.back.push(me);
        // SAFETY: one byte, from a live local, to the pipe's write end. A
        // full pipe answers EAGAIN, and is waking the scheduler already; an
        // unblocking signal sent as the call returned may cut it short.
        while unsafe { libc::write(self.wake[1], [1u8].as_ptr().cast(), 1) } < 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        {}
        Self::wait_in(held, me)
    }

    /// Empty the wake pipe.
    fn drain(&self) {
        let mut bytes = [0u8; 64];
        // SAFETY: reads into a live local buffer of its own length; the read
        // end is nonblocking, so an empty pipe answers -1.
        while unsafe { libc::read(self.wake[0], bytes.as_mut_ptr().cast(), bytes.len()) } > 0 {}
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        for fd in self.wake {
            if fd >= 0 {
                // SAFETY: the lock made both descriptors and nothing else
                // closes them.
                unsafe { libc::close(fd) };
            }
        }
    }
}

/// The calling OS thread, as the kernel names it.
fn native_id() -> i64 {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `gettid` has no preconditions.
        i64::from(unsafe { libc::gettid() })
    }
    #[cfg(target_vendor = "apple")]
    {
        let mut id: u64 = 0;
        // SAFETY: a null thread is the calling one, and `id` is where the
        // answer goes.
        unsafe { libc::pthread_threadid_np(0 as libc::pthread_t, &mut id) };
        id as i64
    }
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    {
        // SAFETY: `pthread_self` has no preconditions.
        unsafe { libc::pthread_self() as i64 }
    }
}

fn ensure_main(scope: &mut HandleScope<'_>) {
    if scope.fibers().threads.entries.is_empty() {
        let heap = scope.heap_ptr();
        let threads = &mut scope.fibers_mut().threads;
        threads.entries.push(ThreadEntry {
            object: Value::NIL,
            state: ThreadState::Running,
            fibers: FiberWorld::default(),
            pending: VecDeque::new(),
            specials: [Value::NIL; 4],
            roots: Vec::new(),
            native_id: native_id(),
        });
        threads.current = 0;
        threads.slice_started = Some(Instant::now());
        threads.lock = Some(Arc::new(Lock::new(heap, 0, 1)));
    }
}

fn lock(scope: &HandleScope<'_>) -> Arc<Lock> {
    let lock = scope.fibers().threads.lock.as_ref();
    Arc::clone(lock.expect("the main thread's entry made the lock"))
}

/// A new thread running `block`, its first frame pushed now so its OS thread,
/// started now and waiting for its turn, can enter it. It runs when the
/// scheduler gets to it.
pub(super) fn spawn(
    scope: &mut HandleScope<'_>,
    call: &Pending,
    object: Value,
    block: Value,
    ids: &mut u64,
) -> Result<usize, Error> {
    ensure_main(scope);
    let stack = Stack::default();
    let mut frames: Vec<Call> = Vec::new();
    // The block is entered with no arguments: `core/thread.rb` hands it a
    // lambda that calls the thread's own block with the thread's.
    push_proc_frame(
        scope,
        &stack,
        &mut frames,
        &block_call(call, block),
        block,
        ids,
    )?;
    let index = scope.fibers().threads.entries.len();
    let lock = lock(scope);
    {
        let mut held = lock.held();
        held.turns.push(Arc::new(Condvar::new()));
        held.blocking.push(None);
    }
    let (told, started) = std::sync::mpsc::channel();
    let its_lock = Arc::clone(&lock);
    let spawned = std::thread::Builder::new()
        .stack_size(OS_STACK)
        .spawn(move || {
            // Signals go to the main thread's OS thread, which runs their
            // handlers and whose waits they cut short (#44).
            crate::shared::signals::block_on_this_thread();
            let _ = told.send(native_id());
            os_thread(&its_lock, index);
        });
    if let Err(error) = spawned {
        let mut held = lock.held();
        held.turns.pop();
        held.blocking.pop();
        drop(held);
        let reason = error
            .raw_os_error()
            .map_or_else(|| error.to_string(), crate::errno::message);
        return Err(Error::raise(
            "ThreadError",
            format!("can't create Thread: {reason}"),
        ));
    }
    let native_id = started.recv().unwrap_or_default();
    scope.fibers_mut().threads.entries.push(ThreadEntry {
        object,
        state: ThreadState::Ready {
            context: Context { stack, frames },
            push: None,
        },
        fibers: FiberWorld::default(),
        pending: VecDeque::new(),
        specials: [Value::NIL; 4],
        roots: Vec::new(),
        native_id,
    });
    Ok(index)
}

/// A thread's OS thread: wait for the first turn, run the thread's loop over
/// its vectors, and hand the heap on when it is over.
fn os_thread(lock: &Lock, me: usize) {
    let heap = lock.wait(me);
    // SAFETY: the heap is this OS thread's until it hands it on. Whoever had
    // it parked what was theirs on it — vectors, root stack — before handing
    // it here, and waits to be handed it back (`hand_to`), so nothing else
    // uses it meanwhile; every later turn is handed the same way. It outlives
    // the thread's turns: only a thread with a turn hands it on, so once the
    // main thread's last evaluation is over nothing hands it on again, and a
    // thread still alive then — one the program's end did not kill
    // (`Thread.__terminate_all__`), or one an embedder never ended — waits
    // for ever, its OS thread with it.
    let heap = unsafe { &mut *heap };
    // Never dropped: dropping truncates the root stack, which by the time
    // this thread is over belongs to whoever it handed the heap to. `finish`
    // lets go of this thread's roots by hand first.
    let mut scope = ManuallyDrop::new(heap.scope());
    hand_to(&mut scope, me, me);
    let (mut stack, mut frames) = (Stack::default(), Vec::new());
    enter(&mut scope, &mut stack, &mut frames, me, true);
    let ids = scope.fibers().frame_ids;
    let (result, ids) = super::run(&mut scope, stack, frames, ids);
    finish(&mut scope, me, result, ids);
}

/// A thread's loop is over: it is dead, and the heap goes on.
fn finish(scope: &mut HandleScope<'_>, me: usize, result: Result<Value, Error>, ids: u64) {
    let _ = scope.swap_roots(Vec::new());
    let fibers = scope.fibers_mut();
    fibers.frame_ids = ids;
    fibers.current = None;
    fibers.root = None;
    fibers.root_parked = None;
    let threads = &mut fibers.threads;
    threads.entries[me].state = ThreadState::Dead;
    threads.entries[me].pending.clear();
    // A refusal nothing in the thread could take is the main thread's to
    // meet. Anything else that ends the loop — an exception its last `ensure`
    // raised, since `core/thread.rb` rescues the rest — is dropped with it,
    // as CRuby's thread would end on it.
    let mut first = None;
    if let Err(error) = result
        && error.is_refusal()
    {
        threads.refusal = Some(error);
        let main = threads.main;
        let entry = &mut threads.entries[main];
        entry.state = match std::mem::replace(&mut entry.state, ThreadState::Dead) {
            ThreadState::Ready { context, .. } | ThreadState::Blocked { context, .. } => {
                first = Some(main);
                ThreadState::Ready {
                    context,
                    push: None,
                }
            }
            // Outside the lock: it meets the refusal at its next turn
            // check, its call ended early where one can be.
            other => other,
        };
        if let Some(lock) = &threads.lock {
            lock.held().unblock(main);
        }
    }
    if me == threads.main {
        // ponytail: this thread forked, and in the child it is the main
        // thread, whose end ends the process. Its end ends the process here,
        // `at_exit` unrun.
        std::process::exit(0);
    }
    release(scope, me, first);
}

/// A call of `block` with no arguments, as `call` would make it.
fn block_call(call: &Pending, block: Value) -> Pending {
    Pending {
        cache: None,
        name: call.name,
        receiver: block,
        args: Vec::new(),
        keywords: Vec::new(),
        block: Value::NIL,
        block_is_literal: false,
        cref: call.cref,
        implicit_self: false,
        variable_call: false,
        public_only: false,
        target: Target::Block(block),
        owner: None,
        defined_as: None,
    }
}

/// The frame `Thread#raise` and `Thread#kill` leave for another thread: a call of `block`,
/// with no arguments and its value dropped, pushed when that thread next runs.
pub(super) fn interrupt_frame(
    scope: &mut HandleScope<'_>,
    call: &Pending,
    block: Value,
    ids: &mut u64,
) -> Result<Call, Error> {
    let stack = Stack::default();
    let mut frames: Vec<Call> = Vec::new();
    push_proc_frame(
        scope,
        &stack,
        &mut frames,
        &block_call(call, block),
        block,
        ids,
    )?;
    let mut frame = frames.pop().expect("push_proc_frame pushed a frame");
    frame.discards_value = true;
    Ok(frame)
}

pub(super) fn current(scope: &mut HandleScope<'_>) -> usize {
    ensure_main(scope);
    scope.fibers().threads.current
}

pub(super) fn main(scope: &mut HandleScope<'_>) -> usize {
    ensure_main(scope);
    scope.fibers().threads.main
}

pub(super) fn set_object(scope: &mut HandleScope<'_>, index: usize, object: Value) {
    ensure_main(scope);
    if let Some(entry) = scope.fibers_mut().threads.entries.get_mut(index) {
        entry.object = object;
    }
}

pub(super) fn object(scope: &HandleScope<'_>, index: usize) -> Value {
    scope
        .fibers()
        .threads
        .entries
        .get(index)
        .map_or(Value::NIL, |e| e.object)
}

/// `:run`, `:sleep` or `:dead` for a thread. One outside the lock sleeps, as
/// CRuby's in a blocking region does.
pub(super) fn status(scope: &HandleScope<'_>, index: usize) -> &'static str {
    match scope.fibers().threads.entries.get(index).map(|e| &e.state) {
        None => "run",
        Some(ThreadState::Running | ThreadState::Ready { .. }) => "run",
        Some(ThreadState::Blocked { .. } | ThreadState::Outside { .. }) => "sleep",
        Some(ThreadState::Dead) => "dead",
    }
}

/// Whether a throw of `tag` is a kill of the running thread: `core/thread.rb`
/// kills a thread by throwing its `Thread`, caught at the thread's bottom.
pub(super) fn is_kill(scope: &HandleScope<'_>, tag: Value) -> bool {
    let threads = &scope.fibers().threads;
    threads.current != threads.main
        && !tag.is_nil()
        && threads
            .entries
            .get(threads.current)
            .is_some_and(|e| e.object == tag)
}

/// Whether a thread is in a call made [`outside`] the lock that nothing can
/// end early: one the program's end leaves to the process's end.
pub(super) fn is_outside_for_good(scope: &HandleScope<'_>, index: usize) -> bool {
    scope.fibers().threads.entries.get(index).is_some_and(|e| {
        matches!(
            e.state,
            ThreadState::Outside {
                unblockable: false,
                ..
            }
        )
    })
}

/// Whether a thread is in a call made outside the lock that an unblocking
/// function can end.
fn is_unblockable(scope: &HandleScope<'_>, index: usize) -> bool {
    scope.fibers().threads.entries.get(index).is_some_and(|e| {
        matches!(
            e.state,
            ThreadState::Outside {
                unblockable: true,
                ..
            }
        )
    })
}

/// Ask thread `index` to stop the call it makes outside the lock, if an
/// unblocking function can end it.
fn unblock(scope: &HandleScope<'_>, index: usize) {
    if let Some(lock) = &scope.fibers().threads.lock {
        lock.held().unblock(index);
    }
}

/// Whether the running thread is the only one alive, which no other thread
/// can interrupt: the main thread.
pub(super) fn alone(scope: &HandleScope<'_>) -> bool {
    let threads = &scope.fibers().threads;
    threads.entries.len() < 2 || !threads.others()
}

/// The OS thread a thread runs on.
pub(super) fn native_thread_id(scope: &mut HandleScope<'_>, index: usize) -> Option<i64> {
    ensure_main(scope);
    let entry = scope.fibers().threads.entries.get(index)?;
    (!matches!(entry.state, ThreadState::Dead)).then_some(entry.native_id)
}

/// Make a waiting thread able to run, its wait answering `reason`. False
/// when it was not waiting.
pub(super) fn wakeup(scope: &mut HandleScope<'_>, index: usize, reason: Value) -> bool {
    let threads = &mut scope.fibers_mut().threads;
    let Some(entry) = threads.entries.get_mut(index) else {
        return false;
    };
    if !matches!(entry.state, ThreadState::Blocked { .. }) {
        return false;
    }
    let ThreadState::Blocked { context, .. } =
        std::mem::replace(&mut entry.state, ThreadState::Dead)
    else {
        unreachable!("matched above")
    };
    entry.state = ThreadState::Ready {
        context,
        push: Some(reason),
    };
    true
}

/// Leave `interrupt` for a thread to meet when it next runs, and wake it: from
/// its wait, or from a call it makes outside the lock that an unblocking
/// function can end, as CRuby's `rb_threadptr_interrupt` calls its `unblock`.
pub(super) fn interrupt(scope: &mut HandleScope<'_>, index: usize, interrupt: Call) {
    let interrupted = Value::symbol(crate::shared::symbols::intern("interrupted"));
    if let Some(entry) = scope.fibers_mut().threads.entries.get_mut(index) {
        if matches!(entry.state, ThreadState::Dead) {
            return;
        }
        entry.pending.push_back(interrupt);
    }
    wakeup(scope, index, interrupted);
    unblock(scope, index);
}

/// Meet the running thread's first interrupt waiting, now, as CRuby's
/// `RUBY_VM_CHECK_INTS` meets one: its frame goes on top, as a call the
/// primitive asking returns to, with nil, the primitive's answer, below it.
/// False, and nothing pushed, when none waits.
pub(super) fn check(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
) -> bool {
    let threads = &mut scope.fibers_mut().threads;
    let me = threads.current;
    let Some(mut frame) = threads
        .entries
        .get_mut(me)
        .and_then(|e| e.pending.pop_front())
    else {
        return false;
    };
    stack.push(Value::NIL);
    frame.floor = stack.len();
    frame.base = stack.len();
    frame.errinfo_on_entry = scope.errinfo();
    // The table held it; the loop's frames are held by handles.
    frame_roots(&frame, &mut |value| {
        if !value.is_immediate() {
            scope.root(value);
        }
    });
    frames.push(frame);
    true
}

/// `close(2)` of `fd` is about to be made: every other thread in a call
/// outside the lock that uses it is asked to stop, and waited for, as CRuby's
/// `rb_notify_fd_close` and `rb_thread_io_close_wait` do, so the descriptor
/// is not closed — and maybe made again for something else — under a call
/// still using it. Each finds its IO closed when it next runs. The heap stays
/// this thread's meanwhile: theirs leave the call without it.
pub(super) fn close_fd(scope: &HandleScope<'_>, fd: libc::c_int) {
    let threads = &scope.fibers().threads;
    let Some(lock) = &threads.lock else {
        return;
    };
    let me = threads.current;
    let mut held = lock.held();
    while held.unblock_users(me, fd) {
        held = lock
            .left
            .wait_timeout(held, RESEND)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

/// How the running thread leaves the processor.
pub(super) enum Leave {
    /// Still able to run: `Thread.pass`, or preempted (`push` none).
    Ready(Option<Value>),
    /// Waiting for one of these, or a wakeup.
    Block {
        deadline: Option<Instant>,
        fds: Vec<(libc::c_int, libc::c_short)>,
        deadlockable: bool,
    },
}

fn reason(name: &str) -> Value {
    Value::symbol(crate::shared::symbols::intern(name))
}

/// Put the threads back from outside the lock back in line.
fn take_back(scope: &mut HandleScope<'_>) -> usize {
    let back = match &scope.fibers().threads.lock {
        Some(lock) => std::mem::take(&mut lock.held().back),
        None => return 0,
    };
    let mut woken = 0;
    for index in back {
        let entry = &mut scope.fibers_mut().threads.entries[index];
        let ThreadState::Outside { context, .. } =
            std::mem::replace(&mut entry.state, ThreadState::Dead)
        else {
            unreachable!("only a thread outside the lock comes back")
        };
        entry.state = ThreadState::Ready {
            context,
            push: None,
        };
        woken += 1;
    }
    woken
}

/// Wake every waiting thread whose deadline has passed or whose descriptor
/// is ready, and every thread back from outside the lock, waiting up to
/// `timeout` for one to be; how many woke.
fn wake_ready(scope: &mut HandleScope<'_>, timeout: Option<Duration>) -> usize {
    // A thread asked to stop a call outside the lock that has not come back
    // is sent the signal again, and the wait below looks again a resend from
    // now: the first may have come before its call entered the kernel.
    let mut timeout = timeout;
    if let Some(lock) = &scope.fibers().threads.lock
        && lock.held().unblock_again()
    {
        timeout = Some(timeout.map_or(RESEND, |t| t.min(RESEND)));
    }
    let now = Instant::now();
    let mut woken = 0;
    let mut polled: Vec<(usize, libc::pollfd)> = Vec::new();
    let mut expired = Vec::new();
    let mut outside = false;
    for (index, entry) in scope.fibers().threads.entries.iter().enumerate() {
        match &entry.state {
            ThreadState::Blocked { deadline, fds, .. } => {
                if deadline.is_some_and(|d| d <= now) {
                    expired.push(index);
                } else {
                    for &(fd, events) in fds {
                        polled.push((
                            index,
                            libc::pollfd {
                                fd,
                                events,
                                revents: 0,
                            },
                        ));
                    }
                }
            }
            ThreadState::Outside { .. } => outside = true,
            _ => {}
        }
    }
    for index in expired {
        if wakeup(scope, index, reason("timeout")) {
            woken += 1;
        }
    }
    woken += wake_for_signal(scope);
    // A signal ends the main thread's wait, or the call it makes outside the
    // lock, on the signals' own wake pipe; a look that waits for nothing has
    // already seen whether one came.
    let main = scope.fibers().threads.main;
    if timeout.is_none_or(|t| !t.is_zero())
        && (main_blocked(scope) || is_unblockable(scope, main))
        && !scope.fibers().threads.trapping
        && let Some(fd) = crate::shared::signals::wake_fd()
    {
        polled.push((
            SIGNALED,
            libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            },
        ));
    }
    // A thread outside the lock is waited for on the wake pipe.
    let wake = if outside {
        woken += take_back(scope);
        let lock = lock(scope);
        polled.push((
            usize::MAX,
            libc::pollfd {
                fd: lock.wake[0],
                events: libc::POLLIN,
                revents: 0,
            },
        ));
        Some(lock)
    } else {
        None
    };
    if woken > 0 || (polled.is_empty() && timeout.is_some_and(|t| t.is_zero())) {
        return woken;
    }
    let ms = match timeout {
        None => -1,
        Some(t) => {
            t.as_millis().min(i32::MAX as u128) as libc::c_int
                + i32::from(t.subsec_nanos() % 1_000_000 != 0)
        }
    };
    let mut fds: Vec<libc::pollfd> = polled.iter().map(|(_, p)| *p).collect();
    // Two threads may wait on one descriptor, which macOS's `poll` cannot be
    // asked twice about.
    if crate::socket::poll(&mut fds, ms).is_ok_and(|ready| ready > 0) {
        for ((index, _), pollfd) in polled.iter().zip(&fds) {
            if pollfd.revents == 0 {
                continue;
            }
            if *index == usize::MAX {
                if let Some(lock) = &wake {
                    lock.drain();
                }
                woken += take_back(scope);
            } else if *index == SIGNALED {
                crate::shared::signals::drain();
            } else if wakeup(scope, *index, reason("ready")) {
                woken += 1;
            }
        }
    }
    // A signal may have cut the `poll` short (EINTR) as well as written.
    woken += wake_for_signal(scope);
    let now = Instant::now();
    let late: Vec<usize> = scope
        .fibers()
        .threads
        .entries
        .iter()
        .enumerate()
        .filter(
            |(_, e)| matches!(e.state, ThreadState::Blocked { deadline: Some(d), .. } if d <= now),
        )
        .map(|(i, _)| i)
        .collect();
    for index in late {
        if wakeup(scope, index, reason("timeout")) {
            woken += 1;
        }
    }
    woken
}

/// The `polled` index of the signals' wake pipe.
const SIGNALED: usize = usize::MAX - 1;

/// Whether the main thread waits.
fn main_blocked(scope: &HandleScope<'_>) -> bool {
    scope
        .fibers()
        .threads
        .entries
        .get(scope.fibers().threads.main)
        .is_some_and(|e| matches!(e.state, ThreadState::Blocked { .. }))
}

/// A signal waiting wakes the main thread from its wait, answering
/// `:signal`, for the wait to run the handlers and go on waiting, or hand
/// back to its caller to look again (`Kernel#__thread_wait__`). How many
/// woke: one or none. A main thread in a call outside the lock is asked to
/// stop it instead, and runs them when it is back (`__check_interrupts__`):
/// the signal itself restarts the call it cuts short (`SA_RESTART`).
fn wake_for_signal(scope: &mut HandleScope<'_>) -> usize {
    if !signal_waits(scope) {
        return 0;
    }
    let main = scope.fibers().threads.main;
    if is_unblockable(scope, main) {
        unblock(scope, main);
        return 0;
    }
    usize::from(wakeup(scope, main, reason("signal")))
}

/// Whether a signal waits to be handled now: none does while a trap
/// handler runs.
pub(super) fn signal_waits(scope: &HandleScope<'_>) -> bool {
    crate::shared::signals::any() && !scope.fibers().threads.trapping
}

/// Whether the running thread owes another a turn the core library kept it
/// from giving at the last turn check (#728).
pub(super) fn turn_owed(scope: &HandleScope<'_>) -> bool {
    scope.fibers().threads.owed
}

/// Whether the top frame is the core library's, which `preempt` does not
/// interleave with another thread.
pub(super) fn in_core_library(frames: &[Call]) -> bool {
    frames
        .last()
        .and_then(|frame| frame.iseq.path.as_deref())
        .is_some_and(|path| path.starts_with("<internal:"))
}

/// Set whether a trap handler runs, answering whether one did.
pub(super) fn set_trapping(scope: &mut HandleScope<'_>, trapping: bool) -> bool {
    std::mem::replace(&mut scope.fibers_mut().threads.trapping, trapping)
}

/// For the loop's turn check when a signal waits: whether the running
/// thread is the main thread, which then handles it; another wakes the main
/// thread if it waits, and otherwise leaves it for its next turn check.
pub(super) fn signal_is_mine(scope: &mut HandleScope<'_>) -> bool {
    ensure_main(scope);
    let threads = &scope.fibers().threads;
    if threads.current == threads.main {
        return true;
    }
    wake_for_signal(scope);
    false
}

/// The next thread able to run, looking after `after` first, round robin.
fn next_ready(scope: &HandleScope<'_>, after: usize) -> Option<usize> {
    let entries = &scope.fibers().threads.entries;
    let n = entries.len();
    (1..=n)
        .map(|step| (after + step) % n)
        .find(|&i| matches!(entries[i].state, ThreadState::Ready { .. }))
}

/// A waiting thread other than `me`, to keep the scheduler going while no
/// thread can run.
fn next_waiting(scope: &HandleScope<'_>, me: usize) -> Option<usize> {
    let entries = &scope.fibers().threads.entries;
    (0..entries.len()).find(|&i| i != me && matches!(entries[i].state, ThreadState::Blocked { .. }))
}

/// When to stop waiting for nothing to be ready: the earliest deadline, or
/// whether anything waits on a descriptor or outside the lock; or that every
/// thread waits for ever, and whether all of them count toward a deadlock.
enum Idle {
    Until(Option<Instant>),
    Forever { deadlock: bool },
}

fn idle(scope: &HandleScope<'_>) -> Idle {
    let mut earliest: Option<Instant> = None;
    let mut waits_on_fds = false;
    let mut deadlock = true;
    for entry in &scope.fibers().threads.entries {
        match &entry.state {
            ThreadState::Blocked {
                deadline,
                fds: entry_fds,
                deadlockable,
                ..
            } => {
                if let Some(d) = deadline {
                    earliest = Some(earliest.map_or(*d, |e| e.min(*d)));
                }
                waits_on_fds |= !entry_fds.is_empty();
                if deadline.is_none() && entry_fds.is_empty() && !deadlockable {
                    deadlock = false;
                }
            }
            // It is coming back.
            ThreadState::Outside { .. } => waits_on_fds = true,
            _ => {}
        }
    }
    if earliest.is_some() || waits_on_fds {
        Idle::Until(earliest)
    } else {
        Idle::Forever { deadlock }
    }
}

/// The next thread to run, looking after `after` first; when none can, wait
/// for the first that can.
fn pick(scope: &mut HandleScope<'_>, after: usize) -> usize {
    loop {
        wake_ready(scope, Some(Duration::ZERO));
        if let Some(next) = next_ready(scope, after) {
            return next;
        }
        match idle(scope) {
            Idle::Until(deadline) => {
                let timeout = deadline.map(|d| d.saturating_duration_since(Instant::now()));
                wake_ready(scope, timeout);
            }
            Idle::Forever { deadlock: true } => {
                // Every thread waits on another: the main thread raises.
                let main = scope.fibers().threads.main;
                wakeup(scope, main, reason("deadlock"));
            }
            Idle::Forever { deadlock: false } => {
                // A `sleep` with nothing to end it but a signal.
                wake_ready(scope, None);
            }
        }
    }
}

/// Hand the heap to `to`, and wait to be handed it back able to run. It can
/// come back first with `me` still waiting — while every thread that could
/// run is outside the lock — for `me` to keep the scheduler going.
fn hand_to(scope: &mut HandleScope<'_>, me: usize, to: usize) {
    let lock = lock(scope);
    let mut to = to;
    loop {
        if to != me {
            // The heap goes to `to`'s OS thread, which uses it until it
            // hands it back; this one touches nothing of it meanwhile. The
            // pointer handed over is this scope's own, so the compiler sees
            // the heap escape into the lock before the wait and come back
            // out of it after.
            lock.give(scope.heap_ptr(), Some(to));
            lock.wait(me);
        }
        if matches!(
            scope.fibers().threads.entries[me].state,
            ThreadState::Ready { .. }
        ) {
            return;
        }
        to = pick(scope, me);
    }
}

/// Hand the heap on from a thread that stops running without waiting for it
/// back — it is over, or it is going outside the lock: to `first`, or to a
/// thread able to run, or to a waiting one to keep the scheduler going, or
/// to nobody, for the first thread back from outside to take.
fn release(scope: &mut HandleScope<'_>, me: usize, first: Option<usize>) {
    let lock = lock(scope);
    loop {
        wake_ready(scope, Some(Duration::ZERO));
        let to = first
            .or_else(|| next_ready(scope, me))
            .or_else(|| next_waiting(scope, me));
        let heap = scope.heap_ptr();
        let mut held = lock.held();
        if to.is_none() && !held.back.is_empty() {
            // One came back meanwhile: it is next.
            continue;
        }
        held.heap = HeapPtr(heap);
        held.holder = to;
        if let Some(to) = to {
            held.turns[to].notify_one();
        }
        return;
    }
}

/// Park the running thread, `from`, in the state `state` makes of its
/// vectors: they, its root stack, its fibers and its specials go here.
fn park(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
    from: usize,
    ids: u64,
    state: impl FnOnce(Context) -> ThreadState,
) {
    let context = Context {
        stack: std::mem::take(stack),
        frames: std::mem::take(frames),
    };
    let specials = [
        scope.last_match(),
        scope.errinfo(),
        scope.global(special("$_")).unwrap_or(Value::NIL),
        scope.global(special("$?")).unwrap_or(Value::NIL),
    ];
    let roots = scope.swap_roots(Vec::new());
    let fibers = scope.fibers_mut();
    fibers.frame_ids = ids;
    let world = FiberWorld {
        current: fibers.current.take(),
        root: fibers.root.take(),
        root_parked: fibers.root_parked.take(),
    };
    let entry = &mut fibers.threads.entries[from];
    entry.fibers = world;
    entry.specials = specials;
    entry.roots = roots;
    entry.state = state(context);
}

/// Take the running thread off the processor as `leave` says, put the next
/// one on, and wait for this one's next turn. A refusal another thread left
/// for the main thread is its answer there.
pub(super) fn switch(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
    leave: Leave,
    ids: &mut u64,
) -> Result<Option<Unwind>, Error> {
    ensure_main(scope);
    let from = scope.fibers().threads.current;
    park(scope, stack, frames, from, *ids, |context| match leave {
        Leave::Ready(push) => ThreadState::Ready { context, push },
        Leave::Block {
            deadline,
            fds,
            deadlockable,
        } => ThreadState::Blocked {
            context,
            deadline,
            fds,
            deadlockable,
        },
    });
    // A thread with an interrupt waiting does not wait.
    if !scope.fibers().threads.entries[from].pending.is_empty() {
        wakeup(scope, from, reason("interrupted"));
    }
    let to = pick(scope, from);
    hand_to(scope, from, to);
    *ids = scope.fibers().frame_ids;
    enter(scope, stack, frames, from, true);
    if from == scope.fibers().threads.main
        && let Some(refusal) = scope.fibers_mut().threads.refusal.take()
    {
        return Err(refusal);
    }
    Ok(None)
}

/// Put the thread `to`, able to run, back in the loop's vectors, and its
/// root stack, fibers and specials back on the heap. An interrupt waiting for
/// it goes on top, as a call what it stopped in returns to — unless it
/// stopped part-way through a primitive (`interruptible` false), which meets
/// the interrupt at its next turn check.
fn enter(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
    to: usize,
    interruptible: bool,
) {
    let fibers = scope.fibers_mut();
    let entry = &mut fibers.threads.entries[to];
    let ThreadState::Ready { context, push } =
        std::mem::replace(&mut entry.state, ThreadState::Running)
    else {
        unreachable!("only a ready thread is entered")
    };
    let world = std::mem::take(&mut entry.fibers);
    let pending = if interruptible {
        entry.pending.pop_front()
    } else {
        None
    };
    let specials = std::mem::replace(&mut entry.specials, [Value::NIL; 4]);
    let roots = std::mem::take(&mut entry.roots);
    fibers.current = world.current;
    fibers.root = world.root;
    fibers.root_parked = world.root_parked;
    fibers.threads.current = to;
    fibers.threads.slice_started = Some(Instant::now());
    fibers.threads.owed = false;
    fibers.threads.woken = push.is_some_and(|value| !value.is_nil());
    let _ = scope.swap_roots(roots);
    *stack = context.stack;
    *frames = context.frames;
    let [last_match, errinfo, last_line, status] = specials;
    scope.set_last_match(last_match);
    scope.set_errinfo(errinfo);
    scope.set_global(special("$_"), last_line);
    scope.set_global(special("$?"), status);
    if let Some(value) = push {
        stack.push(value);
    }
    // The interrupt runs on top of whatever the thread was doing, as a call
    // that call returns to: what it stopped in has its answer below it.
    if let Some(mut frame) = pending {
        frame.floor = stack.len();
        frame.base = stack.len();
        frame.errinfo_on_entry = scope.errinfo();
        frames.push(frame);
    }
    adopt_roots(scope, stack, frames);
}

/// The loop's turn check: when the running thread's slice is used up and
/// another can run, it goes to the back of the line.
pub(super) fn preempt(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
    ids: &mut u64,
) -> Result<Option<Unwind>, Error> {
    let threads = &scope.fibers().threads;
    if threads.current == threads.main && threads.refusal.is_some() {
        // Left by a thread that ended while this one was outside the lock.
        return Err(scope
            .fibers_mut()
            .threads
            .refusal
            .take()
            .expect("checked above"));
    }
    if threads.entries.len() < 2 || !threads.others() {
        if threads.owed {
            // The thread it was owed to is gone.
            scope.fibers_mut().threads.owed = false;
        }
        return Ok(None);
    }
    // Not part-way through the core library: its methods are Ruby here where
    // CRuby's are C, and run whole under the lock there, so they do here —
    // `Array#reject!` is not interleaved with another thread's `<<`. A block
    // the core library calls is the program's again, and can be preempted.
    let internal = in_core_library(frames);
    if internal && threads.owed {
        // Still in the core library: the turn is given once it returns.
        return Ok(None);
    }
    if !threads.owed
        && threads
            .slice_started
            .is_some_and(|started| started.elapsed() < SLICE)
    {
        return Ok(None);
    }
    wake_ready(scope, Some(Duration::ZERO));
    let current = scope.fibers().threads.current;
    // An interrupt that waited behind another is met here, at the latest:
    // leaving and coming straight back is what meets it.
    let queued = !scope.fibers().threads.entries[current].pending.is_empty();
    if !queued && next_ready(scope, current).is_none_or(|next| next == current) {
        let threads = &mut scope.fibers_mut().threads;
        threads.slice_started = Some(Instant::now());
        threads.owed = false;
        return Ok(None);
    }
    // Owed rather than given up: a loop that is mostly core library could be
    // in it at every turn check a slice apart, its period a multiple of the
    // check's, and keep the turn for as long as it runs (#728). The loop
    // checks again at the call or return that leaves the core library.
    if internal {
        scope.fibers_mut().threads.owed = true;
        return Ok(None);
    }
    switch(scope, stack, frames, Leave::Ready(None), ids)
}

/// A deadline `seconds` from now; None for none, and for one too far off to
/// be a deadline.
pub(super) fn deadline(seconds: f64) -> Option<Instant> {
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Instant::now().checked_add(Duration::from_secs_f64(seconds.min(1e9)))
}

/// Make `call`, which blocks in the kernel, outside the lock: the running
/// thread is parked as if it waited, another runs meanwhile, and the thread
/// gets back in line when `call` returns. `call` must not touch the heap —
/// it is not this OS thread's while `call` runs — so a primitive reads what
/// it needs out of its arguments first and makes objects of the answer after.
pub(super) fn outside<R>(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
    ids: &mut u64,
    call: impl FnOnce() -> R,
) -> R {
    if alone(scope) {
        return call();
    }
    let me = scope.fibers().threads.current;
    away(scope, stack, frames, ids, me, false, call)
}

/// What a call made [`outside_interruptibly`] can look at while it runs.
pub(super) struct Unblock {
    /// The lock its thread's unblocking function is kept in; none for the
    /// main thread alone.
    lock: Option<Arc<Lock>>,
    me: usize,
    /// Whether a signal waiting asks it to stop: the main thread's, outside
    /// a trap handler.
    signals: bool,
}

impl Unblock {
    /// Whether something has asked the call to stop — a kill or raise for its
    /// thread, a signal for the main thread, a `close` of its descriptor —
    /// for a call that loops, as a write of what is left does, to come back
    /// with what it did rather than go on.
    pub(super) fn asked(&self) -> bool {
        (self.signals && crate::shared::signals::any())
            || self.lock.as_ref().is_some_and(|lock| {
                lock.held()
                    .blocking
                    .get(self.me)
                    .and_then(Option::as_ref)
                    .is_some_and(|call| call.asked)
            })
    }
}

/// [`outside`], with an unblocking function, as CRuby's blocking region has
/// one: a kill or raise for the thread, a signal for the main thread, a
/// `close` of `fd` in another thread and the program's end each send its OS
/// thread [`UNBLOCK`], which cuts `call` short with EINTR, and `call` can
/// look whether it was asked to stop ([`Unblock::asked`]). None, and `call`
/// not made, when something asked before it began: the primitive answers
/// EINTR, and the Ruby side meets what asked (`Thread.__check_interrupts__`)
/// and makes the call again, as CRuby's does after `RUBY_VM_CHECK_INTS`.
///
/// The main thread alone makes `call` holding the lock, since no other
/// thread needs it, and nothing but a signal can interrupt it; a call that
/// blocks in the kernel there is `shared::signals::restartless`'s to cut
/// short. This is what an FFI call declared `blocking: true` (#105) is to be
/// made through.
pub(super) fn outside_interruptibly<R>(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
    ids: &mut u64,
    fd: Option<libc::c_int>,
    call: impl FnOnce(&Unblock) -> R,
) -> Option<R> {
    let threads = &scope.fibers().threads;
    let me = threads.current;
    let signals = me == threads.main && !threads.trapping;
    let waiting = signals && crate::shared::signals::any();
    if alone(scope) {
        let unblock = Unblock {
            lock: None,
            me,
            signals,
        };
        return (!waiting).then(|| call(&unblock));
    }
    // A kill or raise left for it, a signal for the main thread, or a refusal
    // another thread left for it, is met before it goes.
    if waiting
        || !threads.entries[me].pending.is_empty()
        || (me == threads.main && threads.refusal.is_some())
    {
        return None;
    }
    let lock = lock(scope);
    // Recorded before the heap goes on, so whoever has it next and asks this
    // thread to stop finds the OS thread to send it to.
    lock.held().blocking[me] = Some(Blocking {
        // SAFETY: `pthread_self` has no preconditions.
        os: OsThread(unsafe { libc::pthread_self() }),
        fd,
        asked: false,
        sent: Instant::now(),
    });
    let unblock = Unblock {
        lock: Some(Arc::clone(&lock)),
        me,
        signals,
    };
    away(scope, stack, frames, ids, me, true, || {
        let answer = (!unblock.asked()).then(|| call(&unblock));
        lock.held().blocking[me] = None;
        lock.left.notify_all();
        answer
    })
}

/// Park the running thread, `me`, outside the lock, hand the heap on, make
/// `call`, and get back in line and in again.
fn away<R>(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
    ids: &mut u64,
    me: usize,
    unblockable: bool,
    call: impl FnOnce() -> R,
) -> R {
    let lock = lock(scope);
    park(scope, stack, frames, me, *ids, |context| {
        ThreadState::Outside {
            context,
            unblockable,
        }
    });
    release(scope, me, None);
    let answer = call();
    lock.come_back(me);
    let entry = &mut scope.fibers_mut().threads.entries[me];
    // Put back in line by whoever had the heap, or the heap taken from nobody.
    let (ThreadState::Ready { context, .. } | ThreadState::Outside { context, .. }) =
        std::mem::replace(&mut entry.state, ThreadState::Dead)
    else {
        unreachable!("a thread outside the lock waits for nothing else")
    };
    entry.state = ThreadState::Ready {
        context,
        push: None,
    };
    *ids = scope.fibers().frame_ids;
    enter(scope, stack, frames, me, false);
    answer
}

fn special(name: &str) -> super::SymbolId {
    crate::shared::symbols::intern(name)
}

/// A thread's saved frames, lent out for a backtrace: none for the running
/// thread, whose frames are the loop's, or a finished one.
pub(super) fn take_frames(scope: &mut HandleScope<'_>, index: usize) -> Option<Vec<Call>> {
    let entry = scope.fibers_mut().threads.entries.get_mut(index)?;
    match &mut entry.state {
        ThreadState::Ready { context, .. }
        | ThreadState::Blocked { context, .. }
        | ThreadState::Outside { context, .. } => Some(std::mem::take(&mut context.frames)),
        ThreadState::Running | ThreadState::Dead => None,
    }
}

/// Give back what [`take_frames`] lent.
pub(super) fn give_frames(scope: &mut HandleScope<'_>, index: usize, frames: Vec<Call>) {
    if let Some(entry) = scope.fibers_mut().threads.entries.get_mut(index)
        && let ThreadState::Ready { context, .. }
        | ThreadState::Blocked { context, .. }
        | ThreadState::Outside { context, .. } = &mut entry.state
    {
        context.frames = frames;
    }
}

/// After `fork`, in the child: the thread that forked is the only one there
/// is, as in CRuby, and every other is dead. Their OS threads stayed in the
/// parent, so the lock starts again here, held by the one that is left.
pub(super) fn forked(scope: &mut HandleScope<'_>) {
    let heap = scope.heap_ptr();
    let threads = &mut scope.fibers_mut().threads;
    let current = threads.current;
    threads.main = current;
    for (index, entry) in threads.entries.iter_mut().enumerate() {
        if index != current {
            entry.state = ThreadState::Dead;
            entry.pending.clear();
        }
    }
    if threads.lock.is_some() {
        threads.lock = Some(Arc::new(Lock::new(heap, current, threads.entries.len())));
        threads.entries[current].native_id = native_id();
    }
}
