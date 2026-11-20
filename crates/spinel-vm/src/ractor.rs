//! Ractors (#117): the VM half, under `interp`, beneath `core/ractor.rb`.
//!
//! A Ractor is a heap of its own on an OS thread of its own. Its heap joins
//! the program's [`Vm`](crate::vm::Vm) (`Heap::new_ractor`), so it shares the
//! program's classes and methods and has an object of its own for each class
//! (#857). Nothing else is shared: a Ractor's heap holds no pointer into
//! another's, so what goes between two Ractors goes as a copy
//! ([`Msg`](crate::heap::Msg)), made by the sender and built again by the
//! receiver.
//!
//! What every Ractor of a program shares is here, in one [`Registry`] every
//! heap of the program holds through a [`Program`]: each Ractor's [`Core`] — its id, name, state, the
//! value it ended with — and each port's queue. Ruby objects for them are
//! per heap: a `Ractor` or a `Ractor::Port` is an object holding the id, and
//! `core/ractor.rb` compares them by it.
//!
//! # Waiting
//!
//! A Ractor that waits — for a message, for another's end — waits as a
//! thread waits for a descriptor, so the other threads of its heap go on
//! meanwhile (`threads.rs`). Each Ractor has a pipe and a generation count:
//! whatever another Ractor does that it may be waiting for (a message to one
//! of its ports, the end of a Ractor it waits on) bumps the count and writes
//! a byte. A wait (`__ractor_wait__`) is for the count to move past the one
//! read before the condition was looked at, so no byte is lost between the
//! two; the scheduler empties the pipe when it finds it readable.
//!
//! While a heap waits that way, in the scheduler's `poll(2)`, it lets go of
//! the class lock, so a Ractor that defines a method meanwhile does not wait
//! for it. So does a heap whose last running thread leaves it for a call made
//! outside the per-Ractor lock (`threads::outside`), a `flock` or a read of a
//! blocking pipe that may wait for ever (#119): its one thread lets go around
//! the call, and of several, the one that leaves the heap to nobody lets go as
//! it does, and the one that takes it back from nobody takes the hold back.
//!
//! # Running in parallel
//!
//! Each Ractor runs on its own OS thread, and they run at once (#119): a
//! running heap reads the shared tables taking nothing, and a safe point is
//! one more load while no heap writes or waits to (`vm::ClassLock`). What a
//! running Ractor does every call — a send whose inline cache missed, a
//! `respond_to?`, a block made into a `Proc` — reads the method cache and the
//! interned bodies first, and writes, parking every other Ractor until its
//! next safe point, only to fill them.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::{
    Call, Error, HandleScope, Heap, Pending, Stack, Unwind, Value, array_elements, bool_value,
    new_array, proc_parts, string_new, string_text, threads,
};
use crate::bytecode::{Insn, Iseq};
use crate::class::{Builtin, ClassId, CrefId};
use crate::heap::{ExportError, Msg, Options, Refusal, Stop};
use crate::method::{Definition, RactorOp};

/// What every heap of a program keeps for its Ractors, beside the [`Vm`]:
/// the registry, with whether a Ractor has been asked for yet, and where the
/// main heap is as it last let the class lock go or parked at a safe point —
/// what another Ractor reads the main one's constants from while it stays
/// parked (`HandleScope::pull`). Held outside the `Vm`, whose layout the
/// interpreter's loop is measured with.
///
/// [`Vm`]: crate::vm::Vm
pub(crate) struct Program {
    registry: Registry,
    asked: AtomicBool,
    main_heap: std::sync::atomic::AtomicPtr<Heap>,
}

impl Default for Program {
    fn default() -> Program {
        Program {
            registry: Registry::new(),
            asked: AtomicBool::new(false),
            main_heap: std::sync::atomic::AtomicPtr::default(),
        }
    }
}

impl Program {
    /// The program's Ractors.
    pub(crate) fn ractors(&self) -> &Registry {
        if !self.asked.load(Ordering::Acquire) {
            self.asked.store(true, Ordering::Release);
        }
        &self.registry
    }

    /// Whether the program has asked for a Ractor yet.
    pub(crate) fn has_ractors(&self) -> bool {
        self.asked.load(Ordering::Acquire)
    }

    pub(crate) fn set_main_heap(&self, heap: *mut Heap) {
        self.main_heap.store(heap, Ordering::Release);
    }

    pub(crate) fn main_heap(&self) -> *mut Heap {
        self.main_heap.load(Ordering::Acquire)
    }
}

/// The main Ractor's id. Ids count from it, as CRuby's do.
pub(crate) const MAIN: u32 = 1;

/// Every Ractor and port of one program. See the module docs.
pub(crate) struct Registry {
    inner: Mutex<Inner>,
    /// Whether the first `Ractor.new` has warned that the API is
    /// experimental: once per program, as CRuby warns once per process.
    warned: AtomicBool,
}

#[derive(Default)]
struct Inner {
    ractors: Vec<Arc<Core>>,
    ports: Vec<Arc<Port>>,
    /// The classes whose instances a copy refuses, and `Ractor::MovedObject`,
    /// as `core/ractor.rb` names them at boot.
    refusals: Vec<Refusal>,
    /// The classes `Ractor.shareable?` says no to, frozen or not, and
    /// `Ractor.make_shareable` refuses: `Proc`, but for one made shareable,
    /// and CRuby's `T_DATA` that may not be shared frozen (#118).
    unshareable: Vec<ClassId>,
    moved: Option<ClassId>,
    /// The values Ractors other than the main one put in the class tables,
    /// as copies: see [`Value::shared_ref`].
    boxes: Vec<Arc<Msg>>,
    /// The body a new Ractor's heap starts by running:
    /// `Ractor.__child_main__`, compiled once by the heap that made the first.
    child_main: Option<Arc<Iseq>>,
    /// Ractors that have not ended, the main one among them.
    alive: usize,
}

/// One Ractor, as every Ractor of the program sees it.
pub(crate) struct Core {
    pub(crate) id: u32,
    name: Option<String>,
    /// Where `Ractor.new` was called, as `#inspect` shows it.
    location: Option<String>,
    /// Its default port's id.
    port: u32,
    wake: Wake,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    status: Status,
    /// How it ended: what its block answered, or the exception it raised.
    outcome: Option<(bool, Arc<Msg>)>,
    /// Ractors waiting for it to end, to be woken when it does.
    waiters: Vec<u32>,
    /// Ports `#monitor` registered, sent `:exited` or `:aborted` at its end.
    monitors: Vec<u32>,
    /// What it starts with, taken by its own heap once it runs: its block's
    /// body and lexical scope, and the arguments `Ractor.new` was given.
    start: Option<(Value, CrefId, Msg)>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Status {
    #[default]
    Running,
    /// Waiting in a receive, a `#value` or a `#join`.
    Blocking,
    Terminated,
}

/// A port: a queue of messages only the Ractor that made it receives from.
pub(crate) struct Port {
    owner: u32,
    state: Mutex<PortState>,
}

#[derive(Default)]
struct PortState {
    queue: VecDeque<Arc<Msg>>,
    closed: bool,
}

/// A Ractor's wake pipe and generation count. See the module docs.
///
/// The pipe is made the first time the Ractor waits, not when it is made:
/// the main Ractor's core exists as soon as the program asks for
/// `Ractor.current`, which the standard library does, and two descriptors
/// it never uses would move the numbers the program's own files get.
struct Wake {
    fds: Mutex<Option<[libc::c_int; 2]>>,
    generation: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Wake {
    fn new() -> Wake {
        Wake {
            fds: Mutex::new(None),
            generation: AtomicU64::new(0),
        }
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// The pipe's read end, made now if it was not: what a waiter waits on.
    /// A waiter makes it before it reads the generation, so a `notify` after
    /// that read finds the pipe and writes to it.
    fn read_end(&self) -> libc::c_int {
        let mut fds = lock(&self.fds);
        if let Some([read, _]) = *fds {
            return read;
        }
        let mut made = [-1; 2];
        // SAFETY: `made` is the two descriptors `pipe` fills in; `fcntl` only
        // sets flags on them.
        unsafe {
            if libc::pipe(made.as_mut_ptr()) == 0 {
                for fd in made {
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                    libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
                }
            }
        }
        *fds = Some(made);
        made[0]
    }

    /// The pipe's read end, if a waiter made it.
    fn read_end_if_made(&self) -> Option<libc::c_int> {
        lock(&self.fds).map(|[read, _]| read)
    }

    /// Tell the Ractor something it may wait for happened.
    fn notify(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let Some([_, write]) = *lock(&self.fds) else {
            return;
        };
        // SAFETY: one byte from a live local to the pipe's write end. A full
        // pipe answers EAGAIN and is already waking the reader.
        while unsafe { libc::write(write, [1u8].as_ptr().cast(), 1) } < 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        {}
    }

    /// Empty the pipe.
    fn drain(&self) {
        let Some(read) = self.read_end_if_made() else {
            return;
        };
        let mut bytes = [0u8; 64];
        // SAFETY: reads into a live local of its own length; the read end is
        // nonblocking, so an empty pipe answers -1.
        while unsafe { libc::read(read, bytes.as_mut_ptr().cast(), bytes.len()) } > 0 {}
    }
}

impl Drop for Wake {
    fn drop(&mut self) {
        if let Some(fds) = *lock(&self.fds) {
            for fd in fds {
                if fd >= 0 {
                    // SAFETY: made by `read_end` and closed nowhere else.
                    unsafe { libc::close(fd) };
                }
            }
        }
    }
}

impl Registry {
    pub(crate) fn new() -> Registry {
        let registry = Registry {
            inner: Mutex::new(Inner::default()),
            warned: AtomicBool::new(false),
        };
        registry.add(None, None);
        registry
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        lock(&self.inner)
    }

    /// A new Ractor and its default port; its core.
    fn add(&self, name: Option<String>, location: Option<String>) -> Arc<Core> {
        let mut inner = self.inner();
        let id = u32::try_from(inner.ractors.len() + 1).expect("fewer than 2^32 Ractors");
        let port = add_port(&mut inner, id);
        let core = Arc::new(Core {
            id,
            name,
            location,
            port,
            wake: Wake::new(),
            state: Mutex::new(State::default()),
        });
        inner.ractors.push(Arc::clone(&core));
        inner.alive += 1;
        core
    }

    fn ractor(&self, id: u32) -> Option<Arc<Core>> {
        let inner = self.inner();
        inner.ractors.get((id as usize).wrapping_sub(1)).cloned()
    }

    fn port(&self, id: u32) -> Option<Arc<Port>> {
        let inner = self.inner();
        inner.ports.get((id as usize).wrapping_sub(1)).cloned()
    }

    /// Whether a Ractor but the main one has not ended.
    pub(crate) fn others_alive(&self) -> bool {
        self.inner().alive > 1
    }

    pub(crate) fn refusals(&self) -> Vec<Refusal> {
        self.inner().refusals.clone()
    }

    /// The refusals of a copy that only shareable objects may make: the
    /// classes that are never shareable.
    ///
    /// ponytail: a frozen `Method` is shareable here, as CRuby's is, but a
    /// copy of one sent is still refused (`allocator undefined for Method`),
    /// where CRuby passes the shareable one. Letting it through needs its
    /// `@__found__` checked to hold nothing per heap.
    pub(crate) fn shareable_refusals(&self) -> Vec<Refusal> {
        self.inner()
            .unshareable
            .iter()
            .map(|&class| Refusal {
                class,
                error: String::new(),
                message: String::new(),
            })
            .collect()
    }

    /// Keep `msg` for every heap to build its own of; what the tables hold
    /// in its place.
    pub(crate) fn add_shared_box(&self, msg: Msg) -> Value {
        let mut inner = self.inner();
        inner.boxes.push(Arc::new(msg));
        Value::shared_ref(u32::try_from(inner.boxes.len() - 1).expect("fewer than 2^32 copies"))
    }

    pub(crate) fn shared_box(&self, index: u32) -> Option<Arc<Msg>> {
        self.inner().boxes.get(index as usize).cloned()
    }

    /// Send `msg` to port `id`: false when it is closed.
    fn send(&self, id: u32, msg: Arc<Msg>) -> bool {
        let Some(port) = self.port(id) else {
            return false;
        };
        {
            let mut state = lock(&port.state);
            if state.closed {
                return false;
            }
            state.queue.push_back(msg);
        }
        if let Some(owner) = self.ractor(port.owner) {
            owner.wake.notify();
        }
        true
    }

    /// Ractor `id` has ended, as `ok` says, with `msg`: wake whoever waits.
    fn finish(&self, id: u32, ok: bool, msg: Msg) {
        let Some(core) = self.ractor(id) else {
            return;
        };
        let (waiters, monitors) = {
            let mut state = lock(&core.state);
            state.status = Status::Terminated;
            state.outcome = Some((ok, Arc::new(msg)));
            (
                std::mem::take(&mut state.waiters),
                std::mem::take(&mut state.monitors),
            )
        };
        if let Some(port) = self.port(core.port) {
            lock(&port.state).closed = true;
        }
        self.inner().alive -= 1;
        let word = if ok { "exited" } else { "aborted" };
        let word = Value::symbol(crate::shared::symbols::intern(word));
        for port in monitors {
            self.send(port, Arc::new(Msg::immediate(word)));
        }
        for waiter in waiters {
            if let Some(waiter) = self.ractor(waiter) {
                waiter.wake.notify();
            }
        }
    }
}

fn add_port(inner: &mut Inner, owner: u32) -> u32 {
    inner.ports.push(Arc::new(Port {
        owner,
        state: Mutex::new(PortState::default()),
    }));
    u32::try_from(inner.ports.len()).expect("fewer than 2^32 ports")
}

/// The registry of the program `scope`'s heap belongs to.
fn registry<'a>(scope: &'a HandleScope<'_>) -> &'a Registry {
    scope.program().ractors()
}

/// The id of the Ractor `scope`'s heap is.
fn current(scope: &HandleScope<'_>) -> u32 {
    scope.ractor_id()
}

/// The wake pipe's read end of this heap's Ractor, which the scheduler
/// empties when it finds it readable (`threads::wake_ready`); -1 while the
/// program has made no Ractor.
pub(super) fn wake_fd(scope: &HandleScope<'_>) -> libc::c_int {
    if !scope.program().has_ractors() {
        return -1;
    }
    registry(scope)
        .ractor(current(scope))
        .and_then(|core| core.wake.read_end_if_made())
        .unwrap_or(-1)
}

pub(super) fn drain(scope: &HandleScope<'_>) {
    if let Some(core) = registry(scope).ractor(current(scope)) {
        core.wake.drain();
    }
}

/// A local of a scope outside a block that the block reads or writes.
struct OuterUse {
    name: String,
    /// How many environments out from the block's own it is: 1 for the
    /// scope the block is written in.
    depth: usize,
    slot: u16,
    written: bool,
    /// Assigned more than once where it lives: see
    /// [`Iseq::outer_reassigned`].
    reassigned: bool,
}

/// What a block uses of the scopes outside it: CRuby's `outer_variables`.
struct Outer {
    /// In the order they are first met.
    uses: Vec<OuterUse>,
    yields: bool,
}

impl Outer {
    /// What `block` and the blocks written inside it read and write of the
    /// scopes outside it, and whether they `yield`.
    fn of(block: &Iseq) -> Outer {
        fn walk(iseq: &Iseq, base: usize, outer: &mut Outer) {
            // How many blocks deep inside `block` this body is.
            let level = iseq.outer.len().saturating_sub(base);
            for insn in &iseq.insns {
                let (slot, depth, written) = match *insn {
                    Insn::GetLocal(slot, depth) => (slot, depth, false),
                    Insn::SetLocal(slot, depth) => (slot, depth, true),
                    Insn::Yield(_) => {
                        outer.yields = true;
                        continue;
                    }
                    _ => continue,
                };
                let depth = depth as usize;
                if depth <= level {
                    continue;
                }
                let from_block = depth - level;
                if let Some(found) = outer
                    .uses
                    .iter_mut()
                    .find(|used| used.depth == from_block && used.slot == slot)
                {
                    found.written |= written;
                    continue;
                }
                let name = iseq
                    .outer
                    .get(depth - 1)
                    .and_then(|env| env.get(slot as usize))
                    .map_or_else(|| "?".to_string(), |name| name.to_string());
                let reassigned = iseq
                    .outer_reassigned
                    .get(depth - 1)
                    .is_some_and(|slots| slots.contains(&slot));
                outer.uses.push(OuterUse {
                    name,
                    depth: from_block,
                    slot,
                    written,
                    reassigned,
                });
            }
            for child in &iseq.children {
                // A block written inside sits one environment further in; a
                // method or a class body starts a scope of its own.
                if child.outer.len() > iseq.outer.len() {
                    walk(child, base, outer);
                }
            }
        }
        let mut outer = Outer {
            uses: Vec::new(),
            yields: false,
        };
        walk(block, block.outer.len(), &mut outer);
        outer
    }

    /// CRuby's `ArgumentError` for a block that may not use the variables
    /// outside it that `all` says, or `yield`: every one for `Ractor.new`
    /// ("isolate a Proc"), the ones it writes for a shareable `Proc` ("make
    /// a Proc shareable"). Measured on 4.0.9.
    fn refusal(&self, what: &str, all: bool) -> Option<String> {
        let mut names: Vec<&str> = Vec::new();
        for used in &self.uses {
            if (all || used.written) && !names.contains(&used.name.as_str()) {
                names.push(&used.name);
            }
        }
        match (names.is_empty(), self.yields) {
            (true, false) => None,
            (true, true) => Some(format!("can not {what} because it uses 'yield'.")),
            (false, yields) => Some(format!(
                "can not {what} because it accesses outer variables ({}){}.",
                names.join(", "),
                if yields { " and uses 'yield'" } else { "" }
            )),
        }
    }
}

/// Why a `Proc` may not be made shareable (#118), as CRuby 4.0.9 says it.
enum ProcRefusal {
    /// "Proc's self is not shareable: #{proc.inspect}".
    SelfUnshareable(Value),
    /// An `ArgumentError`, said in full: see [`Outer::refusal`].
    Argument(String),
    /// "cannot make a shareable Proc because the outer variable 'x' may be
    /// reassigned."
    Reassigned(String),
    /// "cannot make a shareable Proc because it can refer unshareable object
    /// #{value.inspect} from variable 'x'".
    Refers(Value, String),
    /// A `Proc` the core library made, `Method#to_proc`'s: "not supported
    /// yet".
    Unsupported,
}

/// Whether `value` is shareable: CRuby's `Ractor.shareable?`.
fn is_shareable(scope: &HandleScope<'_>, value: Value) -> bool {
    if value.is_immediate() {
        return true;
    }
    let refusals = registry(scope).shareable_refusals();
    scope
        .heap_ref()
        .export(
            value,
            &Options {
                refusals: &refusals,
                shareable_only: true,
                ..Options::default()
            },
            None,
        )
        .is_ok()
}

/// Make the `Proc` `block` shareable, as CRuby's
/// `rb_proc_ractor_make_shareable` does: its `self` (or `receiver`, which
/// replaces it, for `Ractor.shareable_proc`) must be shareable; it may not
/// write a variable outside itself or `yield`; each one it reads must be
/// assigned once where it lives and hold a shareable object. Then its
/// environment is a copy holding only those, so it shares nothing that
/// changes, and it is marked shareable and frozen. Checked in CRuby's
/// order: the variables of the innermost scope first, and of one scope,
/// the last read first.
///
/// ponytail: `binding` and `eval` in the `Proc` see the copied environment,
/// nil where it read nothing, where CRuby's refuse ("can not access
/// variable 'x' from isolated Proc"); and a `Symbol#to_proc` `Proc` is
/// refused as not supported, where CRuby's answers it as it is. Both want
/// the isolation marked on the frame, for `binding` and `eval` to consult.
fn make_proc_shareable(
    scope: &mut HandleScope<'_>,
    block: Value,
    receiver: Option<Value>,
) -> Result<(), ProcRefusal> {
    let handle = scope.root(block);
    let Some((iseq, env, own_self, ..)) = proc_parts(scope, block) else {
        return Err(ProcRefusal::Unsupported);
    };
    // The core library's, `Method#to_proc`'s: CRuby's is no block of Ruby.
    if iseq
        .path
        .as_deref()
        .is_some_and(|path| path.starts_with("<internal:"))
    {
        return Err(ProcRefusal::Unsupported);
    }
    if receiver.is_none() && !is_shareable(scope, own_self) {
        return Err(ProcRefusal::SelfUnshareable(scope.get(handle)));
    }
    let outer = Outer::of(&iseq);
    if let Some(message) = outer.refusal("make a Proc shareable", false) {
        return Err(ProcRefusal::Argument(message));
    }
    // The environments the body can reach, innermost first.
    let mut envs = Vec::new();
    let mut at = env;
    while at != Value::NIL && envs.len() < iseq.outer.len() {
        let cell = scope.root(at);
        envs.push(cell);
        at = scope.slot(cell, 0);
    }
    for (index, &cell) in envs.iter().enumerate() {
        for used in outer
            .uses
            .iter()
            .rev()
            .filter(|used| used.depth == index + 1)
        {
            if used.reassigned {
                return Err(ProcRefusal::Reassigned(used.name.clone()));
            }
            let value = scope.slot(cell, super::ENV_HEADER + used.slot as usize);
            if !is_shareable(scope, value) {
                return Err(ProcRefusal::Refers(value, used.name.clone()));
            }
        }
    }
    // The copy, outermost first, each holding only what the body reads.
    let mut parent = scope.root(Value::NIL);
    for (index, &cell) in envs.iter().enumerate().rev() {
        let slots = (scope.len(cell) as usize).saturating_sub(super::ENV_HEADER);
        let copy = super::env_alloc(scope, scope.get(parent), slots);
        let copy = scope.root(copy);
        for used in outer.uses.iter().filter(|used| used.depth == index + 1) {
            let at = super::ENV_HEADER + used.slot as usize;
            let value = scope.slot(cell, at);
            scope.set_slot(copy, at, value);
        }
        parent = copy;
    }
    if !envs.is_empty() {
        let copy = scope.get(parent);
        scope.set_slot(handle, super::PROC_ENV, copy);
    }
    if let Some(receiver) = receiver {
        scope.set_slot(handle, super::PROC_SELF, receiver);
    }
    scope.set_slot(handle, super::PROC_BLOCK, Value::NIL);
    scope.set_shareable(handle);
    Ok(())
}

/// A refusal as `core/ractor.rb` raises it: `[kind, object, text]`.
fn refusal_value(scope: &mut HandleScope<'_>, refusal: ProcRefusal) -> Value {
    let symbol = |name: &str| Value::symbol(crate::shared::symbols::intern(name));
    let (kind, object, text) = match refusal {
        ProcRefusal::SelfUnshareable(block) => ("self", block, None),
        ProcRefusal::Argument(message) => ("argument", Value::NIL, Some(message)),
        ProcRefusal::Reassigned(name) => ("reassigned", Value::NIL, Some(name)),
        ProcRefusal::Refers(value, name) => ("refers", value, Some(name)),
        ProcRefusal::Unsupported => ("unsupported", Value::NIL, None),
    };
    let object = scope.root(object);
    let text = match text {
        Some(text) => string_new(scope, &text),
        None => Value::NIL,
    };
    let text = scope.root(text);
    let (object, text) = (scope.get(object), scope.get(text));
    new_array(scope, &[symbol(kind), object, text])
}

/// The exception a refused copy raises.
fn refused(refusal: &Refusal) -> Error {
    let class: &'static str = match refusal.error.as_str() {
        "Ractor::Error" => "Ractor::Error",
        _ => "TypeError",
    };
    Error::raise(class, refusal.message.clone())
}

/// Copy `value` out of this heap. `taken` collects what a move leaves.
fn export(
    scope: &HandleScope<'_>,
    value: Value,
    taken: Option<&mut Vec<Value>>,
) -> Result<Msg, Error> {
    let refusals = registry(scope).refusals();
    scope
        .heap_ref()
        .export(
            value,
            &Options {
                refusals: &refusals,
                ..Options::default()
            },
            taken,
        )
        .map(|(msg, _)| msg)
        .map_err(|error| match error {
            ExportError::Refused(refusal) => refused(&refusal),
            ExportError::Unshareable => Error::raise("Ractor::Error", "can not copy the object"),
        })
}

/// `msg` built in this heap, as `[value, hashes, frozen]`: see
/// [`crate::heap::Imported`] for the two lists `core/ractor.rb` finishes it
/// with.
fn import(scope: &mut HandleScope<'_>, msg: &Msg) -> Value {
    let imported = scope.import(msg);
    let hashes = new_array(scope, &imported.hashes);
    let hashes = scope.root(hashes);
    let frozen = new_array(scope, &imported.frozen);
    let frozen = scope.root(frozen);
    let (hashes, frozen) = (scope.get(hashes), scope.get(frozen));
    new_array(scope, &[imported.root, hashes, frozen])
}

/// The primitives under `core/ractor.rb`. See [`RactorOp`].
pub(super) fn native(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    frames: &mut Vec<Call>,
    call: &Pending,
    op: RactorOp,
    ids: &mut u64,
) -> Result<Option<Unwind>, Error> {
    let arg = |i: usize| call.args.get(i).copied().unwrap_or(Value::NIL);
    let id_arg = |i: usize| -> u32 {
        arg(i)
            .as_fixnum()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    };
    let int = |n: u32| Value::fixnum(i64::from(n)).expect("an id is a fixnum");
    let symbol = |name: &str| Value::symbol(crate::shared::symbols::intern(name));
    let value = match op {
        RactorOp::Register => {
            let moved = arg(0);
            let moved = {
                let handle = scope.root(moved);
                scope.class_id_of(handle)
            };
            let mut refusals = Vec::new();
            for entry in array_elements(scope, arg(1)).unwrap_or_default() {
                let parts = array_elements(scope, entry).unwrap_or_default();
                let class = parts.first().copied().unwrap_or(Value::NIL);
                let handle = scope.root(class);
                let Some(class) = scope.class_id_of(handle) else {
                    continue;
                };
                let error = parts.get(1).and_then(|&v| string_text(scope, v));
                let message = parts.get(2).and_then(|&v| string_text(scope, v));
                refusals.push(Refusal {
                    class,
                    error: error.unwrap_or_default(),
                    message: message.unwrap_or_default(),
                });
            }
            let mut unshareable = Vec::new();
            for class in array_elements(scope, arg(2)).unwrap_or_default() {
                let handle = scope.root(class);
                if let Some(class) = scope.class_id_of(handle) {
                    unshareable.push(class);
                }
            }
            let mut inner = registry(scope).inner();
            inner.refusals = refusals;
            inner.unshareable = unshareable;
            inner.moved = moved;
            Value::NIL
        }
        RactorOp::WarnOnce => bool_value(!registry(scope).warned.swap(true, Ordering::AcqRel)),
        RactorOp::New => {
            return new(scope, call).map(|id| {
                stack.push(int(id));
                None
            });
        }
        RactorOp::Id => int(current(scope)),
        RactorOp::Count => int(u32::try_from(registry(scope).inner().alive).unwrap_or(0)),
        RactorOp::Info => {
            let Some(core) = registry(scope).ractor(id_arg(0)) else {
                stack.push(Value::NIL);
                return Ok(None);
            };
            let status = match lock(&core.state).status {
                Status::Running => "running",
                Status::Blocking => "blocking",
                Status::Terminated => "terminated",
            };
            let name = match &core.name {
                Some(name) => string_new(scope, name),
                None => Value::NIL,
            };
            let name = scope.root(name);
            let location = match &core.location {
                Some(location) => string_new(scope, location),
                None => Value::NIL,
            };
            let location = scope.root(location);
            let (name, location) = (scope.get(name), scope.get(location));
            new_array(scope, &[name, location, symbol(status)])
        }
        RactorOp::Port => match registry(scope).ractor(id_arg(0)) {
            Some(core) => int(core.port),
            None => Value::NIL,
        },
        RactorOp::PortNew => {
            let owner = current(scope);
            int(add_port(&mut registry(scope).inner(), owner))
        }
        RactorOp::PortOwner => match registry(scope).port(id_arg(0)) {
            Some(port) => int(port.owner),
            None => Value::NIL,
        },
        RactorOp::PortSend => {
            let moving = arg(2).is_truthy();
            let mut taken = Vec::new();
            let msg = export(scope, arg(1), moving.then_some(&mut taken))?;
            let sent = registry(scope).send(id_arg(0), Arc::new(msg));
            if sent && moving {
                leave_moved(scope, &taken);
            }
            bool_value(sent)
        }
        RactorOp::PortTake => {
            mark(scope, Status::Running);
            let Some(port) = registry(scope).port(id_arg(0)) else {
                stack.push(symbol("closed"));
                return Ok(None);
            };
            let taken = {
                let mut state = lock(&port.state);
                match state.queue.pop_front() {
                    Some(msg) => Ok(msg),
                    None => Err(state.closed),
                }
            };
            match taken {
                Ok(msg) => import(scope, &msg),
                Err(true) => symbol("closed"),
                Err(false) => symbol("empty"),
            }
        }
        RactorOp::PortClose => match registry(scope).port(id_arg(0)) {
            Some(port) => {
                let was_open = !std::mem::replace(&mut lock(&port.state).closed, true);
                if let Some(owner) = registry(scope).ractor(port.owner) {
                    owner.wake.notify();
                }
                bool_value(was_open)
            }
            None => Value::FALSE,
        },
        RactorOp::PortClosed => match registry(scope).port(id_arg(0)) {
            Some(port) => bool_value(lock(&port.state).closed),
            None => Value::TRUE,
        },
        RactorOp::Generation => {
            mark(scope, Status::Running);
            match registry(scope).ractor(current(scope)) {
                Some(core) => Value::fixnum(core.wake.generation() as i64 & Value::FIXNUM_MAX)
                    .expect("masked to a fixnum"),
                None => Value::fixnum(0).expect("zero is a fixnum"),
            }
        }
        RactorOp::Wait => {
            let Some(core) = registry(scope).ractor(current(scope)) else {
                stack.push(Value::NIL);
                return Ok(None);
            };
            let seen = arg(0).as_fixnum().unwrap_or(-1);
            let read_end = core.wake.read_end();
            if core.wake.generation() as i64 & Value::FIXNUM_MAX != seen {
                stack.push(Value::NIL);
                return Ok(None);
            }
            mark(scope, Status::Blocking);
            let leave = threads::Leave::Block {
                deadline: None,
                fds: vec![(read_end, libc::POLLIN)],
                deadlockable: false,
            };
            return threads::switch(scope, stack, frames, leave, ids);
        }
        RactorOp::Outcome => {
            let me = current(scope);
            let Some(core) = registry(scope).ractor(id_arg(0)) else {
                stack.push(Value::NIL);
                return Ok(None);
            };
            let outcome = {
                let mut state = lock(&core.state);
                match &state.outcome {
                    Some((ok, msg)) => Some((*ok, Arc::clone(msg))),
                    None => {
                        if !state.waiters.contains(&me) {
                            state.waiters.push(me);
                        }
                        None
                    }
                }
            };
            match outcome {
                None => Value::NIL,
                Some((ok, msg)) => {
                    let built = import(scope, &msg);
                    let built = scope.root(built);
                    let built = scope.get(built);
                    new_array(scope, &[bool_value(ok), built])
                }
            }
        }
        RactorOp::Finish => {
            let ok = arg(0).is_truthy();
            // A value that cannot be copied raises, and `core/ractor.rb`
            // ends the Ractor with that exception instead; an exception that
            // cannot be copied it copies again, stripped.
            let msg = export(scope, arg(1), None)?;
            registry(scope).finish(current(scope), ok, msg);
            Value::NIL
        }
        RactorOp::Start => {
            let start = registry(scope)
                .ractor(current(scope))
                .and_then(|core| lock(&core.state).start.take());
            let Some((body, cref, args)) = start else {
                stack.push(Value::NIL);
                return Ok(None);
            };
            let receiver = arg(0);
            let receiver = scope.root(receiver);
            let block = block_proc(scope, body, cref, scope.get(receiver))?;
            let block = scope.root(block);
            let args = import(scope, &args);
            let args = scope.root(args);
            let (block, args) = (scope.get(block), scope.get(args));
            new_array(scope, &[block, args])
        }
        RactorOp::Shareable => bool_value(is_shareable(scope, arg(0))),
        RactorOp::Monitor => {
            let port = id_arg(1);
            let add = arg(2).is_truthy();
            let Some(core) = registry(scope).ractor(id_arg(0)) else {
                stack.push(Value::NIL);
                return Ok(None);
            };
            let ended = {
                let mut state = lock(&core.state);
                if add {
                    if state.status == Status::Terminated {
                        state.outcome.as_ref().map(|(ok, _)| *ok)
                    } else {
                        state.monitors.push(port);
                        None
                    }
                } else {
                    state.monitors.retain(|&p| p != port);
                    None
                }
            };
            if let Some(ok) = ended {
                let word = symbol(if ok { "exited" } else { "aborted" });
                registry(scope).send(port, Arc::new(Msg::immediate(word)));
            }
            Value::NIL
        }
        RactorOp::MakeShareable => {
            let root = arg(0);
            let root = scope.root(root);
            let unshareable = registry(scope).inner().unshareable.clone();
            // Each `Proc` the walk stops at is made shareable, and the walk
            // starts again, passing over it: what came before it is frozen
            // already, and freezes again for nothing.
            let (hashes, error) = loop {
                let (hashes, stop) = scope.make_shareable(scope.get(root), &unshareable);
                let hashes: Vec<_> = hashes.iter().map(|&hash| scope.root(hash)).collect();
                let error = match stop {
                    None => Value::NIL,
                    Some(Stop::Refused(object)) => {
                        let object = scope.root(object);
                        let object = scope.get(object);
                        new_array(scope, &[symbol("refused"), object, Value::NIL])
                    }
                    Some(Stop::Proc(block)) => match make_proc_shareable(scope, block, None) {
                        Ok(()) => continue,
                        Err(refusal) => refusal_value(scope, refusal),
                    },
                };
                break (hashes, error);
            };
            let error = scope.root(error);
            let hashes: Vec<Value> = hashes.iter().map(|&hash| scope.get(hash)).collect();
            let hashes = new_array(scope, &hashes);
            let error = scope.get(error);
            new_array(scope, &[hashes, error])
        }
        RactorOp::Copy => {
            let msg = export(scope, arg(0), None)?;
            import(scope, &msg)
        }
        RactorOp::ShareableProc => {
            // `__ractor_shareable_proc__(block, receiver, lambda)`: a copy of
            // `block` made shareable with `receiver` for its `self`, as
            // `[proc, nil]`, or `[nil, refusal]`.
            let block = arg(0);
            let receiver = scope.root(arg(1));
            let lambda = arg(2).is_truthy();
            let Some((iseq, env, own_self, captured, was_lambda, cref)) = proc_parts(scope, block)
            else {
                let error = refusal_value(scope, ProcRefusal::Unsupported);
                stack.push(new_array(scope, &[Value::NIL, error]));
                return Ok(None);
            };
            let (home, _) = super::proc_links(scope, block);
            let method = super::proc_method(scope, block);
            let proc_class = super::class_handle(scope, Builtin::Proc);
            let copy = super::make_proc(
                scope,
                proc_class,
                &iseq,
                env,
                own_self,
                captured,
                lambda || was_lambda,
                cref,
                home,
                method,
            );
            let copy = scope.root(copy);
            let receiver = scope.get(receiver);
            match make_proc_shareable(scope, scope.get(copy), Some(receiver)) {
                Ok(()) => {
                    let copy = scope.get(copy);
                    new_array(scope, &[copy, Value::NIL])
                }
                Err(refusal) => {
                    let error = refusal_value(scope, refusal);
                    new_array(scope, &[Value::NIL, error])
                }
            }
        }
        RactorOp::Freeze => {
            for value in array_elements(scope, arg(0)).unwrap_or_default() {
                if !value.is_immediate() {
                    let handle = scope.root(value);
                    scope.freeze(handle);
                }
            }
            Value::NIL
        }
    };
    stack.push(value);
    Ok(None)
}

/// Record this heap's Ractor as waiting, or as running again.
fn mark(scope: &HandleScope<'_>, status: Status) {
    if !scope.program().has_ractors() {
        return;
    }
    if let Some(core) = registry(scope).ractor(current(scope)) {
        let mut state = lock(&core.state);
        if state.status != Status::Terminated {
            state.status = status;
        }
    }
}

/// What a move leaves in the sender: each object the copy took becomes an
/// instance of `Ractor::MovedObject`, which answers every call with
/// `Ractor::MovedError`, as CRuby's does.
fn leave_moved(scope: &mut HandleScope<'_>, taken: &[Value]) {
    let Some(moved) = registry(scope).inner().moved else {
        return;
    };
    let class = scope.class_object(moved);
    for &value in taken {
        let handle = scope.root(value);
        scope.set_class(handle, class);
    }
}

/// `Ractor.new(*args, name:, &block)` (`__ractor_new__(args, name, block,
/// location)`): a new Ractor, started on an OS thread of its own; its id.
fn new(scope: &mut HandleScope<'_>, call: &Pending) -> Result<u32, Error> {
    let arg = |i: usize| call.args.get(i).copied().unwrap_or(Value::NIL);
    let block = arg(2);
    let Some((iseq, ..)) = proc_parts(scope, block) else {
        return Err(Error::raise("ArgumentError", "must be called with a block"));
    };
    if let Some(message) = Outer::of(&iseq).refusal("isolate a Proc", true) {
        return Err(Error::raise("ArgumentError", message));
    }
    let block_handle = scope.root(block);
    let body = super::proc_body(scope, block).expect("a Proc's body");
    let cref = proc_parts(scope, scope.get(block_handle))
        .map(|parts| parts.5)
        .unwrap_or(CrefId::ROOT);
    let args = export(scope, arg(0), None)?;
    let name = string_text(scope, arg(1));
    let location = string_text(scope, arg(3));
    let child_main = child_main(scope)?;

    let core = registry(scope).add(name, location);
    lock(&core.state).start = Some((body, cref, args));
    let child = scope.new_ractor_heap(core.id);
    spawn(child, Arc::clone(&core), child_main, scope.program_arc()).inspect_err(|_| {
        registry(scope).finish(core.id, false, Msg::immediate(Value::NIL));
    })?;
    Ok(core.id)
}

/// The body a new Ractor's heap runs first, compiled once per program.
fn child_main(scope: &mut HandleScope<'_>) -> Result<Arc<Iseq>, Error> {
    if let Some(iseq) = registry(scope).inner().child_main.clone() {
        return Ok(iseq);
    }
    let parser = scope.parser().ok_or(Error::Unknowable {
        what: "a Ractor in a heap with no parser",
        needs: "a parser installed with `HandleScope::set_parser`",
    })?;
    let parsed = parser(
        "<internal:ractor>",
        b"Ractor.__child_main__",
        1,
        false,
        None,
        &[],
    )
    .map_err(|_| Error::raise("RuntimeError", "can not start a Ractor"))?;
    let iseq = crate::compile::program_with(&parsed.program, "<internal:ractor>", None)
        .map_err(|_| Error::raise("RuntimeError", "can not start a Ractor"))?;
    let iseq = Arc::new(iseq);
    registry(scope).inner().child_main = Some(Arc::clone(&iseq));
    Ok(iseq)
}

/// A new Ractor's heap, moved to the OS thread that runs it.
struct Moved(Heap);

// SAFETY: the heap was made for this OS thread (`Heap::new_ractor`) and no
// other uses it: the heap that made it holds no reference to it.
unsafe impl Send for Moved {}

/// The OS thread a Ractor's heap runs on: its signals go to the main
/// thread, as every thread's do; it runs `Ractor.__child_main__`, which
/// runs the block and ends the Ractor; then its heap lets the class lock
/// go for good.
fn spawn(
    heap: Heap,
    core: Arc<Core>,
    child_main: Arc<Iseq>,
    program: Arc<Program>,
) -> Result<(), Error> {
    let moved = Moved(heap);
    let spawned = std::thread::Builder::new()
        .name(format!("ractor {}", core.id))
        .stack_size(8 << 20)
        .spawn(move || {
            crate::shared::signals::block_on_this_thread();
            let mut moved = moved;
            {
                let mut scope = moved.0.scope();
                let mut frame = super::Frame::new(child_main.locals.len());
                let _ = super::eval_in(&mut scope, &mut frame, &child_main);
                // `Ractor.__child_main__` ends the Ractor; a refusal that
                // stopped it first ends it here.
                if lock(&core.state).status != Status::Terminated {
                    program
                        .ractors()
                        .finish(core.id, false, Msg::immediate(Value::NIL));
                }
            }
            moved.0.release_classes();
            // The heap's memory goes with it. Nothing outside it points in.
            drop(moved);
        });
    match spawned {
        Ok(_) => Ok(()),
        Err(error) => {
            let reason = error
                .raw_os_error()
                .map_or_else(|| error.to_string(), crate::errno::message);
            Err(Error::raise(
                "ThreadError",
                format!("can't create Thread: {reason}"),
            ))
        }
    }
}

/// The block of `Ractor.new` again, in the new Ractor's heap: a `Proc` of
/// the same body and lexical scope whose `self` is the Ractor. It reads no
/// variable outside itself (`Outer::refusal`), so it captures none.
fn block_proc(
    scope: &mut HandleScope<'_>,
    body: Value,
    cref: CrefId,
    receiver: Value,
) -> Result<Value, Error> {
    let iseq = match scope.definitions().get(body) {
        Some(Definition::Iseq(iseq)) => Arc::clone(iseq),
        _ => return Err(Error::raise("RuntimeError", "can not start a Ractor")),
    };
    let receiver = scope.root(receiver);
    let proc_class = scope.class_object(Builtin::Proc.id());
    let proc_class = scope.root(proc_class);
    let env = super::env_alloc(scope, Value::NIL, 0);
    let receiver = scope.get(receiver);
    Ok(super::make_proc(
        scope,
        proc_class,
        &iseq,
        env,
        receiver,
        Value::NIL,
        false,
        cref,
        0,
        (None, None),
    ))
}

#[cfg(test)]
mod tests {
    //! Shareability (#118) without the parser, so miri reaches the walk's
    //! writes to headers, a `Proc`'s environment made again, and a marked
    //! `Proc` copied.
    use super::*;
    use crate::heap::Heap;

    fn int(n: i64) -> Value {
        Value::fixnum(n).expect("small")
    }

    /// The walk freezes an object before what it holds, in order, and stops
    /// at the first object of a refused class, leaving what comes after it
    /// as it was.
    #[test]
    fn make_shareable_freezes_in_order_and_stops_at_a_refusal() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        scope.bootstrap();
        let first = new_array(&mut scope, &[int(1)]);
        let first = scope.root(first);
        let refused = string_new(&mut scope, "x");
        let refused = scope.root(refused);
        let last = new_array(&mut scope, &[int(2)]);
        let last = scope.root(last);
        let parts = [scope.get(first), scope.get(refused), scope.get(last)];
        let outer = new_array(&mut scope, &parts);
        let outer = scope.root(outer);
        let string = Builtin::String.id();
        let (hashes, stop) = scope.make_shareable(scope.get(outer), &[string]);
        assert!(hashes.is_empty());
        assert!(matches!(stop, Some(Stop::Refused(value)) if value == scope.get(refused)));
        assert!(scope.is_frozen(outer));
        assert!(scope.is_frozen(first));
        assert!(!scope.is_frozen(refused));
        assert!(!scope.is_frozen(last), "after the refusal");
        let (_, stop) = scope.make_shareable(scope.get(outer), &[]);
        assert!(stop.is_none());
        assert!(scope.is_frozen(last));
    }

    /// A `Proc` reading one variable outside itself is made shareable with
    /// an environment of its own holding that one alone, marked and frozen,
    /// and then copies out of the heap, where an unmarked one is refused.
    #[test]
    fn a_proc_made_shareable_has_its_own_environment_and_copies() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        scope.bootstrap();
        let iseq = Arc::new(Iseq {
            name: "block".into(),
            insns: vec![Insn::GetLocal(1, 1), Insn::Leave],
            max_stack: 1,
            scope_barrier: false,
            outer: vec![vec!["unread".into(), "read".into()]],
            ..Iseq::default()
        });
        let held = new_array(&mut scope, &[int(3)]);
        let held = scope.root(held);
        scope.freeze(held);
        let unread = new_array(&mut scope, &[]);
        let unread = scope.root(unread);
        let env = super::super::env_alloc(&mut scope, Value::NIL, 2);
        let env = scope.root(env);
        let (unread_value, held_value) = (scope.get(unread), scope.get(held));
        scope.set_slot(env, super::super::ENV_HEADER, unread_value);
        scope.set_slot(env, super::super::ENV_HEADER + 1, held_value);
        let proc_class = super::super::class_handle(&mut scope, Builtin::Proc);
        let env_value = scope.get(env);
        let block = super::super::make_proc(
            &mut scope,
            proc_class,
            &iseq,
            env_value,
            Value::NIL,
            Value::NIL,
            false,
            CrefId::ROOT,
            1,
            (None, None),
        );
        let block = scope.root(block);
        scope.set_slot(block, super::super::PROC_BREAK, int(2));
        let refusal = Refusal {
            class: Builtin::Proc.id(),
            error: "TypeError".into(),
            message: "allocator undefined for Proc".into(),
        };
        let options = Options {
            refusals: std::slice::from_ref(&refusal),
            shareable_only: true,
            known: None,
        };
        assert!(
            scope
                .heap_ref()
                .export(scope.get(block), &options, None)
                .is_err()
        );

        let value = scope.get(block);
        assert!(make_proc_shareable(&mut scope, value, None).is_ok());
        assert!(scope.is_frozen(block));
        let copy = scope.slot(block, super::super::PROC_ENV);
        assert_ne!(copy, scope.get(env), "its environment is its own");
        let copy = scope.root(copy);
        assert_eq!(
            scope.slot(copy, super::super::ENV_HEADER + 1),
            scope.get(held)
        );
        assert_eq!(scope.slot(copy, super::super::ENV_HEADER), Value::NIL);
        assert_eq!(scope.slot(env, super::super::ENV_HEADER), scope.get(unread));

        let (msg, _) = scope
            .heap_ref()
            .export(scope.get(block), &options, None)
            .expect("a shareable Proc copies");
        let imported = scope.import(&msg);
        let built = scope.root(imported.root);
        // What was frozen is frozen by the caller, as `Ractor.__built__` does.
        for &value in &imported.frozen {
            let handle = scope.root(value);
            scope.freeze(handle);
        }
        assert_eq!(scope.class_of(built), Some(Builtin::Proc.id()));
        assert_eq!(scope.slot(block, super::super::PROC_HOME), int(1));
        assert_eq!(scope.slot(block, super::super::PROC_BREAK), int(2));
        assert_eq!(
            scope.slot(built, super::super::PROC_HOME),
            int(0),
            "the copy names no frame of the heap it came from"
        );
        assert_eq!(scope.slot(built, super::super::PROC_BREAK), int(0));
        assert!(
            scope
                .heap_ref()
                .export(scope.get(built), &options, None)
                .is_ok(),
            "the copy is marked shareable too"
        );
    }

    /// A variable outside the block that holds a mutable object is refused,
    /// and the `Proc` is left as it was.
    #[test]
    fn a_proc_reading_a_mutable_object_is_refused_and_left_as_it_was() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        scope.bootstrap();
        let iseq = Arc::new(Iseq {
            name: "block".into(),
            insns: vec![Insn::GetLocal(0, 1), Insn::Leave],
            max_stack: 1,
            scope_barrier: false,
            outer: vec![vec!["x".into()]],
            ..Iseq::default()
        });
        let mutable = new_array(&mut scope, &[]);
        let mutable = scope.root(mutable);
        let env = super::super::env_alloc(&mut scope, Value::NIL, 1);
        let env = scope.root(env);
        let value = scope.get(mutable);
        scope.set_slot(env, super::super::ENV_HEADER, value);
        let proc_class = super::super::class_handle(&mut scope, Builtin::Proc);
        let env_value = scope.get(env);
        let block = super::super::make_proc(
            &mut scope,
            proc_class,
            &iseq,
            env_value,
            Value::NIL,
            Value::NIL,
            false,
            CrefId::ROOT,
            0,
            (None, None),
        );
        let block = scope.root(block);
        let value = scope.get(block);
        let refused = make_proc_shareable(&mut scope, value, None);
        assert!(
            matches!(refused, Err(ProcRefusal::Refers(value, ref name)) if value == scope.get(mutable) && name == "x")
        );
        assert!(!scope.is_frozen(block));
        assert!(!scope.is_frozen(mutable));
        assert_eq!(scope.slot(block, super::super::PROC_ENV), scope.get(env));
    }
}
