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
//! While a heap waits that way, or a call blocks outside the per-Ractor
//! lock, it lets go of the class lock, so a Ractor that defines a method
//! meanwhile does not wait for it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::{
    Call, Error, HandleScope, Heap, Pending, Stack, Unwind, Value, array_elements, bool_value,
    new_array, proc_parts, string_new, string_text, threads,
};
use crate::bytecode::{Insn, Iseq};
use crate::class::{Builtin, ClassId, CrefId};
use crate::heap::{ExportError, Msg, Options, Refusal};
use crate::method::{Definition, RactorOp};

/// What every heap of a program keeps for its Ractors, beside the [`Vm`]:
/// the registry, made when the first Ractor is asked for, and where the main
/// heap is as it last let the class lock go or parked at a safe point —
/// what another Ractor reads the main one's constants from while it stays
/// parked (`HandleScope::pull`). Held outside the `Vm`, whose layout the
/// interpreter's loop is measured with.
///
/// [`Vm`]: crate::vm::Vm
#[derive(Default)]
pub(crate) struct Program {
    registry: std::sync::OnceLock<Registry>,
    main_heap: std::sync::atomic::AtomicPtr<Heap>,
}

impl Program {
    /// The program's Ractors.
    pub(crate) fn ractors(&self) -> &Registry {
        self.registry.get_or_init(Registry::new)
    }

    /// Whether the program has asked for a Ractor yet.
    pub(crate) fn has_ractors(&self) -> bool {
        self.registry.get().is_some()
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

/// The names of the local variables of the scopes outside it that `block`
/// reads or writes, in the order they are first met: what
/// `Ractor.new` refuses (CRuby's "can not isolate a Proc because it accesses
/// outer variables (x).").
fn outer_variables(block: &Iseq) -> Vec<String> {
    fn walk(iseq: &Iseq, base: usize, names: &mut Vec<String>) {
        // How many blocks deep inside `block` this body is.
        let level = iseq.outer.len().saturating_sub(base);
        for insn in &iseq.insns {
            let (Insn::GetLocal(slot, depth) | Insn::SetLocal(slot, depth)) = *insn else {
                continue;
            };
            let depth = depth as usize;
            if depth <= level {
                continue;
            }
            let name = iseq
                .outer
                .get(depth - 1)
                .and_then(|env| env.get(slot as usize))
                .map_or_else(|| "?".to_string(), |name| name.to_string());
            if !names.contains(&name) {
                names.push(name);
            }
        }
        for child in &iseq.children {
            // A block written inside sits one environment further in; a
            // method or a class body starts a scope of its own.
            if child.outer.len() > iseq.outer.len() {
                walk(child, base, names);
            }
        }
    }
    let mut names = Vec::new();
    walk(block, block.outer.len(), &mut names);
    names
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
            let mut inner = registry(scope).inner();
            inner.refusals = refusals;
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
            let msg = match export(scope, arg(1), None) {
                Ok(msg) => msg,
                // A value that cannot be copied ends the Ractor with the
                // exception copying it raised.
                Err(_) if ok => Msg::immediate(Value::NIL),
                Err(error) => return Err(error),
            };
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
        RactorOp::Shareable => bool_value(
            arg(0).is_immediate()
                || scope
                    .heap_ref()
                    .export(
                        arg(0),
                        &Options {
                            shareable_only: true,
                            ..Options::default()
                        },
                        None,
                    )
                    .is_ok(),
        ),
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
        RactorOp::FreezeDeep => {
            let root = arg(0);
            let handle = scope.root(root);
            let hashes = scope.freeze_deep(scope.get(handle));
            new_array(scope, &hashes)
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
    let outer = outer_variables(&iseq);
    if !outer.is_empty() {
        return Err(Error::raise(
            "ArgumentError",
            format!(
                "can not isolate a Proc because it accesses outer variables ({}).",
                outer.join(", ")
            ),
        ));
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
/// variable outside itself (`outer_variables`), so it captures none.
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
    ))
}
