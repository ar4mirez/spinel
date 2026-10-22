//! Threads (#45): the VM half, under `interp`.
//!
//! A thread is what a fiber is — the interpreter's two vectors, its value
//! stack and its frames — plus the fiber bookkeeping of its own: which fiber
//! of the thread runs, its root fiber's object, and the root's vectors while a
//! fiber was transferred to. Switching threads is swapping all of that, the
//! way `resume` swaps a fiber's. Every heap has one table of them, beside its
//! fibers, and the collector traces both.
//!
//! What a thread *does* — `join`, `Mutex`, `Queue`, `Thread#raise`'s
//! exception, `report_on_exception` — is `core/thread.rb`. This file is the
//! scheduler under it: who runs, who waits for what, and the switches. A
//! thread that waits says what for, a deadline or a descriptor becoming
//! ready, or nothing (another thread's `wakeup`); when no thread can run the
//! scheduler waits in `poll(2)` for the first of them; and when every thread
//! waits on another for ever, the main thread is woken to raise the deadlock.
//! The interpreter loop preempts the running thread every time slice, so a
//! thread that never waits does not starve the others.
//!
//! ponytail: the threads of a Ractor take turns on its one OS thread — the
//! lock `docs/engine.md` describes, held by one OS thread for all of them — so
//! a blocking call no thread scheduled around (a `getaddrinfo`, an FFI call)
//! stops them all. Waits that go through `IO.select`, `IO#wait`, `sleep`,
//! `Queue`, `Mutex` and `join` all overlap. One OS thread per Ruby thread,
//! handing the heap over under the lock, is the upgrade; the table and the
//! switch here are what it would keep.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::{
    Call, Context, Error, HandleScope, Pending, Stack, Target, Unwind, Value, adopt_roots,
    frame_roots, push_proc_frame,
};

/// How long a thread runs before the loop lets another have a turn.
const SLICE: Duration = Duration::from_millis(10);

/// Instructions between the loop's looks at the clock.
pub(super) const SLICE_INSNS: u64 = 4096;

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
    /// Finished.
    Dead,
}

/// What another thread asked of one, met when it next runs.
pub(super) enum Interrupt {
    /// A kill: the throw tagged with the thread.
    Unwind(Unwind),
    /// `Thread#raise`: a frame, built in the thread that raised, that raises
    /// the exception here — so it is asked for `#exception` again in this
    /// thread and takes this thread's backtrace, where it was interrupted, as
    /// CRuby's does (#328).
    Call(Box<Call>),
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
    /// raise to be met, as CRuby's queue of interrupts does.
    pub(super) pending: VecDeque<Interrupt>,
    /// Its `$~`, `$!`, `$_` and `$?` while another thread runs: one slot
    /// each per heap in the VM, one each per thread in Ruby.
    pub(super) specials: [Value; 4],
}

/// Every thread a heap has made. Empty until the first one is: the main
/// thread is entry 0, made then.
#[derive(Default)]
pub(crate) struct ThreadTable {
    pub(super) entries: Vec<ThreadEntry>,
    pub(super) current: usize,
    pub(super) slice_started: Option<Instant>,
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
                ThreadState::Blocked { context: c, .. } => context_roots(c, f),
                ThreadState::Running | ThreadState::Dead => {}
            }
            if let Some(root) = entry.fibers.root {
                f(root);
            }
            if let Some(parked) = &entry.fibers.root_parked {
                context_roots(parked, f);
            }
            for interrupt in &entry.pending {
                match interrupt {
                    Interrupt::Unwind(unwind) => super::unwind_roots(*unwind, f),
                    Interrupt::Call(frame) => frame_roots(frame, f),
                }
            }
            entry.specials.iter().copied().for_each(&mut *f);
        }
    }

    /// Whether a thread other than the running one is alive.
    pub(super) fn others(&self) -> bool {
        self.entries
            .iter()
            .enumerate()
            .any(|(i, e)| i != self.current && !matches!(e.state, ThreadState::Dead))
    }

    /// The end of an evaluation: every thread but the main one is gone, as
    /// CRuby kills them when the main thread ends.
    pub(super) fn abandon(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if index != 0 {
                entry.state = ThreadState::Dead;
                entry.pending.clear();
            }
        }
        self.entries[0].state = ThreadState::Running;
        self.current = 0;
    }
}

fn context_roots(context: &Context, f: &mut impl FnMut(Value)) {
    context.stack.iter().copied().for_each(&mut *f);
    for frame in &context.frames {
        frame_roots(frame, f);
    }
}

fn ensure_main(scope: &mut HandleScope<'_>) {
    let threads = &mut scope.fibers_mut().threads;
    if threads.entries.is_empty() {
        threads.entries.push(ThreadEntry {
            object: Value::NIL,
            state: ThreadState::Running,
            fibers: FiberWorld::default(),
            pending: VecDeque::new(),
            specials: [Value::NIL; 4],
        });
        threads.current = 0;
        threads.slice_started = Some(Instant::now());
    }
}

/// A new thread running `block`, its first frame pushed now so any later
/// switch can enter it. It runs when the scheduler gets to it.
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
    let threads = &mut scope.fibers_mut().threads;
    threads.entries.push(ThreadEntry {
        object,
        state: ThreadState::Ready {
            context: Context { stack, frames },
            push: None,
        },
        fibers: FiberWorld::default(),
        pending: VecDeque::new(),
        specials: [Value::NIL; 4],
    });
    Ok(threads.entries.len() - 1)
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

/// The frame `Thread#raise` leaves for another thread: a call of `block`,
/// with no arguments and its value dropped, pushed when that thread next runs.
pub(super) fn raise_frame(
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

/// `:run`, `:sleep` or `:dead` for a thread.
pub(super) fn status(scope: &HandleScope<'_>, index: usize) -> &'static str {
    match scope.fibers().threads.entries.get(index).map(|e| &e.state) {
        None => "run",
        Some(ThreadState::Running | ThreadState::Ready { .. }) => "run",
        Some(ThreadState::Blocked { .. }) => "sleep",
        Some(ThreadState::Dead) => "dead",
    }
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

/// Leave `interrupt` for a thread to meet when it next runs, and wake it.
pub(super) fn interrupt(scope: &mut HandleScope<'_>, index: usize, interrupt: Interrupt) {
    let interrupted = Value::symbol(crate::shared::symbols::intern("interrupted"));
    if let Some(entry) = scope.fibers_mut().threads.entries.get_mut(index) {
        if matches!(entry.state, ThreadState::Dead) {
            return;
        }
        entry.pending.push_back(interrupt);
    }
    wakeup(scope, index, interrupted);
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
    /// Its block has finished.
    Exit,
}

fn reason(name: &str) -> Value {
    Value::symbol(crate::shared::symbols::intern(name))
}

/// Wake every waiting thread whose deadline has passed or whose descriptor
/// is ready, waiting up to `timeout` for one to be; how many woke.
fn wake_ready(scope: &mut HandleScope<'_>, timeout: Option<Duration>) -> usize {
    let now = Instant::now();
    let mut woken = 0;
    let mut polled: Vec<(usize, libc::pollfd)> = Vec::new();
    let mut expired = Vec::new();
    for (index, entry) in scope.fibers().threads.entries.iter().enumerate() {
        if let ThreadState::Blocked { deadline, fds, .. } = &entry.state {
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
    }
    for index in expired {
        if wakeup(scope, index, reason("timeout")) {
            woken += 1;
        }
    }
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
    // SAFETY: `fds` is `fds.len()` `pollfd`s, read and written by `poll`.
    let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
    if ready > 0 {
        for ((index, _), pollfd) in polled.iter().zip(&fds) {
            if pollfd.revents != 0 && wakeup(scope, *index, reason("ready")) {
                woken += 1;
            }
        }
    }
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

/// The next thread able to run, looking after `after` first, round robin.
fn next_ready(scope: &HandleScope<'_>, after: usize) -> Option<usize> {
    let entries = &scope.fibers().threads.entries;
    let n = entries.len();
    (1..=n)
        .map(|step| (after + step) % n)
        .find(|&i| matches!(entries[i].state, ThreadState::Ready { .. }))
}

/// When to stop waiting for nothing to be ready: the earliest deadline, or
/// whether anything waits on a descriptor; or that every thread waits for
/// ever, and whether all of them count toward a deadlock.
enum Idle {
    Until(Option<Instant>),
    Forever { deadlock: bool },
}

fn idle(scope: &HandleScope<'_>) -> Idle {
    let mut earliest: Option<Instant> = None;
    let mut waits_on_fds = false;
    let mut deadlock = true;
    for entry in &scope.fibers().threads.entries {
        if let ThreadState::Blocked {
            deadline,
            fds: entry_fds,
            deadlockable,
            ..
        } = &entry.state
        {
            if let Some(d) = deadline {
                earliest = Some(earliest.map_or(*d, |e| e.min(*d)));
            }
            waits_on_fds |= !entry_fds.is_empty();
            if deadline.is_none() && entry_fds.is_empty() && !deadlockable {
                deadlock = false;
            }
        }
    }
    if earliest.is_some() || waits_on_fds {
        Idle::Until(earliest)
    } else {
        Idle::Forever { deadlock }
    }
}

/// Take the running thread off the processor as `leave` says and put the
/// next one on; the answer is an unwind to start the new one with, when it
/// has an interrupt waiting.
pub(super) fn switch(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
    leave: Leave,
) -> Option<Unwind> {
    ensure_main(scope);
    let from = scope.fibers().threads.current;
    // Park the running thread, and its fibers.
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
    let fibers = scope.fibers_mut();
    let world = FiberWorld {
        current: fibers.current.take(),
        root: fibers.root.take(),
        root_parked: fibers.root_parked.take(),
    };
    let entry = &mut fibers.threads.entries[from];
    entry.fibers = world;
    entry.specials = specials;
    entry.state = match leave {
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
        Leave::Exit => ThreadState::Dead,
    };
    // A thread with an interrupt waiting does not wait.
    if !entry.pending.is_empty() {
        wakeup(scope, from, reason("interrupted"));
    }
    // The next to run: one able to now, or the first to wake.
    let to = loop {
        wake_ready(scope, Some(Duration::ZERO));
        if let Some(next) = next_ready(scope, from) {
            break next;
        }
        match idle(scope) {
            Idle::Until(deadline) => {
                let timeout = deadline.map(|d| d.saturating_duration_since(Instant::now()));
                wake_ready(scope, timeout);
            }
            Idle::Forever { deadlock: true } => {
                // Every thread waits on another: the main thread raises.
                wakeup(scope, 0, reason("deadlock"));
            }
            Idle::Forever { deadlock: false } => {
                // A `sleep` with nothing to end it but a signal.
                wake_ready(scope, None);
            }
        }
    };
    enter(scope, stack, frames, to)
}

fn enter(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
    to: usize,
) -> Option<Unwind> {
    let fibers = scope.fibers_mut();
    let entry = &mut fibers.threads.entries[to];
    let ThreadState::Ready { context, push } =
        std::mem::replace(&mut entry.state, ThreadState::Running)
    else {
        unreachable!("only a ready thread is entered")
    };
    let world = std::mem::take(&mut entry.fibers);
    let pending = entry.pending.pop_front();
    let specials = std::mem::replace(&mut entry.specials, [Value::NIL; 4]);
    fibers.current = world.current;
    fibers.root = world.root;
    fibers.root_parked = world.root_parked;
    fibers.threads.current = to;
    fibers.threads.slice_started = Some(Instant::now());
    *stack = context.stack;
    *frames = context.frames;
    let [last_match, errinfo, last_line, status] = specials;
    scope.set_last_match(last_match);
    scope.set_errinfo(errinfo);
    scope.set_global(special("$_"), last_line);
    scope.set_global(special("$?"), status);
    match pending {
        Some(Interrupt::Unwind(unwind)) => {
            adopt_roots(scope, stack, frames);
            Some(unwind)
        }
        // The raise runs on top of whatever the thread was doing, as a call
        // that call returns to: what it stopped in has its answer below it.
        Some(Interrupt::Call(mut frame)) => {
            if let Some(value) = push {
                stack.push(value);
            }
            frame.base = stack.len();
            frame.errinfo_on_entry = scope.errinfo();
            frames.push(*frame);
            adopt_roots(scope, stack, frames);
            None
        }
        None => {
            adopt_roots(scope, stack, frames);
            if let Some(value) = push {
                stack.push(value);
            }
            None
        }
    }
}

/// The loop's turn check: when the running thread's slice is used up and
/// another can run, it goes to the back of the line.
pub(super) fn preempt(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
) -> Result<Option<Unwind>, Error> {
    let threads = &scope.fibers().threads;
    if threads.entries.len() < 2 || !threads.others() {
        return Ok(None);
    }
    // Not part-way through the core library: its methods are Ruby here where
    // CRuby's are C, and run whole under the lock there, so they do here —
    // `Array#reject!` is not interleaved with another thread's `<<`. A block
    // the core library calls is the program's again, and can be preempted.
    if frames
        .last()
        .and_then(|frame| frame.iseq.path.as_deref())
        .is_some_and(|path| path.starts_with("<internal:"))
    {
        return Ok(None);
    }
    if threads
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
        scope.fibers_mut().threads.slice_started = Some(Instant::now());
        return Ok(None);
    }
    Ok(switch(scope, stack, frames, Leave::Ready(None)))
}

/// A deadline `seconds` from now; None for none, and for one too far off to
/// be a deadline.
pub(super) fn deadline(seconds: f64) -> Option<Instant> {
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Instant::now().checked_add(Duration::from_secs_f64(seconds.min(1e9)))
}

/// The running thread is over with a refusal nothing in it can catch: it
/// ends, and the main thread is put back on the processor, wherever it was,
/// for the refusal to unwind there.
pub(super) fn abort_to_main(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
) {
    let from = scope.fibers().threads.current;
    std::mem::take(stack);
    std::mem::take(frames);
    let fibers = scope.fibers_mut();
    fibers.current = None;
    fibers.root = None;
    fibers.root_parked = None;
    fibers.threads.entries[from].state = ThreadState::Dead;
    let main = &mut fibers.threads.entries[0];
    let context = match std::mem::replace(&mut main.state, ThreadState::Dead) {
        ThreadState::Ready { context, .. } | ThreadState::Blocked { context, .. } => context,
        other => {
            main.state = other;
            return;
        }
    };
    main.state = ThreadState::Ready {
        context,
        push: None,
    };
    let _ = enter(scope, stack, frames, 0);
}

/// A thread's saved frames, lent out for a backtrace: none for the running
/// thread, whose frames are the loop's, or a finished one.
pub(super) fn take_frames(scope: &mut HandleScope<'_>, index: usize) -> Option<Vec<Call>> {
    let entry = scope.fibers_mut().threads.entries.get_mut(index)?;
    match &mut entry.state {
        ThreadState::Ready { context, .. } | ThreadState::Blocked { context, .. } => {
            Some(std::mem::take(&mut context.frames))
        }
        ThreadState::Running | ThreadState::Dead => None,
    }
}

/// Give back what [`take_frames`] lent.
pub(super) fn give_frames(scope: &mut HandleScope<'_>, index: usize, frames: Vec<Call>) {
    if let Some(entry) = scope.fibers_mut().threads.entries.get_mut(index)
        && let ThreadState::Ready { context, .. } | ThreadState::Blocked { context, .. } =
            &mut entry.state
    {
        context.frames = frames;
    }
}

fn special(name: &str) -> super::SymbolId {
    crate::shared::symbols::intern(name)
}

/// After `fork`, in the child: the thread that forked is the only one there
/// is, as in CRuby, and every other is dead.
pub(super) fn forked(scope: &mut HandleScope<'_>) {
    let threads = &mut scope.fibers_mut().threads;
    let current = threads.current;
    for (index, entry) in threads.entries.iter_mut().enumerate() {
        if index != current {
            entry.state = ThreadState::Dead;
            entry.pending.clear();
        }
    }
}
