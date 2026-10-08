//! The interpreter loop.
//!
//! Non-recursive from the first commit, because engine.md requires it: a
//! Ruby-to-Ruby call pushes a frame and continues the same loop rather than
//! recursing on the Rust stack, which is what fibers and Ruby's own recursion
//! limits depend on. Since
//! [#11](https://github.com/ar4mirez/spinel/issues/11) there are real frames in
//! that `Vec`, and the shape held.
//!
//! # Scopes
//!
//! A frame's locals live in a heap environment whose first slot links to the
//! enclosing one, so a block reads an outer local by walking `depth` links.
//! Making it an ordinary slots object is what lets the collector trace a
//! captured variable without knowing what a closure is.
//!
//! A method body sets a *scope barrier* and a block body does not, which is the
//! one place the two differ: `def` cannot see the locals it was written among,
//! and a block can.
//!
//! # Rooting
//!
//! Every value the loop puts on its stack is either an immediate or an object
//! allocated through the [`HandleScope`] it was handed, so the collector can see
//! all of them for as long as the scope lives.
//!
//! ponytail: that also means an object allocated inside a loop is not reclaimed
//! until the whole evaluation ends — the scope only pops on drop — and #11 added
//! an environment per call to what accumulates there. Releasing a frame's roots
//! on return needs the operand stack to be a root source first, or a returned
//! value would be unrooted while still on the stack. That is the same fix
//! [#7](https://github.com/ar4mirez/spinel/issues/7) shaped `shade` to accept,
//! and engine.md puts the VM stack in the Ractor where fibers need it, so it
//! lands with fibers rather than here.

use std::os::unix::ffi::OsStringExt as _;
use std::path::PathBuf;
use std::sync::Arc;

use crate::bytecode::{
    BinOp, BlockRef, CallSite, CatchKind, ClassDef, ConstScope, DefKind, Insn, Iseq, Literal,
    MatchRef, ParamSpec,
};
use crate::class::Builtin;
use crate::class::{ClassId, CrefId, Kind, Method, ScopeDefault, Visibility};
use crate::heap::{Handle, HandleScope, Heap, Payload};
use crate::method::{
    BindingOp, BitOp, CvarOp, Definition, FiberOp, FsOp, IvarOp, Native, ReflectOp, StrOp, SysOp,
};
use crate::shape::ShapeId;
use crate::value::SymbolId;
use crate::value::Value;

/// Why an evaluation stopped early.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The operand types are not on an instruction's fast path, and the send
    /// that would be behind it does not exist yet.
    ///
    /// Not "wrong": *not yet dispatchable*. When #11 lands the calling
    /// convention, [`Insn::BinOp`] grows a real send here and every call site
    /// that already emits it starts working on every type.
    NoDispatch {
        op: &'static str,
        /// What the operands were, for the report that decides the next slice.
        operands: &'static str,
    },
    /// Ruby would raise. [#12](https://github.com/ar4mirez/spinel/issues/12)
    /// turns this into an exception object with a class and a backtrace; until
    /// then it is a reason an example could not be run.
    ///
    /// The message is built at the point that knows it — the binder, for an
    /// arity error — because ruby/spec asserts on the text and #12 should find
    /// the wording already correct rather than have to rediscover it.
    Raise {
        class: &'static str,
        message: String,
    },
    /// A Ruby exception that no `rescue` wanted, carrying the class and message
    /// it reached the top with.
    ///
    /// Distinct from [`Error::Raise`], which is the VM *deciding* to raise, and
    /// which becomes an object the moment it leaves an instruction. By the time
    /// this exists the object has been built, matched against every handler on
    /// the way out, and found none — so the class name may be one the program
    /// defined, which is why it is a `String` and not `&'static str`.
    Uncaught { class: String, message: String },
    /// A loop that ran past its budget. Guards the harness against a spec that
    /// depends on a construct the compiler silently made non-terminating.
    Budget,
    /// A construct the compiler could not lower, reached at run time. See
    /// [`crate::bytecode::Insn::Refuse`].
    NotCompiled(&'static str),
    /// A question this heap cannot answer, where the answer Ruby gives would be
    /// indistinguishable from a wrong one.
    ///
    /// Distinct from [`Error::NoDispatch`], which is about an operand type, and
    /// from [`Error::Raise`], which is Ruby's own behaviour. This is the VM
    /// declining to guess: `defined?` on a name that is missing only because
    /// nothing loaded the file that defines it would answer `nil`, and `nil` is
    /// also Ruby's answer for a name that is genuinely undefined.
    ///
    /// It reads in the blocked-reason report, which is how the next slice gets
    /// chosen, so the text names the question and the slice that settles it.
    Unknowable {
        what: &'static str,
        /// The slice that makes the question answerable.
        needs: &'static str,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoDispatch { op, operands } => {
                write!(f, "`{op}` on {operands} needs method dispatch")
            }
            Error::Raise { class, message } if message.is_empty() => {
                write!(f, "would raise {class}")
            }
            Error::Raise { class, message } => write!(f, "would raise {class}: {message}"),
            Error::Uncaught { class, message } if message.is_empty() => {
                write!(f, "{class}")
            }
            Error::Uncaught { class, message } => write!(f, "{class}: {message}"),
            Error::Budget => write!(f, "ran past the instruction budget"),
            Error::NotCompiled(node) => write!(f, "{node} is not compiled yet"),
            Error::Unknowable { what, needs } => {
                write!(f, "{what} cannot be answered before {needs}")
            }
        }
    }
}

impl Error {
    /// A raise with Ruby's own message text.
    fn raise(class: &'static str, message: impl Into<String>) -> Error {
        Error::Raise {
            class,
            message: message.into(),
        }
    }
}

impl std::error::Error for Error {}

/// How many instructions one evaluation may run before it is assumed stuck.
///
/// Generous enough that no ruby/spec example approaches it, small enough that a
/// non-terminating loop is a reported failure rather than a hung run.
pub const BUDGET: u64 = 50_000_000;

/// The top-level scope an evaluation runs in.
///
/// The harness keeps one across an example's statements, which is how `a = 1` in
/// the first line is visible to `a.should == 1` in the last without the VM
/// knowing what a matcher is. Its locals live in a heap environment like every
/// other scope's, so a block written at the top level captures them for real
/// rather than seeing a copy.
#[derive(Debug, Clone)]
pub struct Frame {
    /// The environment holding this scope's locals. `NIL` until the first
    /// evaluation, because building one needs a heap and `new` has none.
    env: Value,
    slots: usize,
    /// Top-level `self`: Ruby's `main`. Built on first evaluation, because a
    /// receiverless call has to dispatch on *something* and `nil` has no class
    /// yet — which is also true in Ruby, where `main` is an ordinary `Object`.
    receiver: Value,
    /// The lexical scope statements run in. A file's top level is
    /// [`CrefId::ROOT`], and a `class` body pushes its own; this is only ever
    /// `ROOT` because the harness evaluates one top-level statement at a time.
    cref: CrefId,
}

impl Frame {
    /// A frame with room for `slots` locals, all `nil`.
    #[must_use]
    pub fn new(slots: usize) -> Frame {
        Frame {
            env: Value::NIL,
            slots,
            receiver: Value::NIL,
            cref: CrefId::ROOT,
        }
    }

    /// Grow to hold at least `slots` locals, keeping the ones already set.
    ///
    /// The harness compiles an example's statements separately against one
    /// shared slot map, and a later statement may be the first to mention a
    /// local; the frame follows rather than being rebuilt.
    pub fn reserve(&mut self, slots: usize) {
        self.slots = self.slots.max(slots);
    }

    /// Write `value` into local `slot`, growing the frame to hold it.
    ///
    /// The harness parks a value it already computed here so a compiled
    /// expression can read it back by name — which is how `x.should == y`
    /// dispatches `==` to Ruby without evaluating `x` a second time.
    pub fn set_local(&mut self, scope: &mut HandleScope<'_>, slot: usize, value: Value) {
        let value = scope.root(value);
        self.reserve(slot + 1);
        let env = self.env(scope);
        let env = scope.root(env);
        let value = scope.get(value);
        scope.set_slot(env, ENV_HEADER + slot, value);
    }

    /// This frame's environment, allocated or grown to fit `slots`.
    fn env(&mut self, scope: &mut HandleScope<'_>) -> Value {
        let needed = self.slots;
        if let Some(existing) = env_len(scope, self.env)
            && existing >= needed
        {
            return self.env;
        }
        let grown = env_alloc(scope, Value::NIL, needed);
        if self.env != Value::NIL {
            let (old, new) = (scope.root(self.env), scope.root(grown));
            for slot in 0..(scope.len(old) as usize - ENV_HEADER) {
                let value = scope.slot(old, ENV_HEADER + slot);
                scope.set_slot(new, ENV_HEADER + slot, value);
            }
        }
        self.env = grown;
        grown
    }

    #[must_use]
    pub fn local(&self, scope: &mut HandleScope<'_>, slot: usize) -> Option<Value> {
        let len = env_len(scope, self.env)?;
        (slot < len).then(|| env_get(scope, self.env, slot))
    }
}

// ---------------------------------------------------------------------------
// Environments
// ---------------------------------------------------------------------------

/// Slot 0 of an environment is the enclosing environment; locals start after it.
const ENV_HEADER: usize = 1;

/// Allocate an environment for `slots` locals, linked to `outer`.
///
/// A plain `Slots` object, so the collector traces the locals and the parent
/// link with no code that knows what an environment is.
///
// ponytail: one of these per call that has a frame, rather than only per call a
// block actually captures. engine.md wants a `captured` bit from the resolve
// pass deciding it, which is a compiler pass this slice does not need to be
// correct — only to be fast. The uniform version keeps one code path for
// `GetLocal`; upgrade it when `bench/` has a call-heavy number to move.
fn env_alloc(scope: &mut HandleScope<'_>, outer: Value, slots: usize) -> Value {
    let len = u32::try_from(ENV_HEADER + slots).expect("a scope has fewer than 4 billion locals");
    let handle = scope.alloc(None, Payload::Slots, len);
    scope.set_slot(handle, 0, outer);
    for slot in 0..slots {
        scope.set_slot(handle, ENV_HEADER + slot, Value::NIL);
    }
    scope.get(handle)
}

fn env_len(scope: &mut HandleScope<'_>, env: Value) -> Option<usize> {
    if env == Value::NIL {
        return None;
    }
    let handle = scope.root(env);
    Some(scope.len(handle) as usize - ENV_HEADER)
}

/// Walk `depth` links out and read `slot`.
fn env_get(scope: &mut HandleScope<'_>, env: Value, slot: usize) -> Value {
    let handle = scope.root(env);
    scope.slot(handle, ENV_HEADER + slot)
}

fn env_set(scope: &mut HandleScope<'_>, env: Value, slot: usize, value: Value) {
    let handle = scope.root(env);
    scope.set_slot(handle, ENV_HEADER + slot, value);
}

/// The environment `depth` scopes out from `env`.
fn env_outer(scope: &mut HandleScope<'_>, mut env: Value, depth: u16) -> Value {
    for _ in 0..depth {
        let handle = scope.root(env);
        env = scope.slot(handle, 0);
    }
    env
}

/// Compile-and-run's other half: run `iseq` in a fresh frame on `heap`.
pub fn eval(heap: &mut Heap, iseq: &Iseq) -> Result<Value, Error> {
    let mut frame = Frame::new(iseq.locals.len());
    let mut scope = heap.scope();
    eval_in(&mut scope, &mut frame, iseq)
}

/// One Ruby-to-Ruby call in flight.
///
/// A frame is a value in a `Vec`, not a Rust stack frame: `Send` pushes one and
/// the loop continues, which is what keeps the interpreter non-recursive and is
/// what fibers will need when they own a VM stack of these.
struct Call {
    iseq: Arc<Iseq>,
    /// This `Iseq`'s symbol pool, interned once per frame rather than per
    /// instruction.
    symbols: Vec<SymbolId>,
    /// Where this `Iseq`'s run of inline caches starts, resolved once per frame
    /// for the same reason `symbols` is: the lookup is a hash probe, and it
    /// belongs on frame entry rather than on every `Send`. A call-site id is
    /// this plus the instruction's operand.
    cache_base: u32,
    /// This frame's own locals.
    env: Value,
    receiver: Value,
    /// The block this frame was called with, as a `Proc` or `nil`. Reached by
    /// `yield` and by `block_given?`, and never by a slot, so an anonymous
    /// block costs nothing.
    block: Value,
    /// What a `def` in *this* scope becomes — a bare `private` sets it (#161),
    /// and a bare `module_function` sets it too (#211). CRuby's `CREF_SCOPE_VISI`, kept on the frame rather than on the
    /// scope because it belongs to one execution of a body:
    ///
    /// ```ruby
    /// class A
    ///   private
    ///   [1].each { def in_block; end }   # private: a block shares the scope
    ///   def outer; def nested; end; end  # public: a method body resets it
    /// end
    /// ```
    ///
    /// Both measured on ruby 4.0.6. A block takes the value from the frame it
    /// was written in — the one its `home` link already names — and a method
    /// body starts public, which is the whole difference between those lines.
    scope_default: ScopeDefault,
    /// The lexical scope this frame's code was written in: the enclosing
    /// `class`/`module` chain, which is what a bare constant resolves against.
    /// Inherited from the `Method` for a call, from the `Proc` for a block, and
    /// pushed fresh by [`Insn::OpenClass`].
    cref: CrefId,
    /// The block a `Proc` frame runs with is the one its *defining* frame had,
    /// which is what makes `yield` inside a block reach the method's block.
    pc: usize,
    /// Where this frame's operands start in the shared value stack.
    base: usize,
    /// Leave the value already below `base` rather than what the body computed.
    ///
    /// Only `Class#new` sets it: `initialize` may return anything and `new`
    /// still answers the object. A flag on the frame rather than a re-entrant
    /// `eval`, so a Ruby `initialize` still costs no Rust stack.
    keeps_receiver: bool,
    /// Raise the value below `base` when this frame leaves, rather than
    /// answering it.
    ///
    /// Set with `keeps_receiver` by `raise Klass, msg` when `Klass` has an
    /// `initialize` written in Ruby: Ruby's `raise` is `Klass.exception(msg)`,
    /// which is `new`, so that `initialize` has to run before anything is
    /// raised — and a native cannot sequence the frame and then the raise.
    raises_receiver: bool,
    /// Drop this frame's value when it leaves (#28): a definition hook —
    /// `method_added`, `inherited`, `included` — runs as a frame pushed by the
    /// instruction that defined something, and what the definition answers is
    /// already on the stack below it.
    discards_value: bool,
    /// Answer `true` or `false` by this frame's value's truthiness (#28):
    /// `respond_to?` answers what a program's `respond_to_missing?` did,
    /// as a strict boolean. Measured.
    booleanizes_value: bool,
    /// This frame's identity, unique for the whole evaluation.
    ///
    /// `break` and `return` out of a block name the frame they end, and an
    /// index into `frames` would be reused the moment a frame is popped — so a
    /// `Proc` outliving its call would end whatever call happened to be at that
    /// depth instead of raising `LocalJumpError`.
    id: u64,
    /// The frame a `return` in this body leaves. Its own `id` for a method or a
    /// lambda, and the defining method's for a block.
    home: u64,
    /// The frame a `break` in this body ends: the call the block was passed to.
    /// Zero when this body is not a block, where `break` is a `LocalJumpError`.
    breaks: u64,
    /// The `catch` tag this frame is the boundary for, if it is one.
    tag: Option<Value>,
    /// The exception a `rescue` in this frame is currently handling.
    ///
    /// Ruby spells this `$!`, and globals do not exist yet. Scoping it to the
    /// frame is enough for what the keyword needs it for: a bare `raise` inside
    /// a `rescue` body re-raises what that body caught.
    rescued: Option<Value>,
    /// `$!` as this frame found it, restored when the frame is popped (#206).
    ///
    /// A frame leaves the cell exactly as it found it, which is what makes a
    /// `return` or a `break` out of a `rescue` clause restore it: neither runs
    /// the compiler's own restore, and both go out through a `frames.pop()`.
    errinfo_on_entry: Value,
    /// The class this frame's method was found on, and the name it was found
    /// under. `None` for a body that is not a method — the top level, a class
    /// body — where `super` has nowhere to start (#187).
    owner: Option<ClassId>,
    /// See [`Call::owner`].
    defined_as: Option<SymbolId>,
    /// Reasons parked by an `ensure` in this frame, innermost last.
    ///
    /// A `Vec` rather than one slot because an `ensure` body can contain
    /// another `begin`/`ensure`, and the outer reason has to survive the inner
    /// one running to completion.
    parked: Vec<Parked>,
    /// Nonzero for the block of a refusal boundary (#145): how many
    /// instructions it may run. A refusal — or running past that — inside it
    /// unwinds to here and becomes the call's value, rather than ending the
    /// evaluation. See `Native::RefusalBoundary`.
    boundary_limit: u64,
}

/// How a callee takes the arguments it was handed.
///
/// Two rules travel together and are the same distinction: a block spreads a
/// lone `Array` across its parameters and pads what it was not given, and a
/// method or a lambda does neither and insists on the count.
///
/// It was a `lambda: bool` until this slice, and a method was passed `false` —
/// so `def foo(a, b); end; foo(1)` answered `nil` instead of raising
/// `ArgumentError`. Nothing caught it because nothing could *catch*: the
/// example that asserts on it is `-> { foo 1 }.should.raise(ArgumentError)`,
/// which was reported blocked until the matcher in `spec/harness` could run a
/// proc and look at what came out. A named type rather than a second `bool`,
/// because the reason the first one was wrong is that its name described one of
/// the two rules and was read as describing the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    /// A method or a lambda: exact arity, no destructuring.
    Strict,
    /// A block or a proc: a lone `Array` spreads, and a wrong count is padded
    /// or dropped rather than refused.
    Loose,
}

/// How a new frame is tied to the frames a non-local exit has to find.
///
/// Passed as one value rather than three parameters because every caller of
/// [`push_frame`] has to decide all three together, and a body that is a block
/// gets two of them from the `Proc` rather than from the call.
#[derive(Debug, Clone, Copy)]
struct Links {
    id: u64,
    home: u64,
    breaks: u64,
    /// See [`Call::scope_default`].
    scope_default: ScopeDefault,
}

/// What one instruction did.
///
/// The interpreter's `match` returns this rather than acting on the loop
/// directly, so that everything Ruby would raise leaves as an ordinary `Err`
/// and exactly one place — the unwinder — decides where it lands.
enum Step {
    /// Carry on with the next instruction.
    Next,
    /// The outermost frame returned; this is the program's value.
    Done(Value),
    /// Control is leaving this instruction's frame abnormally.
    Unwind(Unwind),
}

/// A reason control is leaving a frame, travelling up the frame stack.
///
/// One enum for all four because they are the same mechanism: `ensure` bodies
/// run for every one of them, in the same order, and the only difference is
/// what stops the search.
#[derive(Debug, Clone, Copy)]
enum Unwind {
    /// A raised exception object. Stops at a `rescue` whose class list matches.
    Exception(Value),
    /// `throw tag, value`. Stops at the frame `catch` opened for that tag,
    /// compared by identity as Ruby does.
    Throw { tag: Value, value: Value },
    /// `break value`. Stops at the frame the block was passed to.
    Break { frame: u64, value: Value },
    /// `return value` from a block. Stops at the method it was written in.
    Return { frame: u64, value: Value },
    /// A jump inside one frame that has `ensure` bodies to run on the way.
    /// Stops at its own frame, at `target`, without popping it.
    Goto {
        frame: u64,
        target: usize,
        depth: usize,
        /// What `break` is carrying out, if anything.
        value: Option<Value>,
    },
}

// ---------------------------------------------------------------------------
// Fibers (#16)
// ---------------------------------------------------------------------------
//
// The interpreter never recurses on the Rust stack for a Ruby call: a call is a
// `Call` pushed onto `frames`, with its operands on `stack`. So a fiber is a
// second pair of those two vectors, and switching fibers is swapping which pair
// the loop is running. `resume`, `Fiber.yield`, `raise`, `kill` and `transfer`
// are natives that do the swap through the `&mut` vectors every native is
// handed; nothing above them knows a switch happened. A fiber whose last frame
// returns hands its value to whoever resumed it, and one whose exception
// reaches its last frame goes on unwinding there — `leave_fiber`.
//
// The vectors of a fiber that is not running live in `FiberTable`, per heap,
// so a fiber can be resumed by a later evaluation than the one that made it —
// the spec harness runs an example one statement at a time. They are traced by
// the collector through `FiberTable::each_root`.

/// The two vectors a fiber runs on.
#[derive(Default)]
pub(crate) struct Context {
    stack: Vec<Value>,
    frames: Vec<Call>,
}

enum FiberState {
    /// Made and not yet resumed: its block has not started.
    Created,
    /// Running now: its vectors are the loop's.
    Resumed,
    /// Stopped at `Fiber.yield`, or transferred away from, with its vectors here.
    Suspended(Context),
    /// It resumed another fiber, which holds its vectors as that one's resumer.
    Resuming,
    /// Its block finished, raised out, or it was killed.
    Terminated,
}

struct FiberEntry {
    /// The Ruby `Fiber` object.
    object: Value,
    block: Value,
    state: FiberState,
    /// Who to return to when this one yields or ends: their vectors, and which
    /// fiber they are (`None` for the root).
    resumer: Option<(Context, Option<usize>)>,
    /// Entered by `transfer` rather than `resume`: ending returns to the root,
    /// as CRuby's transferred fiber does, and it cannot then be resumed.
    transferred: bool,
}

/// Every fiber a heap has made, and which one is running.
#[derive(Default)]
pub(crate) struct FiberTable {
    entries: Vec<FiberEntry>,
    /// The running fiber; `None` is the root.
    current: Option<usize>,
    /// The root fiber's object, made the first time `Fiber.current` asks.
    root: Option<Value>,
    /// The root's vectors while a transferred-to fiber runs.
    root_parked: Option<Context>,
    /// Frame ids, handed out across evaluations so that a fiber resumed by a
    /// later one cannot share an id with a frame that one makes.
    frame_ids: u64,
}

impl FiberTable {
    /// Every value a parked fiber's vectors hold, for the collector.
    pub(crate) fn each_root(&self, mut f: impl FnMut(Value)) {
        let mut context = |context: &Context| {
            context.stack.iter().copied().for_each(&mut f);
            for frame in &context.frames {
                for value in [
                    frame.env,
                    frame.receiver,
                    frame.block,
                    frame.errinfo_on_entry,
                    frame.rescued.unwrap_or(Value::NIL),
                    frame.tag.unwrap_or(Value::NIL),
                ] {
                    f(value);
                }
                for parked in &frame.parked {
                    match *parked {
                        Parked::Value(value) => f(value),
                        Parked::Unwind(unwind) => unwind_roots(unwind, &mut f),
                    }
                }
            }
        };
        for entry in &self.entries {
            if let FiberState::Suspended(saved) = &entry.state {
                context(saved);
            }
            if let Some((saved, _)) = &entry.resumer {
                context(saved);
            }
        }
        if let Some(saved) = &self.root_parked {
            context(saved);
        }
        for entry in &self.entries {
            f(entry.object);
            f(entry.block);
        }
        if let Some(root) = self.root {
            f(root);
        }
    }

    /// Leave the root running and every fiber that was mid-switch dead: the
    /// end of an evaluation that stopped inside a fiber, where nothing is left
    /// to return to.
    fn abandon(&mut self) {
        if self.current.is_none() && self.root_parked.is_none() {
            return;
        }
        for entry in &mut self.entries {
            if matches!(entry.state, FiberState::Resumed | FiberState::Resuming) {
                entry.state = FiberState::Terminated;
                entry.resumer = None;
            }
        }
        self.current = None;
        self.root_parked = None;
    }
}

fn unwind_roots(unwind: Unwind, f: &mut impl FnMut(Value)) {
    match unwind {
        Unwind::Exception(value) => f(value),
        Unwind::Throw { tag, value } => {
            f(tag);
            f(value);
        }
        Unwind::Break { value, .. } | Unwind::Return { value, .. } => f(value),
        Unwind::Goto { value, .. } => {
            if let Some(value) = value {
                f(value);
            }
        }
    }
}

/// How a fiber's last frame went.
enum FiberExit {
    Value(Value),
    Unwind(Unwind),
}

/// The running fiber has no frames left: give control back to its resumer.
///
/// `Err(())` when the root is running — the caller's own end-of-evaluation
/// applies. Otherwise the resumer's vectors are back in the loop's hands, and
/// the answer is what to do there: nothing more (`None`, with the fiber's value
/// pushed as `resume`'s result) or an unwind to carry on with. An exception
/// keeps going in the resumer; `break` and `return` out of a fiber's block are
/// LocalJumpErrors there, and a kill — a throw tagged with the fiber itself —
/// answers the fiber. All measured.
fn leave_fiber(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    exit: FiberExit,
) -> Result<Option<Unwind>, ()> {
    let Some(id) = scope.fibers().current else {
        return Err(());
    };
    let fibers = scope.fibers_mut();
    let entry = &mut fibers.entries[id];
    entry.state = FiberState::Terminated;
    let object = entry.object;
    let back = entry.resumer.take();
    let transferred = entry.transferred;
    // A fiber with a resumer goes back to it, however it was entered since.
    let _ = transferred;
    let (context, resumer) = match back {
        Some(back) => back,
        // A transferred fiber that ends goes back to the root — and when the
        // root is part-way through resuming a fiber that then transferred
        // away, to that fiber: measured, `a = Fiber.new { b.transfer }`
        // resumed from the root has `b`'s last value come back in `a`.
        _ => {
            let resumed_by_root = fibers.entries.iter().position(|e| {
                matches!(e.state, FiberState::Suspended(_))
                    && e.transferred
                    && matches!(e.resumer, Some((_, None)))
            });
            match resumed_by_root {
                Some(owner) => {
                    let FiberState::Suspended(context) =
                        std::mem::replace(&mut fibers.entries[owner].state, FiberState::Resumed)
                    else {
                        unreachable!("matched as suspended above")
                    };
                    fibers.entries[owner].transferred = false;
                    (context, Some(owner))
                }
                None => (fibers.root_parked.take().unwrap_or_default(), None),
            }
        }
    };
    *stack = context.stack;
    *frames = context.frames;
    fibers.current = resumer;
    if let Some(resumer) = resumer {
        fibers.entries[resumer].state = FiberState::Resumed;
    }
    // Nothing to return to — every fiber above was abandoned — is the end of
    // the evaluation, as the root's own last frame would be.
    if frames.is_empty() {
        return Err(());
    }
    match exit {
        FiberExit::Value(value) => {
            stack.push(value);
            Ok(None)
        }
        FiberExit::Unwind(Unwind::Throw { tag, .. }) if tag == object => {
            stack.push(object);
            Ok(None)
        }
        FiberExit::Unwind(Unwind::Exception(exception)) => Ok(Some(Unwind::Exception(exception))),
        FiberExit::Unwind(Unwind::Break { .. }) => Ok(Some(Unwind::Exception(exception_new(
            scope,
            "LocalJumpError",
            "break from proc-closure",
        )))),
        FiberExit::Unwind(Unwind::Return { .. }) => Ok(Some(Unwind::Exception(exception_new(
            scope,
            "LocalJumpError",
            "unexpected return",
        )))),
        FiberExit::Unwind(Unwind::Throw { tag, .. }) => {
            let message = format!("uncaught throw {}", inspect(scope, tag));
            Ok(Some(Unwind::Exception(exception_new(
                scope,
                "UncaughtThrowError",
                &message,
            ))))
        }
        FiberExit::Unwind(Unwind::Goto { .. }) => {
            unreachable!("a goto never leaves its own frame")
        }
    }
}

/// The `String` and `Encoding` primitives (#19). See [`StrOp`].
///
/// Thin on purpose: these read and write bytes and encoding indexes, and
/// `core/string.rb` and `core/encoding.rb` do the argument conversion, the
/// frozen checks and every rule about encodings.
fn str_native(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    call: &Pending,
    op: crate::method::StrOp,
) -> Result<Option<Unwind>, Error> {
    use crate::method::StrOp;
    use crate::strings;
    let arg = |index: usize| call.args.get(index).copied().unwrap_or(Value::NIL);
    let index_arg = |index: usize| -> Result<i64, Error> {
        arg(index).as_fixnum().ok_or(Error::NoDispatch {
            op: "a String primitive",
            operands: "an index that is not an Integer",
        })
    };
    if op == StrOp::NeedsCharTable {
        return Err(unknown_encoding("a character in this encoding"));
    }
    if op == StrOp::NeedsPointers {
        return Err(Error::Unknowable {
            what: "`pack`/`unpack` with `p` or `P`, which move raw pointers",
            needs: "a foreign-memory API, which is `Spinel::FFI`'s (#105)",
        });
    }
    if op == StrOp::FloatBits || op == StrOp::FloatFromBits {
        let wide = index_arg(0)? == 64;
        let value = if op == StrOp::FloatBits {
            let Some(f) = call.receiver.as_flonum() else {
                return Err(Error::NoDispatch {
                    op: "Float#__bits__",
                    operands: "a Float that is not an immediate",
                });
            };
            let bits = if wide {
                f.to_bits()
            } else {
                u64::from((f as f32).to_bits())
            };
            crate::bignum::value(scope, &num_bigint::BigInt::from(bits))
        } else {
            let bits = match call.receiver.as_fixnum() {
                Some(n) => n as u64,
                None => crate::bignum::read(scope, call.receiver)
                    .and_then(|big| u64::try_from(big).ok())
                    .ok_or(Error::NoDispatch {
                        op: "Integer#__float_from_bits__",
                        operands: "bits that are not an unsigned 64-bit Integer",
                    })?,
            };
            let f = if wide {
                f64::from_bits(bits)
            } else {
                f64::from(f32::from_bits(bits as u32))
            };
            Value::flonum(f).ok_or(Error::Unknowable {
                what: "a Float that needs the heap — NaN, Infinity, -0.0 or an extreme",
                needs: "boxed Floats (#18)",
            })?
        };
        stack.push(value);
        return Ok(None);
    }
    if op == StrOp::FloatFormat {
        let Some(f) = call.receiver.as_flonum() else {
            return Err(Error::NoDispatch {
                op: "Float#__format__",
                operands: "a Float that is not an immediate",
            });
        };
        let conversion = index_arg(0)?;
        let precision = usize::try_from(index_arg(1)?).unwrap_or(6);
        let alternate = arg(2).is_truthy();
        let Some(text) =
            crate::strings::format_float(f.abs(), conversion as u8, precision, alternate)
        else {
            return Err(Error::NoDispatch {
                op: "format",
                operands: "a float conversion other than f, e and g",
            });
        };
        let value = string_bytes_in(
            scope,
            Builtin::String.id(),
            text.as_bytes(),
            crate::strings::US_ASCII,
        );
        stack.push(value);
        return Ok(None);
    }
    // The install primitive is asked of `Encoding`, not of a string.
    if op == StrOp::EncodingInstall {
        let Some(class_id) = class_id_of(scope, call.receiver) else {
            return Err(Error::NoDispatch {
                op: "__encoding_install__",
                operands: "a receiver that is not Encoding",
            });
        };
        let class = scope.root(call.receiver);
        let mut list = Vec::with_capacity(crate::encoding_table::ENCODINGS.len());
        for (index, &(names, compatible, dummy)) in
            crate::encoding_table::ENCODINGS.iter().enumerate()
        {
            let mut name_values = Vec::with_capacity(names.len());
            for name in names {
                let value = string_bytes_in(
                    scope,
                    Builtin::String.id(),
                    name.as_bytes(),
                    crate::strings::US_ASCII,
                );
                let handle = scope.root(value);
                scope.freeze(handle);
                name_values.push(value);
            }
            let names_array = new_array(scope, &name_values);
            let names_handle = scope.root(names_array);
            scope.freeze(names_handle);
            let object = alloc_ivar_object(scope, Some(class));
            let object = scope.get(object);
            let index = Value::fixnum(index as i64).expect("an index is a fixnum");
            ivar_set(scope, object, symbol("@__index__"), index)?;
            ivar_set(scope, object, symbol("@__names__"), names_array)?;
            ivar_set(
                scope,
                object,
                symbol("@__ascii_compatible__"),
                bool_value(compatible),
            )?;
            ivar_set(scope, object, symbol("@__dummy__"), bool_value(dummy))?;
            let handle = scope.root(object);
            scope.freeze(handle);
            list.push(object);
        }
        let list_value = new_array(scope, &list);
        let list_handle = scope.root(list_value);
        scope.freeze(list_handle);
        scope
            .classes_mut()
            .const_set(class_id, symbol("LIST"), list_value);
        for &(name, index) in crate::encoding_table::CONSTANTS {
            scope
                .classes_mut()
                .const_set(class_id, symbol(name), list[usize::from(index)]);
        }
        stack.push(Value::NIL);
        return Ok(None);
    }
    if !is_string(scope, call.receiver) {
        return Err(Error::NoDispatch {
            op: "a String primitive",
            operands: "a receiver that is not a String",
        });
    }
    let receiver = scope.root(call.receiver);
    let bytes = strings::bytes(scope, receiver);
    let encoding = strings::encoding(scope, receiver);
    let fixnum = |n: i64| Value::fixnum(n).expect("a string offset fits a fixnum");
    let value = match op {
        StrOp::EncodingIndex => fixnum(i64::from(encoding)),
        StrOp::ForceEncoding => {
            let index = u8::try_from(index_arg(0)?).map_err(|_| Error::NoDispatch {
                op: "force_encoding",
                operands: "an encoding index out of range",
            })?;
            strings::set_encoding(scope, receiver, index);
            call.receiver
        }
        StrOp::Splice => {
            let start = usize::try_from(index_arg(0)?).unwrap_or(0);
            let len = usize::try_from(index_arg(1)?).unwrap_or(0);
            let Some(with) = string_bytes(scope, arg(2)) else {
                return Err(Error::NoDispatch {
                    op: "String splice",
                    operands: "a replacement that is not a String",
                });
            };
            strings::splice(scope, receiver, start, len, &with);
            call.receiver
        }
        StrOp::GetByte => {
            let index = index_arg(0)?;
            let index = if index < 0 {
                index + bytes.len() as i64
            } else {
                index
            };
            usize::try_from(index)
                .ok()
                .and_then(|i| bytes.get(i))
                .map_or(Value::NIL, |&b| fixnum(i64::from(b)))
        }
        StrOp::SetByte => {
            let index = usize::try_from(index_arg(0)?).unwrap_or(usize::MAX);
            let byte = index_arg(1)?;
            if index >= bytes.len() {
                return Err(Error::NoDispatch {
                    op: "String setbyte",
                    operands: "an index the Ruby side did not check",
                });
            }
            strings::splice(scope, receiver, index, 1, &[(byte & 0xff) as u8]);
            arg(1)
        }
        StrOp::ByteSlice => {
            let start = usize::try_from(index_arg(0)?).unwrap_or(0).min(bytes.len());
            let len = usize::try_from(index_arg(1)?).unwrap_or(0);
            let end = (start + len).min(bytes.len());
            string_bytes_in(scope, Builtin::String.id(), &bytes[start..end], encoding)
        }
        StrOp::Bytes => {
            let items: Vec<Value> = bytes.iter().map(|&b| fixnum(i64::from(b))).collect();
            new_array(scope, &items)
        }
        StrOp::ValidEncoding => match strings::valid(encoding, &bytes) {
            Some(valid) => bool_value(valid),
            None => return Err(unknown_encoding("`valid_encoding?`")),
        },
        StrOp::AsciiOnly => bool_value(strings::ascii_compatible(encoding) && bytes.is_ascii()),
        StrOp::CharOffsets => match strings::char_offsets(encoding, &bytes) {
            Some(offsets) => {
                let items: Vec<Value> = offsets.iter().map(|&o| fixnum(o as i64)).collect();
                new_array(scope, &items)
            }
            None => return Err(unknown_encoding("a character operation")),
        },
        StrOp::ByteIndex | StrOp::ByteRindex => {
            let Some(needle) = string_bytes(scope, arg(0)) else {
                return Err(Error::NoDispatch {
                    op: "String search",
                    operands: "a needle that is not a String",
                });
            };
            let start = usize::try_from(index_arg(1)?).unwrap_or(0);
            let found = if op == StrOp::ByteIndex {
                crate::strings::find(&bytes, &needle, start)
            } else {
                crate::strings::rfind(&bytes, &needle, start)
            };
            found.map_or(Value::NIL, |at| fixnum(at as i64))
        }
        StrOp::Transcode => {
            let source = u8::try_from(index_arg(0)?).unwrap_or(0);
            let destination = u8::try_from(index_arg(1)?).unwrap_or(0);
            let start = usize::try_from(index_arg(2)?).unwrap_or(0).min(bytes.len());
            if !crate::transcode::supported(source) || !crate::transcode::supported(destination) {
                stack.push(Value::NIL);
                return Ok(None);
            }
            let step = crate::transcode::step(source, destination, &bytes, start);
            let (stop, code) = match step.stop {
                crate::transcode::Stop::Done => ("done", None),
                crate::transcode::Stop::Invalid => ("invalid", None),
                crate::transcode::Stop::Incomplete => ("incomplete", None),
                crate::transcode::Stop::Undefined(code) => ("undefined", code),
            };
            let output = string_bytes_in(scope, Builtin::String.id(), &step.output, destination);
            let error = string_bytes_in(
                scope,
                Builtin::String.id(),
                &step.error,
                crate::strings::BINARY,
            );
            let readagain = string_bytes_in(
                scope,
                Builtin::String.id(),
                &step.readagain,
                crate::strings::BINARY,
            );
            let code = code.map_or(Value::NIL, |c| fixnum(i64::from(c)));
            new_array(
                scope,
                &[
                    output,
                    Value::symbol(symbol(stop)),
                    error,
                    readagain,
                    fixnum(step.next as i64),
                    code,
                ],
            )
        }
        StrOp::Succ => {
            let Some(next) = strings::succ(encoding, &bytes) else {
                return Err(unknown_encoding("`succ`"));
            };
            string_bytes_in(scope, Builtin::String.id(), &next, encoding)
        }
        StrOp::CaseMap => {
            let kind = u8::try_from(index_arg(0)?).unwrap_or(0);
            let ascii_only = arg(1).is_truthy();
            let turkic = arg(2).is_truthy();
            let Some(mapped) = strings::case_map(encoding, &bytes, kind, ascii_only, turkic) else {
                return Err(unknown_encoding("case mapping"));
            };
            string_bytes_in(scope, Builtin::String.id(), &mapped, encoding)
        }
        StrOp::Compatible => {
            let other = arg(0);
            let (Some(other_bytes), Some(other_enc)) =
                (string_bytes(scope, other), string_encoding(scope, other))
            else {
                return Err(Error::NoDispatch {
                    op: "Encoding.compatible?",
                    operands: "an argument that is not a String",
                });
            };
            strings::compatible((encoding, &bytes), (other_enc, &other_bytes))
                .map_or(Value::NIL, |index| fixnum(i64::from(index)))
        }
        StrOp::EncodingInstall
        | StrOp::FloatFormat
        | StrOp::NeedsCharTable
        | StrOp::NeedsPointers
        | StrOp::FloatBits
        | StrOp::FloatFromBits => {
            unreachable!("answered above")
        }
    };
    stack.push(value);
    Ok(None)
}

/// The refusal for a character operation in an encoding whose boundaries this
/// VM does not know: a wrong character count would be worse than none.
fn unknown_encoding(what: &'static str) -> Error {
    Error::Unknowable {
        what,
        needs: "character boundaries for an encoding other than UTF-8, US-ASCII and BINARY (#19)",
    }
}

/// `Encoding::CompatibilityError`'s message: CRuby names BINARY as
/// `BINARY (ASCII-8BIT)` there, as `inspect` does.
fn incompatible_encodings(left: u8, right: u8) -> Error {
    let show = |index: u8| {
        if index == crate::strings::BINARY {
            "BINARY (ASCII-8BIT)".to_owned()
        } else {
            crate::strings::name(index).to_owned()
        }
    };
    Error::raise(
        "Encoding::CompatibilityError",
        format!(
            "incompatible character encodings: {} and {}",
            show(left),
            show(right)
        ),
    )
}

/// The class-table reads and writes reflection is built on (#28). Every
/// argument check and message is `core/module.rb`'s; these trust their input.
fn reflect_native(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    call: &Pending,
    op: ReflectOp,
) -> Result<Option<Unwind>, Error> {
    let arg = |index: usize| call.args.get(index).copied().unwrap_or(Value::NIL);
    let module_arg = |scope: &mut HandleScope<'_>, index: usize| {
        class_id_of(scope, arg(index)).ok_or(Error::NoDispatch {
            op: "reflection",
            operands: "a receiver that is not a Module",
        })
    };
    let symbol_arg = |index: usize| {
        arg(index).as_symbol().ok_or(Error::NoDispatch {
            op: "reflection",
            operands: "a name that is not a Symbol",
        })
    };
    let value = match op {
        ReflectOp::ConstLookup => {
            let module = module_arg(scope, 0)?;
            let name = symbol_arg(1)?;
            // `:qualified` is `A::B`'s walk — the ancestors without `Object`'s
            // fallback — which `const_get` uses past a path's first segment.
            if arg(2).as_symbol() == Some(symbol("qualified")) {
                let found = scope.classes().const_get_qualified(module, name);
                let value = match found {
                    Some(value) => new_array(scope, &[value]),
                    None => Value::NIL,
                };
                stack.push(value);
                return Ok(None);
            }
            let inherit = arg(2).is_truthy();
            let mut found = scope.classes().const_get_here(module, name);
            if found.is_none() && inherit {
                // The ancestors, and then — for a module, which has no
                // `Object` in its chain — `Object`'s: `Comparable.const_get
                // (:String)` is String. Measured.
                for ancestor in scope.classes().ancestors(module) {
                    if let Some(value) = scope.classes().const_get_here(ancestor, name) {
                        found = Some(value);
                        break;
                    }
                }
                if found.is_none() && scope.classes().kind(module) == Kind::Module {
                    for ancestor in scope.classes().ancestors(Builtin::Object.id()) {
                        if let Some(value) = scope.classes().const_get_here(ancestor, name) {
                            found = Some(value);
                            break;
                        }
                    }
                }
            }
            match found {
                Some(value) => new_array(scope, &[value]),
                None => Value::NIL,
            }
        }
        ReflectOp::ConstSet => {
            let module = module_arg(scope, 0)?;
            let name = symbol_arg(1)?;
            let value = arg(2);
            scope.classes_mut().const_set(module, name, value);
            // `m.const_set(:X, Module.new)` names the module, as `X = ...` would.
            name_if_anonymous(scope, module, name, value);
            value
        }
        ReflectOp::ConstNames => {
            let module = module_arg(scope, 0)?;
            let inherit = arg(1).is_truthy();
            let mut tables = vec![module];
            if inherit {
                // Superclasses and mixins, stopping at `Object` when the walk
                // only passes through it — so neither its table nor
                // `Kernel`'s and `BasicObject`'s past it. CRuby's
                // `rb_mod_const_of`, measured.
                for ancestor in scope.classes().ancestors(module) {
                    if ancestor == Builtin::Object.id() && module != ancestor {
                        break;
                    }
                    if ancestor != module {
                        tables.push(ancestor);
                    }
                }
            }
            let mut names: Vec<SymbolId> = Vec::new();
            for table in tables {
                for (name, private) in scope.classes().const_names(table) {
                    if !private && !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
            let names: Vec<Value> = names.into_iter().map(Value::symbol).collect();
            new_array(scope, &names)
        }
        ReflectOp::ConstRemove => {
            let module = module_arg(scope, 0)?;
            let name = symbol_arg(1)?;
            match scope.classes_mut().const_remove(module, name) {
                Some(value) => new_array(scope, &[value]),
                None => Value::NIL,
            }
        }
        ReflectOp::ConstPublic => {
            let module = module_arg(scope, 0)?;
            let name = symbol_arg(1)?;
            let held = scope.classes().const_get_here(module, name).is_some();
            scope.classes_mut().mark_const_public(module, name);
            bool_value(held)
        }
        ReflectOp::MethodNames => {
            let module = module_arg(scope, 0)?;
            let inherit = arg(1).is_truthy();
            let which = arg(2).as_fixnum().unwrap_or(0);
            let chain = if inherit {
                scope.classes().ancestors(module)
            } else {
                vec![module]
            };
            // The first definition along the chain decides: an `undef`, or a
            // subclass narrowing a method to private, hides what is further
            // up.
            let mut seen: Vec<SymbolId> = Vec::new();
            let mut names: Vec<Value> = Vec::new();
            for class in chain {
                let mut entries = scope.classes().own_method_entries(class);
                entries.sort_by_key(|&(name, _, _)| crate::shared::symbols::name(name));
                for (name, body, visibility) in entries {
                    if seen.contains(&name) {
                        continue;
                    }
                    seen.push(name);
                    if body == Value::UNDEF || is_core_helper(name) {
                        continue;
                    }
                    let wanted = match which {
                        1 => visibility == Visibility::Public,
                        2 => visibility == Visibility::Protected,
                        3 => visibility == Visibility::Private,
                        _ => visibility != Visibility::Private,
                    };
                    if wanted {
                        names.push(Value::symbol(name));
                    }
                }
            }
            new_array(scope, &names)
        }
        ReflectOp::ClassOf => {
            let class = class_of(scope, arg(0)).ok_or_else(|| no_class(arg(0)))?;
            scope.classes().object(class)
        }
        ReflectOp::SingletonClass => {
            let target = arg(0);
            if let Some(n) = target.as_fixnum() {
                let _ = n;
                return Err(Error::raise("TypeError", "can't define singleton"));
            }
            if target.as_symbol().is_some() || target.as_flonum().is_some() {
                return Err(Error::raise("TypeError", "can't define singleton"));
            }
            let class = singleton_of(scope, target)?;
            scope.classes().object(class)
        }
        ReflectOp::RemoveMethod => {
            let module = module_arg(scope, 0)?;
            let name = symbol_arg(1)?;
            let defined = scope.classes().method_defined_here(module, name);
            bool_value(defined && scope.classes_mut().remove_method(module, name))
        }
        ReflectOp::IsSingleton => {
            let module = module_arg(scope, 0)?;
            bool_value(scope.classes().is_singleton(module))
        }
        ReflectOp::Attached => {
            let module = module_arg(scope, 0)?;
            singleton_attached(scope, module)
        }
        ReflectOp::ModuleKind => {
            match class_id_of(scope, arg(0)).map(|id| scope.classes().kind(id)) {
                Some(Kind::Class) => Value::symbol(symbol("class")),
                Some(Kind::Module) => Value::symbol(symbol("module")),
                None => Value::NIL,
            }
        }
    };
    stack.push(value);
    Ok(None)
}

/// Which fiber a `Fiber` object is: its table index, or `None` for the root.
/// The index lives in the object's hidden `@__fiber__`; the root's is -1.
fn fiber_index(scope: &mut HandleScope<'_>, object: Value) -> Result<Option<usize>, Error> {
    let id = ivar_get(scope, object, symbol("@__fiber__"))?
        .as_fixnum()
        .ok_or_else(|| Error::raise("FiberError", "uninitialized fiber"))?;
    Ok(usize::try_from(id).ok())
}

/// What a `resume` or `Fiber.yield` hands across: nothing is nil, one value is
/// itself, several are an Array. Measured.
fn fiber_pack(scope: &mut HandleScope<'_>, args: &[Value]) -> Value {
    match args {
        [] => Value::NIL,
        [one] => *one,
        many => new_array(scope, many),
    }
}

/// The arguments a `core/fiber.rb` wrapper gathered into one Array.
fn fiber_args(scope: &mut HandleScope<'_>, call: &Pending, index: usize) -> Vec<Value> {
    call.args
        .get(index)
        .and_then(|&args| array_elements(scope, args))
        .unwrap_or_default()
}

/// Hand the loop fiber `id`'s vectors, keeping the current ones as its
/// resumer's. A fiber not yet started gets its block's frame, with `args` as
/// the block's arguments; a suspended one gets its own vectors back and `then`
/// decides what its `Fiber.yield` answers.
#[allow(clippy::too_many_arguments)]
fn fiber_enter(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    call: &Pending,
    id: usize,
    args: Vec<Value>,
    transfer: bool,
    ids: &mut u64,
) -> Result<bool, Error> {
    let saved = Context {
        stack: std::mem::take(stack),
        frames: std::mem::take(frames),
    };
    let fibers = scope.fibers_mut();
    let previous = fibers.current;
    if transfer {
        // The one transferring away is parked where a later transfer back can
        // find it: the root apart, a fiber keeps its own vectors.
        match previous {
            None => fibers.root_parked = Some(saved),
            Some(from) => {
                fibers.entries[from].state = FiberState::Suspended(saved);
                fibers.entries[from].transferred = true;
            }
        }
        fibers.entries[id].transferred = true;
    } else {
        if let Some(from) = previous {
            fibers.entries[from].state = FiberState::Resuming;
        }
        fibers.entries[id].resumer = Some((saved, previous));
    }
    fibers.current = Some(id);
    let state = std::mem::replace(&mut fibers.entries[id].state, FiberState::Resumed);
    match state {
        FiberState::Created => {
            let block = scope.fibers().entries[id].block;
            let inner = Pending {
                cache: None,
                name: call.name,
                receiver: block,
                args,
                keywords: Vec::new(),
                block: Value::NIL,
                block_is_literal: false,
                cref: call.cref,
                implicit_self: false,
                public_only: false,
                target: Target::Block(block),
                owner: None,
                defined_as: None,
            };
            push_proc_frame(scope, stack, frames, &inner, block, ids)?;
            Ok(true)
        }
        FiberState::Suspended(context) => {
            *stack = context.stack;
            *frames = context.frames;
            Ok(false)
        }
        _ => unreachable!("callers check the state before entering"),
    }
}

fn fiber_native(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    call: &Pending,
    op: FiberOp,
    ids: &mut u64,
) -> Result<Option<Unwind>, Error> {
    let fiber_error = |message: &str| Error::raise("FiberError", message.to_owned());
    match op {
        FiberOp::New => {
            let object = call.args.first().copied().unwrap_or(Value::NIL);
            let block = call.args.get(1).copied().unwrap_or(Value::NIL);
            let fibers = scope.fibers_mut();
            fibers.entries.push(FiberEntry {
                object,
                block,
                state: FiberState::Created,
                resumer: None,
                transferred: false,
            });
            let id = fibers.entries.len() - 1;
            let id = Value::fixnum(id as i64).expect("a fiber index is a fixnum");
            ivar_set(scope, object, symbol("@__fiber__"), id)?;
            stack.push(object);
            Ok(None)
        }

        FiberOp::Current => {
            let object = match scope.fibers().current {
                Some(id) => scope.fibers().entries[id].object,
                None => match scope.fibers().root {
                    Some(root) => root,
                    None => {
                        // The root fiber is an ordinary `Fiber`, made the first
                        // time anything asks for it.
                        let class = scope
                            .classes()
                            .const_get_here(Builtin::Object.id(), symbol("Fiber"))
                            .expect("core/fiber.rb defines Fiber");
                        let class = scope.root(class);
                        let handle = alloc_ivar_object(scope, Some(class));
                        let root = scope.get(handle);
                        let minus_one = Value::fixnum(-1).expect("-1 is a fixnum");
                        ivar_set(scope, root, symbol("@__fiber__"), minus_one)?;
                        scope.fibers_mut().root = Some(root);
                        root
                    }
                },
            };
            stack.push(object);
            Ok(None)
        }

        FiberOp::Alive | FiberOp::Status => {
            let object = call.args.first().copied().unwrap_or(Value::NIL);
            let state = match fiber_index(scope, object)? {
                None => "resumed",
                Some(id) => match scope.fibers().entries[id].state {
                    FiberState::Created => "created",
                    FiberState::Resumed | FiberState::Resuming => "resumed",
                    FiberState::Suspended(_) => "suspended",
                    FiberState::Terminated => "terminated",
                },
            };
            let answer = if op == FiberOp::Alive {
                bool_value(state != "terminated")
            } else {
                Value::symbol(crate::shared::symbols::intern(state))
            };
            stack.push(answer);
            Ok(None)
        }

        FiberOp::Resume | FiberOp::Transfer => {
            let object = call.args.first().copied().unwrap_or(Value::NIL);
            let args = fiber_args(scope, call, 1);
            let transfer = op == FiberOp::Transfer;
            let Some(id) = fiber_index(scope, object)? else {
                // The root: resuming it is always an error; transferring to it
                // goes back to it, which is how a transferred fiber returns.
                if !transfer {
                    return Err(fiber_error(if scope.fibers().current.is_none() {
                        "attempt to resume the current fiber"
                    } else {
                        "attempt to resume a resuming fiber"
                    }));
                }
                let Some(context) = scope.fibers_mut().root_parked.take() else {
                    let value = fiber_pack(scope, &args);
                    stack.push(value);
                    return Ok(None);
                };
                let saved = Context {
                    stack: std::mem::take(stack),
                    frames: std::mem::take(frames),
                };
                let fibers = scope.fibers_mut();
                if let Some(from) = fibers.current {
                    fibers.entries[from].state = FiberState::Suspended(saved);
                    fibers.entries[from].transferred = true;
                }
                fibers.current = None;
                *stack = context.stack;
                *frames = context.frames;
                let value = fiber_pack(scope, &args);
                stack.push(value);
                return Ok(None);
            };
            let entry = &scope.fibers().entries[id];
            match (&entry.state, transfer) {
                (FiberState::Terminated, false) => {
                    return Err(fiber_error("attempt to resume a terminated fiber"));
                }
                (FiberState::Terminated, true) => {
                    return Err(fiber_error("attempt to transfer to a terminated fiber"));
                }
                (FiberState::Resumed, false) => {
                    return Err(fiber_error("attempt to resume the current fiber"));
                }
                (FiberState::Resuming, false) => {
                    return Err(fiber_error("attempt to resume a resuming fiber"));
                }
                (FiberState::Resuming, true) => {
                    return Err(fiber_error("attempt to transfer to a resuming fiber"));
                }
                (FiberState::Suspended(_), false) if entry.transferred => {
                    return Err(fiber_error("attempt to resume a transferring fiber"));
                }
                (FiberState::Suspended(_), true) if !entry.transferred => {
                    return Err(fiber_error("attempt to transfer to a yielding fiber"));
                }
                (FiberState::Resumed, true) => {
                    // Transferring to the running fiber hands the value straight
                    // back.
                    let value = fiber_pack(scope, &args);
                    stack.push(value);
                    return Ok(None);
                }
                _ => {}
            }
            let started = fiber_enter(scope, stack, frames, call, id, args.clone(), transfer, ids)?;
            if !started {
                let value = fiber_pack(scope, &args);
                stack.push(value);
            }
            Ok(None)
        }

        FiberOp::Yield => {
            let args = fiber_args(scope, call, 0);
            let Some(id) = scope.fibers().current else {
                return Err(fiber_error("attempt to yield on a not resumed fiber"));
            };
            if scope.fibers().entries[id].resumer.is_none() {
                return Err(fiber_error("attempt to yield on a not resumed fiber"));
            }
            let value = fiber_pack(scope, &args);
            let suspended = Context {
                stack: std::mem::take(stack),
                frames: std::mem::take(frames),
            };
            let fibers = scope.fibers_mut();
            let entry = &mut fibers.entries[id];
            entry.state = FiberState::Suspended(suspended);
            // Suspended by `yield`, whatever entered it: a transfer to it is now
            // a FiberError, measured.
            entry.transferred = false;
            let (context, previous) = entry.resumer.take().expect("checked above");
            *stack = context.stack;
            *frames = context.frames;
            fibers.current = previous;
            if let Some(previous) = previous {
                fibers.entries[previous].state = FiberState::Resumed;
            }
            stack.push(value);
            Ok(None)
        }

        FiberOp::Raise | FiberOp::Kill => {
            let object = call.args.first().copied().unwrap_or(Value::NIL);
            let id = fiber_index(scope, object)?;
            let running_here = id == scope.fibers().current;
            // The unwind the fiber is entered with: the exception `raise`
            // builds, or for `kill` a throw tagged with the fiber itself, which
            // runs every `ensure` on the way out and no `rescue`.
            let unwind = if op == FiberOp::Raise {
                let args = fiber_args(scope, call, 1);
                let args = if args.is_empty() {
                    vec![string_new(scope, "unhandled exception")]
                } else {
                    args
                };
                Unwind::Exception(raise_argument(scope, frames, &args)?)
            } else {
                Unwind::Throw {
                    tag: object,
                    value: object,
                }
            };
            if running_here {
                return Ok(Some(unwind));
            }
            let Some(id) = id else {
                return Err(fiber_error("attempt to resume a resuming fiber"));
            };
            match scope.fibers().entries[id].state {
                FiberState::Created if op == FiberOp::Kill => {
                    scope.fibers_mut().entries[id].state = FiberState::Terminated;
                    stack.push(object);
                    return Ok(None);
                }
                FiberState::Created => {
                    return Err(fiber_error("cannot raise exception on unborn fiber"));
                }
                FiberState::Terminated if op == FiberOp::Kill => {
                    stack.push(object);
                    return Ok(None);
                }
                FiberState::Terminated => {
                    return Err(fiber_error("attempt to resume a terminated fiber"));
                }
                FiberState::Resumed | FiberState::Suspended(_) | FiberState::Resuming => {}
            }
            // A fiber that is resuming another is reached through the fiber
            // it resumed, and that one's, to the innermost: measured, raising
            // on a parent from its child is the child's own `raise`.
            let mut target = id;
            while let Some(next) = scope
                .fibers()
                .entries
                .iter()
                .position(|e| matches!(e.resumer, Some((_, Some(r))) if r == target))
            {
                if !matches!(scope.fibers().entries[next].state, FiberState::Terminated) {
                    target = next;
                } else {
                    break;
                }
            }
            if Some(target) == scope.fibers().current {
                return Ok(Some(unwind));
            }
            if !matches!(
                scope.fibers().entries[target].state,
                FiberState::Suspended(_)
            ) {
                return Err(fiber_error("attempt to resume a resuming fiber"));
            }
            let transfer = scope.fibers().entries[target].transferred;
            fiber_enter(
                scope,
                stack,
                frames,
                call,
                target,
                Vec::new(),
                transfer,
                ids,
            )?;
            Ok(Some(unwind))
        }
    }
}

/// A reason an `ensure` body was entered, parked until it finishes.
#[derive(Debug, Clone, Copy)]
enum Parked {
    /// The normal path: the protected body's value, to push back afterwards.
    Value(Value),
    /// An unwind in flight, to resume afterwards.
    Unwind(Unwind),
}

/// Run `iseq` in `frame`, which may already hold locals from an earlier run.
pub fn eval_in(
    scope: &mut HandleScope<'_>,
    frame: &mut Frame,
    iseq: &Iseq,
) -> Result<Value, Error> {
    frame.reserve(iseq.locals.len());
    let env = frame.env(scope);
    if frame.receiver == Value::NIL {
        // `main`. A plain `Object`, as Ruby has it: `def` at the top level
        // lands on its class, a receiverless call finds it there, and `@a` at
        // the top level is an ivar on this object like any other. One per
        // heap, kept by it.
        if scope.main() == Value::NIL {
            let object = class_handle(scope, Builtin::Object);
            let handle = alloc_ivar_object(scope, Some(object));
            let main = scope.get(handle);
            scope.set_main(main);
        }
        frame.receiver = scope.main();
    }

    // Rooted once rather than per allocation: `alloc` needs a handle to the
    // class, and taking one inside the loop would grow the root stack by an
    // entry per literal.
    let string_class = class_handle(scope, Builtin::String);
    let proc_class = class_handle(scope, Builtin::Proc);

    let mut stack: Vec<Value> = Vec::with_capacity(iseq.max_stack);
    // Owned before the frame is built so the call caches can key off the same
    // `Arc` the frame holds.
    let script = Arc::new(iseq.clone());
    let cache_base = scope.call_caches_mut().base(&script);
    let mut frames: Vec<Call> = vec![Call {
        iseq: script,
        symbols: iseq.link(),
        cache_base,
        env,
        receiver: frame.receiver,
        cref: frame.cref,
        block: Value::NIL,
        pc: 0,
        base: 0,
        keeps_receiver: false,
        raises_receiver: false,
        discards_value: false,
        booleanizes_value: false,
        // A `def` at a script's top level is a *private* instance method of
        // `Object` — `def m; end; Object.new.m` raises. CRuby gives the top
        // level cref `METHOD_VISI_PRIVATE`, and this is that (#161).
        scope_default: ScopeDefault::Private,
        // The outermost frame is its own `return` target and has no `break`
        // target: `return` at the top level ends the script, and `break` there
        // has no call to end.
        id: 1,
        home: 1,
        breaks: 0,
        tag: None,
        rescued: None,
        errinfo_on_entry: Value::NIL,
        // The top level is not a method body, so `super` there has no owner to
        // start from and raises rather than resolving to something.
        owner: None,
        defined_as: None,
        parked: Vec::new(),
        boundary_limit: 0,
    }];
    // `u64::MAX` is no budget at all: what `spinel run` asks for, since a
    // program is allowed to run for as long as it runs.
    let mut budget = scope.budget().unwrap_or(u64::MAX);
    // Frame ids, handed out in order. Zero means "no such frame", which is what
    // a body with nowhere to `break` to carries.
    // Frame ids continue from the last evaluation: a fiber resumed here may
    // hold frames an earlier one made, and an id must name one frame.
    scope.fibers_mut().abandon();
    let mut ids: u64 = scope.fibers().frame_ids.max(1);
    frames[0].id = ids;
    frames[0].home = ids;

    // The refusal boundaries on the stack, innermost last, as
    // `(frame index, frame id, budget deadline)`. Refreshed only when the frame
    // count changes, so the instruction loop pays one comparison for them.
    let mut boundaries: Vec<Boundary> = Vec::new();
    let mut seen_frames = frames.len();
    let mut deadline: u64 = 0;

    let result = (|| -> Result<Value, Error> {
        Ok(loop {
            if frames.len() != seen_frames {
                seen_frames = frames.len();
                deadline = refresh_boundaries(scope, &frames, &mut boundaries, budget);
            }
            budget = budget.saturating_sub(1);
            if budget <= deadline {
                // Out of budget: the whole evaluation's, or a boundary's.
                let unwind = refuse(
                    scope,
                    &mut stack,
                    &mut frames,
                    &mut boundaries,
                    Error::Budget,
                )?;
                seen_frames = usize::MAX;
                if let Some(value) = unwind_to_handler(scope, &mut stack, &mut frames, unwind)? {
                    break value;
                }
                continue;
            }
            let top = frames.len() - 1;
            let insn = frames[top].iseq.insns[frames[top].pc];
            frames[top].pc += 1;

            // One instruction, run in a closure so that `?` still reads as it did
            // before there was an unwinder. Everything Ruby would raise leaves here
            // as an ordinary `Err`, and exactly one place below decides where it
            // lands — which is what makes `1 / 0` inside a `begin` a catchable
            // `ZeroDivisionError` rather than the end of the evaluation.
            let stepped = (|| -> Result<Step, Error> {
                match insn {
                    Insn::PushNil => stack.push(Value::NIL),
                    Insn::PushTrue => stack.push(Value::TRUE),
                    Insn::PushFalse => stack.push(Value::FALSE),
                    Insn::PushSelf => stack.push(frames[top].receiver),
                    Insn::PushInt(n) => {
                        stack.push(Value::fixnum(n).ok_or(Error::NoDispatch {
                            op: "Integer",
                            operands: "a value wider than a fixnum",
                        })?);
                    }
                    Insn::PushLit(index) => {
                        let literal = frames[top].iseq.literals[index as usize].clone();
                        let value = materialise(scope, &literal, string_class)?;
                        stack.push(value);
                    }
                    Insn::PushSym(index) => {
                        let symbol = frames[top].symbols[index as usize];
                        stack.push(Value::symbol(symbol));
                    }
                    Insn::Intern => {
                        let value = stack.pop().expect("a string to intern");
                        // The parts were joined by `String#+`, so the only way this
                        // is not a String is a program that redefined `+` to return
                        // something else. CRuby raises `TypeError` there too.
                        let Some(bytes) = string_bytes(scope, value) else {
                            return Err(Error::raise("TypeError", "can't convert to Symbol"));
                        };
                        // ponytail: the symbol table is `String`-keyed, so a symbol
                        // whose bytes are not UTF-8 cannot be interned. Ruby allows
                        // one; reaching it needs binary string operations that wait
                        // on the Encoding slice, and raising beats interning
                        // something the program did not write. Upgrade with the
                        // table: key it by bytes when `Encoding` lands.
                        let Ok(name) = String::from_utf8(bytes) else {
                            return Err(Error::raise(
                                "ArgumentError",
                                "a symbol that is not UTF-8 waits for the Encoding class",
                            ));
                        };
                        stack.push(Value::symbol(crate::shared::symbols::intern(&name)));
                    }

                    Insn::Pop => {
                        stack.pop();
                    }
                    Insn::Dup => {
                        let value = *stack.last().expect("dup on an empty stack");
                        stack.push(value);
                    }

                    Insn::CaptureSplat => {
                        let value = stack.pop().expect("a splat to capture");
                        let captured = match array_elements(scope, value) {
                            Some(elements) => new_array(scope, &elements),
                            None => value,
                        };
                        stack.push(captured);
                    }

                    Insn::GetIvar(name) => {
                        let symbol = frames[top].symbols[name as usize];
                        let receiver = frames[top].receiver;
                        stack.push(ivar_get(scope, receiver, symbol)?);
                    }

                    Insn::SetIvar(name) => {
                        let symbol = frames[top].symbols[name as usize];
                        let receiver = frames[top].receiver;
                        let value = stack.pop().expect("setivar on an empty stack");
                        ivar_set(scope, receiver, symbol, value)?;
                    }

                    Insn::DefinedIvar(name) => {
                        let symbol = frames[top].symbols[name as usize];
                        let receiver = frames[top].receiver;
                        // `defined_word`, not `defined_answer`: an object this
                        // heap holds is the authority on which ivars it has, so a
                        // miss really is Ruby's `nil` and not #39's "never seen".
                        let held = ivar_defined(scope, receiver, symbol)?;
                        let value =
                            defined_word(scope, string_class, held.then_some("instance-variable"));
                        stack.push(value);
                    }

                    // Globals (#166). One table per heap; the regexp specials
                    // never reach here — the compiler sends those to
                    // `Insn::LastMatch`, so a name in this table is always one an
                    // assignment put there.
                    Insn::GetGlobal(name) => {
                        let symbol = frames[top].symbols[name as usize];
                        // A name aliased to a regexp special runs the derivation
                        // rather than reading a cell, so it tracks the *current*
                        // match: measured, a second `=~` moves it.
                        let value = match scope.global_special(symbol) {
                            Some(which) => last_match_part(scope, &which)?,
                            // An unset global reads `nil` rather than raising:
                            // Ruby warns under `-w` and answers nil, and the
                            // warning needs #39's `$stderr`.
                            None => scope.global(symbol).unwrap_or(Value::NIL),
                        };
                        stack.push(value);
                    }
                    Insn::SetGlobal(name) => {
                        let symbol = frames[top].symbols[name as usize];
                        // `alias $x $&` makes `$x` read-only, and Ruby names the
                        // alias rather than the special in the message. Measured.
                        if scope.global_special(symbol).is_some()
                            || scope.global_is_readonly(symbol)
                        {
                            return Err(Error::raise(
                                "NameError",
                                // `symbol_name` already carries the leading `$`.
                                format!("{} is a read-only variable", symbol_name(symbol)),
                            ));
                        }
                        // Pops, like `SetLocal` and `SetIvar`: the callers that
                        // want assignment to be an expression emit `Dup` first.
                        let value = stack.pop().expect("setglobal on an empty stack");
                        scope.set_global(symbol, value);
                    }
                    Insn::DefinedGlobal(name) => {
                        let symbol = frames[top].symbols[name as usize];
                        // Presence, not truthiness: `$a = nil` is defined.
                        let held = scope.global(symbol).is_some();
                        let value =
                            defined_word(scope, string_class, held.then_some("global-variable"));
                        stack.push(value);
                    }

                    Insn::GetLocal(slot, depth) => {
                        let env = env_outer(scope, frames[top].env, depth);
                        stack.push(env_get(scope, env, slot as usize));
                    }
                    Insn::SetLocal(slot, depth) => {
                        let value = stack.pop().expect("setlocal on an empty stack");
                        let env = env_outer(scope, frames[top].env, depth);
                        env_set(scope, env, slot as usize, value);
                    }

                    Insn::Jump(displacement) => frames[top].pc = jump(frames[top].pc, displacement),
                    Insn::Refuse(index) => {
                        return Err(Error::NotCompiled(
                            frames[top].iseq.refusals[index as usize],
                        ));
                    }
                    Insn::JumpUnless(displacement) => {
                        if !stack.pop().expect("jump on an empty stack").is_truthy() {
                            frames[top].pc = jump(frames[top].pc, displacement);
                        }
                    }
                    Insn::JumpIf(displacement) => {
                        if stack.pop().expect("jump on an empty stack").is_truthy() {
                            frames[top].pc = jump(frames[top].pc, displacement);
                        }
                    }
                    Insn::JumpUnlessKeep(displacement) => {
                        if !stack.last().expect("jump on an empty stack").is_truthy() {
                            frames[top].pc = jump(frames[top].pc, displacement);
                        }
                    }
                    Insn::JumpIfKeep(displacement) => {
                        if stack.last().expect("jump on an empty stack").is_truthy() {
                            frames[top].pc = jump(frames[top].pc, displacement);
                        }
                    }
                    Insn::JumpIfNilKeep(displacement) => {
                        if *stack.last().expect("jump on an empty stack") == Value::NIL {
                            frames[top].pc = jump(frames[top].pc, displacement);
                        }
                    }

                    Insn::BinOp(op) => {
                        let right = stack.pop().expect("binop on an empty stack");
                        let left = stack.pop().expect("binop on an empty stack");
                        // `1 == obj` for an object that is not a number: CRuby's
                        // `num_equal` asks the object, `obj == 1`, so a class's
                        // own `==` decides. Measured.
                        let swapped = op == BinOp::Eq
                            && !right.is_immediate()
                            && (num(left).is_some() || crate::bignum::is_big(scope, left))
                            && heap_kind(scope, right).is_none()
                            && !crate::bignum::is_big(scope, right);
                        let result = if swapped {
                            Err(Error::NoDispatch {
                                op: "==",
                                operands: "a number and an object that decides",
                            })
                        } else if !matches!(op, BinOp::Eq | BinOp::Neq)
                            && !operator_fast_path(scope, left, op as usize, &frames[top])
                        {
                            Err(Error::NoDispatch {
                                op: op.name(),
                                operands: "a number whose class redefined the operator",
                            })
                        } else {
                            binop(scope, op, left, right)
                        };
                        let (left, right) = if swapped {
                            (right, left)
                        } else {
                            (left, right)
                        };
                        match result {
                            Ok(value) => stack.push(value),
                            // The send behind the fast path. `BinOp`'s own docs have
                            // said since #10 that one belongs here and that #11's
                            // calling convention was what it waited for; #11 landed and
                            // nothing wired it up. An operand the fast path does not
                            // cover is an ordinary method call on the left-hand side,
                            // which is what the operator *is* in Ruby — and it is what
                            // lets `Array#+` and any user-defined operator work at all.
                            Err(Error::NoDispatch { .. }) => {
                                let call = Pending {
                                    // ponytail: `Insn::BinOp` has no call-site
                                    // index, so an operator that misses the fixnum
                                    // fast path pays the full probe. Give the
                                    // instruction a site when a benchmark shows
                                    // operator dispatch on user-defined classes
                                    // mattering.
                                    cache: None,
                                    name: crate::shared::symbols::intern(op.name()),
                                    receiver: left,
                                    args: vec![right],
                                    keywords: Vec::new(),
                                    block: Value::NIL,
                                    block_is_literal: false,
                                    cref: frames[top].cref,
                                    implicit_self: false,
                                    public_only: false,
                                    target: Target::Method,
                                    owner: None,
                                    defined_as: None,
                                };
                                if let Some(unwind) = dispatch(
                                    scope,
                                    &mut stack,
                                    &mut frames,
                                    call,
                                    proc_class,
                                    &mut ids,
                                )? {
                                    return Ok(Step::Unwind(unwind));
                                }
                            }
                            Err(other) => return Err(other),
                        }
                    }
                    Insn::Neg => {
                        let value = stack.pop().expect("neg on an empty stack");
                        let result = if operator_fast_path(scope, value, NEG_GUARD, &frames[top]) {
                            negate(scope, value)
                        } else {
                            Err(Error::NoDispatch {
                                op: "-@",
                                operands: "a number whose class redefined the operator",
                            })
                        };
                        match result {
                            Ok(negated) => stack.push(negated),
                            // The send behind the fast path, as `Insn::BinOp`
                            // has: `-x` is `x.-@`, for whatever `x` defines one.
                            Err(Error::NoDispatch { .. }) => {
                                let call = Pending {
                                    cache: None,
                                    name: crate::shared::symbols::intern("-@"),
                                    receiver: value,
                                    args: Vec::new(),
                                    keywords: Vec::new(),
                                    block: Value::NIL,
                                    block_is_literal: false,
                                    cref: frames[top].cref,
                                    implicit_self: false,
                                    public_only: false,
                                    target: Target::Method,
                                    owner: None,
                                    defined_as: None,
                                };
                                if let Some(unwind) = dispatch(
                                    scope,
                                    &mut stack,
                                    &mut frames,
                                    call,
                                    proc_class,
                                    &mut ids,
                                )? {
                                    return Ok(Step::Unwind(unwind));
                                }
                            }
                            Err(other) => return Err(other),
                        }
                    }
                    Insn::Not => {
                        let value = stack.pop().expect("not on an empty stack");
                        // `!x` is `x.!`, and a class may define one (#239). The
                        // instruction answers itself while the method a send
                        // would find is still `BasicObject#!`.
                        //
                        // ponytail: an immediate is answered without the
                        // lookup, so a `!` defined on `NilClass`, `Integer` or
                        // `Symbol` is not seen. Route those through the lookup
                        // too if a program turns out to define one.
                        let bang = crate::shared::symbols::intern("!");
                        let redefined = !value.is_immediate()
                            && class_of(scope, value).is_some_and(|class| {
                                scope
                                    .classes_mut()
                                    .lookup(class, bang)
                                    .is_some_and(|m| m.owner != Builtin::BasicObject.id())
                            });
                        if redefined {
                            let call = Pending {
                                cache: None,
                                name: bang,
                                receiver: value,
                                args: Vec::new(),
                                keywords: Vec::new(),
                                block: Value::NIL,
                                block_is_literal: false,
                                cref: frames[top].cref,
                                implicit_self: false,
                                public_only: false,
                                target: Target::Method,
                                owner: None,
                                defined_as: None,
                            };
                            if let Some(unwind) = dispatch(
                                scope,
                                &mut stack,
                                &mut frames,
                                call,
                                proc_class,
                                &mut ids,
                            )? {
                                return Ok(Step::Unwind(unwind));
                            }
                        } else {
                            stack.push(bool_value(!value.is_truthy()));
                        }
                    }

                    Insn::NewArray(count) => {
                        let at = stack.len() - count as usize;
                        let elements: Vec<Value> = stack.drain(at..).collect();
                        let value = new_array(scope, &elements);
                        stack.push(value);
                    }

                    // `$~ = m`. Writes the frame's match, which is what makes
                    // `$1` and `$&` follow it. `nil` clears; anything that is not
                    // a `MatchData` is Ruby's `TypeError`, measured, because the
                    // readers above would otherwise treat a plain object as a
                    // match and answer nonsense.
                    Insn::SetLastMatch => {
                        let value = stack.pop().expect("setlastmatch on an empty stack");
                        if value != Value::NIL && !is_match_data(scope, value) {
                            let got = class_name_of(scope, value);
                            return Err(Error::raise(
                                "TypeError",
                                format!("wrong argument type {got} (expected MatchData)"),
                            ));
                        }
                        scope.set_last_match(value);
                    }

                    // `$!` (#206). The read is the cell; the write is the
                    // compiler's own restore after a `begin` and never a Ruby
                    // assignment, which is why it takes any value rather than only
                    // an exception — `$! = e` is a `NameError` in Ruby.
                    Insn::Errinfo => {
                        let value = scope.errinfo();
                        stack.push(value);
                    }
                    Insn::SetErrinfo => {
                        let value = stack.pop().expect("seterrinfo on an empty stack");
                        scope.set_errinfo(value);
                    }
                    Insn::DefinedMatch(which) => {
                        let held = defined_match(scope, &which);
                        let value =
                            defined_word(scope, string_class, held.then_some("global-variable"));
                        stack.push(value);
                    }
                    Insn::LastMatch(which) => {
                        let value = last_match_part(scope, &which)?;
                        stack.push(value);
                    }

                    Insn::CaseEq => {
                        let condition = stack.pop().expect("caseeq on an empty stack");
                        let subject = stack.pop().expect("caseeq on an empty stack");
                        // `when c` and `in c` both ask `c === subject`, receiver
                        // first. For an immediate, a String or an Array, `===` is
                        // `==` and the fast path answers it.
                        match case_eq(scope, condition, subject) {
                            Ok(answer) => stack.push(bool_value(answer)),
                            // The send behind the fast path, the same one
                            // `Insn::BinOp` grew: a `Module`, a `Range` or a `Proc`
                            // in condition position means something other than
                            // `==`, and each of those defines the `===` that says
                            // what. Refusing here instead was what made `when
                            // Integer` — and every `in Integer` (#165) —
                            // undispatchable.
                            Err(Error::NoDispatch { .. }) => {
                                let call = Pending {
                                    cache: None,
                                    name: crate::shared::symbols::intern("==="),
                                    receiver: condition,
                                    args: vec![subject],
                                    keywords: Vec::new(),
                                    block: Value::NIL,
                                    block_is_literal: false,
                                    cref: frames[top].cref,
                                    implicit_self: false,
                                    public_only: false,
                                    target: Target::Method,
                                    owner: None,
                                    defined_as: None,
                                };
                                if let Some(unwind) = dispatch(
                                    scope,
                                    &mut stack,
                                    &mut frames,
                                    call,
                                    proc_class,
                                    &mut ids,
                                )? {
                                    return Ok(Step::Unwind(unwind));
                                }
                            }
                            Err(other) => return Err(other),
                        }
                    }

                    Insn::MakeProc(child, lambda) => {
                        let iseq = Arc::clone(&frames[top].iseq.children[child as usize]);
                        let value = make_proc(
                            scope,
                            proc_class,
                            &iseq,
                            frames[top].env,
                            frames[top].receiver,
                            frames[top].block,
                            lambda,
                            frames[top].cref,
                            frames[top].home,
                        );
                        stack.push(value);
                    }

                    Insn::DefineMethod(index) => {
                        let (name, child) = frames[top].iseq.definitions[index as usize];
                        let iseq = Arc::clone(&frames[top].iseq.children[child as usize]);
                        let symbol = frames[top].symbols[name as usize];
                        let cref = frames[top].cref;
                        let default = frames[top].scope_default;
                        if scope.classes().cref_refuses_def(cref) {
                            return Err(Error::raise("TypeError", "can't define singleton"));
                        }
                        let owner = scope.classes().cref_class(cref);
                        define_method_on(
                            scope,
                            owner,
                            symbol,
                            Arc::clone(&iseq),
                            cref,
                            default.visibility(),
                        );
                        if default == ScopeDefault::ModuleFunction {
                            // The public half. A second definition rather than a
                            // move: the instance copy stays, privately, which is
                            // what makes `module_function` different from
                            // `def self.` (#211).
                            let singleton = scope.singleton_class(owner);
                            define_method_on(
                                scope,
                                singleton,
                                symbol,
                                iseq,
                                cref,
                                Visibility::Public,
                            );
                        }
                        stack.push(Value::symbol(symbol));
                        // The hooks (#28), after the definition and before the
                        // next instruction. Frames run last-pushed first, so the
                        // module function's `singleton_method_added` is pushed
                        // first and `method_added` runs before it: CRuby's
                        // order, measured.
                        let module = scope.classes().object(owner);
                        if default == ScopeDefault::ModuleFunction
                            && let Some(unwind) = fire_hook(
                                scope,
                                &mut stack,
                                &mut frames,
                                proc_class,
                                &mut ids,
                                module,
                                "singleton_method_added",
                                vec![Value::symbol(symbol)],
                            )?
                        {
                            return Ok(Step::Unwind(unwind));
                        }
                        let (hook, receiver) = if scope.classes().is_singleton(owner) {
                            ("singleton_method_added", singleton_attached(scope, owner))
                        } else {
                            ("method_added", module)
                        };
                        if let Some(unwind) = fire_hook(
                            scope,
                            &mut stack,
                            &mut frames,
                            proc_class,
                            &mut ids,
                            receiver,
                            hook,
                            vec![Value::symbol(symbol)],
                        )? {
                            return Ok(Step::Unwind(unwind));
                        }
                    }

                    // `/a#{b}c/`. The source was concatenated on the stack; this
                    // compiles it. Deliberately not `regexp_literal`'s cache: two
                    // evaluations of an interpolated literal are two objects,
                    // measured, and a cache keyed by the built source would make
                    // them one.
                    Insn::NewRegexp(options) => {
                        let source = stack.pop().expect("newregexp on an empty stack");
                        let Some(text) = string_text(scope, source) else {
                            return Err(Error::NoDispatch {
                                op: "a regexp literal",
                                operands: "an interpolation that is not a String",
                            });
                        };
                        let value = regexp_new(scope, &text, options)?;
                        stack.push(value);
                    }

                    Insn::OnceGet(displacement, site) => {
                        let iseq = Arc::clone(&frames[top].iseq);
                        if let Some(cached) = scope.regexps().once_cached(&iseq, site) {
                            stack.push(cached);
                            frames[top].pc = jump(frames[top].pc, displacement);
                        }
                    }
                    Insn::OnceSet(site) => {
                        let value = *stack.last().expect("onceset on an empty stack");
                        let iseq = Arc::clone(&frames[top].iseq);
                        scope.regexps_mut().cache_once(&iseq, site, value);
                    }

                    // `/a#{b}c/o`. The source is still built every time — the
                    // flag changes what happens after, not before — and then the
                    // site's first answer is reused for ever. Measured.
                    Insn::NewRegexpOnce(options, site) => {
                        let source = stack.pop().expect("newregexponce on an empty stack");
                        let iseq = Arc::clone(&frames[top].iseq);
                        if let Some(cached) = scope.regexps().once_cached(&iseq, site) {
                            stack.push(cached);
                        } else {
                            let Some(text) = string_text(scope, source) else {
                                return Err(Error::NoDispatch {
                                    op: "a regexp literal",
                                    operands: "an interpolation that is not a String",
                                });
                            };
                            let value = regexp_new(scope, &text, options)?;
                            scope.regexps_mut().cache_once(&iseq, site, value);
                            stack.push(value);
                        }
                    }

                    // Class variables (#188's enabling work). Read off the
                    // frame's cref, like `def`, then along that class's ancestors
                    // — never lexically, which is what separates one from a
                    // constant. A write lands on the ancestor that already holds
                    // the name, so a subclass assigning `@@a` changes the
                    // superclass's; measured.
                    Insn::GetCvar(name) => {
                        let symbol = frames[top].symbols[name as usize];
                        let owner = cvar_owner(scope, frames[top].cref)?;
                        match cvar_read(scope, owner, symbol)? {
                            Some(value) => stack.push(value),
                            None => {
                                let where_ = scope
                                    .classes()
                                    .name(owner)
                                    .map_or_else(|| "an anonymous class".to_owned(), str::to_owned);
                                return Err(Error::raise(
                                    "NameError",
                                    format!(
                                        "uninitialized class variable {} in {where_}",
                                        symbol_name(symbol),
                                    ),
                                ));
                            }
                        }
                    }
                    Insn::SetCvar(name) => {
                        let symbol = frames[top].symbols[name as usize];
                        let owner = cvar_owner(scope, frames[top].cref)?;
                        let value = stack.pop().expect("setcvar on an empty stack");
                        scope.classes_mut().cvar_set(owner, symbol, value);
                    }
                    Insn::DefinedCvar(name) => {
                        let symbol = frames[top].symbols[name as usize];
                        // `defined?` never raises, so a top-level `@@a` is `nil`
                        // here rather than the `RuntimeError` reading one is.
                        let held = match cvar_owner(scope, frames[top].cref) {
                            Ok(owner) => scope.classes().cvar_defined(owner, symbol),
                            Err(_) => false,
                        };
                        let value =
                            defined_word(scope, string_class, held.then_some("class variable"));
                        stack.push(value);
                    }

                    // `alias` and `undef` write into the frame's definee — the
                    // same cref `def` writes into, not `self`. A name nothing in
                    // the chain defines is Ruby's `NameError` in both.
                    Insn::Alias(new, old) => {
                        if scope.classes().cref_refuses_def(frames[top].cref) {
                            return Err(Error::raise("TypeError", "can't define singleton"));
                        }
                        let new = frames[top].symbols[new as usize];
                        let old = frames[top].symbols[old as usize];
                        let owner = scope.classes().cref_class(frames[top].cref);
                        alias_into(scope, owner, new, old)?;
                        if let Some(unwind) = fire_method_hook(
                            scope,
                            &mut stack,
                            &mut frames,
                            proc_class,
                            &mut ids,
                            owner,
                            "added",
                            new,
                        )? {
                            return Ok(Step::Unwind(unwind));
                        }
                    }
                    // `alias :"m#{n}" :m`. The names were built by the frame, old
                    // on top because it was pushed second — which is also Ruby's
                    // evaluation order for the pair.
                    Insn::AliasFromStack => {
                        let old = pop_name(&mut stack, "alias")?;
                        let new = pop_name(&mut stack, "alias")?;
                        let owner = scope.classes().cref_class(frames[top].cref);
                        alias_into(scope, owner, new, old)?;
                        if let Some(unwind) = fire_method_hook(
                            scope,
                            &mut stack,
                            &mut frames,
                            proc_class,
                            &mut ids,
                            owner,
                            "added",
                            new,
                        )? {
                            return Ok(Step::Unwind(unwind));
                        }
                    }
                    Insn::Undef(name) => {
                        let symbol = frames[top].symbols[name as usize];
                        let owner = scope.classes().cref_class(frames[top].cref);
                        undef_from(scope, owner, symbol)?;
                        if let Some(unwind) = fire_method_hook(
                            scope,
                            &mut stack,
                            &mut frames,
                            proc_class,
                            &mut ids,
                            owner,
                            "undefined",
                            symbol,
                        )? {
                            return Ok(Step::Unwind(unwind));
                        }
                    }
                    Insn::UndefFromStack => {
                        let symbol = pop_name(&mut stack, "undef")?;
                        let owner = scope.classes().cref_class(frames[top].cref);
                        undef_from(scope, owner, symbol)?;
                        if let Some(unwind) = fire_method_hook(
                            scope,
                            &mut stack,
                            &mut frames,
                            proc_class,
                            &mut ids,
                            owner,
                            "undefined",
                            symbol,
                        )? {
                            return Ok(Step::Unwind(unwind));
                        }
                    }
                    Insn::AliasGlobal(new, old) => {
                        let new = frames[top].symbols[new as usize];
                        let old = frames[top].symbols[old as usize];
                        scope.alias_global(new, old);
                    }
                    Insn::AliasGlobalSpecial(new, which) => {
                        let new = frames[top].symbols[new as usize];
                        scope.alias_global_special(new, which);
                    }
                    Insn::DefineSingleton(index) => {
                        let (name, child) = frames[top].iseq.definitions[index as usize];
                        let iseq = Arc::clone(&frames[top].iseq.children[child as usize]);
                        let symbol = frames[top].symbols[name as usize];
                        let cref = frames[top].cref;
                        let receiver = stack.pop().expect("a receiver to define on");
                        // `def frozen_obj.m` raises: a singleton method is stored on
                        // the object, and a frozen object does not take stores.
                        frozen_check(scope, receiver, "object")?;
                        let owner = singleton_of(scope, receiver)?;
                        // Always public: a bare `private` in the class body does
                        // not reach `def self.m`, measured on ruby 4.0.6.
                        define_method_on(scope, owner, symbol, iseq, cref, Visibility::Public);
                        stack.push(Value::symbol(symbol));
                        // `singleton_method_added` on the receiver (#28).
                        if let Some(unwind) = fire_hook(
                            scope,
                            &mut stack,
                            &mut frames,
                            proc_class,
                            &mut ids,
                            receiver,
                            "singleton_method_added",
                            vec![Value::symbol(symbol)],
                        )? {
                            return Ok(Step::Unwind(unwind));
                        }
                    }

                    Insn::GetConst(name, how) => {
                        let symbol = frames[top].symbols[name as usize];
                        let cref = frames[top].cref;
                        let from = const_base(scope, &mut stack, cref, how)?;
                        let found = match how {
                            ConstScope::Lexical => scope.classes().const_get(cref, symbol),
                            ConstScope::Qualified | ConstScope::Top => {
                                scope.classes().const_get_qualified(from, symbol)
                            }
                        };
                        let Some(value) = found else {
                            return Err(uninitialized(scope, from, symbol, how));
                        };
                        stack.push(value);
                    }

                    Insn::SetConst(name, how) => {
                        let symbol = frames[top].symbols[name as usize];
                        let cref = frames[top].cref;
                        let value = stack.pop().expect("a value to assign");
                        let target = const_base(scope, &mut stack, cref, how)?;
                        scope.classes_mut().const_set(target, symbol, value);
                        // `Foo = Class.new` is how an anonymous class gets a name,
                        // and the only way one ever does. Only the first assignment
                        // names it: `A = Class.new; B = A` leaves both `"A"`.
                        name_if_anonymous(scope, target, symbol, value);
                        // Assignment is an expression, and its value is what was
                        // assigned — not the module it landed on.
                        stack.push(value);
                        // `const_added` on the module it landed on (#28).
                        let module = scope.classes().object(target);
                        if let Some(unwind) = fire_hook(
                            scope,
                            &mut stack,
                            &mut frames,
                            proc_class,
                            &mut ids,
                            module,
                            "const_added",
                            vec![Value::symbol(symbol)],
                        )? {
                            return Ok(Step::Unwind(unwind));
                        }
                    }

                    Insn::DefinedConst(name, how) => {
                        let symbol = frames[top].symbols[name as usize];
                        let cref = frames[top].cref;
                        let from = const_base(scope, &mut stack, cref, how)?;
                        let found = match how {
                            ConstScope::Lexical => scope.classes().const_get(cref, symbol),
                            ConstScope::Qualified | ConstScope::Top => {
                                scope.classes().const_get_qualified(from, symbol)
                            }
                        };
                        let value = defined_answer(scope, string_class, found.map(|_| "constant"))?;
                        stack.push(value);
                    }

                    Insn::DefinedMethod(name) | Insn::DefinedSelfMethod(name) => {
                        let symbol = frames[top].symbols[name as usize];
                        let receiver = match insn {
                            Insn::DefinedMethod(_) => stack.pop().expect("a receiver to ask about"),
                            _ => frames[top].receiver,
                        };
                        // `nil`, `true`, and `false` have no class yet, so "does it have
                        // this method" has no answer rather than the answer `no`.
                        let class = class_of(scope, receiver).ok_or_else(|| no_class(receiver))?;
                        let found = scope.classes_mut().lookup(class, symbol);
                        // A method this site could not call is not "method" (#161).
                        // `defined?` refuses `self.priv` where the call allows it,
                        // so `self_receiver_ok` is false here — measured, not
                        // inferred.
                        let implicit = matches!(insn, Insn::DefinedSelfMethod(_));
                        let caller = frames[top].receiver;
                        let refused = found.is_some_and(|method| {
                            visibility_refusal_at(
                                scope,
                                method,
                                implicit,
                                receiver,
                                Some(caller),
                                false,
                            )
                            .is_some()
                        });
                        // A method that exists and this site may not call is a
                        // definite `nil`, not an unknowable one: no `require` can
                        // change the answer, so #39's refusal must not fire here.
                        let value = if refused {
                            defined_word(scope, string_class, None)
                        } else {
                            defined_answer(scope, string_class, found.map(|_| "method"))?
                        };
                        stack.push(value);
                    }

                    Insn::DefinedYield => {
                        // A frame either has a block or does not, and this heap is the
                        // authority on which. So `nil` here is an answer, not a gap.
                        let answer = (frames[top].block != Value::NIL).then_some("yield");
                        let value = defined_word(scope, string_class, answer);
                        stack.push(value);
                    }

                    Insn::OpenClass(index) => {
                        let iseq = Arc::clone(&frames[top].iseq);
                        let def = &iseq.class_defs[index as usize];
                        if let Some(unwind) = open_class(
                            scope,
                            &mut stack,
                            &mut frames,
                            def,
                            &iseq,
                            proc_class,
                            &mut ids,
                        )? {
                            return Ok(Step::Unwind(unwind));
                        }
                    }

                    Insn::Send(index) => {
                        let iseq = Arc::clone(&frames[top].iseq);
                        let site = &iseq.call_sites[index as usize];
                        let mut call =
                            pop_call(scope, &mut stack, site, &frames[top], proc_class, true)?;
                        // The call-site id: this `Iseq`'s run, plus the operand.
                        call.cache = Some(frames[top].cache_base + index);
                        if let Some(unwind) =
                            dispatch(scope, &mut stack, &mut frames, call, proc_class, &mut ids)?
                        {
                            return Ok(Step::Unwind(unwind));
                        }
                    }

                    Insn::Yield(index) => {
                        let iseq = Arc::clone(&frames[top].iseq);
                        let site = &iseq.call_sites[index as usize];
                        // The block is a field of the frame rather than a slot, so an
                        // anonymous block costs nothing and `yield` needs no name.
                        let block = frames[top].block;
                        let mut call =
                            pop_call(scope, &mut stack, site, &frames[top], proc_class, false)?;
                        if block == Value::NIL {
                            return Err(Error::raise("LocalJumpError", "no block given (yield)"));
                        }
                        call.receiver = block;
                        call.target = Target::Block(block);
                        if let Some(unwind) =
                            dispatch(scope, &mut stack, &mut frames, call, proc_class, &mut ids)?
                        {
                            return Ok(Step::Unwind(unwind));
                        }
                    }

                    // `super` (#187). The receiver, the name, and the class to
                    // start past all come from the frame; the site carries only the
                    // arguments, which is why its own name is unused.
                    Insn::Super(index) => {
                        let iseq = Arc::clone(&frames[top].iseq);
                        let site = &iseq.call_sites[index as usize];
                        let (owner, name) = (frames[top].owner, frames[top].defined_as);
                        // Forward the frame's block only when the site passed none
                        // at all. `super(&nil)` passes a block that is `nil`, which
                        // is a different thing and means no block — asserted by
                        // `super_spec.rb`'s "can pass no block using &nil".
                        let frame_block = match site.block {
                            BlockRef::None => frames[top].block,
                            _ => Value::NIL,
                        };
                        let mut call =
                            pop_call(scope, &mut stack, site, &frames[top], proc_class, false)?;
                        let (Some(owner), Some(name)) = (owner, name) else {
                            // A body that is not a method's and was not written in
                            // one. Ruby raises here rather than at parse time.
                            return Err(Error::raise(
                                "RuntimeError",
                                "super called outside of method",
                            ));
                        };
                        call.name = name;
                        call.target = Target::Super { owner };
                        // `super` reaches a private method the way a receiverless
                        // call does.
                        call.implicit_self = true;
                        // Both `super` and `super()` forward the current block:
                        // measured, `B#m` calling `super` reaches an `A#m` that
                        // yields. An explicit `&b` at the site wins.
                        if call.block == Value::NIL {
                            call.block = frame_block;
                        }
                        if let Some(unwind) =
                            dispatch(scope, &mut stack, &mut frames, call, proc_class, &mut ids)?
                        {
                            return Ok(Step::Unwind(unwind));
                        }
                    }

                    Insn::JumpUnlessUndef(displacement) => {
                        if stack.pop().expect("jump on an empty stack") != Value::UNDEF {
                            frames[top].pc = jump(frames[top].pc, displacement);
                        }
                    }

                    Insn::Leave => {
                        let value = stack.pop().unwrap_or(Value::NIL);
                        let done = frames.pop().expect("a frame to leave");
                        scope.set_errinfo(done.errinfo_on_entry);
                        stack.truncate(done.base);
                        if frames.is_empty() {
                            // A fiber's block returning is `resume` returning in
                            // its resumer; only the root's last frame ends the run.
                            return match leave_fiber(
                                scope,
                                &mut stack,
                                &mut frames,
                                FiberExit::Value(value),
                            ) {
                                Err(()) => Ok(Step::Done(value)),
                                Ok(None) => Ok(Step::Next),
                                Ok(Some(unwind)) => Ok(Step::Unwind(unwind)),
                            };
                        }
                        // `Class#new` left the object below the base; `initialize`'s own
                        // value is dropped.
                        if done.raises_receiver {
                            let exception = stack
                                .pop()
                                .expect("`raise` left the exception below the base");
                            return Ok(Step::Unwind(Unwind::Exception(exception)));
                        }
                        let value = if done.booleanizes_value {
                            bool_value(value.is_truthy())
                        } else {
                            value
                        };
                        if !done.keeps_receiver && !done.discards_value {
                            stack.push(value);
                        }
                    }

                    Insn::LeaveThroughEnsure => {
                        let value = stack.pop().unwrap_or(Value::NIL);
                        // The same unwind `return` uses, aimed at *this* frame
                        // rather than the method the block was written in: the
                        // search pops to it, running every `ensure` on the way, and
                        // leaves `value` behind as the block's value.
                        return Ok(Step::Unwind(Unwind::Return {
                            frame: frames[top].id,
                            value,
                        }));
                    }

                    Insn::Return => {
                        let value = stack.pop().unwrap_or(Value::NIL);
                        let frame = frames[top].home;
                        // A method or a lambda homes to itself, so this is the ordinary
                        // case and the search below finds it immediately. A block homes
                        // to the method it was written in, and if that method has
                        // already returned there is nothing to return *from*.
                        if !frames.iter().any(|call| call.id == frame) {
                            return Err(Error::raise("LocalJumpError", "unexpected return"));
                        }
                        return Ok(Step::Unwind(Unwind::Return { frame, value }));
                    }

                    Insn::Break => {
                        let value = stack.pop().unwrap_or(Value::NIL);
                        let frame = frames[top].breaks;
                        if frame == 0 || !frames.iter().any(|call| call.id == frame) {
                            return Err(Error::raise("LocalJumpError", "break from proc-closure"));
                        }
                        return Ok(Step::Unwind(Unwind::Break { frame, value }));
                    }

                    Insn::Goto(displacement, depth) | Insn::GotoValue(displacement, depth) => {
                        let value = match insn {
                            Insn::GotoValue(_, _) => Some(stack.pop().unwrap_or(Value::NIL)),
                            _ => None,
                        };
                        let target = jump(frames[top].pc, displacement);
                        return Ok(Step::Unwind(Unwind::Goto {
                            frame: frames[top].id,
                            target,
                            depth: depth as usize,
                            value,
                        }));
                    }

                    Insn::Raise => {
                        let exception = stack.pop().expect("raise on an empty stack");
                        return Ok(Step::Unwind(Unwind::Exception(exception)));
                    }

                    Insn::CheckMatch => {
                        let class = stack.pop().expect("a rescue class on the stack");
                        let exception = *stack.last().expect("an exception to match against");
                        let matched = exception_matches(scope, exception, class)?;
                        stack.push(bool_value(matched));
                    }

                    Insn::CheckMatchAny => {
                        let list = stack.pop().expect("a rescue class list on the stack");
                        let exception = *stack.last().expect("an exception to match against");
                        let mut matched = false;
                        // Rooted and read one element at a time rather than collected:
                        // `exception_matches` can allocate, and a `Value` held across
                        // an allocation is exactly what the handle discipline is for.
                        let list = scope.root(list);
                        // In order, and stopping at the first hit. `exception_matches`
                        // raises the `TypeError` for a non-`Module`, so stopping early
                        // is also what keeps `rescue RuntimeError, *[42]` from raising
                        // one when the `RuntimeError` already matched (measured).
                        for index in 0..array_len(scope, list) {
                            let class = array_get(scope, list, index);
                            if exception_matches(scope, exception, class)? {
                                matched = true;
                                break;
                            }
                        }
                        stack.push(bool_value(matched));
                    }

                    Insn::EnterEnsure => {
                        let value = stack.pop().unwrap_or(Value::NIL);
                        frames[top].parked.push(Parked::Value(value));
                    }

                    Insn::LeaveEnsure => {
                        match frames[top].parked.pop().expect("an ensure to leave") {
                            Parked::Value(value) => stack.push(value),
                            Parked::Unwind(unwind) => return Ok(Step::Unwind(unwind)),
                        }
                    }
                }
                Ok(Step::Next)
            })();

            let unwind = match stepped {
                Ok(Step::Next) => continue,
                Ok(Step::Done(value)) => break value,
                Ok(Step::Unwind(unwind)) => unwind,
                // A raise becomes an object here and nowhere else, so every site
                // that has been emitting `Error::Raise` since #11 starts being
                // catchable without being touched.
                Err(Error::Raise { class, message }) => {
                    Unwind::Exception(exception_new(scope, class, &message))
                }
                // `NoDispatch`, `Budget`, and `Unknowable` are not Ruby semantics:
                // they say this VM cannot run the program. A `rescue` must never
                // turn "not implemented yet" into "caught", or the harness would
                // report a missing feature as an exception a spec handled. A
                // refusal boundary is not a `rescue`: it ends the block it ran
                // and answers why, and nothing else sees it (#145).
                Err(other) => {
                    seen_frames = usize::MAX;
                    refuse(scope, &mut stack, &mut frames, &mut boundaries, other)?
                }
            };

            // Where the exception starts is where its backtrace is taken: every
            // raise, whether `raise` or the VM's own, comes through here with the
            // raising frame still on the stack.
            if let Unwind::Exception(exception) = unwind {
                attach_backtrace(scope, &frames, exception);
            }
            if let Some(value) = unwind_to_handler(scope, &mut stack, &mut frames, unwind)? {
                break value;
            }
        })
    })();

    scope.fibers_mut().frame_ids = ids;
    scope.fibers_mut().abandon();
    result
}

/// A refusal boundary on some stack of frames: which fiber's (`None` for the
/// root), which frame, and the budget it may not run below.
struct Boundary {
    fiber: Option<usize>,
    index: usize,
    id: u64,
    deadline: u64,
}

/// Drop the boundaries whose frames have left, register one whose block has
/// just started, and answer the innermost deadline — 0 when there is none.
fn refresh_boundaries(
    scope: &HandleScope<'_>,
    frames: &[Call],
    boundaries: &mut Vec<Boundary>,
    budget: u64,
) -> u64 {
    prune_boundaries(scope, frames, boundaries);
    let fiber = scope.fibers().current;
    if let Some(top) = frames.len().checked_sub(1) {
        let frame = &frames[top];
        let known = boundaries
            .iter()
            .any(|b| b.fiber == fiber && b.id == frame.id);
        if frame.boundary_limit != 0 && !known {
            boundaries.push(Boundary {
                fiber,
                index: top,
                id: frame.id,
                deadline: budget.saturating_sub(frame.boundary_limit),
            });
        }
    }
    // The running stack's innermost boundary binds; with none, the root's.
    boundaries
        .iter()
        .rev()
        .find(|b| b.fiber == fiber)
        .or_else(|| boundaries.iter().rev().find(|b| b.fiber.is_none()))
        .map_or(0, |b| b.deadline)
}

/// Forget the boundaries whose frames have left: on the running stack, by
/// looking; on a fiber that has finished, all of them. A suspended stack's
/// boundaries wait for it.
fn prune_boundaries(scope: &HandleScope<'_>, frames: &[Call], boundaries: &mut Vec<Boundary>) {
    let fibers = scope.fibers();
    let current = fibers.current;
    boundaries.retain(|b| {
        if b.fiber == current {
            frames.get(b.index).is_some_and(|frame| frame.id == b.id)
        } else {
            b.fiber
                .is_none_or(|f| !matches!(fibers.entries[f].state, FiberState::Terminated))
        }
    });
}

/// [`catch_refusal`], across fibers: a fiber is its own stack of frames, and
/// one with no boundary on it ends where it refused — terminated, as an
/// exception would leave it — so its resumer's boundary can take the refusal.
fn refuse(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    boundaries: &mut Vec<Boundary>,
    error: Error,
) -> Result<Unwind, Error> {
    let mut error = error;
    loop {
        match catch_refusal(scope, frames, boundaries, error) {
            Ok(unwind) => return Ok(unwind),
            Err(unhandled) => {
                let refusal = matches!(
                    unhandled,
                    Error::NoDispatch { .. }
                        | Error::Unknowable { .. }
                        | Error::Budget
                        | Error::NotCompiled(_)
                );
                if !refusal || scope.fibers().current.is_none() {
                    return Err(unhandled);
                }
                if leave_fiber(scope, stack, frames, FiberExit::Value(Value::NIL)).is_err() {
                    return Err(unhandled);
                }
                error = unhandled;
            }
        }
    }
}

/// End the innermost refusal boundary's block with `error`'s reason as its
/// value, or answer `error` when no boundary is open.
///
/// The ending is a `return` aimed at the boundary's frame: every `ensure` on
/// the way out runs — mspec's output matchers put `$stdout` back in one — and
/// no `rescue` matches, because it is not an exception. The boundary stops
/// being one first, so an `ensure` that runs out of budget too answers to the
/// boundary outside it rather than to this one again.
fn catch_refusal(
    scope: &mut HandleScope<'_>,
    frames: &mut [Call],
    boundaries: &mut Vec<Boundary>,
    error: Error,
) -> Result<Unwind, Error> {
    if !matches!(
        error,
        Error::NoDispatch { .. } | Error::Unknowable { .. } | Error::Budget | Error::NotCompiled(_)
    ) {
        return Err(error);
    }
    prune_boundaries(scope, frames, boundaries);
    let fiber = scope.fibers().current;
    let Some(position) = boundaries.iter().rposition(|b| b.fiber == fiber) else {
        return Err(error);
    };
    let Boundary { index, id, .. } = boundaries.remove(position);
    frames[index].boundary_limit = 0;
    let reason = string_new(scope, &error.to_string());
    Ok(Unwind::Return {
        frame: id,
        value: reason,
    })
}

/// Walk out through the frames until something wants this unwind.
///
/// The whole of Ruby's non-local control flow is this one search. An exception
/// stops at a `rescue` range whose handler accepts it; a `throw` stops at the
/// frame `catch` opened for its tag; `break` and `return` stop at the frame they
/// named. Every `ensure` range on the way out is entered first, with the reason
/// parked on its frame, which is what makes "runs on every exit path" a property
/// of the search rather than of the compiler.
///
/// `Ok(None)` means a handler took it and the interpreter carries on.
/// `Ok(Some(value))` means it unwound past the outermost frame carrying a value
/// — a `return` at the top level. `Err` means nothing wanted it.
fn unwind_to_handler(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    mut unwind: Unwind,
) -> Result<Option<Value>, Error> {
    loop {
        let top = frames.len() - 1;
        // `pc` has already moved past the instruction that unwound, and the
        // range covers that instruction rather than the next one.
        let faulting = u32::try_from(frames[top].pc.saturating_sub(1)).unwrap_or(u32::MAX);
        let entry = frames[top].iseq.catch_table.iter().copied().find(|entry| {
            entry.covers(faulting)
                && match entry.kind {
                    // Whether any *clause* matches is the handler's own
                    // bytecode to decide; the table only knows a `rescue` is
                    // for exceptions and an `ensure` is for everything.
                    CatchKind::Rescue => matches!(unwind, Unwind::Exception(_)),
                    // ...everything except a jump that is not leaving this
                    // `begin` at all. `begin; while c; next; end; ensure; E;
                    // end` lands back inside the protected range, and E must
                    // run when the `begin` ends, not once per iteration.
                    CatchKind::Ensure => match unwind {
                        Unwind::Goto { target, .. } => {
                            !entry.covers(u32::try_from(target).unwrap_or(u32::MAX))
                        }
                        _ => true,
                    },
                }
        });

        // A same-frame jump lands here rather than unwinding out of the frame:
        // every `ensure` it was leaving has run by now.
        if let Unwind::Goto {
            frame,
            target,
            depth,
            value,
        } = unwind
            && frame == frames[top].id
            && entry.is_none()
        {
            stack.truncate(frames[top].base + depth);
            if let Some(value) = value {
                stack.push(value);
            }
            frames[top].pc = target;
            return Ok(None);
        }

        if let Some(entry) = entry {
            stack.truncate(frames[top].base + entry.stack_depth as usize);
            match entry.kind {
                CatchKind::Rescue => match unwind {
                    Unwind::Exception(exception) => {
                        // Ruby's `$!`, scoped to the frame: what a bare `raise`
                        // inside this handler re-raises.
                        frames[top].rescued = Some(exception);
                        // And `$!` proper, which is not frame-scoped: a method
                        // called from inside the clause sees it (#206). The
                        // clause's own bytecode restores it on the way out.
                        scope.set_errinfo(exception);
                        stack.push(exception);
                    }
                    _ => unreachable!("a rescue entry only accepts an exception"),
                },
                CatchKind::Ensure => {
                    // An `ensure` runs with `$!` set while an exception is
                    // still propagating through it — measured — and with it
                    // nil on every other path, including after a `rescue`
                    // already handled one.
                    if let Unwind::Exception(exception) = unwind {
                        scope.set_errinfo(exception);
                    }
                    frames[top].parked.push(Parked::Unwind(unwind));
                }
            }
            frames[top].pc = entry.target as usize;
            return Ok(None);
        }

        // Nothing in this frame wanted it. The frame is done either way, and
        // its own `ensure`s have already run — they are entries in the table
        // that was just searched.
        let done = frames.pop().expect("a frame to unwind out of");
        // Nothing here wanted it, so this frame is done and gives `$!` back as
        // it found it. Whichever frame does catch it sets the cell above.
        scope.set_errinfo(done.errinfo_on_entry);
        stack.truncate(done.base);
        let landed = match unwind {
            Unwind::Break { frame, value } | Unwind::Return { frame, value } => {
                (frame == done.id).then_some(value)
            }
            // Ruby compares a `catch` tag by identity, not by `==`.
            Unwind::Throw { tag, value } => (done.tag == Some(tag)).then_some(value),
            Unwind::Exception(_) => None,
            // Handled above: a goto never leaves its own frame. Reaching here
            // means the frame it named is gone, which the compiler cannot emit.
            Unwind::Goto { .. } => None,
        };
        if let Some(value) = landed {
            if frames.is_empty() {
                match leave_fiber(scope, stack, frames, FiberExit::Value(value)) {
                    Err(()) => return Ok(Some(value)),
                    Ok(None) => return Ok(None),
                    Ok(Some(next)) => {
                        unwind = next;
                        continue;
                    }
                }
            }
            if done.raises_receiver && matches!(unwind, Unwind::Return { .. }) {
                // An explicit `return` out of an `initialize` that `raise` ran
                // is still `initialize` finishing: the object is raised.
                let exception = stack
                    .pop()
                    .expect("`raise` left the exception below the base");
                attach_backtrace(scope, frames, exception);
                unwind = Unwind::Exception(exception);
                continue;
            }
            if done.keeps_receiver {
                // `Class#new` left the object below this frame's base so that
                // `initialize`'s own value is dropped. A `break` out of a block
                // `initialize` was given is not `initialize` returning, though:
                // `Array.new(2) { break :x }` answers `:x`, not the array. So
                // the object goes and the break value takes its place.
                if matches!(unwind, Unwind::Break { .. }) {
                    stack.pop();
                    stack.push(value);
                }
            } else if !done.discards_value {
                stack.push(value);
            }
            return Ok(None);
        }
        if frames.is_empty() {
            // Out of a fiber's last frame, the unwind carries on in its
            // resumer — or becomes the LocalJumpError a `break` or `return`
            // out of a fiber's block is there.
            match leave_fiber(scope, stack, frames, FiberExit::Unwind(unwind)) {
                Err(()) => return Err(escaped(scope, unwind)),
                Ok(None) => return Ok(None),
                Ok(Some(next)) => {
                    if let Unwind::Exception(exception) = next {
                        attach_backtrace(scope, frames, exception);
                    }
                    unwind = next;
                    continue;
                }
            }
        }
    }
}

/// What an unwind that left the outermost frame is, as a reportable error.
fn escaped(scope: &mut HandleScope<'_>, unwind: Unwind) -> Error {
    match unwind {
        Unwind::Exception(exception) => {
            // Left in `$!`, so the embedder can ask what ended the program —
            // `SystemExit#status` is the process's exit status.
            scope.set_errinfo(exception);
            Error::Uncaught {
                class: class_name_of(scope, exception),
                message: exception_message(scope, exception),
            }
        }
        Unwind::Throw { tag, .. } => Error::Uncaught {
            class: "UncaughtThrowError".to_owned(),
            message: format!("uncaught throw {}", inspect(scope, tag)),
        },
        // Both are checked at the instruction, so reaching here means a frame
        // was popped between the check and the search.
        Unwind::Break { .. } => Error::Uncaught {
            class: "LocalJumpError".to_owned(),
            message: "break from proc-closure".to_owned(),
        },
        Unwind::Return { .. } => Error::Uncaught {
            class: "LocalJumpError".to_owned(),
            message: "unexpected return".to_owned(),
        },
        Unwind::Goto { .. } => Error::Uncaught {
            class: "LocalJumpError".to_owned(),
            message: "a jump left its own frame".to_owned(),
        },
    }
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

/// A `Proc` is five slots: what to run, where its locals came from, what `self`
/// was where it was written, the block that scope had, and whether it is a
/// lambda.
///
/// The block is captured rather than taken from whoever calls the `Proc`,
/// because `yield` inside a block reaches the *enclosing method's* block:
///
/// ```ruby
/// def inner; yield 10; end
/// def outer; inner { yield 1 }; end   # this `yield` is outer's, not inner's
/// outer { |a| a + 100 }               #=> 101
/// ```
///
/// Reading it from the calling frame instead makes that block yield to itself,
/// which is not a wrong answer but an infinite loop.
const PROC_BODY: usize = 0;
const PROC_ENV: usize = 1;
const PROC_SELF: usize = 2;
const PROC_LAMBDA: usize = 3;
const PROC_BLOCK: usize = 4;
/// The lexical scope the block was *written* in, as a fixnum [`CrefId`]. A block
/// in a `class C` body resolves a bare constant against `C`, wherever it is
/// later called from.
const PROC_CREF: usize = 5;
/// The frame a `return` in this body leaves: the method the block was *written*
/// in, fixed when the `Proc` was made.
const PROC_HOME: usize = 6;
/// The frame a `break` in this body ends: the call the block was *passed* to,
/// which is not known until the call happens, so `dispatch` writes it — once.
/// A `Proc` handed on with `&blk` keeps the first call's frame, which is Ruby:
/// `def a(&b) = c(&b)` with `a { break 1 }` ends `a`, not `c`.
const PROC_BREAK: usize = 7;
const PROC_SLOTS: u32 = 8;

/// What a call is going to run.
enum Target {
    /// Resolve `name` against the receiver's class.
    Method,
    /// Already resolved: a block or a `Proc`, called directly.
    Block(Value),
    /// `super`: resolve `name` from one step past `owner` in the receiver's
    /// ancestor chain, rather than from the top of it.
    Super { owner: ClassId },
}

/// One call, assembled from the stack and not yet dispatched.
struct Pending {
    name: SymbolId,
    receiver: Value,
    args: Vec<Value>,
    /// The keywords this call passes, in source order.
    ///
    /// The key is a `Value`, not a `SymbolId`: since #193 a call may write a
    /// key that is not a symbol — `m("a" => 1)` — and it has to reach a `**kw`
    /// intact. A symbol key is `Value::symbol`, an immediate, so matching a
    /// declared keyword is still one integer compare.
    keywords: Vec<(Value, Value)>,
    /// The block this call passes on, as a `Proc` or `nil`.
    block: Value,
    /// Whether that block was written as a literal `{ }` or `do end` rather than
    /// handed over with `&`. `Kernel#lambda` is the one caller that cares: it
    /// has required a literal block since Ruby 3.0, and `lambda(&a_proc)`
    /// raises `ArgumentError` rather than quietly making a lambda of it.
    block_is_literal: bool,
    /// The lexical scope the *callee's* body was written in. Filled by
    /// `pop_call` with the caller's scope and overwritten by `dispatch` once the
    /// method — and so the scope its `def` appeared in — is known.
    cref: CrefId,
    target: Target,
    /// A receiverless send, so a private method is reachable (#161).
    ///
    /// `Kernel#send` sets it too: `send` calls a private method on purpose.
    implicit_self: bool,
    /// `public_send`: only a public method, with none of the exceptions.
    ///
    /// Not the same as `!implicit_self`. An ordinary `obj.m` still reaches a
    /// protected method from inside the family and a private one through
    /// `self`, and `public_send` refuses both — it is the strict form of the
    /// question rather than the receiver-shaped one.
    public_only: bool,
    /// Which inline cache entry may answer this call, if any.
    ///
    /// `Some` only for `Insn::Send`. `Yield` resolves no name, and
    /// `Native::Send` re-dispatches under a name the call site never mentioned,
    /// so neither may borrow the site's entry.
    cache: Option<u32>,
    /// The class the callee's method was found on, and the name it was found
    /// under — what a `super` in its body starts from (#187).
    ///
    /// Filled by `dispatch` from the resolved `Method`, so it is the *running*
    /// method's identity rather than the call site's: `alias` and
    /// `define_method` make those two different things.
    owner: Option<ClassId>,
    /// See [`Pending::owner`]. Both or neither.
    defined_as: Option<SymbolId>,
}

/// Take a call site's operands off the stack.
///
/// Reverse of the order the compiler pushed them: a passed block, then keyword
/// values, then positional arguments, then the receiver.
fn pop_call<'h>(
    scope: &mut HandleScope<'h>,
    stack: &mut Vec<Value>,
    site: &CallSite,
    frame: &Call,
    proc_class: Handle<'h>,
    has_receiver: bool,
) -> Result<Pending, Error> {
    let block = match site.block {
        BlockRef::Pass => {
            let value = stack.pop().expect("block pass on an empty stack");
            // `&nil` passes no block, which is how `foo(&nil)` differs from a
            // missing argument.
            if value == Value::NIL {
                Value::NIL
            } else if proc_body(scope, value).is_some() {
                value
            } else {
                // `&obj` calls `obj.to_proc`, which needs a method that does
                // not exist yet.
                return Err(Error::NoDispatch {
                    op: "&",
                    operands: "a block argument that is not a Proc",
                });
            }
        }
        BlockRef::Literal(child) => {
            let iseq = Arc::clone(&frame.iseq.children[child as usize]);
            make_proc(
                scope,
                proc_class,
                &iseq,
                frame.env,
                frame.receiver,
                frame.block,
                false,
                frame.cref,
                frame.home,
            )
        }
        BlockRef::None => Value::NIL,
    };

    // A `**splat` or a non-symbol key arrives as one `Hash`, and the compiler
    // leaves `site.keywords` empty when it does — the whole group became that
    // hash, so its pairs are already in source order with duplicates resolved
    // (#193). The two are never both present, which is what lets this pick one
    // shape rather than inventing an order to merge them in: Ruby's depends on
    // where the splat was written, and `Hash` is where that is decided.
    let keywords = if site.kwsplat {
        debug_assert!(
            site.keywords.is_empty(),
            "a keyword splat and named keywords on one site"
        );
        let hash = stack.pop().expect("a keyword splat on an empty stack");
        hash_pairs(scope, hash).ok_or(Error::NoDispatch {
            op: "**",
            operands: "a keyword argument that is not a Hash",
        })?
    } else {
        let mut named = Vec::with_capacity(site.keywords.len());
        for &name in site.keywords.iter().rev() {
            let value = stack.pop().expect("keyword on an empty stack");
            named.push((Value::symbol(frame.symbols[name as usize]), value));
        }
        named.reverse();
        named
    };

    let at = stack.len() - site.argc as usize;
    let mut args: Vec<Value> = stack.drain(at..).collect();
    if !site.splats.is_empty() {
        args = expand_splats(scope, args, &site.splats)?;
    }

    let receiver = if has_receiver {
        stack.pop().expect("send on an empty stack")
    } else {
        frame.receiver
    };

    Ok(Pending {
        // `Insn::Send` fills this in; `Insn::Yield` leaves it, having no name
        // to resolve.
        cache: None,
        name: frame.symbols[site.name as usize],
        receiver,
        args,
        keywords,
        block,
        block_is_literal: matches!(site.block, BlockRef::Literal(_)),
        // A placeholder: `dispatch` replaces it with the callee's own scope once
        // the method is resolved. It only survives for a native method, which
        // never looks a constant up.
        cref: frame.cref,
        implicit_self: site.implicit_self,
        public_only: false,
        target: Target::Method,
        // `dispatch` fills both once the method — and so which class it was
        // found on — is known.
        owner: None,
        defined_as: None,
    })
}

/// `f(a, *b)`: splice the elements of the splatted arguments into the list.
///
/// Only the positions the call site marked. Expanding every array instead would
/// make `f(a, *b)` with an array `a` pass `a`'s elements as separate arguments,
/// which is a wrong answer rather than a missing feature.
///
/// `*nil` contributes nothing, because `nil.to_a` is `[]` — `f(*nil)` passes no
/// arguments at all, and `yield(1, 2, *nil)` yields two.
///
/// A `Hash` whose `to_a` is still `core/hash.rb`'s is spliced as its pairs,
/// read straight from the table — `f(*{a: 1})` passes `[:a, 1]`, measured —
/// which is that method's answer without a Ruby call. An object without `to_a`
/// is passed as itself, which is Ruby.
///
// ponytail: any other object that *has* a `to_a` is refused rather than sent
// it: sending means re-entering the interpreter from argument assembly, and
// passing it through was a wrong answer. The upgrade is to expand splats in the
// compiler instead, where a send is just another instruction.
fn expand_splats(
    scope: &mut HandleScope<'_>,
    args: Vec<Value>,
    splats: &[u16],
) -> Result<Vec<Value>, Error> {
    let to_a = crate::shared::symbols::intern("to_a");
    // Rooted, every one: the arguments have left the stack, and copying a
    // Hash's pairs allocates, which may collect.
    let args: Vec<_> = args.into_iter().map(|arg| scope.root(arg)).collect();
    let mut out = Vec::with_capacity(args.len());
    for (index, arg) in args.into_iter().enumerate() {
        let arg = scope.get(arg);
        if splats.contains(&(index as u16)) {
            if arg == Value::NIL {
                continue;
            }
            if let Some(elements) = array_elements(scope, arg) {
                out.extend(elements.into_iter().map(|e| scope.root(e)));
                continue;
            }
            let owner = class_of(scope, arg)
                .and_then(|class| scope.classes_mut().lookup(class, to_a))
                .map(|method| method.owner);
            match owner {
                None => {}
                Some(owner) if owner == Builtin::Hash.id() => {
                    let pairs = ivar_get(scope, arg, symbol("@__pairs__"))?;
                    let pairs = scope.root(pairs);
                    let count = array_elements(scope, scope.get(pairs)).map_or(0, |p| p.len());
                    for at in 0..count {
                        let pair = array_get(scope, pairs, at);
                        let parts = array_elements(scope, pair).unwrap_or_default();
                        let copy = new_array(scope, &parts);
                        out.push(scope.root(copy));
                    }
                    continue;
                }
                Some(_) => {
                    return Err(Error::NoDispatch {
                        op: "*",
                        operands: "a splatted argument whose `to_a` is Ruby",
                    });
                }
            }
        }
        out.push(scope.root(arg));
    }
    Ok(out.into_iter().map(|handle| scope.get(handle)).collect())
}

/// Push a frame for the call, or compute it outright when it is a primitive.
fn dispatch<'h>(
    scope: &mut HandleScope<'h>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    call: Pending,
    proc_class: Handle<'h>,
    ids: &mut u64,
) -> Result<Option<Unwind>, Error> {
    match call.target {
        Target::Block(block) => {
            push_proc_frame(scope, stack, frames, &call, block, ids)?;
            Ok(None)
        }
        Target::Method | Target::Super { .. } => {
            let class = class_of(scope, call.receiver).ok_or_else(|| no_class(call.receiver))?;
            let found = match call.target {
                // `super` starts one past the class the running method was
                // found on, which is a question about a *pair* and so cannot
                // use the inline caches: those key off the receiver's class and
                // serial alone. Rare enough next to an ordinary send that the
                // chain walk is the whole implementation.
                Target::Super { owner } => scope.classes().lookup_super(class, owner, call.name),
                // The inline cache, in front of the per-class method cache,
                // which is itself in front of the chain walk. A hit is two
                // integer compares against a `Vec` entry; the probe it skips is
                // what `bench/` calls the cached lookup.
                //
                // Only hits are memoised. A miss falls through to `lookup`,
                // whose own memo answers it, and then goes on to build an
                // exception or to find `method_missing` — not a path worth
                // widening an entry for.
                _ => match call.cache {
                    Some(slot) => {
                        let serial = scope.classes().serial(class);
                        match scope.call_caches().get(slot, class, serial) {
                            hit @ Some(_) => hit,
                            None => {
                                let found = scope.classes_mut().lookup(class, call.name);
                                if let Some(method) = found {
                                    scope.call_caches_mut().fill(slot, class, serial, method);
                                }
                                found
                            }
                        }
                    }
                    None => scope.classes_mut().lookup(class, call.name),
                },
            };
            // R8: an unknown method raises rather than answering `nil`. The
            // harness reports a statement that merely evaluates as a passing
            // effect, so a `nil` here would turn every matcher this VM does not
            // implement into a spec that passes without asserting anything.
            let is_super = matches!(call.target, Target::Super { .. });
            let Some(method) = found else {
                // A `method_missing` of the program's own answers instead
                // (#28), which is also what keeps a missing method from
                // being recorded as a gap: it is not one.
                if !is_super && let Some(redirected) = to_method_missing(scope, class, &call) {
                    return dispatch(scope, stack, frames, redirected, proc_class, ids);
                }
                // Ruby's answer for a method that is not there, as an ordinary
                // raise a `rescue` can catch (#170). Built here, not in the
                // loop's `Err` arm, because it carries the receiver — see
                // `no_method_error`.
                //
                // `super` off the end of the chain is the same class of error
                // with its own message, measured:
                // "super: no superclass method 'm' for an instance of Z".
                let exception = if is_super {
                    super_missing_error(scope, call.receiver, call.name)
                } else {
                    no_method_error(scope, call.receiver, call.name)
                };
                return Ok(Some(Unwind::Exception(exception)));
            };
            // Visibility (#161). Read off the cached `Method`, so a warm site
            // pays nothing for it and a `private :m` on an already-called
            // method still takes effect — `set_visibility` bumps the class
            // serial, which is what the entry re-checks.
            //
            // `super` is exempt: it names no receiver and reaches a private
            // method the way a receiverless call does.
            if !is_super && let Some(refused) = visibility_refusal(scope, frames, &call, method) {
                // Measured: a private method called from outside goes to the
                // program's `method_missing` too, when it has one.
                if let Some(redirected) = to_method_missing(scope, class, &call) {
                    return dispatch(scope, stack, frames, redirected, proc_class, ids);
                }
                let exception = visibility_error(scope, call.receiver, call.name, refused);
                return Ok(Some(Unwind::Exception(exception)));
            }
            match scope.definitions().get(method.body).cloned() {
                Some(Definition::Iseq(iseq)) => {
                    // The body resolves constants against the scope its `def`
                    // was written in, which is not the caller's and need not be
                    // reachable from `owner`'s ancestors.
                    let call = Pending {
                        cref: method.cref,
                        // Where a `super` in this body starts: the class the
                        // method was actually found on, under the name it was
                        // found by — not the call site's name, which `alias`
                        // and `define_method` make a different thing.
                        owner: Some(method.owner),
                        defined_as: Some(call.name),
                        ..call
                    };
                    *ids += 1;
                    // This frame is what a `break` in the block it was handed
                    // ends, so the block learns its target here — the first
                    // time it is passed anywhere, and never again.
                    set_break_target(scope, call.block, *ids);
                    let links = Links {
                        id: *ids,
                        home: *ids,
                        breaks: 0,
                        // A method body starts public, whatever the class body
                        // around it said.
                        scope_default: ScopeDefault::Public,
                    };
                    push_frame(
                        scope,
                        stack,
                        frames,
                        &call,
                        &iseq,
                        Value::NIL,
                        Binding::Strict,
                        links,
                    )?;
                    Ok(None)
                }
                Some(Definition::Proc(block)) => {
                    // `define_method`'s body (#28): the `Proc` run as this
                    // method, so `super` inside it starts from the class it was
                    // found on, under the name it was found by.
                    let call = Pending {
                        owner: Some(method.owner),
                        defined_as: Some(call.name),
                        ..call
                    };
                    push_proc_frame_as(scope, stack, frames, &call, block, ids, ProcRole::Method)?;
                    let last = frames.len() - 1;
                    set_break_target(scope, call.block, frames[last].id);
                    Ok(None)
                }
                Some(Definition::Native(native)) => {
                    let name = call.name;
                    match native_call(scope, stack, frames, call, native, proc_class, ids) {
                        // A primitive that raises is a frame in CRuby's
                        // backtrace — "in 'Integer#/'" at the caller's line —
                        // though it pushed none here. It becomes the exception
                        // now, while which method it was is still known.
                        Err(Error::Raise { class, message }) => {
                            let exception = exception_new(scope, class, &message);
                            let method_name =
                                crate::shared::symbols::name(name).unwrap_or_default();
                            let label = qualified_method_name(scope, method.owner, &method_name);
                            attach_backtrace_from(scope, frames, exception, Some(label));
                            Ok(Some(Unwind::Exception(exception)))
                        }
                        other => other,
                    }
                }
                None => unreachable!("a method body that is not in the definition table"),
            }
        }
    }
}

/// The frames a block body inherits from its `Proc`: where `return` goes, and
/// where `break` goes.
fn proc_links(scope: &mut HandleScope<'_>, block: Value) -> (u64, u64) {
    if proc_body(scope, block).is_none() {
        return (0, 0);
    }
    let handle = scope.root(block);
    let read = |scope: &mut HandleScope<'_>, slot: usize| {
        scope
            .slot(handle, slot)
            .as_fixnum()
            .and_then(|n| u64::try_from(n).ok())
            .unwrap_or(0)
    };
    (read(scope, PROC_HOME), read(scope, PROC_BREAK))
}

/// Record which frame a `break` in this block ends, if it does not know yet.
///
/// Only the first call wins, because Ruby ties `break` to the call the block
/// literal was written at, not to whatever later re-passes it.
fn set_break_target(scope: &mut HandleScope<'_>, block: Value, frame: u64) {
    if proc_body(scope, block).is_none() {
        return;
    }
    let handle = scope.root(block);
    if scope.slot(handle, PROC_BREAK) != Value::fixnum(0).expect("zero is a fixnum") {
        return;
    }
    let value = Value::fixnum(frame as i64).expect("a frame id fits in a fixnum");
    scope.set_slot(handle, PROC_BREAK, value);
}

/// Call a `Proc`: its own body, its captured environment, its own `self`.
fn push_proc_frame(
    scope: &mut HandleScope<'_>,
    stack: &[Value],
    frames: &mut Vec<Call>,
    call: &Pending,
    block: Value,
    ids: &mut u64,
) -> Result<(), Error> {
    push_proc_frame_as(scope, stack, frames, call, block, ids, ProcRole::Block)
}

/// What a `Proc`'s body is being run as.
#[derive(Clone, Copy)]
enum ProcRole {
    /// A block or a `Proc#call`: its own captured `self`.
    Block,
    /// A method `define_method` made of it (#28): `self` is the receiver,
    /// arity is a method's, `return` and `break` leave the method, and `super`
    /// starts from the method's owner — all taken from `call`.
    Method,
    /// `instance_eval`, `class_eval` and their `_exec` forms (#28): `self` is
    /// `receiver`, and a `def` in the body lands on `definee`. Constants still
    /// resolve where the block was written.
    Eval {
        receiver: Value,
        definee: ClassId,
        refuses_def: bool,
    },
}

fn push_proc_frame_as(
    scope: &mut HandleScope<'_>,
    stack: &[Value],
    frames: &mut Vec<Call>,
    call: &Pending,
    block: Value,
    ids: &mut u64,
    role: ProcRole,
) -> Result<(), Error> {
    let Some((iseq, env, captured_self, captured, lambda, cref)) = proc_parts(scope, block) else {
        return Err(Error::NoDispatch {
            op: "call",
            operands: "a receiver that is not a Proc",
        });
    };
    let receiver = match role {
        ProcRole::Block => captured_self,
        ProcRole::Method => call.receiver,
        ProcRole::Eval { receiver, .. } => receiver,
    };
    let cref = match role {
        ProcRole::Eval {
            definee,
            refuses_def: true,
            ..
        } => scope
            .classes_mut()
            .push_eval_cref_refusing_def(cref, definee),
        ProcRole::Eval { definee, .. } => scope.classes_mut().push_eval_cref(cref, definee),
        _ => cref,
    };
    let method_owner = (call.owner, call.defined_as);
    let call = Pending {
        // A block, not a name: nothing to memoise.
        cache: None,
        receiver,
        name: call.name,
        args: call.args.clone(),
        keywords: call.keywords.clone(),
        // The block this body sees is the one its *defining* scope had, not the
        // one the caller has. See `PROC_BLOCK`.
        block: if call.block == Value::NIL {
            captured
        } else {
            call.block
        },
        block_is_literal: call.block_is_literal,
        // A block resolves constants where it was written, not where it is
        // called: `class C; X = 1; [1].each { X }; end` finds `C::X`.
        cref,
        implicit_self: call.implicit_self,
        public_only: call.public_only,
        target: Target::Method,
        // Filled from the home frame below: `super` inside a block resolves
        // against the method the block was *written* in.
        owner: None,
        defined_as: None,
    };
    // A block takes both links from the `Proc`: `return` leaves the method it
    // was *written* in, `break` ends the call it was *passed* to. A lambda is a
    // method as far as *both* are concerned, so it homes to itself and breaks
    // to itself — `-> { break :v }.call` is `:v`, not a `LocalJumpError`.
    let (home, breaks) = proc_links(scope, block);
    // A block defines methods with the visibility of the scope it was *written*
    // in, which is the frame `home` already names. A lambda is a method body as
    // far as `return` and `break` go, but not for this: under a bare `private`,
    // `-> { def m; end }` still defines a private `m`.
    let home_frame = frames.iter().find(|frame| frame.id == home);
    let scope_default = match role {
        ProcRole::Block => home_frame.map_or(ScopeDefault::Public, |frame| frame.scope_default),
        // A method body and a class body start public.
        ProcRole::Method | ProcRole::Eval { .. } => ScopeDefault::Public,
    };
    // `super` inside a block resolves against the method the block was
    // *written* in — the frame `home` already names, for the same reason
    // `return` does.
    //
    // ponytail: a `Proc` that outlives its defining frame finds nothing here,
    // and a `super` in it raises "outside a method" rather than resolving. Give
    // the `Proc` two slots of its own, beside `PROC_HOME`, if a spec needs it.
    let (owner, defined_as) = match role {
        ProcRole::Method => method_owner,
        _ => home_frame.map_or((None, None), |f| (f.owner, f.defined_as)),
    };
    let call = Pending {
        owner,
        defined_as,
        ..call
    };
    *ids += 1;
    // A method's body is its own `return` and `break` target, however the
    // `Proc` was made.
    let own_target = lambda || matches!(role, ProcRole::Method);
    let links = Links {
        id: *ids,
        home: if own_target { *ids } else { home },
        breaks: if own_target { *ids } else { breaks },
        scope_default,
    };
    let binding = if lambda || matches!(role, ProcRole::Method) {
        Binding::Strict
    } else {
        Binding::Loose
    };
    push_frame(scope, stack, frames, &call, &iseq, env, binding, links)
}

/// Bind the arguments and push the frame.
// Eight, and each one is a different question the callee has to be told the
// answer to. The three that travel together — which frames a non-local exit
// looks for — are already one `Links`; grouping any of the rest would be a
// struct built to satisfy a lint rather than to name something.
#[allow(clippy::too_many_arguments)]
fn push_frame(
    scope: &mut HandleScope<'_>,
    stack: &[Value],
    frames: &mut Vec<Call>,
    call: &Pending,
    iseq: &Arc<Iseq>,
    outer: Value,
    binding: Binding,
    links: Links,
) -> Result<(), Error> {
    let env = env_alloc(scope, outer, iseq.locals.len());
    let symbols = iseq.link();
    // Beside `link`, and for the same reason: one hash probe on frame entry
    // instead of one per `Send`.
    let cache_base = scope.call_caches_mut().base(iseq);
    bind(scope, env, &iseq.params, &symbols, call, binding)?;
    frames.push(Call {
        iseq: Arc::clone(iseq),
        symbols,
        cache_base,
        env,
        receiver: call.receiver,
        cref: call.cref,
        block: call.block,
        pc: 0,
        base: stack.len(),
        keeps_receiver: false,
        raises_receiver: false,
        discards_value: false,
        booleanizes_value: false,
        scope_default: links.scope_default,
        id: links.id,
        home: links.home,
        breaks: links.breaks,
        tag: None,
        rescued: None,
        errinfo_on_entry: scope.errinfo(),
        // What a `super` in this body starts from. Set for a method by
        // `dispatch` and for a block by `push_proc_frame`; `None` everywhere
        // else, where `super` has no method to be one step past.
        owner: call.owner,
        defined_as: call.defined_as,
        parked: Vec::new(),
        boundary_limit: 0,
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Argument binding
// ---------------------------------------------------------------------------

/// Fill a frame's parameter slots from a call's arguments.
///
/// R2 and R3: one function, and the only difference between a method or lambda
/// and a block is `lambda` — whether a count mismatch raises or is padded, and
/// whether a lone `Array` is spread across the parameters.
fn bind(
    scope: &mut HandleScope<'_>,
    env: Value,
    spec: &ParamSpec,
    symbols: &[SymbolId],
    call: &Pending,
    binding: Binding,
) -> Result<(), Error> {
    let lambda = binding == Binding::Strict;
    let mut args = call.args.clone();

    // A callee that declares no keywords takes keywords as one trailing
    // positional Hash (#22): `def m(h) = h; m(a: 1)` is `{a: 1}`, and it counts
    // toward the arity like any argument. `**nil` is the one callee that
    // refuses instead, which `bind_keywords` says.
    if !spec.no_keywords
        && spec.keywords.is_empty()
        && spec.kwrest.is_none()
        && !call.keywords.is_empty()
    {
        let rooted: Vec<_> = args.iter().map(|&arg| scope.root(arg)).collect();
        let hash = hash_of_pairs(scope, &call.keywords)?;
        args = rooted.iter().map(|&handle| scope.get(handle)).collect();
        args.push(hash);
    }

    // A block with room for more than one value spreads a single Array across
    // its parameters; `{ |a| }` and `{ |*a| }` do not. This is most of what
    // `block_spec.rb` checks.
    if !lambda && args.len() == 1 && spreads(spec) {
        if let Some(elements) = array_elements(scope, args[0]) {
            args = elements;
        } else if defines_to_ary(scope, args[0]) {
            // Ruby spreads anything that answers `#to_ary`, not just an Array,
            // and calling it means dispatching from inside the binder — which
            // is on the Rust stack, not the interpreter loop, and cannot push a
            // frame. Binding `a` to the object and `b` to `nil` instead would
            // be a wrong answer where Ruby has a right one, and worse, it hides
            // a `#to_ary` that raises: `block_spec.rb` asserts on exactly that.
            return Err(Error::Unknowable {
                what: "a block parameter list spreading an object with `#to_ary`",
                needs: "the binder can call Ruby, which re-entrant primitives bring with fibers (#16)",
            });
        }
    }

    if lambda {
        check_arity(spec, args.len())?;
    }

    let required = spec.required.len();
    let post = spec.post.len();
    let optional = spec.optional.len();

    // Required from the left, post-required from the right, optionals from
    // whatever is left in between. Ruby's order, and the reason `post` is
    // counted separately rather than added to `required`.
    let available = args.len();
    let leading = required.min(available);
    // The slot a parameter owns rather than its position: a destructuring
    // parameter binds several names out of one argument, so `{ |(a, b), c| }`
    // writes the second argument to slot 2 and leaves slot 1 to `b` (#209).
    for (index, value) in args.iter().take(leading).enumerate() {
        env_set(scope, env, spec.required[index] as usize, *value);
    }
    // Ruby pads a block's missing required parameters with `nil`; a lambda
    // never gets here, because `check_arity` refused first.
    for index in leading..required {
        env_set(scope, env, spec.required[index] as usize, Value::NIL);
    }

    let after_required = available.saturating_sub(leading);
    let trailing = post.min(after_required);
    let optional_taken = optional.min(after_required - trailing);

    let mut cursor = leading;
    for (index, entry) in spec.optional.iter().enumerate() {
        let value = if index < optional_taken {
            let value = args[cursor];
            cursor += 1;
            value
        } else {
            // Not supplied: the body's guarded default fills it in.
            Value::UNDEF
        };
        env_set(scope, env, entry.slot as usize, value);
    }

    // With a splat the post-required parameters are taken from the right, and
    // the splat absorbs whatever is between. Without one there is nothing to
    // absorb the middle, so binding stays left-to-right and the extras are
    // dropped — which is why `{ |a, b=5, c=6, d, e| }` given six values binds
    // the first five and ignores the sixth rather than sliding to the end.
    let post_from = if let Some(slot) = spec.rest {
        let rest_end = available - trailing;
        let elements: Vec<Value> = if cursor < rest_end {
            args[cursor..rest_end].to_vec()
        } else {
            Vec::new()
        };
        let array = new_array(scope, &elements);
        env_set(scope, env, slot as usize, array);
        available - trailing
    } else {
        cursor
    };

    for (index, &slot) in spec.post.iter().enumerate() {
        let value = if index < trailing {
            args[post_from + index]
        } else {
            Value::NIL
        };
        env_set(scope, env, slot as usize, value);
    }

    bind_keywords(scope, env, spec, symbols, call)?;

    if let Some(slot) = spec.block {
        env_set(scope, env, slot as usize, call.block);
    }
    Ok(())
}

/// Whether a block spreads a single `Array` argument across its parameters.
///
/// More than one place to put a value, or one place plus a splat: `{ |a, b| }`
/// and `{ |a,| }` spread, `{ |a| }` and `{ |*a| }` and `{ |a = 1| }` do not.
fn spreads(spec: &ParamSpec) -> bool {
    let places = spec.required.len() + spec.optional.len() + spec.post.len();
    places > 1 || ((spec.rest.is_some() || spec.trailing_comma) && places > 0)
}

fn bind_keywords(
    scope: &mut HandleScope<'_>,
    env: Value,
    spec: &ParamSpec,
    symbols: &[SymbolId],
    call: &Pending,
) -> Result<(), Error> {
    // `def m(**nil)`: the method says it takes none, and one is an error rather
    // than something to collect. Measured: "no keywords accepted".
    if spec.no_keywords && !call.keywords.is_empty() {
        return Err(Error::raise("ArgumentError", "no keywords accepted"));
    }
    // A callee that declares no keywords at all does not *reject* them: Ruby
    // packs them into a trailing positional Hash, which `bind` already did.
    if spec.keywords.is_empty() && spec.kwrest.is_none() {
        return Ok(());
    }

    for keyword in &spec.keywords {
        let name = Value::symbol(symbols[keyword.name as usize]);
        let supplied = call.keywords.iter().find(|(k, _)| *k == name);
        match (supplied, keyword.required) {
            (Some((_, value)), _) => env_set(scope, env, keyword.slot as usize, *value),
            (None, true) => {
                return Err(Error::raise(
                    "ArgumentError",
                    format!("missing keyword: :{}", keyword_label(name)),
                ));
            }
            (None, false) => env_set(scope, env, keyword.slot as usize, Value::UNDEF),
        }
    }

    // What no named parameter claimed. With `**kw` it collects; without one it
    // is an unknown keyword, which is an error in Ruby.
    let declared = |name: Value| {
        spec.keywords
            .iter()
            .any(|k| Value::symbol(symbols[k.name as usize]) == name)
    };
    let Some(slot) = spec.kwrest else {
        for (name, _) in &call.keywords {
            if !declared(*name) {
                return Err(Error::raise(
                    "ArgumentError",
                    format!("unknown keyword: :{}", keyword_label(*name)),
                ));
            }
        }
        return Ok(());
    };
    // An `Array` of `[key, value]` pairs, which the body's prologue turns into
    // a `Hash`. Built here rather than there because only the binder can see
    // which keywords were left over, and built as an `Array` because a `Hash`
    // is `core/hash.rb` and this cannot send. Duplicate keys cannot reach here:
    // a call that could write one was lowered to a hash literal, which already
    // resolved them.
    let mut rest = Vec::new();
    for (name, value) in &call.keywords {
        if !declared(*name) {
            rest.push(new_array(scope, &[*name, *value]));
        }
    }
    let collected = new_array(scope, &rest);
    env_set(scope, env, slot as usize, collected);
    Ok(())
}

/// A `Hash` holding `pairs`, built the way `Hash.allocate` builds one: the
/// three ivars `core/hash.rb` reads. The binder cannot send `Hash.[]`, and a
/// Hash is only these ivars, so it writes them.
///
// ponytail: the second place outside `core/hash.rb` that knows the
// representation, beside `hash_pairs`; both move behind a primitive when the
// open-addressed table arrives.
fn hash_of_pairs(scope: &mut HandleScope<'_>, pairs: &[(Value, Value)]) -> Result<Value, Error> {
    let rooted: Vec<_> = pairs
        .iter()
        .map(|&(key, value)| (scope.root(key), scope.root(value)))
        .collect();
    let class = class_handle(scope, Builtin::Hash);
    let handle = alloc_ivar_object(scope, Some(class));
    let mut entries = Vec::with_capacity(rooted.len());
    for &(key, value) in &rooted {
        let parts = [scope.get(key), scope.get(value)];
        let entry = new_array(scope, &parts);
        entries.push(scope.root(entry));
    }
    let entries: Vec<Value> = entries.iter().map(|&h| scope.get(h)).collect();
    let list = new_array(scope, &entries);
    let object = scope.get(handle);
    ivar_set(scope, object, symbol("@__pairs__"), list)?;
    ivar_set(scope, object, symbol("@__default__"), Value::NIL)?;
    ivar_set(scope, object, symbol("@__default_is_proc__"), Value::FALSE)?;
    Ok(object)
}

/// How a keyword name reads in an `ArgumentError`.
///
/// Almost always a symbol; a non-symbol key can only reach a `**kw`, and one
/// never reaches a message that names a *declared* keyword.
fn keyword_label(name: Value) -> String {
    name.as_symbol().map_or_else(|| "?".to_owned(), symbol_name)
}

fn check_arity(spec: &ParamSpec, given: usize) -> Result<(), Error> {
    let min = spec.min_positional();
    let max = spec.max_positional();
    let ok = given >= min && max.is_none_or(|max| given <= max);
    if ok {
        return Ok(());
    }
    // R9: ruby/spec asserts on this text.
    let expected = match max {
        None => format!("{min}+"),
        Some(max) if max == min => format!("{min}"),
        Some(max) => format!("{min}..{max}"),
    };
    Err(Error::raise(
        "ArgumentError",
        format!("wrong number of arguments (given {given}, expected {expected})"),
    ))
}

// ---------------------------------------------------------------------------
// Procs, methods, and the classes of things
// ---------------------------------------------------------------------------

/// Build a `Proc` capturing `env` and `receiver`.
#[allow(clippy::too_many_arguments)]
fn make_proc<'h>(
    scope: &mut HandleScope<'h>,
    proc_class: Handle<'h>,
    iseq: &Arc<Iseq>,
    env: Value,
    receiver: Value,
    block: Value,
    lambda: bool,
    cref: CrefId,
    home: u64,
) -> Value {
    let body = scope
        .definitions_mut()
        .intern_iseq(iseq, Arc::as_ptr(iseq) as usize);
    let handle = scope.alloc(Some(proc_class), Payload::Slots, PROC_SLOTS);
    scope.set_slot(handle, PROC_BODY, body);
    scope.set_slot(handle, PROC_ENV, env);
    scope.set_slot(handle, PROC_SELF, receiver);
    scope.set_slot(handle, PROC_LAMBDA, bool_value(lambda));
    scope.set_slot(handle, PROC_BLOCK, block);
    scope.set_slot(handle, PROC_CREF, cref_value(cref));
    // Where a `return` in this body goes, fixed now: the method the block is
    // being *written* in. Where a `break` goes is not knowable yet — no call has
    // been handed this block — so it stays zero until `dispatch` fills it in.
    scope.set_slot(
        handle,
        PROC_HOME,
        Value::fixnum(home as i64).expect("a frame id fits in a fixnum"),
    );
    scope.set_slot(
        handle,
        PROC_BREAK,
        Value::fixnum(0).expect("zero is a fixnum"),
    );
    scope.get(handle)
}

/// An exception is two instance variables: what it says, and where it came
/// from.
///
/// They were two fixed slots until #151, for the same reason a `Proc`'s six
/// are: there was nowhere else to put them. Now that there is, `Exception`'s
/// accessors are three lines of `core/exception.rb` rather than two natives.
///
/// `@backtrace` is always `nil`. A real backtrace needs source positions the
/// compiler does not record, and `[]` would be a plausible-but-wrong answer
/// rather than an absent one — see the non-goals in PRD 0012.
const EXC_MESSAGE: &str = "@message";
const EXC_BACKTRACE: &str = "@backtrace";
/// See [`attach_backtrace`].
const EXC_LOCATIONS: &str = "@__backtrace_locations__";
/// `Exception#cause`, set on the first raise. See [`attach_backtrace`].
const EXC_CAUSE: &str = "@__cause__";

/// `NameError#name` and `NameError#receiver`, over the same two slots.
///
/// Only a failed dispatch writes them. The `NameError`s the VM raises through
/// [`Error::raise`] — an uninitialized constant, a bad ivar name — carry a
/// message and nothing else, so `name` answers `nil` there where CRuby answers
/// a symbol.
///
// ponytail: the ceiling is that `Error` holds `String`s, so a raise decided
// inside `uninitialized` cannot carry the receiver. Lifting it means returning
// an `Unwind` out of `const_base` and its callers, which is a slice of its own
// (#170 asked for the dispatch failure) — see the scope note in PRD 0019.
const EXC_NAME: &str = "@name";
const EXC_RECEIVER: &str = "@receiver";

/// A Ruby `String` holding `text`, in UTF-8.
fn string_new(scope: &mut HandleScope<'_>, text: &str) -> Value {
    let class = class_handle(scope, Builtin::String);
    string_alloc(scope, class, text.as_bytes(), crate::strings::UTF_8)
}

/// A `String` of class `class` holding `bytes` in `encoding` — the one place a
/// string is made. See `strings.rs` for the three slots.
fn string_alloc<'h>(
    scope: &mut HandleScope<'h>,
    class: Handle<'h>,
    bytes: &[u8],
    encoding: u8,
) -> Value {
    let handle = scope.alloc(Some(class), Payload::Slots, crate::strings::SLOTS);
    crate::strings::init(scope, handle, bytes, encoding);
    scope.get(handle)
}

/// Whether `value` is a `String` or an instance of a subclass of one.
fn is_string(scope: &mut HandleScope<'_>, value: Value) -> bool {
    heap_kind(scope, value) == Some(HeapKind::Str)
}

/// A `String`'s encoding index, or `None` when the value is not one.
fn string_encoding(scope: &mut HandleScope<'_>, value: Value) -> Option<u8> {
    if !is_string(scope, value) {
        return None;
    }
    let handle = scope.root(value);
    Some(crate::strings::encoding(scope, handle))
}

/// One line of a backtrace: the file, the line, and what was running there.
struct BacktraceLine {
    path: Arc<str>,
    line: i32,
    label: String,
}

/// The backtrace of the frames on the stack, innermost first (#29).
///
/// `frames[i].pc` has already moved past the instruction each frame is in the
/// middle of — the send that called the frame above, or the one that raised —
/// so the line is the one before it.
///
/// Core-library frames follow CRuby 3.4, which prints a method written in Ruby
/// inside the VM (`<internal:...>`) the way it prints one written in C: under
/// its own label but at its caller's position, and only where the program
/// called it. `[2].map { raise }` is "in 'block in <main>'", "in 'Array#map'",
/// "in '<main>'" — not the `each` and the block `core/array.rb` runs `map` on.
fn backtrace_of(scope: &mut HandleScope<'_>, frames: &[Call]) -> Vec<BacktraceLine> {
    let raw: Vec<(bool, BacktraceLine)> = frames
        .iter()
        .rev()
        .map(|frame| {
            let path = frame.iseq.path.clone().unwrap_or_else(|| Arc::from(""));
            let line = frame.iseq.line_at(frame.pc.saturating_sub(1)).unwrap_or(0);
            let internal = path.starts_with("<internal:");
            let label = frame_label(scope, frame);
            (internal, BacktraceLine { path, line, label })
        })
        .collect();
    let mut out = Vec::with_capacity(raw.len());
    for (index, (internal, entry)) in raw.iter().enumerate() {
        if !internal {
            out.push(BacktraceLine {
                path: Arc::clone(&entry.path),
                line: entry.line,
                label: entry.label.clone(),
            });
            continue;
        }
        // Only the core method the program itself called, at the program's
        // position: the next frame out is where that call was written.
        match raw.get(index + 1) {
            Some((false, caller)) if !entry.label.starts_with("block ") => {
                out.push(BacktraceLine {
                    path: Arc::clone(&caller.path),
                    line: caller.line,
                    label: entry.label.clone(),
                });
            }
            _ => {}
        }
    }
    out
}

/// What CRuby 3.4 calls a frame: `Foo#bar`, `Foo.bar`, `block (2 levels) in
/// Foo#bar`, `<main>`, `<class:Foo>`.
fn frame_label(scope: &mut HandleScope<'_>, frame: &Call) -> String {
    // A method frame is named after the `def`, and a block after the body it
    // was written in, which the compiler already made its `Iseq`'s name.
    let written = &*frame.iseq.name;
    let base = match frame.owner {
        Some(owner) => qualified_method_name(scope, owner, written),
        None => written.to_owned(),
    };
    match frame.iseq.block_level {
        0 => base,
        1 => format!("block in {base}"),
        levels => format!("block ({levels} levels) in {base}"),
    }
}

/// `Foo#bar` for an instance method, `Foo.bar` for one on `Foo`'s singleton
/// class, and a bare `bar` for one whose owner has no name — measured:
/// `Class.new { def z = raise }.new.z` is "in 'z'".
fn qualified_method_name(scope: &mut HandleScope<'_>, owner: ClassId, method: &str) -> String {
    let classes = scope.classes();
    let Some(name) = classes.name(owner) else {
        return method.to_owned();
    };
    if classes.is_singleton(owner) {
        // `#<Class:Foo>` for the singleton of a named module; anything else
        // (`#<Class:#<Object:...>>`) is a singleton of an object.
        return match name
            .strip_prefix("#<Class:")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            Some(attached) if !attached.starts_with("#<") => format!("{attached}.{method}"),
            _ => method.to_owned(),
        };
    }
    format!("{name}#{method}")
}

/// `path:line:in 'label'`, CRuby 3.4's spelling of one backtrace line.
fn backtrace_string(line: &BacktraceLine) -> String {
    format!("{}:{}:in '{}'", line.path, line.line, line.label)
}

/// Give `exception` the backtrace of `frames`, unless it already has one.
///
/// An exception raised a second time keeps the backtrace of the first raise —
/// `raise e` in a `rescue` is how a handler passes one on, and the position
/// that matters is where it started. Two ivars: `@backtrace` is the Array of
/// Strings `Exception#backtrace` answers, and `@__backtrace_locations__` is the
/// same lines as `[path, line, label]` for `backtrace_locations`, which
/// `core/exception.rb` turns into `Thread::Backtrace::Location`s on demand.
fn attach_backtrace(scope: &mut HandleScope<'_>, frames: &[Call], exception: Value) {
    attach_backtrace_from(scope, frames, exception, None);
}

/// [`attach_backtrace`], for an exception a primitive raised: `native` is its
/// label, a frame of its own at the caller's position, as CRuby prints a
/// method written in C.
fn attach_backtrace_from(
    scope: &mut HandleScope<'_>,
    frames: &[Call],
    exception: Value,
    native: Option<String>,
) {
    if exception.is_immediate() || !is_exception(scope, exception) {
        return;
    }
    let exception = scope.root(exception);
    // `cause` is the exception being handled when this one is first raised —
    // `$!` — and is decided once: a re-raise does not change it, and neither
    // does raising it while it is itself `$!`. An explicit `raise ..., cause:`
    // has already set it, which this leaves alone.
    let object = scope.get(exception);
    if !ivar_defined(scope, object, symbol(EXC_CAUSE)).unwrap_or(true) {
        let current = scope.errinfo();
        let cause = if current == object {
            Value::NIL
        } else {
            current
        };
        let _ = ivar_set(scope, object, symbol(EXC_CAUSE), cause);
    }
    let object = scope.get(exception);
    let Ok(existing) = ivar_get(scope, object, symbol(EXC_BACKTRACE)) else {
        return;
    };
    if existing != Value::NIL {
        return;
    }
    let mut lines = backtrace_of(scope, frames);
    // Only a primitive the program called itself, like a core method written
    // in Ruby: one `core/*.rb` called is its business, not the program's.
    if let Some(label) = native
        && let Some(caller) = frames.last()
        && !caller
            .iseq
            .path
            .as_deref()
            .is_some_and(|p| p.starts_with("<internal:"))
        && let Some(first) = lines.first()
    {
        let line = BacktraceLine {
            path: Arc::clone(&first.path),
            line: first.line,
            label,
        };
        lines.insert(0, line);
    }
    let (strings, locations) = backtrace_values(scope, &lines);
    let strings = scope.root(strings);
    let locations = scope.root(locations);
    let object = scope.get(exception);
    let strings = scope.get(strings);
    let locations = scope.get(locations);
    // A frozen exception keeps no backtrace, as in CRuby; it is still raised.
    let _ = ivar_set(scope, object, symbol(EXC_BACKTRACE), strings);
    let _ = ivar_set(scope, object, symbol(EXC_LOCATIONS), locations);
}

/// The two Arrays a backtrace is kept as: Strings, and `[path, line, label]`.
fn backtrace_values(scope: &mut HandleScope<'_>, lines: &[BacktraceLine]) -> (Value, Value) {
    let mut strings = Vec::with_capacity(lines.len());
    let mut triples = Vec::with_capacity(lines.len());
    for line in lines {
        let text = string_new(scope, &backtrace_string(line));
        strings.push(scope.root(text));
        let path = string_new(scope, &line.path);
        let path = scope.root(path);
        let label = string_new(scope, &line.label);
        let label = scope.root(label);
        let number = Value::fixnum(i64::from(line.line)).expect("a line number is a fixnum");
        let parts = [scope.get(path), number, scope.get(label)];
        let triple = new_array(scope, &parts);
        triples.push(scope.root(triple));
    }
    let strings: Vec<Value> = strings.iter().map(|&h| scope.get(h)).collect();
    let strings = new_array(scope, &strings);
    let strings = scope.root(strings);
    let triples: Vec<Value> = triples.iter().map(|&h| scope.get(h)).collect();
    let triples = new_array(scope, &triples);
    (scope.get(strings), triples)
}

/// An instance of `class`, an already-resolved exception class object.
fn exception_of(scope: &mut HandleScope<'_>, class: Value, message: &str) -> Value {
    // The message is allocated first and stays rooted in the scope, so the
    // allocation below cannot collect it out from under the slot write.
    let text = string_new(scope, message);
    let class = scope.root(class);
    let handle = alloc_ivar_object(scope, Some(class));
    let object = scope.get(handle);
    let set = |scope: &mut HandleScope<'_>, name, value| {
        ivar_set(scope, object, symbol(name), value)
            .expect("a fresh exception is unfrozen and holds instance variables");
    };
    set(scope, EXC_MESSAGE, text);
    set(scope, EXC_BACKTRACE, Value::NIL);
    object
}

/// An instance of the exception class `class` names at the top level.
///
/// This is what turns every [`Error::Raise`] the VM has emitted since #11 into
/// something a `rescue` can catch. The class name and the message text were
/// already correct — measured against CRuby where ruby/spec asserts on them —
/// so nothing here has to rediscover Ruby's wording.
fn exception_new(scope: &mut HandleScope<'_>, class: &str, message: &str) -> Value {
    // A path walks from `Object`: `Encoding::CompatibilityError` is defined by
    // `core/encoding.rb`, under a class the exception table does not list.
    let mut object = scope.classes().object(Builtin::Object.id());
    for segment in class.split("::") {
        let owner = class_id_of(scope, object).expect("a path segment names a module");
        let symbol = crate::shared::symbols::intern(segment);
        object = scope
            .classes()
            .const_get_here(owner, symbol)
            .expect("every class the VM raises is defined");
    }
    exception_of(scope, object, message)
}

/// The `NoMethodError` a call to a method the heap does not have raises.
///
/// Built here rather than in the interpreter loop's `Err` arm, where every
/// [`Error::Raise`] becomes an object, because this one carries `@receiver` and
/// an [`Error`] holds only `String`s. A `Value` parked in an `Error` would be
/// unrooted for the whole unwind, and the first `rescue` that allocated would
/// collect the receiver out from under `NameError#receiver`.
/// `lookup`, honouring a `Module#method_defined?`-style `inherit` argument.
///
/// `inherit` defaults to true and is the *second* argument of the
/// `*_method_defined?` family — not `respond_to?`'s `include_all`, which
/// selects on visibility instead. Two different questions ruby/spec asks with
/// the same shape, so they are answered apart.
fn lookup_inherited(
    scope: &mut HandleScope<'_>,
    id: ClassId,
    name: SymbolId,
    inherit: Option<&Value>,
) -> Option<Method> {
    if inherit.is_some_and(|&v| !v.is_truthy()) {
        return scope.classes().own_method(id, name);
    }
    scope.classes_mut().lookup(id, name)
}

/// Whether this call site may not reach `method`, and under which rule.
///
/// A private method is refused an explicit receiver, with the exception Ruby
/// carved out for `self.m` — the receiver being the caller's own `self` is a
/// call-site shape, not a fourth visibility. A protected method is reachable
/// only while `self` is a kind of the class that owns it, which is what makes
/// `a == b` work inside `Comparable` and not from outside.
fn visibility_refusal(
    scope: &mut HandleScope<'_>,
    frames: &[Call],
    call: &Pending,
    method: Method,
) -> Option<Visibility> {
    if call.public_only {
        return (method.visibility != Visibility::Public).then_some(method.visibility);
    }
    visibility_refusal_at(
        scope,
        method,
        call.implicit_self,
        call.receiver,
        frames.last().map(|frame| frame.receiver),
        // `self.m` reaches a private method (Ruby 2.7+), and `self.m = v`
        // always could.
        true,
    )
}

/// The rule itself, without a [`Pending`] — `defined?` asks it too.
///
/// `self_receiver_ok` is what separates the two askers, and it is a measured
/// quirk rather than a simplification. `self.priv` *runs*, and yet
/// `defined?(self.priv)` is `nil` on ruby 4.0.6:
///
/// ```ruby
/// class C
///   private def priv; end
///   def probe = [defined?(self.priv), defined?(priv)]   # [nil, "method"]
/// end
/// ```
///
/// So dispatch passes true and `defined?` passes false, and neither is
/// guessing at the other's answer.
fn visibility_refusal_at(
    scope: &mut HandleScope<'_>,
    method: Method,
    implicit_self: bool,
    receiver: Value,
    caller: Option<Value>,
    self_receiver_ok: bool,
) -> Option<Visibility> {
    match method.visibility {
        Visibility::Public => None,
        Visibility::Private => {
            if implicit_self || (self_receiver_ok && caller == Some(receiver)) {
                return None;
            }
            Some(Visibility::Private)
        }
        Visibility::Protected => {
            if implicit_self {
                return None;
            }
            let Some(caller) = caller else {
                return Some(Visibility::Protected);
            };
            let reachable = class_of(scope, caller)
                .is_some_and(|id| scope.classes().ancestors(id).contains(&method.owner));
            (!reachable).then_some(Visibility::Protected)
        }
    }
}

/// Ruby's `NoMethodError` for a method that exists but this site may not call.
///
/// The class is the same one a missing method raises and the receiver half of
/// the message is the same too; only the prefix differs, and ruby/spec asserts
/// on both. Measured on ruby 4.0.6:
///
/// ```text
/// private method 'p1' called for an instance of C
/// undefined method 'p1' for an instance of C
/// ```
fn visibility_error(
    scope: &mut HandleScope<'_>,
    receiver: Value,
    name: SymbolId,
    visibility: Visibility,
) -> Value {
    let method = symbol_name(name);
    let message = format!(
        "{} method '{method}' called for {}",
        visibility.name(),
        describe_receiver(scope, receiver)
    );
    // Deliberately *not* `note_missing_method`: this is Ruby behaviour the spec
    // may be checking, not a gap in Spinel, and marking it would report an
    // example that asserts on it as blocked.
    let rooted = scope.root(receiver);
    let object = exception_new(scope, "NoMethodError", &message);
    let handle = scope.root(object);
    let receiver = scope.get(rooted);
    let object = scope.get(handle);
    let set = |scope: &mut HandleScope<'_>, ivar, value| {
        ivar_set(scope, object, symbol(ivar), value)
            .expect("a fresh exception is unfrozen and holds instance variables");
    };
    set(scope, EXC_NAME, Value::symbol(name));
    set(scope, EXC_RECEIVER, receiver);
    object
}

/// `super` with nothing above it on the chain.
///
/// Its own message rather than `no_method_error`'s, because Ruby's names the
/// keyword: "super: no superclass method 'm' for an instance of Z". A spec
/// that asserts on the message is asserting on that word.
fn super_missing_error(scope: &mut HandleScope<'_>, receiver: Value, name: SymbolId) -> Value {
    let method = symbol_name(name);
    let message = format!(
        "super: no superclass method '{method}' for {}",
        describe_receiver(scope, receiver)
    );
    // Not `note_missing_method`: this is Ruby behaviour a spec may be checking
    // — `super_spec.rb` asserts on it — rather than a gap in Spinel.
    let rooted = scope.root(receiver);
    let object = exception_new(scope, "NoMethodError", &message);
    let handle = scope.root(object);
    let receiver = scope.get(rooted);
    let object = scope.get(handle);
    let set = |scope: &mut HandleScope<'_>, ivar, value| {
        ivar_set(scope, object, symbol(ivar), value)
            .expect("a fresh exception is unfrozen and holds instance variables");
    };
    set(scope, EXC_NAME, Value::symbol(name));
    set(scope, EXC_RECEIVER, receiver);
    object
}

fn no_method_error(scope: &mut HandleScope<'_>, receiver: Value, name: SymbolId) -> Value {
    let method = symbol_name(name);
    let message = format!(
        "undefined method '{method}' for {}",
        describe_receiver(scope, receiver)
    );
    // The heap remembers that a gap was raised for, so a spec that swallows it
    // in a `rescue` and then fails is reported blocked rather than as a
    // disagreement. See `Heap::missing_method`.
    scope.note_missing_method(&message);
    // Rooted across the allocations below. The collector is mark-sweep and does
    // not move, so the `Value` read back stays valid; the handle is what keeps
    // it from being swept while `exception_new` allocates.
    let rooted = scope.root(receiver);
    let object = exception_new(scope, "NoMethodError", &message);
    let handle = scope.root(object);
    let receiver = scope.get(rooted);
    let object = scope.get(handle);
    let set = |scope: &mut HandleScope<'_>, ivar, value| {
        ivar_set(scope, object, symbol(ivar), value)
            .expect("a fresh exception is unfrozen and holds instance variables");
    };
    set(scope, EXC_NAME, Value::symbol(name));
    set(scope, EXC_RECEIVER, receiver);
    object
}

/// How CRuby names a receiver in a `NoMethodError`, measured on ruby 4.0.6.
///
/// There is no one wording: `nil`, `true`, `false`, a class, and a module each
/// read differently from an ordinary instance, and three of those are text a
/// ruby/spec example asserts on. The rows are in
/// `crates/spinel-vm/tests/eval.txt`, so `scripts/eval-oracle.rb` re-checks
/// them against a real Ruby rather than against this comment.
fn describe_receiver(scope: &mut HandleScope<'_>, receiver: Value) -> String {
    use crate::value::Unpacked;
    match receiver.unpack() {
        Unpacked::Nil => return "nil".to_owned(),
        Unpacked::True => return "true".to_owned(),
        Unpacked::False => return "false".to_owned(),
        _ => {}
    }
    // A class or module names itself; anything else names its class. CRuby
    // writes `#<Class:0x0000…>` for an anonymous one and the address differs
    // per run, so no table can hold it — Spinel keeps its own wording, which is
    // what every other report in this file already says.
    if let Some(id) = class_id_of(scope, receiver) {
        let kind = scope.classes().kind(id);
        let (word, fallback) = match kind {
            crate::class::Kind::Module => ("module", "an anonymous module"),
            crate::class::Kind::Class => ("class", "an anonymous class"),
        };
        return match scope.classes().name(id) {
            Some(name) => format!("{word} {name}"),
            None => fallback.to_owned(),
        };
    }
    format!("an instance of {}", class_name_of(scope, receiver))
}

/// An exception's message, for a report. Empty when it is not one.
fn exception_message(scope: &mut HandleScope<'_>, exception: Value) -> String {
    if exception.is_immediate() {
        return String::new();
    }
    let Ok(text) = ivar_get(scope, exception, symbol(EXC_MESSAGE)) else {
        return String::new();
    };
    if text.is_immediate() {
        return String::new();
    }
    match string_bytes(scope, text) {
        Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        None => String::new(),
    }
}

/// The name of a value's class, for a report.
/// `class 'Foo'` or `module 'Bar'`, the way `NameError` names an owner.
///
/// Measured: `class Zz; alias b nope; end` says `undefined method 'nope' for
/// class 'Zz'`, and the same inside a `module` says `module 'Mm'`. An anonymous
/// one gets `#<Class:0x...>`, which is `Module#to_s` and lives in Ruby, so this
/// prints the name it has and leaves the address to a slice that can send.
fn class_display_name(scope: &mut HandleScope<'_>, id: crate::class::ClassId) -> String {
    let kind = match scope.classes().kind(id) {
        crate::class::Kind::Class => "class",
        crate::class::Kind::Module => "module",
    };
    match scope.classes().name(id) {
        Some(name) => format!("{kind} '{name}'"),
        None => format!("an anonymous {kind}"),
    }
}

fn class_name_of(scope: &mut HandleScope<'_>, value: Value) -> String {
    class_of(scope, value)
        .and_then(|id| scope.classes().name(id).map(str::to_owned))
        .unwrap_or_else(|| "an anonymous class".to_owned())
}

/// Whether `exception` is an instance of the class `class` names.
///
/// `rescue` against something that is not a class or module is Ruby's
/// `TypeError`, not a quiet `false`: a spec that writes `rescue 1` is asserting
/// on that message.
fn exception_matches(
    scope: &mut HandleScope<'_>,
    exception: Value,
    class: Value,
) -> Result<bool, Error> {
    let Some(wanted) = class_id_of(scope, class) else {
        return Err(Error::raise(
            "TypeError",
            "class or module required for rescue clause",
        ));
    };
    // Ruby's `rescue X` is `X === exception`, and `Module#===` is `is_a?` —
    // the ancestor walk below, which is why it has always agreed. A class that
    // *overrides* `===` gets a different answer, and Spinel cannot ask it from
    // here: `CheckMatch` is one instruction and a Ruby call needs a frame.
    // `core/module.rb`'s `Module#===` is the default, so the override test is
    // "the method this lookup found is not that one".
    let comparison = crate::shared::symbols::intern("===");
    if let Some(class_of_class) = class_of(scope, class)
        && let Some(method) = scope.classes_mut().lookup(class_of_class, comparison)
        && method.owner != Builtin::Module.id()
    {
        return Err(Error::Unknowable {
            what: "`rescue` against a class with its own `===`",
            needs: "a Ruby call from the exception matcher (#28)",
        });
    }
    let Some(actual) = class_of(scope, exception) else {
        return Ok(false);
    };
    Ok(scope.classes().ancestors(actual).contains(&wanted))
}

/// A [`CrefId`] as a `Value`, for the slots that must hold one.
///
/// A fixnum, the way a method body is a fixnum into `Definitions`: the arena
/// index is meaningful only inside its own heap, and a `Proc` never outlives it.
fn cref_value(cref: CrefId) -> Value {
    Value::fixnum(cref.index() as i64).expect("a heap holds far under a fixnum of scopes")
}

/// Read a [`CrefId`] back out of a slot written by [`cref_value`].
fn cref_from(value: Value) -> CrefId {
    match value.unpack() {
        crate::value::Unpacked::Fixnum(n) if n >= 0 => CrefId::from_index(n as usize),
        // A `Proc` built before this slot existed, or a slot the collector has
        // not written. The top level is the only scope that is always valid.
        _ => CrefId::ROOT,
    }
}

/// The definition id inside a `Proc`, or `None` if the value is not one.
fn proc_body(scope: &mut HandleScope<'_>, value: Value) -> Option<Value> {
    if value.is_immediate() {
        return None;
    }
    let handle = scope.root(value);
    if scope.payload(handle) != Payload::Slots || scope.len(handle) != PROC_SLOTS {
        return None;
    }
    // Likewise a subclass of `Proc`: the slot count above has already
    // established the shape, and the representation is what says whose it is.
    let class = scope.class_of(handle)?;
    (scope.classes().repr(class) == Some(Builtin::Proc)).then(|| scope.slot(handle, PROC_BODY))
}

/// Everything needed to call a `Proc`.
fn proc_parts(
    scope: &mut HandleScope<'_>,
    value: Value,
) -> Option<(Arc<Iseq>, Value, Value, Value, bool, CrefId)> {
    let body = proc_body(scope, value)?;
    let iseq = match scope.definitions().get(body)? {
        Definition::Iseq(iseq) => Arc::clone(iseq),
        Definition::Native(_) | Definition::Proc(_) => return None,
    };
    let handle = scope.root(value);
    Some((
        iseq,
        scope.slot(handle, PROC_ENV),
        scope.slot(handle, PROC_SELF),
        scope.slot(handle, PROC_BLOCK),
        scope.slot(handle, PROC_LAMBDA).is_truthy(),
        cref_from(scope.slot(handle, PROC_CREF)),
    ))
}

/// Open a `class`, `module`, or `class << obj` body in a new frame.
///
/// Three steps, in Ruby's order:
///
/// 1. find the definee — reopen it, or create it and bind the constant;
/// 2. push a lexical scope inside the enclosing one;
/// 3. push a frame whose `self` *is* the module, which is what makes `def`
///    land on it and `def self.x` reach its singleton.
///
/// The body's value is the frame's value, so `x = class C; 42; end` is `42`.
fn open_class<'h>(
    scope: &mut HandleScope<'h>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    def: &ClassDef,
    iseq: &Arc<Iseq>,
    proc_class: Handle<'h>,
    ids: &mut u64,
) -> Result<Option<Unwind>, Error> {
    let top = frames.len() - 1;
    let outer = frames[top].cref;

    let (id, hooks) = match def.kind {
        DefKind::Singleton => {
            let object = stack.pop().expect("an object to open the singleton of");
            (singleton_of(scope, object)?, Vec::new())
        }
        kind => {
            let superclass = def
                .superclass
                .then(|| stack.pop().expect("a superclass to inherit from"));
            // `class A::B` names its definee; a plain `class B` uses the scope
            // it is written in.
            let cbase = match def.scoped {
                true => {
                    let value = stack.pop().expect("a module to define in");
                    class_id_of(scope, value).ok_or_else(|| {
                        Error::raise(
                            "TypeError",
                            format!("{} is not a class/module", inspect(scope, value)),
                        )
                    })?
                }
                // `cref_base`: `class Deep; end` written inside `Class.new`'s
                // block defines `Deep` at the enclosing scope, not under the
                // anonymous class. Measured — `anonymous.txt`.
                false => scope.classes().cref_base(outer),
            };
            let name = frames[top].symbols[def.name as usize];
            // `class A::B` is a qualified reference, so a private `B` is a
            // `NameError` — while a plain `class B` written inside `A`'s own
            // body reopens it, because that is the scope it is private to.
            // Measured; the two "cannot be reopened" examples in
            // `language/constants_spec.rb` are this (#185).
            if def.scoped && scope.classes().const_is_private(cbase, name) {
                return Err(uninitialized(scope, cbase, name, ConstScope::Qualified));
            }
            define_or_reopen(scope, cbase, name, kind, superclass)?
        }
    };

    let cref = scope.classes_mut().push_cref(outer, id);
    let receiver = scope.classes().object(id);
    let body = Arc::clone(&iseq.children[def.body as usize]);
    let env = env_alloc(scope, Value::NIL, body.locals.len());
    let symbols = body.link();
    let cache_base = scope.call_caches_mut().base(&body);
    *ids += 1;
    // Which frame a `return` written in this body leaves.
    //
    // A `class`/`module` body is its own target, and Prism refuses a `return`
    // written directly in one — "Invalid return in class/module body" — so this
    // only ever answers a `return` reached through a block, where Ruby's answer
    // is `LocalJumpError`. It is its own id for the same reason it always was:
    // homing it outward would walk into the enclosing method instead.
    //
    // A `class << obj` body is *transparent* instead: `return` in one leaves the
    // enclosing method, so it inherits whatever its opener would have returned
    // from. Measured on ruby 4.0.6 — directly in the body, from a block or a
    // `proc` inside it, and through a nested singleton body, all four return
    // from the method (#204).
    //
    // Except at the top level, where there is no method to leave and Ruby raises
    // `LocalJumpError` rather than ending the script. `home: 0` names no frame,
    // which is how `break` outside a block already says the same thing.
    let home = match def.kind {
        DefKind::Singleton if frames[top].home != frames[0].id => frames[top].home,
        DefKind::Singleton => 0,
        _ => *ids,
    };
    let links = Links {
        id: *ids,
        home,
        breaks: 0,
        // A class body starts public every time it is entered, which is why
        // reopening a class after a bare `private` is public again.
        scope_default: ScopeDefault::Public,
    };
    frames.push(Call {
        iseq: body,
        symbols,
        cache_base,
        env,
        receiver,
        cref,
        scope_default: links.scope_default,
        // A class body is not a method body, so a `super` written directly in
        // one has no owner to be a step past.
        owner: None,
        defined_as: None,
        // A class body is not called with a block, so `yield` inside one is a
        // `LocalJumpError` — which is Ruby.
        block: Value::NIL,
        pc: 0,
        base: stack.len(),
        keeps_receiver: false,
        raises_receiver: false,
        discards_value: false,
        booleanizes_value: false,
        // Where a `return` in this body goes — see `links` above, which is the
        // one place that decides it. A `class`/`module` body is its own target;
        // a `class << obj` body is transparent and inherits its opener's.
        id: links.id,
        home: links.home,
        breaks: 0,
        tag: None,
        rescued: None,
        errinfo_on_entry: scope.errinfo(),
        parked: Vec::new(),
        boundary_limit: 0,
    });
    // A new definition's hooks run before its body: each is a frame above the
    // body's, pushed last-first so they run in order.
    for (receiver, name, args) in hooks.into_iter().rev() {
        if let Some(unwind) =
            fire_hook(scope, stack, frames, proc_class, ids, receiver, name, args)?
        {
            return Ok(Some(unwind));
        }
    }
    Ok(None)
}

/// Find the module `name` names on `cbase`, or create it.
///
/// The existence check is `cbase`'s **own** table, never its ancestors, which is
/// what makes this true:
///
/// ```ruby
/// class P; class Inner; end; end
/// class Q < P
///   class Inner; end     # Q::Inner — a new class, not a reopening of P::Inner
/// end
/// ```
/// The hooks a definition owes, in the order they run: receiver, name, args.
type Hooks = Vec<(Value, &'static str, Vec<Value>)>;

fn define_or_reopen(
    scope: &mut HandleScope<'_>,
    cbase: ClassId,
    name: SymbolId,
    kind: DefKind,
    superclass: Option<Value>,
) -> Result<(ClassId, Hooks), Error> {
    let wanted = match superclass {
        None => None,
        Some(value) => {
            let id =
                class_id_of(scope, value).filter(|&id| scope.classes().kind(id) == Kind::Class);
            let Some(id) = id else {
                return Err(Error::raise(
                    "TypeError",
                    format!(
                        "superclass must be an instance of Class (given an instance of {})",
                        class_name(scope, value)
                    ),
                ));
            };
            Some(id)
        }
    };

    if let Some(existing) = scope.classes().const_get_here(cbase, name) {
        let Some(id) = class_id_of(scope, existing) else {
            return Err(Error::raise(
                "TypeError",
                format!("{} is not a class", symbol_name(name)),
            ));
        };
        let found = scope.classes().kind(id);
        if found != kind_of(kind) {
            let noun = match kind {
                DefKind::Module => "module",
                _ => "class",
            };
            return Err(Error::raise(
                "TypeError",
                format!("{} is not a {noun}", symbol_name(name)),
            ));
        }
        // Reopening with an explicit superclass must name the same one. Ruby
        // checks this before running a line of the body.
        if let Some(wanted) = wanted
            && scope.classes().superclass(id) != Some(wanted)
        {
            return Err(Error::raise(
                "TypeError",
                format!("superclass mismatch for class {}", symbol_name(name)),
            ));
        }
        return Ok((id, Vec::new()));
    }

    let path = qualified_name(scope, cbase, name);
    let id = match kind {
        DefKind::Module => scope.define_module(Some(&path)),
        // No superclass named means `Object`, which is Ruby's default and is
        // what makes a bare `class C` an `Object` subclass.
        _ => scope.define_class(Some(&path), Some(wanted.unwrap_or(Builtin::Object.id()))),
    };
    // CRuby builds a class's metaclass in `rb_define_class`, not on first ask,
    // and the reason is inheritance: `class B < A` with `def self.m` on `A`
    // reaches `m` through `#<Class:B> < #<Class:A>`. Left lazy, `B` would still
    // point at `Class` and the call would miss. `HandleScope::define_class`
    // stays lazy; it is the `class` *keyword* that owes the link.
    if kind != DefKind::Module {
        scope.singleton_class(id);
    }
    let object = scope.classes().object(id);
    scope.classes_mut().const_set(cbase, name, object);
    // The hooks a new definition owes (#28), in CRuby's order, measured:
    // `const_added` on the module it is named in — `class A::C` is a constant
    // assignment like any other — then, for a class, `inherited` on its
    // superclass, both before the body runs. Reopening fires neither, and
    // returned above.
    let mut hooks: Hooks = vec![(
        scope.classes().object(cbase),
        "const_added",
        vec![Value::symbol(name)],
    )];
    if kind != DefKind::Module {
        let parent = scope
            .classes()
            .object(wanted.unwrap_or(Builtin::Object.id()));
        hooks.push((parent, "inherited", vec![object]));
    }
    Ok((id, hooks))
}

/// `Module#name`: `"A::B"` inside `A`, and `"B"` at the top level.
/// `Class.new` and `Module.new`: a class or module with no name.
///
/// The block, when there is one, is `module_eval`: it runs with `self` set to
/// the new class and with a cref that is the *definee* only — `def` inside it
/// lands on the class, while a constant read, a constant assignment, and a
/// bare `class Foo` all resolve in the scope the block was written in. See
/// [`CrefNode::pushed_by_eval`] and `crates/spinel-vm/tests/anonymous.txt`.
///
/// [`CrefNode::pushed_by_eval`]: crate::class::CrefNode
fn anonymous_module<'h>(
    scope: &mut HandleScope<'h>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    call: &Pending,
    receiver: ClassId,
    proc_class: Handle<'h>,
    ids: &mut u64,
) -> Result<Option<Unwind>, Error> {
    let building_class = receiver == Builtin::Class.id();
    let given = call.args.len() + call.keywords.len();

    // `Module.new` takes nothing; `Class.new` takes an optional superclass.
    // Ruby's own wording, because ruby/spec checks the message text.
    let most = usize::from(building_class);
    if given > most {
        let expected = if building_class { "0..1" } else { "0" };
        return Err(Error::raise(
            "ArgumentError",
            format!("wrong number of arguments (given {given}, expected {expected})"),
        ));
    }

    let id = if building_class {
        let superclass = match call.args.first() {
            None => Builtin::Object.id(),
            Some(&value) => superclass_of(scope, value)?,
        };
        let id = scope.define_class(None, Some(superclass));
        // The same link `class C < A` owes: `#<Class:C> < #<Class:A>` is what
        // makes an inherited `def self.m` reachable. See `define_or_reopen`.
        scope.singleton_class(id);
        id
    } else {
        scope.define_module(None)
    };

    let object = scope.classes().object(id);
    // Below the block's frame base, so `Insn::Leave` leaves it behind when the
    // block ends: `Class.new { 42 }` answers the class, not 42. The same trick
    // `new` uses to answer the object rather than what `initialize` returned.
    stack.push(object);

    // `inherited` on the superclass (#28), before the block, measured — a
    // frame above the block's, so it runs first.
    let inherited = building_class.then(|| {
        let parent = scope
            .classes()
            .superclass(id)
            .unwrap_or(Builtin::Object.id());
        scope.classes().object(parent)
    });

    if call.block == Value::NIL {
        if let Some(parent) = inherited {
            return fire_hook(
                scope,
                stack,
                frames,
                proc_class,
                ids,
                parent,
                "inherited",
                vec![object],
            );
        }
        return Ok(None);
    }

    let block = call.block;
    let inner = Pending {
        cache: None,
        name: call.name,
        receiver: block,
        // Ruby yields the new module to the block as well as making it `self`,
        // so `Module.new { |mod| ... }` and `Module.new { self }` agree.
        args: vec![object],
        keywords: Vec::new(),
        block: Value::NIL,
        block_is_literal: false,
        cref: call.cref,
        implicit_self: false,
        public_only: false,
        target: Target::Block(block),
        owner: None,
        defined_as: None,
    };
    push_proc_frame(scope, stack, frames, &inner, block, ids)?;
    let cref = scope.classes_mut().push_eval_cref(inner.cref, id);
    let last = frames.len() - 1;
    frames[last].receiver = object;
    frames[last].cref = cref;
    frames[last].keeps_receiver = true;
    // `Class.new { ... }` runs its block as a *class body*, so it starts public
    // however visible the scope that wrote the literal was — a top-level block
    // would otherwise inherit the private default and give
    // `Class.new { attr_writer :a }` a setter nobody can call.
    frames[last].scope_default = ScopeDefault::Public;
    if let Some(parent) = inherited {
        return fire_hook(
            scope,
            stack,
            frames,
            proc_class,
            ids,
            parent,
            "inherited",
            vec![object],
        );
    }
    Ok(None)
}

/// The `ClassId` a value may be made the superclass of, or Ruby's refusal.
fn superclass_of(scope: &mut HandleScope<'_>, value: Value) -> Result<ClassId, Error> {
    let id = class_id_of(scope, value).filter(|&id| scope.classes().kind(id) == Kind::Class);
    let Some(id) = id else {
        return Err(Error::raise(
            "TypeError",
            format!(
                "superclass must be an instance of Class (given an instance of {})",
                class_name(scope, value)
            ),
        ));
    };
    // Both of these are a `Class` and so pass the check above, and both are
    // refused for the same reason: the subclass would need a metaclass Ruby
    // cannot build. The messages are Ruby's own.
    if scope.classes().is_singleton(id) {
        return Err(Error::raise(
            "TypeError",
            "can't make subclass of singleton class",
        ));
    }
    if id == Builtin::Class.id() {
        return Err(Error::raise("TypeError", "can't make subclass of Class"));
    }
    Ok(id)
}

/// Refuse an operation that Ruby would answer by calling a definition hook.
///
/// Ruby fires `inherited`, `method_added`, `const_added`, `included`,
/// `prepended` and their `*_features` partners at the moment a definition
/// happens. Spinel does not: firing one needs a primitive that pushes the
/// hook's frame and then carries on with the definition, which a native cannot
/// do. Going ahead without it would leave a program that watches its own
/// definitions reporting a state it never reached — silently, and with a green
/// result — which is strictly worse than saying the VM cannot get there.
///
/// That is #15's rule for `singleton_method_added`, generalised. The hooks
/// themselves are #28's, and this is the list it deletes.
///
/// `owner` is the object the hook would be called *on*: the superclass for
/// `inherited`, the module for `method_added` and `const_added`, the module
/// being mixed in for `included` and `prepended`. Costs one cached method
/// lookup per definition, and only a heap where someone wrote a hook refuses.
/// `@@a` from `id`'s ancestry, or Ruby's `RuntimeError` where two classes in it
/// hold the name.
fn cvar_read(
    scope: &mut HandleScope<'_>,
    id: ClassId,
    name: SymbolId,
) -> Result<Option<Value>, Error> {
    match scope.classes().cvar_get(id, name) {
        Ok(value) => Ok(value),
        Err(overtaken) => {
            let of = class_or_anonymous(scope, id);
            let by = class_or_anonymous(scope, overtaken);
            Err(Error::raise(
                "RuntimeError",
                format!(
                    "class variable {} of {of} is overtaken by {by}",
                    symbol_name(name)
                ),
            ))
        }
    }
}

fn class_or_anonymous(scope: &mut HandleScope<'_>, id: ClassId) -> String {
    scope
        .classes()
        .name(id)
        .map_or_else(|| "an anonymous class".to_owned(), str::to_owned)
}

/// The class a `@@a` in this frame belongs to.
///
/// The cref's class, the same one `def` writes into. At the top level there is
/// no class to own one and Ruby raises rather than reaching for `Object`:
/// `class variable access from toplevel`, measured.
fn cvar_owner(scope: &mut HandleScope<'_>, cref: CrefId) -> Result<ClassId, Error> {
    let cref = scope.classes().lexical_cref(cref);
    if cref == CrefId::ROOT {
        return Err(Error::raise(
            "RuntimeError",
            "class variable access from toplevel",
        ));
    }
    // `class << self; @@a = 1; end` writes the class's own `@@a`: through a
    // singleton class to the module it belongs to, as CRuby does.
    let mut class = scope.classes().cref_class(cref);
    while scope.classes().is_singleton(class) {
        let attached = singleton_attached(scope, class);
        match class_id_of(scope, attached) {
            Some(module) => class = module,
            None => break,
        }
    }
    Ok(class)
}

/// Whether a method name is one of the core library's own `__name__` helpers,
/// which reflection leaves out. CRuby's own five are real methods and stay.
fn is_core_helper(name: SymbolId) -> bool {
    let Some(text) = crate::shared::symbols::name(name) else {
        return false;
    };
    text.len() > 4
        && text.starts_with("__")
        && text.ends_with("__")
        && !matches!(
            text.as_str(),
            "__send__" | "__id__" | "__method__" | "__callee__" | "__dir__"
        )
}

/// The call re-aimed at the receiver's `method_missing`, when the program
/// defines one (#28): the original name first, then the original arguments,
/// keywords and block. `BasicObject#method_missing` — the default, which
/// raises — does not count; the caller builds that error itself, with the
/// frames it has in hand.
fn to_method_missing(
    scope: &mut HandleScope<'_>,
    class: ClassId,
    call: &Pending,
) -> Option<Pending> {
    let symbol = crate::shared::symbols::intern("method_missing");
    let method = scope.classes_mut().lookup(class, symbol)?;
    if method.owner == Builtin::BasicObject.id() {
        return None;
    }
    let mut args = Vec::with_capacity(call.args.len() + 1);
    args.push(Value::symbol(call.name));
    args.extend_from_slice(&call.args);
    Some(Pending {
        cache: None,
        name: symbol,
        receiver: call.receiver,
        args,
        keywords: call.keywords.clone(),
        block: call.block,
        block_is_literal: call.block_is_literal,
        cref: call.cref,
        // `method_missing` is private, and calling it is the VM's doing, not
        // the caller's.
        implicit_self: true,
        public_only: false,
        target: Target::Method,
        owner: None,
        defined_as: None,
    })
}

/// The object a singleton class belongs to, or nil for one that is not.
fn singleton_attached(scope: &mut HandleScope<'_>, class: ClassId) -> Value {
    scope.classes().attached(class).unwrap_or(Value::NIL)
}

/// `method_<event>` on `owner`, or `singleton_method_<event>` on the object
/// behind it when `owner` is a singleton class — `added`, `removed` or
/// `undefined`, after the change it reports.
#[allow(clippy::too_many_arguments)]
fn fire_method_hook<'h>(
    scope: &mut HandleScope<'h>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    proc_class: Handle<'h>,
    ids: &mut u64,
    owner: ClassId,
    event: &str,
    symbol: crate::value::SymbolId,
) -> Result<Option<Unwind>, Error> {
    let (hook, receiver) = if scope.classes().is_singleton(owner) {
        (
            format!("singleton_method_{event}"),
            singleton_attached(scope, owner),
        )
    } else {
        (format!("method_{event}"), scope.classes().object(owner))
    };
    fire_hook(
        scope,
        stack,
        frames,
        proc_class,
        ids,
        receiver,
        &hook,
        vec![Value::symbol(symbol)],
    )
}

/// Run a definition hook (#28): `receiver.name(*args)`, when something
/// overrides the core library's default.
///
/// The defaults are private no-ops in `core/*.rb` on `BasicObject`, `Module`
/// and `Class`, so a hook only fires when the method found is not one of
/// theirs — the common case, a class with no hook, costs one cached lookup.
/// The hook runs as a frame on top of the stack, so it runs before the
/// instruction that fired it carries on, and its value is discarded: what the
/// definition answers is already below it. Hooks are private, so the call is
/// receiverless.
#[allow(clippy::too_many_arguments)]
fn fire_hook<'h>(
    scope: &mut HandleScope<'h>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    proc_class: Handle<'h>,
    ids: &mut u64,
    receiver: Value,
    name: &str,
    args: Vec<Value>,
) -> Result<Option<Unwind>, Error> {
    let symbol = crate::shared::symbols::intern(name);
    let Some(class) = class_of(scope, receiver) else {
        return Ok(None);
    };
    // An `undef` of the default sends the call on to `method_missing` or a
    // NoMethodError, as in CRuby; no method at all is the core library still
    // loading, before the defaults exist.
    match scope.classes_mut().lookup(class, symbol) {
        None if !scope.classes().undefined_along(class, symbol) => return Ok(None),
        None => {}
        Some(method) => {
            let defaults = [
                Builtin::BasicObject.id(),
                Builtin::Module.id(),
                Builtin::Class.id(),
                Builtin::Kernel.id(),
            ];
            if defaults.contains(&method.owner) {
                return Ok(None);
            }
        }
    }
    let cref = frames.last().map_or(CrefId::ROOT, |frame| frame.cref);
    let call = Pending {
        cache: None,
        name: symbol,
        receiver,
        args,
        keywords: Vec::new(),
        block: Value::NIL,
        block_is_literal: false,
        cref,
        implicit_self: true,
        public_only: false,
        target: Target::Method,
        owner: None,
        defined_as: None,
    };
    let (depth, height) = (frames.len(), stack.len());
    if let Some(unwind) = dispatch(scope, stack, frames, call, proc_class, ids)? {
        return Ok(Some(unwind));
    }
    if frames.len() > depth {
        let last = frames.len() - 1;
        frames[last].discards_value = true;
    } else {
        // A native hook answered on the stack; that answer is not wanted.
        stack.truncate(height);
    }
    Ok(None)
}

/// Name a class or module that a constant assignment just gave a name to.
///
/// Ruby has no `Module#name=`: assigning an anonymous class to a constant is
/// the naming operation, and the name is the constant's *path*, so
/// `NS::Inner = Class.new` answers `"NS::Inner"` rather than `"Inner"`.
///
/// A no-op for anything that is not a class, and for a class that already has a
/// name — including one this heap named a moment ago through another constant.
fn name_if_anonymous(scope: &mut HandleScope<'_>, cbase: ClassId, name: SymbolId, value: Value) {
    let Some(id) = class_id_of(scope, value) else {
        return;
    };
    if scope.classes().name(id).is_some() {
        return;
    }
    let path = qualified_name(scope, cbase, name);
    scope.classes_mut().name_if_anonymous(id, &path);
}

fn qualified_name(scope: &mut HandleScope<'_>, cbase: ClassId, name: SymbolId) -> String {
    let leaf = symbol_name(name);
    match scope.classes().name(cbase) {
        Some(outer) if cbase != Builtin::Object.id() => format!("{outer}::{leaf}"),
        // Under an anonymous module the path starts with that module's
        // `inspect`, measured: `m = Module.new; module m::N; end` names it
        // "#<Module:0x...>::N" — the address being `object_id` in hex, which
        // is what `Module#to_s` prints for `m` itself.
        //
        // ponytail: CRuby keeps that as a *temporary* name and renames `N` to
        // "A::N" when `m` is later assigned to `A`. Here it is permanent;
        // `set_temporary_name` and the rename belong to #28's reflection slice.
        None if cbase != Builtin::Object.id() => {
            let object = scope.classes().object(cbase);
            let handle = scope.root(object);
            let id = scope.address(handle) >> 4;
            let noun = match scope.classes().kind(cbase) {
                Kind::Module => "Module",
                Kind::Class => "Class",
            };
            format!("#<{noun}:0x{id:x}>::{leaf}")
        }
        _ => leaf,
    }
}

fn kind_of(kind: DefKind) -> Kind {
    match kind {
        DefKind::Module => Kind::Module,
        _ => Kind::Class,
    }
}

/// The name of a value's class, for a message that has to name a type.
/// What Ruby's messages mean by "an instance of X".
///
/// Singletons are skipped, the same walk `Object#class` does: `Comparable` is
/// an instance of `Module`, not of `#<Class:Comparable>`, and a module only
/// grows a singleton when someone defines a method on it — so without the skip
/// the wording of a `TypeError` would depend on whether anything had.
fn class_name(scope: &mut HandleScope<'_>, value: Value) -> String {
    let Some(mut id) = class_of(scope, value) else {
        return inspect(scope, value);
    };
    while scope.classes().is_singleton(id) {
        let Some(up) = scope.classes().superclass(id) else {
            break;
        };
        id = up;
    }
    scope
        .classes()
        .name(id)
        .map(str::to_string)
        .unwrap_or_else(|| inspect(scope, value))
}

/// Define a method, remembering the scope its `def` was written in.
fn define_method_on(
    scope: &mut HandleScope<'_>,
    owner: ClassId,
    name: SymbolId,
    iseq: Arc<Iseq>,
    cref: CrefId,
    visibility: Visibility,
) {
    let body = scope
        .definitions_mut()
        .intern_iseq(&iseq, Arc::as_ptr(&iseq) as usize);
    scope
        .classes_mut()
        .define_method_visibly(owner, name, body, cref, visibility);
}

/// The singleton class of a value, allocating it on the first ask.
///
/// `def self.foo`, `def obj.foo`, and `class << obj` all land here. An immediate
/// — a fixnum, a symbol, `nil` — has no singleton in Ruby either; the message is
/// the one Ruby uses.
fn singleton_of(scope: &mut HandleScope<'_>, receiver: Value) -> Result<ClassId, Error> {
    if let Some(id) = class_id_of(scope, receiver) {
        // A class or module: its singleton is where `def self.foo` goes.
        return Ok(scope.singleton_class(id));
    }
    // `nil`, `true` and `false` are immediates that do have singleton classes:
    // Ruby answers `NilClass`, `TrueClass` and `FalseClass`, which *are* the
    // singleton, so a definition on one goes straight into that class. #15 is
    // what this was waiting for, and #15 has landed.
    match receiver.unpack() {
        crate::value::Unpacked::Nil => return Ok(Builtin::NilClass.id()),
        crate::value::Unpacked::True => return Ok(Builtin::TrueClass.id()),
        crate::value::Unpacked::False => return Ok(Builtin::FalseClass.id()),
        _ => {}
    }
    if receiver.is_immediate() {
        // Ruby's text exactly: no receiver in it, and no article.
        return Err(Error::raise("TypeError", "can't define singleton"));
    }
    let handle = scope.root(receiver);
    Ok(scope.singleton_class_of(handle))
}

/// The class table entry a value *is*, as opposed to the one it is an instance
/// of. `Some` only for a class or module object.
fn class_id_of(scope: &mut HandleScope<'_>, value: Value) -> Option<ClassId> {
    if value.is_immediate() {
        return None;
    }
    let handle = scope.root(value);
    scope.class_id_of(handle)
}

/// Where a constant reference reads from, and what it pops to get there.
fn const_base(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    cref: CrefId,
    how: ConstScope,
) -> Result<ClassId, Error> {
    match how {
        // `const_get` walks the chain itself; the innermost scope is only what
        // a `NameError` would name. `cref_base`, not `cref_class`: `X = 1` in
        // the block of `Class.new` writes to the enclosing scope, not the class.
        ConstScope::Lexical => Ok(scope.classes().cref_base(cref)),
        ConstScope::Top => Ok(Builtin::Object.id()),
        ConstScope::Qualified => {
            let value = stack.pop().expect("a module to look the constant up in");
            class_id_of(scope, value).ok_or_else(|| {
                Error::raise(
                    "TypeError",
                    format!("{} is not a class/module", inspect(scope, value)),
                )
            })
        }
    }
}

/// Ruby's message for a constant that is not there. Qualified references name
/// the module they searched; a bare one names only the constant.
fn uninitialized(
    scope: &mut HandleScope<'_>,
    from: ClassId,
    name: SymbolId,
    how: ConstScope,
) -> Error {
    let constant = symbol_name(name);
    match how {
        ConstScope::Lexical | ConstScope::Top => {
            Error::raise("NameError", format!("uninitialized constant {constant}"))
        }
        ConstScope::Qualified => {
            let owner = scope
                .classes()
                .name(from)
                .unwrap_or("an anonymous module")
                .to_string();
            // A name the walk found and refused reads differently from one it
            // never found, and ruby/spec asserts on both (#185).
            if scope.classes().const_private_qualified(from, name) {
                return Error::raise(
                    "NameError",
                    format!("private constant {owner}::{constant} referenced"),
                );
            }
            Error::raise(
                "NameError",
                format!("uninitialized constant {owner}::{constant}"),
            )
        }
    }
}

/// The symbol an `alias`/`undef` name was built into.
///
/// The compiler only ever pushes a symbol here — a symbol literal or #231's
/// `Intern` over an interpolated one — so anything else is a compiler bug
/// rather than a program error, and it is reported as a refusal instead of
/// being guessed at.
fn pop_name(
    stack: &mut Vec<Value>,
    keyword: &'static str,
) -> Result<crate::value::SymbolId, Error> {
    let value = stack.pop().expect("a name to alias or undef");
    value.as_symbol().ok_or(Error::NoDispatch {
        op: keyword,
        operands: "a method name that is not a Symbol",
    })
}

/// A frozen module refuses a change to its method table, naming itself the
/// way every mutator's `FrozenError` does: "can't modify frozen Class: C".
fn module_frozen_check(scope: &mut HandleScope<'_>, id: ClassId) -> Result<(), Error> {
    let object = scope.classes().object(id);
    let handle = scope.root(object);
    if scope.is_frozen(handle) {
        let class = class_name(scope, object);
        let shown = inspect(scope, object);
        return Err(Error::raise(
            "FrozenError",
            format!("can't modify frozen {class}: {shown}"),
        ));
    }
    Ok(())
}

/// `alias new old` in `owner`, however the two names were spelled.
fn alias_into<'h>(
    scope: &mut HandleScope<'h>,
    owner: ClassId,
    new: crate::value::SymbolId,
    old: crate::value::SymbolId,
) -> Result<(), Error> {
    module_frozen_check(scope, owner)?;
    if !scope.classes_mut().alias_method(owner, new, old) {
        return Err(Error::raise(
            "NameError",
            format!(
                "undefined method '{}' for {}",
                symbol_name(old),
                class_display_name(scope, owner),
            ),
        ));
    }
    Ok(())
}

/// `undef name` in `owner`, however the name was spelled.
fn undef_from<'h>(
    scope: &mut HandleScope<'h>,
    owner: ClassId,
    symbol: crate::value::SymbolId,
) -> Result<(), Error> {
    module_frozen_check(scope, owner)?;
    if !scope.classes_mut().undef_method(owner, symbol) {
        return Err(Error::raise(
            "NameError",
            format!(
                "undefined method '{}' for {}",
                symbol_name(symbol),
                class_display_name(scope, owner),
            ),
        ));
    }
    Ok(())
}

/// `defined?`'s answer for a *name*: the string, `nil`, or a report that this
/// heap cannot tell "undefined" from "never loaded".
///
/// Ruby answers `nil` for a name that is not defined, and since `require`
/// landed (#39) so does Spinel — a program loads what it needs. The exception
/// is a heap marked partial: `spec/harness` preloads a spec's fixtures itself,
/// skipping one it cannot compile and leaving one that raised half-run, and
/// there a miss may be a name the skipped part would have defined. Answering
/// `nil` would pass `defined?(SomeFixture).should be_nil` while the VM had
/// simply never heard of the fixture: a wrong answer wearing a passing spec.
/// So in a partial heap a miss is *not yet knowable* rather than *no*. This is
/// R8 of PRD 0011 one layer down.
fn defined_answer<'h>(
    scope: &mut HandleScope<'h>,
    string_class: Handle<'h>,
    answer: Option<&str>,
) -> Result<Value, Error> {
    if answer.is_none() && scope.partial() {
        return Err(Error::Unknowable {
            what: "`defined?` of a name a fixture that did not load may define",
            needs: "mspec loading the fixtures itself (#145)",
        });
    }
    Ok(defined_word(scope, string_class, answer))
}

/// `defined?`'s answer when this heap really is the authority: the string, or
/// `nil` meaning `nil`.
fn defined_word<'h>(
    scope: &mut HandleScope<'h>,
    string_class: Handle<'h>,
    answer: Option<&str>,
) -> Value {
    let Some(answer) = answer else {
        return Value::NIL;
    };
    let value = string_alloc(
        scope,
        string_class,
        answer.as_bytes(),
        crate::strings::UTF_8,
    );
    // `defined?` answers a frozen string in Ruby, and `defined_spec.rb` asserts
    // it on every literal it asks about.
    let handle = scope.root(value);
    scope.freeze(handle);
    value
}

/// Why a receiver could not be dispatched on.
///
/// Names the kind rather than saying "a method call", because the reason lands
/// in a report someone reads to choose the next slice, and "a Float has no
/// class yet" points at the class table while "a method call" points here.
fn no_class(value: Value) -> Error {
    use crate::value::Unpacked;
    Error::NoDispatch {
        op: "a method call",
        operands: match value.unpack() {
            Unpacked::Nil => "nil, whose NilClass the VM has not created yet",
            Unpacked::True => "true, whose TrueClass the VM has not created yet",
            Unpacked::False => "false, whose FalseClass the VM has not created yet",
            Unpacked::Flonum(_) => "a Float, whose class the VM has not created yet",
            _ => "a receiver whose class the VM has not created yet",
        },
    }
}

/// The class a value dispatches on.
///
/// Immediates need a mapping the class table cannot give: `1` is an `Integer`
/// without being a heap object with a class pointer. The ones with no bootstrap
/// class yet — `nil`, `true`, `false`, floats — say so rather than borrowing a
/// class that would answer the wrong methods.
fn class_of(scope: &mut HandleScope<'_>, value: Value) -> Option<ClassId> {
    use crate::value::Unpacked;
    match value.unpack() {
        Unpacked::Fixnum(_) => Some(Builtin::Integer.id()),
        Unpacked::Symbol(_) => Some(Builtin::Symbol.id()),
        Unpacked::Nil => Some(Builtin::NilClass.id()),
        Unpacked::True => Some(Builtin::TrueClass.id()),
        Unpacked::False => Some(Builtin::FalseClass.id()),
        Unpacked::Flonum(_) => Some(Builtin::Float.id()),
        // `undefined` is the marker a call leaves in a slot no argument filled.
        // It is not a Ruby value and has no class on purpose: reaching one
        // through `class_of` means the binder let it escape.
        Unpacked::Undef => None,
        Unpacked::Heap(_) => {
            let handle = scope.root(value);
            let class = scope.class_of(handle)?;
            // A singleton class gets its own singleton on first dispatch,
            // the way CRuby's `ENSURE_EIGENCLASS` does: without it, a call on
            // `k.singleton_class` would skip `K`'s class methods, which its
            // metaclass inherits. Other classes have theirs from the start.
            if class == Builtin::Class.id()
                && let Some(id) = class_id_of(scope, value)
                && scope.classes().is_singleton(id)
            {
                return singleton_of(scope, value).ok();
            }
            Some(class)
        }
    }
}

fn symbol_name(id: SymbolId) -> String {
    crate::shared::symbols::name(id).unwrap_or_else(|| format!("<symbol {}>", id.0))
}

// ---------------------------------------------------------------------------
// Array
// ---------------------------------------------------------------------------
//
// An `Array` is two slots, `[storage, length]`. `storage` is a separate
// `Payload::Slots` object of `capacity` elements and `length` is a fixnum of how
// many of them are live.
//
// The indirection is what makes `a << 1` mutate the array the caller is
// holding. A heap cell cannot grow — mark-sweep with size classes hands out a
// fixed cell — so elements living directly in the `Array`'s own slots could
// only grow by becoming a *different* object, and Ruby would see the identity
// change. Growing replaces the storage instead; the `Array`'s address never
// moves. CRuby's `RARRAY` is a pointer and a length for the same reason.
//
// The storage object has no class. It is never handed to Ruby, `class_of` is
// never asked about it, and the collector traces its slots either way.

/// Slot of the storage object inside an `Array`.
const ARRAY_STORAGE: usize = 0;
/// Slot of the live element count inside an `Array`.
const ARRAY_LENGTH: usize = 1;
/// Slots in the `Array` object itself. Not the element count.
const ARRAY_SLOTS: u32 = 2;
/// Capacity of the first storage object. Growth doubles from here, which is
/// `Vec`'s amortisation argument and the one every growable array makes.
const ARRAY_MIN_CAPACITY: u32 = 4;

/// The live element count of an `Array`, given a handle to one.
fn array_len<'h>(scope: &mut HandleScope<'h>, array: Handle<'h>) -> usize {
    scope
        .slot(array, ARRAY_LENGTH)
        .as_fixnum()
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
}

/// Element `index`, or `nil` past the end — which is `Array#[]`'s answer for a
/// slot inside capacity but past `length`, and must not read stale storage.
fn array_get<'h>(scope: &mut HandleScope<'h>, array: Handle<'h>, index: usize) -> Value {
    if index >= array_len(scope, array) {
        return Value::NIL;
    }
    let storage = scope.slot(array, ARRAY_STORAGE);
    let storage = scope.root(storage);
    scope.slot(storage, index)
}

/// Storage with room for at least `wanted` elements, copying the live ones
/// across when a bigger object is needed. Stored back into the `Array`.
fn array_reserve<'h>(scope: &mut HandleScope<'h>, array: Handle<'h>, wanted: usize) {
    let current = scope.slot(array, ARRAY_STORAGE);
    let capacity = if current == Value::NIL {
        0
    } else {
        let handle = scope.root(current);
        scope.len(handle) as usize
    };
    if capacity >= wanted {
        return;
    }
    let mut grown = ARRAY_MIN_CAPACITY as usize;
    while grown < wanted {
        grown *= 2;
    }
    let live = array_len(scope, array);
    let fresh = scope.alloc(None, Payload::Slots, grown as u32);
    if current != Value::NIL {
        let old = scope.root(current);
        for index in 0..live.min(capacity) {
            let value = scope.slot(old, index);
            scope.set_slot(fresh, index, value);
        }
    }
    let fresh = scope.get(fresh);
    scope.set_slot(array, ARRAY_STORAGE, fresh);
}

/// Write element `index`, growing storage and extending `length` with `nil`s if
/// the write lands past the end. That is `Array#[]=`, which never raises for a
/// non-negative index.
fn array_set<'h>(scope: &mut HandleScope<'h>, array: Handle<'h>, index: usize, value: Value) {
    array_reserve(scope, array, index + 1);
    let storage = scope.slot(array, ARRAY_STORAGE);
    let storage = scope.root(storage);
    let live = array_len(scope, array);
    for gap in live..index {
        scope.set_slot(storage, gap, Value::NIL);
    }
    scope.set_slot(storage, index, value);
    if index >= live {
        array_set_len(scope, array, index + 1);
    }
}

fn array_set_len<'h>(scope: &mut HandleScope<'h>, array: Handle<'h>, length: usize) {
    let length = Value::fixnum(length as i64).expect("an array length fits a fixnum");
    scope.set_slot(array, ARRAY_LENGTH, length);
}

/// Append, growing storage when it is full. The `Array` object is unchanged.
fn array_push<'h>(scope: &mut HandleScope<'h>, array: Handle<'h>, value: Value) {
    let live = array_len(scope, array);
    array_set(scope, array, live, value);
}

/// A handle to the receiver, or a refusal naming the method that wanted one.
fn expect_array<'h>(
    scope: &mut HandleScope<'h>,
    value: Value,
    op: &'static str,
) -> Result<Handle<'h>, Error> {
    if heap_kind(scope, value) != Some(HeapKind::Array) {
        return Err(Error::NoDispatch {
            op,
            operands: "a receiver that is not an Array",
        });
    }
    Ok(scope.root(value))
}

/// A Ruby index against a length: negative counts back from the end, and out of
/// range is `None` rather than a clamp.
fn resolve_index(index: i64, length: usize) -> Option<usize> {
    let length = length as i64;
    let index = if index < 0 { index + length } else { index };
    (index >= 0 && index < length).then_some(index as usize)
}

/// An index argument a native can read without sending `to_int`: an Integer,
/// or a Float truncated toward zero as `Float#to_int` does.
fn index_arg(value: Value) -> Option<i64> {
    if let Some(n) = value.as_fixnum() {
        return Some(n);
    }
    let f = value.as_flonum()?;
    (f.is_finite() && f.abs() < 9.0e18).then(|| f.trunc() as i64)
}

/// `a[start, count]`'s window, or `None` for nil — CRuby's `rb_ary_subseq`: a
/// negative start counts from the end, a start past the end or a negative
/// count is nil, and a start exactly at the end is the empty Array.
fn subseq_bounds(start: i64, count: i64, length: usize) -> Option<(usize, usize)> {
    let length = length as i64;
    let start = if start < 0 { start + length } else { start };
    if start < 0 || start > length || count < 0 {
        return None;
    }
    let count = count.min(length - start);
    Some((start as usize, count as usize))
}

/// Whether `value` is a `Range`, or an instance of a subclass of one.
fn is_range(scope: &mut HandleScope<'_>, value: Value) -> bool {
    let Some(class) = class_of(scope, value) else {
        return false;
    };
    let Some(range) = scope
        .classes()
        .const_get_here(
            Builtin::Object.id(),
            crate::shared::symbols::intern("Range"),
        )
        .and_then(|object| class_id_of(scope, object))
    else {
        return false;
    };
    scope.classes().ancestors(class).contains(&range)
}

/// A Range's ends as indexes — nil for a beginless or endless one — and
/// whether it excludes its end. `None` when an end is something a native
/// cannot read as an index.
type RangeParts = (Option<i64>, Option<i64>, bool);

fn range_parts(scope: &mut HandleScope<'_>, range: Value) -> Option<RangeParts> {
    let first = ivar_get(scope, range, symbol("@__begin__")).ok()?;
    let last = ivar_get(scope, range, symbol("@__end__")).ok()?;
    let exclusive = ivar_get(scope, range, symbol("@__exclude_end__")).ok()? == Value::TRUE;
    let first = if first == Value::NIL {
        None
    } else {
        Some(index_arg(first)?)
    };
    let last = if last == Value::NIL {
        None
    } else {
        Some(index_arg(last)?)
    };
    Some((first, last, exclusive))
}

/// CRuby's `rb_range_component_beg_len` over a sequence of `length`: the
/// start and count a Range selects. For a read (`splice == false`) a range
/// that starts outside is nil; for a splice it may start past the end, and one
/// starting before the front is a RangeError naming the range.
fn range_beg_len(
    scope: &mut HandleScope<'_>,
    range: Value,
    (first, last, exclusive): RangeParts,
    length: usize,
    splice: bool,
) -> Result<Option<(usize, usize)>, Error> {
    let size = length as i64;
    let mut start = first.unwrap_or(0);
    let mut end = last.unwrap_or(size);
    if start < 0 {
        start += size;
        if start < 0 {
            return out_of_range(scope, range, splice);
        }
    }
    if end < 0 {
        end += size;
    }
    if last.is_some() && !exclusive {
        end += 1;
    }
    if !splice {
        if start > size {
            return Ok(None);
        }
        end = end.min(size);
    }
    let count = (end - start).max(0);
    Ok(Some((start as usize, count as usize)))
}

fn out_of_range(
    scope: &mut HandleScope<'_>,
    range: Value,
    splice: bool,
) -> Result<Option<(usize, usize)>, Error> {
    if !splice {
        return Ok(None);
    }
    let parts = range_parts(scope, range);
    let text = match parts {
        Some((first, last, exclusive)) => format!(
            "{}{}{}",
            first.map_or_else(String::new, |n| n.to_string()),
            if exclusive { "..." } else { ".." },
            last.map_or_else(String::new, |n| n.to_string())
        ),
        None => "range".to_owned(),
    };
    Err(Error::raise("RangeError", format!("{text} out of range")))
}

/// Replace `count` elements at `start` with `replacement`, padding with nil
/// when `start` is past the end — `rb_ary_splice`. The Array object is kept;
/// its storage is rewritten.
fn array_splice<'h>(
    scope: &mut HandleScope<'h>,
    array: Handle<'h>,
    start: usize,
    count: usize,
    replacement: &[Value],
) {
    let length = array_len(scope, array);
    let mut elements: Vec<Value> = (0..length).map(|i| array_get(scope, array, i)).collect();
    if start >= length {
        elements.resize(start, Value::NIL);
        elements.extend_from_slice(replacement);
    } else {
        let end = (start + count).min(length);
        elements.splice(start..end, replacement.iter().copied());
    }
    // Every value here is reachable from the array or from the call's
    // arguments, both rooted, so growing the storage cannot lose one.
    array_reserve(scope, array, elements.len());
    let storage = scope.slot(array, ARRAY_STORAGE);
    let storage = scope.root(storage);
    for (index, &value) in elements.iter().enumerate() {
        scope.set_slot(storage, index, value);
    }
    // Slots past the new end let go of what they held, so a shrinking splice
    // does not keep its removed elements alive.
    let capacity = scope.len(storage) as usize;
    for index in elements.len()..capacity.min(length) {
        scope.set_slot(storage, index, Value::NIL);
    }
    array_set_len(scope, array, elements.len());
}

// ---------------------------------------------------------------------------
// Instance variables
// ---------------------------------------------------------------------------
//
// Slot 0 of an ivar-capable object holds its ivar storage: a classless
// `Payload::Slots` object of `capacity` values, or `nil` before the first
// write. The object's shape says which index a name lands at — see `shape.rs`.
//
// The indirection is the one an `Array` already pays, and for the same reason
// spelled out above `ARRAY_STORAGE`: a heap cell cannot grow, so an object that
// held its ivars directly could only gain the fourth one by becoming a
// different cell, and Ruby would see `equal?` change. Growing replaces the
// storage; the object's address never moves.
//
// `Heap::mark` needs no change for any of this. The storage hangs off a traced
// slot of a `Payload::Slots` object, which is exactly what the collector
// already descends into.

/// Whether an ivar is the core library's own, spelled `@__name__`: hidden from
/// `instance_variables`, as CRuby keeps the same state out of Ruby's sight.
fn is_internal_ivar(name: SymbolId) -> bool {
    crate::shared::symbols::name(name)
        .is_some_and(|name| name.len() > 5 && name.starts_with("@__") && name.ends_with("__"))
}

/// Whether `name` spells an instance variable: `@` and then an identifier.
///
/// `:@0` and `:@@x` are `NameError`s in Ruby, not misses — `@0` is not a name
/// and `@@x` is a class variable — so the check is the grammar rather than a
/// leading `@`.
fn is_ivar_name(name: &str) -> bool {
    let mut chars = name.chars();
    if chars.next() != Some('@') {
        return false;
    }
    match chars.next() {
        Some(first) if first == '_' || first.is_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_alphanumeric())
}

/// A method or ivar name given as a Symbol or a String, as every reflective
/// method in Ruby accepts both.
fn attribute_name(scope: &mut HandleScope<'_>, value: Value) -> Option<String> {
    if let Some(id) = value.as_symbol() {
        return crate::shared::symbols::name(id);
    }
    let bytes = string_bytes(scope, value)?;
    String::from_utf8(bytes).ok()
}

/// An interned name, for the handful of ivars the VM writes itself.
pub(crate) fn symbol(name: &str) -> SymbolId {
    crate::shared::symbols::intern(name)
}

/// Slot of the ivar storage inside an ivar-capable object.
const SLOT_IVARS: usize = 0;
/// Slots in such an object, before its class's own representation adds any.
const IVAR_SLOTS: u32 = 1;
/// Capacity of the first storage object. Doubling from here, as `Array` does.
const IVAR_MIN_CAPACITY: u32 = 2;

/// Allocate a plain object of `class`: one slot, and a shape that says the slot
/// is for instance variables.
///
/// The one place [`ShapeId::ROOT`] is handed out. Everything that can hold an
/// ivar comes from here, so "which objects can" is a question with one answer
/// rather than a rule spread across the allocators.
pub(crate) fn alloc_ivar_object<'h>(
    scope: &mut HandleScope<'h>,
    class: Option<Handle<'h>>,
) -> Handle<'h> {
    let handle = scope.alloc(class, Payload::Slots, IVAR_SLOTS);
    scope.set_slot(handle, SLOT_IVARS, Value::NIL);
    scope.set_shape(handle, ShapeId::ROOT);
    handle
}

/// Why this object cannot hold an instance variable.
///
/// Named per representation rather than bucketed, because the blocked-reason
/// ranking is how the next slice gets chosen and one bucket for every built-in
/// tells nobody which representation to give a slot to first.
fn ivar_refusal(scope: &mut HandleScope<'_>, value: Value) -> Error {
    let mut nested = scope.nested();
    let handle = nested.root(value);
    let class = nested.class(handle);
    let classes = nested.classes();
    let named = |builtin: Builtin| class == Some(classes.object(builtin.id()));
    let what = if named(Builtin::Array) {
        "an instance variable on an `Array`"
    } else if named(Builtin::String) {
        "an instance variable on a `String`"
    } else if named(Builtin::Proc) {
        "an instance variable on a `Proc`"
    } else if named(Builtin::Regexp) {
        "an instance variable on a `Regexp`"
    } else if named(Builtin::MatchData) {
        "an instance variable on a `MatchData`"
    } else {
        "an instance variable on a built-in with a representation of its own"
    };
    Error::Unknowable {
        what,
        needs: "that representation reserves an ivar slot (#151)",
    }
}

/// The value of `name` on `object`, or `nil` — which is Ruby's answer for an
/// instance variable that was never assigned.
///
/// Refuses only where the object has nowhere to put one, which is a different
/// thing from holding none.
pub(crate) fn ivar_get(
    scope: &mut HandleScope<'_>,
    object: Value,
    name: SymbolId,
) -> Result<Value, Error> {
    // Ruby answers `nil` for `@a` on an immediate, and warns about nothing.
    // There is no storage to consult and no question about what it holds.
    if object.is_immediate() {
        return Ok(Value::NIL);
    }
    let mut nested = scope.nested();
    let handle = nested.root(object);
    let shape = nested.shape(handle);
    if shape == ShapeId::NONE {
        return Err(ivar_refusal(&mut nested, object));
    }
    let Some(index) = nested.shapes().index_of(shape, name) else {
        return Ok(Value::NIL);
    };
    let storage = nested.slot(handle, SLOT_IVARS);
    debug_assert!(storage != Value::NIL, "a shape with no storage behind it");
    let storage = nested.root(storage);
    Ok(nested.slot(storage, index as usize))
}

/// Whether `object` holds an instance variable called `name`.
///
/// The question `defined?(@a)` asks, and the one #13 could not answer: a VM
/// with no instance variables saying `nil` would have passed
/// `defined?(@nope).should be_nil` without being able to represent the
/// question. It can now, so `nil` here is a measurement.
fn ivar_defined(scope: &mut HandleScope<'_>, object: Value, name: SymbolId) -> Result<bool, Error> {
    if object.is_immediate() {
        return Ok(false);
    }
    let mut nested = scope.nested();
    let handle = nested.root(object);
    let shape = nested.shape(handle);
    if shape == ShapeId::NONE {
        return Err(ivar_refusal(&mut nested, object));
    }
    Ok(nested.shapes().index_of(shape, name).is_some())
}

/// Set `name` on `object` to `value`, transitioning its shape if this is the
/// first time it has been given one by that name.
pub(crate) fn ivar_set(
    scope: &mut HandleScope<'_>,
    object: Value,
    name: SymbolId,
    value: Value,
) -> Result<Value, Error> {
    // `1.instance_variable_set(:@a, 1)` is a `FrozenError` in Ruby, because
    // every immediate is frozen. That is a real answer rather than a refusal,
    // so it is given.
    if object.is_immediate() {
        return Err(Error::raise(
            "FrozenError",
            format!("can't modify frozen {}", class_name(scope, object)),
        ));
    }
    let mut nested = scope.nested();
    let handle = nested.root(object);
    // Named only on refusal. `class_name` reaches `class_of`, which reads a
    // class object's own `@__id__` through this function — asking for it on
    // every write would make writing that very ivar a regress.
    if nested.is_frozen(handle) {
        let class = class_name(&mut nested, object);
        return Err(Error::raise(
            "FrozenError",
            format!("can't modify frozen {class}"),
        ));
    }
    let shape = nested.shape(handle);
    if shape == ShapeId::NONE {
        return Err(ivar_refusal(&mut nested, object));
    }
    let (index, value) = match nested.shapes().index_of(shape, name) {
        Some(index) => (index, value),
        None => {
            let Some((grown, index)) = nested.shapes_mut().transition(shape, name) else {
                return Err(Error::Unknowable {
                    what: "an instance variable past this heap's 65,534th shape",
                    needs: "a shape id wider than the header's two bytes (#7)",
                });
            };
            // Reserve before the shape is recorded: a collection inside the
            // allocation must not find an object whose shape promises a slot
            // its storage does not have. The value comes back rather than
            // being reused, because that allocation is a collection point and
            // the caller handed it over unrooted.
            let value = ivar_reserve(&mut nested, handle, index + 1, value);
            nested.set_shape(handle, grown);
            (index, value)
        }
    };
    let storage = nested.slot(handle, SLOT_IVARS);
    let storage = nested.root(storage);
    nested.set_slot(storage, index as usize, value);
    Ok(value)
}

/// Storage room for at least `wanted` instance variables, copying the ones
/// already there into a bigger object when there is not.
///
/// `keep` is the value about to be written. It is passed in and re-rooted
/// because the allocation below can collect, and a value living only on the
/// operand stack of the caller would not survive it.
fn ivar_reserve<'h>(
    scope: &mut HandleScope<'h>,
    object: Handle<'h>,
    wanted: u16,
    keep: Value,
) -> Value {
    let current = scope.slot(object, SLOT_IVARS);
    let capacity = if current == Value::NIL {
        0
    } else {
        let handle = scope.root(current);
        scope.len(handle)
    };
    if capacity >= u32::from(wanted) {
        return keep;
    }
    let mut grown = IVAR_MIN_CAPACITY;
    while grown < u32::from(wanted) {
        grown *= 2;
    }
    let keep = scope.root(keep);
    let fresh = scope.alloc(None, Payload::Slots, grown);
    if current != Value::NIL {
        let old = scope.root(current);
        for index in 0..capacity as usize {
            let value = scope.slot(old, index);
            scope.set_slot(fresh, index, value);
        }
    }
    let fresh = scope.get(fresh);
    scope.set_slot(object, SLOT_IVARS, fresh);
    scope.get(keep)
}

/// The names `object` holds, in the order it acquired them — which is the
/// order `Object#instance_variables` answers in.
fn ivar_names(scope: &mut HandleScope<'_>, object: Value) -> Vec<SymbolId> {
    if object.is_immediate() {
        return Vec::new();
    }
    let mut nested = scope.nested();
    let handle = nested.root(object);
    let shape = nested.shape(handle);
    if shape == ShapeId::NONE {
        return Vec::new();
    }
    nested.shapes().names(shape)
}

/// Refuse to mutate a frozen object, the way Ruby does.
fn frozen_check(scope: &mut HandleScope<'_>, value: Value, what: &str) -> Result<(), Error> {
    if value.is_immediate() {
        return Ok(());
    }
    let handle = scope.root(value);
    if scope.is_frozen(handle) {
        return Err(Error::raise(
            "FrozenError",
            format!("can't modify frozen {what}"),
        ));
    }
    Ok(())
}

/// A fixnum, or a refusal when the answer does not fit one. Spinel has no
/// bignum, and a wrapped answer would be wrong rather than missing.
fn fixnum_or_refuse(n: i64, op: &'static str) -> Result<Value, Error> {
    Value::fixnum(n).ok_or(Error::NoDispatch {
        op,
        operands: "a result wider than a fixnum",
    })
}

/// How deep `hash` will follow nested arrays before refusing.
///
/// Ruby answers for a self-referential array — `a = []; a << a; a.hash` — by
/// noticing the cycle. Spinel does not track one, so a bound is what keeps the
/// recursion from running off the stack, and hitting it is a refusal rather
/// than an answer for a structure it did not finish reading.
const HASH_DEPTH: usize = 32;

/// `Object#hash`, written into `hasher`.
///
/// Content for a `String` and an `Array`, because `==` on those is content;
/// identity for everything else, because `==` on those is identity. That
/// equivalence — `a == b` implies `a.hash == b.hash` — is the whole contract.
fn hash_value(
    scope: &mut HandleScope<'_>,
    value: Value,
    hasher: &mut impl std::hash::Hasher,
    depth: usize,
) -> Result<(), Error> {
    use std::hash::Hash as _;
    if depth > HASH_DEPTH {
        return Err(Error::Unknowable {
            what: "`hash` of a deeply nested or self-referential array",
            needs: "cycle detection, which this VM does not track",
        });
    }
    // A bignum is a heap object with content equality, so it hashes by value:
    // `(2**70).hash == (2**70).hash` although the two are different objects.
    if crate::bignum::is_big(scope, value)
        && let Some(n) = crate::bignum::read(scope, value)
    {
        3u8.hash(hasher);
        n.hash(hasher);
        return Ok(());
    }
    match heap_kind(scope, value) {
        Some(HeapKind::Str) => {
            // CRuby mixes the encoding in only for a string that is not pure
            // ASCII, which is what keeps `hash` agreeing with `==`: `"a"` and
            // `"a".b` are equal, `"é"` and `"é".b` are not.
            let handle = scope.root(value);
            let bytes = crate::strings::bytes(scope, handle);
            let encoding = crate::strings::encoding(scope, handle);
            0u8.hash(hasher);
            bytes.hash(hasher);
            if !(bytes.is_ascii() && crate::strings::ascii_compatible(encoding)) {
                encoding.hash(hasher);
            }
        }
        Some(HeapKind::Array) => {
            let handle = scope.root(value);
            let len = array_len(scope, handle);
            1u8.hash(hasher);
            len.hash(hasher);
            for index in 0..len {
                let element = array_get(scope, handle, index);
                hash_value(scope, element, hasher, depth + 1)?;
            }
        }
        // An immediate hashes by its bits, so two `1`s and two `:a`s agree.
        // A heap object with no content equality hashes by identity, and its
        // `Value` *is* its address.
        None => {
            2u8.hash(hasher);
            value.to_bits().hash(hasher);
        }
    }
    Ok(())
}

/// A method name given as a `Symbol` or a `String`, as Ruby's reflection takes
/// either.
fn method_name_of(scope: &mut HandleScope<'_>, value: Value) -> Option<String> {
    if let Some(id) = value.as_symbol() {
        return crate::shared::symbols::name(id);
    }
    string_bytes(scope, value).and_then(|bytes| String::from_utf8(bytes).ok())
}

/// `Float#to_s`, in Ruby's shape.
///
/// Two rules, both measured against CRuby rather than reasoned about: a float
/// always shows a fractional part (`1.0`, not `1`), and one whose magnitude is
/// outside `[1e-4, 1e15)` is written in exponent form (`1.0e+20`, `1.0e-05`)
/// with a two-digit, signed exponent.
fn float_to_s(f: f64) -> String {
    if f == 0.0 {
        // `-0.0` prints its sign, and `0.0.fract()` is 0 either way.
        return if f.is_sign_negative() {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }
    let magnitude = f.abs();
    if (1e-4..1e15).contains(&magnitude) {
        let plain = if f.fract() == 0.0 {
            format!("{f:.1}")
        } else {
            f.to_string()
        };
        return plain;
    }
    // `{:e}` gives `1e20`; Ruby wants `1.0e+20`.
    let scientific = format!("{f:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("`{:e}` always writes an exponent");
    let mantissa = if mantissa.contains('.') {
        mantissa.to_owned()
    } else {
        format!("{mantissa}.0")
    };
    let (sign, digits) = match exponent.strip_prefix('-') {
        Some(digits) => ("-", digits),
        None => ("+", exponent),
    };
    format!("{mantissa}e{sign}{digits:0>2}")
}

/// A new `String` of class `id` holding `bytes` in `encoding`.
fn string_bytes_in(scope: &mut HandleScope<'_>, id: ClassId, bytes: &[u8], encoding: u8) -> Value {
    let class = scope.classes().object(id);
    let class = scope.root(class);
    string_alloc(scope, class, bytes, encoding)
}

/// `Object#dup`: a shallow copy of the cell, unfrozen.
///
/// `Array` overrides this in `core/array.rb`, because copying an `Array`'s two
/// slots would hand the copy the *same* storage object and `b << 1` would show
/// up in `a`.
fn dup_value(scope: &mut HandleScope<'_>, value: Value) -> Result<Value, Error> {
    // An immediate has no cell to copy, and Ruby answers with itself.
    if value.is_immediate() {
        return Ok(value);
    }
    let source = scope.root(value);
    let payload = scope.payload(source);
    let len = scope.len(source);
    // `dup` does not carry the singleton class across — that is what
    // `singleton_class_spec.rb` means by a constant on it not being preserved.
    // `clone` does, which is the difference between the two and the reason this
    // walks rather than copying the header.
    let class = dup_class_of(scope, value).map(|class| scope.root(class));
    let copy = scope.alloc(class, payload, len);
    match payload {
        Payload::Bytes => {
            let bytes = scope.bytes(source).to_vec();
            scope.bytes_mut(copy).copy_from_slice(&bytes);
        }
        Payload::Slots => {
            for index in 0..len as usize {
                let slot = scope.slot(source, index);
                scope.set_slot(copy, index, slot);
            }
            // A String's bytes live in a buffer its slots point at; the copy
            // gets its own, or `s.dup << "x"` would change `s` too.
            if is_string(scope, value) {
                crate::strings::unshare(scope, copy);
            }
        }
    }
    // The shape travels with the slots, and the ivar storage is copied rather
    // than shared: `b = a.dup; b.instance_variable_set(:@x, 1)` must not reach
    // into `a`. Ruby copies instance variables on `dup`, so the copy wears the
    // same shape and owns its own storage.
    let shape = scope.shape(source);
    scope.set_shape(copy, shape);
    if shape != ShapeId::NONE {
        let storage = scope.slot(source, SLOT_IVARS);
        if storage != Value::NIL {
            let storage = scope.root(storage);
            let capacity = scope.len(storage);
            let fresh = scope.alloc(None, Payload::Slots, capacity);
            for index in 0..capacity as usize {
                let ivar = scope.slot(storage, index);
                scope.set_slot(fresh, index, ivar);
            }
            let fresh = scope.get(fresh);
            scope.set_slot(copy, SLOT_IVARS, fresh);
        }
    }
    Ok(scope.get(copy))
}

/// The class a copy of `value` should wear: its own, with any singleton
/// class walked past.
///
/// An object's singleton *becomes* its class, so the header is not a reliable
/// answer for "what is this an instance of" once a `def obj.m` has run.
fn dup_class_of(scope: &mut HandleScope<'_>, value: Value) -> Option<Value> {
    let mut id = class_of(scope, value)?;
    while scope.classes().is_singleton(id) {
        id = scope.classes().superclass(id)?;
    }
    Some(scope.classes().object(id))
}

/// Which class `allocate` refused on, as a `&'static str` the reason can carry.
///
/// [`Error::Unknowable`]'s fields are static so that a reason costs nothing to
/// build on a path that runs per blocked example. One arm per class buys the
/// ranking a separate line per class, which is what makes it plannable.
const fn allocate_refusal(builtin: Builtin) -> &'static str {
    match builtin {
        Builtin::BasicObject => "`allocate` on `BasicObject`",
        Builtin::Object => "`allocate` on `Object`",
        Builtin::Module => "`allocate` on `Module`",
        Builtin::Class => "`allocate` on `Class`",
        Builtin::Kernel => "`allocate` on `Kernel`",
        Builtin::Comparable => "`allocate` on `Comparable`",
        Builtin::Enumerable => "`allocate` on `Enumerable`",
        Builtin::Numeric => "`allocate` on `Numeric`",
        Builtin::Symbol => "`allocate` on `Symbol`",
        Builtin::String => "`allocate` on `String`",
        Builtin::Integer => "`allocate` on `Integer`",
        Builtin::Array => "`allocate` on `Array`",
        Builtin::Hash => "`allocate` on `Hash`",
        Builtin::Proc => "`allocate` on `Proc`",
        Builtin::Exception => "`allocate` on `Exception`",
        Builtin::Regexp => "`allocate` on `Regexp`",
        Builtin::MatchData => "`allocate` on `MatchData`",
        Builtin::NilClass => "`allocate` on `NilClass`",
        Builtin::TrueClass => "`allocate` on `TrueClass`",
        Builtin::FalseClass => "`allocate` on `FalseClass`",
        Builtin::Float => "`allocate` on `Float`",
    }
}

/// `Class#allocate`: an uninitialised instance, in the representation its class
/// expects.
///
/// A class Spinel has no shape for refuses rather than handing back a bare
/// zero-slot object wearing that class — which is what made `Proc#lambda?` read
/// past the end of one before #13 closed the door.
fn allocate_instance(scope: &mut HandleScope<'_>, id: ClassId) -> Result<Value, Error> {
    // The *representation*, not the class. A subclass of a built-in inherits
    // the shape and keeps its own class object, so `MyString.new("b")` is a
    // string that answers `MyString` — which is what asking `Builtin::ALL` by
    // class id could not do, because a class the bootstrap did not create is
    // `None` for its own id however built-in its ancestry.
    match scope.classes().repr(id) {
        // A user-defined class, or `Object` itself: one slot, for the ivar
        // storage a shape transition will hang off it.
        None | Some(Builtin::Object | Builtin::BasicObject) => {
            let class = scope.classes().object(id);
            let class = scope.root(class);
            let handle = alloc_ivar_object(scope, Some(class));
            Ok(scope.get(handle))
        }
        Some(Builtin::Array) => {
            let handle = empty_array_of(scope, id);
            Ok(scope.get(handle))
        }
        // Empty and BINARY, which is what `String.new` with no arguments
        // answers; `initialize` sets contents and encoding from there (#19).
        Some(Builtin::String) => Ok(string_bytes_in(scope, id, b"", crate::strings::BINARY)),
        Some(Builtin::Hash) => {
            // Three ordinary instance variables: the association list, the
            // default, and whether that default is a block. They were three
            // fixed slots with three `Getter`/`Setter` pairs in front of them
            // until shapes landed; `core/hash.rb` now reads `@__pairs__` like any
            // other Ruby class.
            let class = scope.classes().object(id);
            let class = scope.root(class);
            let handle = alloc_ivar_object(scope, Some(class));
            let object = scope.get(handle);
            let pairs = new_array(scope, &[]);
            ivar_set(scope, object, symbol("@__pairs__"), pairs)?;
            ivar_set(scope, object, symbol("@__default__"), Value::NIL)?;
            ivar_set(scope, object, symbol("@__default_is_proc__"), Value::FALSE)?;
            Ok(object)
        }
        Some(other) => {
            if is_exception_class(scope, id) {
                let class = scope.classes().object(id);
                return Ok(exception_of(scope, class, ""));
            }
            // `Proc.new` without a block, `Integer.new`, `Symbol.new` all raise
            // in Ruby. Saying so needs each class's rule; refusing says the VM
            // does not know it yet, which is the true answer.
            //
            // The class is *in* the reason rather than behind a "this built-in
            // class", because the blocked-reason ranking is how the next slice
            // is chosen and one bucket of every class tells nobody which to
            // write next.
            Err(Error::Unknowable {
                what: allocate_refusal(other),
                needs: match other {
                    // #162 made `Class.new` and `Module.new` real. What is left
                    // is `allocate` itself, which in Ruby answers a class that
                    // is not initialised — `superclass` raises `TypeError:
                    // uninitialized class` on it and `new` refuses to
                    // instantiate it. That is the bare-object-wearing-a-class
                    // hazard #13 shut the door on, so it stays shut.
                    Builtin::Class => "a class can exist uninitialised (#28)",
                    Builtin::Module => "`Module.allocate` is a `NoMethodError` in Ruby (#28)",
                    Builtin::Proc => "`Proc.new` can be given the block it is",
                    Builtin::Regexp => "a pattern can be compiled from a run-time value",
                    Builtin::Integer | Builtin::Symbol | Builtin::Float => {
                        "the rule by which Ruby refuses it too"
                    }
                    Builtin::NilClass | Builtin::TrueClass | Builtin::FalseClass => {
                        "the rule that these classes have no `new`"
                    }
                    _ => "`core/*.rb` defines its representation",
                },
            })
        }
    }
}

/// The elements of an `Array`, or `None` if the value is not one.
/// A `Hash`'s pairs, for a `**` argument (#193).
///
/// Reads `@__pairs__` — the association list `core/hash.rb` keeps — rather than
/// sending `each_pair`, because this runs from inside argument assembly and
/// re-entering the interpreter there is what `expand_splats` already refuses to
/// do. An ivar read is a shape lookup, not a call.
///
// ponytail: the one place outside `core/hash.rb` that knows the
// representation, and that file's own note says nothing else should. Give
// `Hash` a primitive that hands out its pairs when the open-addressed table
// that note promises arrives, and this reads that instead.
fn hash_pairs(scope: &mut HandleScope<'_>, value: Value) -> Option<Vec<(Value, Value)>> {
    let pairs = ivar_get(scope, value, symbol("@__pairs__")).ok()?;
    let mut out = Vec::new();
    for pair in array_elements(scope, pairs)? {
        let entry = array_elements(scope, pair)?;
        let [key, value] = entry[..] else { return None };
        out.push((key, value));
    }
    Some(out)
}

fn array_elements(scope: &mut HandleScope<'_>, value: Value) -> Option<Vec<Value>> {
    if heap_kind(scope, value) != Some(HeapKind::Array) {
        return None;
    }
    let handle = scope.root(value);
    let live = array_len(scope, handle);
    Some(
        (0..live)
            .map(|index| array_get(scope, handle, index))
            .collect(),
    )
}

/// An empty `Array`, with storage left unallocated until something is written.
fn empty_array<'h>(scope: &mut HandleScope<'h>) -> Handle<'h> {
    empty_array_of(scope, Builtin::Array.id())
}

/// The same, wearing `id` — what a subclass of `Array` allocates.
fn empty_array_of<'h>(scope: &mut HandleScope<'h>, id: ClassId) -> Handle<'h> {
    let class = scope.classes().object(id);
    let class = scope.root(class);
    let handle = scope.alloc(Some(class), Payload::Slots, ARRAY_SLOTS);
    scope.set_slot(handle, ARRAY_STORAGE, Value::NIL);
    array_set_len(scope, handle, 0);
    handle
}

fn new_array(scope: &mut HandleScope<'_>, elements: &[Value]) -> Value {
    let handle = empty_array(scope);
    array_reserve(scope, handle, elements.len());
    let storage = scope.slot(handle, ARRAY_STORAGE);
    if storage != Value::NIL {
        let storage = scope.root(storage);
        for (index, value) in elements.iter().enumerate() {
            scope.set_slot(storage, index, *value);
        }
    }
    array_set_len(scope, handle, elements.len());
    scope.get(handle)
}

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

/// The operations Ruby cannot define in Ruby. See [`Native`].
fn native_call<'h>(
    scope: &mut HandleScope<'h>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    call: Pending,
    native: Native,
    proc_class: Handle<'h>,
    ids: &mut u64,
) -> Result<Option<Unwind>, Error> {
    match native {
        // Two that push a frame rather than returning a value, which is why
        // `Native` is an enum the loop matches and not a function pointer.
        Native::Call => {
            push_proc_frame(scope, stack, frames, &call, call.receiver, ids)?;
            Ok(None)
        }
        Native::Send { public_only } => {
            let mut args = call.args.clone();
            if args.is_empty() {
                return Err(Error::raise(
                    "ArgumentError",
                    "no method name given".to_owned(),
                ));
            }
            let name = args.remove(0);
            let Some(name) = name.as_symbol() else {
                // `send("name")` also works in Ruby, once a String can be
                // turned into a symbol without a method call.
                return Err(Error::NoDispatch {
                    op: "send",
                    operands: "a method name that is not a Symbol",
                });
            };
            let forwarded = Pending {
                // `send` resolves a name the call site never mentioned, so the
                // site's entry is not about this call.
                cache: None,
                // `send` forwards whatever block it was given, and how that
                // block was written travels with it.
                block_is_literal: call.block_is_literal,
                name,
                receiver: call.receiver,
                args,
                keywords: call.keywords,
                block: call.block,
                cref: call.cref,
                // `send` is documented to reach a private method; `public_send`
                // is the one that does not, and refuses protected too.
                implicit_self: !public_only,
                public_only,
                target: Target::Method,
                owner: None,
                defined_as: None,
            };
            dispatch(scope, stack, frames, forwarded, proc_class, ids)
        }

        Native::MakeProc { lambda } => {
            // Ruby 3.0 made `lambda` require a literal block: `lambda(&a_proc)`
            // raises rather than converting, because the conversion would
            // silently change what `return` inside that proc means.
            if lambda && !call.block_is_literal && call.block != Value::NIL {
                return Err(Error::raise(
                    "ArgumentError",
                    "the lambda method requires a literal block",
                ));
            }
            if call.block == Value::NIL {
                return Err(Error::raise(
                    "ArgumentError",
                    "tried to create Proc object without a block",
                ));
            }
            // `lambda { }` marks the block it was given; `proc { }` does not.
            let value = if lambda {
                relambda(scope, proc_class, call.block)?
            } else {
                call.block
            };
            stack.push(value);
            Ok(None)
        }
        Native::IsLambda => {
            // Guarded like `Arity`, not indexed directly: `Proc#lambda?` is
            // reachable on any receiver whose class is `Proc`, and reading slot
            // 3 of something that is not one is a panic rather than an answer.
            let Some((.., lambda, _)) = proc_parts(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "lambda?",
                    operands: "a receiver that is not a Proc",
                });
            };
            stack.push(bool_value(lambda));
            Ok(None)
        }
        Native::Arity => {
            let Some((iseq, ..)) = proc_parts(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "arity",
                    operands: "a receiver that is not a Proc",
                });
            };
            stack.push(Value::fixnum(iseq.params.arity()).expect("an arity fits a fixnum"));
            Ok(None)
        }
        Native::BlockGiven => {
            // The block of the frame that called `block_given?`, which is the
            // one still on top: a primitive does not push a frame.
            let block = frames.last().map_or(Value::NIL, |f| f.block);
            stack.push(bool_value(block != Value::NIL));
            Ok(None)
        }
        Native::New => {
            let Some(id) = class_id_of(scope, call.receiver) else {
                return Err(Error::raise(
                    "NoMethodError",
                    format!(
                        "undefined method 'new' for an instance of {}",
                        class_name(scope, call.receiver)
                    ),
                ));
            };
            if scope.classes().is_singleton(id) {
                return Err(Error::raise(
                    "TypeError",
                    "can't create instance of singleton class",
                ));
            }
            // A bootstrap class other than `Object` has a representation this
            // cannot build: a `Proc` is six slots, a `String` is bytes, and a
            // bare zero-slot object wearing their class is a value every
            // primitive on them would then misread — `Proc.new` used to reach
            // `Proc#lambda?` and index past the end of the object. `Object` and
            // `BasicObject` really are plain, so they are allowed.
            // `Class.new` and `Module.new` build a class rather than an
            // instance of one, so they never reach `allocate` — which is also
            // Ruby: `core/class/new_spec.rb` pins that a `self.allocate` that
            // raises is not called. They push a frame when given a block, so
            // they live here rather than in `allocate_instance`.
            if id == Builtin::Class.id() || id == Builtin::Module.id() {
                return anonymous_module(scope, stack, frames, &call, id, proc_class, ids);
            }

            // An exception class is allocatable: `raise ArgumentError.new("x")`
            // and `rescue Klass => e` are everywhere in the corpus, and the
            // representation is two slots this module owns rather than one
            // `core/*.rb` has yet to define.
            // A class whose `initialize` is written in Ruby is not this
            // module's business at all: it falls through to the ordinary
            // allocate-then-`initialize` path below and reaches
            // `Exception#initialize` through `super`. That is what lets
            // `core/exception.rb` own `SystemExit`, `NameError`, `KeyError`
            // and the rest (#29) — the fast path here is for the classes
            // that still have no Ruby `initialize` to dispatch to.
            // Owned *below* `Exception`: since #29 `Exception#initialize` is
            // itself Ruby, so "has a Ruby initialize" would be true of every
            // exception class and would take `UncaughtThrowError.new("x")`
            // off the refusal it still needs. What matters is whether the
            // class or one of its ancestors under `Exception` wrote its own.
            let written_in_ruby = {
                let initialize = crate::shared::symbols::intern("initialize");
                scope
                    .classes()
                    .lookup_uncached(id, initialize)
                    .is_some_and(|method| method.owner != Builtin::Exception.id())
            };
            if is_exception_class(scope, id) && !written_in_ruby {
                // ...unless CRuby gives it an `initialize` of its own, which
                // Spinel does not have. `UncaughtThrowError.new("x")` raises
                // there and would quietly succeed here, which is a wrong answer
                // rather than a missing one. Measured by the oracle, not judged.
                if scope
                    .classes()
                    .name(id)
                    .is_some_and(crate::class::exception_defines_initialize)
                {
                    return Err(Error::Unknowable {
                        what: "`new` on an exception class with its own `initialize`",
                        needs: "`core/*.rb` defines that initialize (#15)",
                    });
                }
                // `Exception#initialize` takes 0..1 arguments, and an extra one
                // is an error rather than something to ignore. Measured: a
                // second argument raises `ArgumentError` in Ruby, and accepting
                // it here let `NetHTTPExceptionsSpecs::Simple.new(msg, resp)`
                // build an object Ruby refuses to build — a pass
                // `scripts/verify-passes.rb` caught and this removes.
                if call.args.len() > 1 {
                    return Err(Error::raise(
                        "ArgumentError",
                        format!(
                            "wrong number of arguments (given {}, expected 0..1)",
                            call.args.len()
                        ),
                    ));
                }
                // A nil message is no message, measured:
                // `RuntimeError.new(nil).message` is "RuntimeError".
                let message = match call.args.first().filter(|&&v| v != Value::NIL) {
                    Some(&argument) => match string_bytes(scope, argument) {
                        Some(text) => String::from_utf8_lossy(&text).into_owned(),
                        None => inspect(scope, argument),
                    },
                    // Measured: `StandardError.new.message` is "StandardError".
                    None => scope
                        .classes()
                        .name(id)
                        .map_or_else(String::new, str::to_owned),
                };
                let class = scope.classes().object(id);
                let exception = exception_of(scope, class, &message);
                stack.push(exception);
                return Ok(None);
            }
            if scope.classes().kind(id) == Kind::Module {
                return Err(Error::raise(
                    "NoMethodError",
                    format!(
                        "undefined method 'new' for module {}",
                        scope.classes().name(id).unwrap_or("an anonymous module")
                    ),
                ));
            }
            // `Regexp.new(source)` for the same reason: `allocate` refuses on
            // `Regexp` because a pattern cannot exist uninitialised, so there
            // is nothing for allocate-then-initialize to allocate. Answered
            // here rather than as a singleton method on the class, because
            // building `Regexp`'s metaclass at bootstrap would build `Object`'s
            // and `BasicObject`'s with it — a singleton class is observable,
            // and none of the three should exist until something asks.
            if scope.classes().repr(id) == Some(Builtin::Regexp) {
                let value = regexp_new_from(scope, &call)?;
                stack.push(value);
                return Ok(None);
            }
            // Since #15 the shape per class lives in one place, and `new` is
            // what Ruby says it is: allocate, then `initialize`. A class with no
            // shape refuses inside `allocate` and names which one, rather than
            // handing back a bare object wearing a class that would misread it.
            let object = allocate_instance(scope, id)?;
            let initialize = crate::shared::symbols::intern("initialize");
            match scope.classes_mut().lookup(id, initialize) {
                None => {
                    // No `initialize` anywhere is `BasicObject#initialize`,
                    // which takes none: `BasicObject.new("x")` raises in Ruby
                    // and was silently accepted here.
                    if !call.args.is_empty() || !call.keywords.is_empty() {
                        let given = call.args.len() + call.keywords.len();
                        return Err(Error::raise(
                            "ArgumentError",
                            format!("wrong number of arguments (given {given}, expected 0)"),
                        ));
                    }
                    stack.push(object);
                    Ok(None)
                }
                Some(method) => {
                    // `new` answers the object, never what `initialize`
                    // returned. The object goes on the stack *below* the
                    // frame's base and the frame is told to leave it there,
                    // which keeps this a frame push rather than a re-entrant
                    // `eval` — PRD 0011's R7.
                    stack.push(object);
                    let call = Pending {
                        name: initialize,
                        receiver: object,
                        cref: method.cref,
                        // `super` inside `initialize` is ordinary: `def
                        // initialize(*a); super(*a); end` on an `Enumerable`
                        // fixture is all over the corpus. Reached through
                        // `new`, so the owner has to be set here too — the
                        // `dispatch` arm that usually does it is not on this
                        // path.
                        owner: Some(method.owner),
                        defined_as: Some(initialize),
                        ..call
                    };
                    match scope.definitions().get(method.body).cloned() {
                        Some(Definition::Iseq(iseq)) => {
                            // `initialize` is an ordinary method frame: its
                            // own `return` target, and no `break` target.
                            *ids += 1;
                            set_break_target(scope, call.block, *ids);
                            let links = Links {
                                id: *ids,
                                home: *ids,
                                breaks: 0,
                                scope_default: ScopeDefault::Public,
                            };
                            push_frame(
                                scope,
                                stack,
                                frames,
                                &call,
                                &iseq,
                                Value::NIL,
                                Binding::Strict,
                                links,
                            )?;
                            let last = frames.len() - 1;
                            frames[last].keeps_receiver = true;
                            Ok(None)
                        }
                        // A native `initialize` is `Object#initialize`, which
                        // does nothing; there is no other one yet.
                        _ => Ok(None),
                    }
                }
            }
        }
        Native::ClassOf => {
            let mut class =
                class_of(scope, call.receiver).ok_or_else(|| no_class(call.receiver))?;
            // `Object#class` skips singletons: `C.class` is `Class`, not
            // `#<Class:C>`, and `obj.class` is unchanged by `class << obj`.
            // The header points at the singleton once one exists, which is what
            // makes dispatch find singleton methods, so the skip belongs here.
            while scope.classes().is_singleton(class) {
                class = scope
                    .classes()
                    .superclass(class)
                    .expect("a singleton class has a superclass");
            }
            stack.push(scope.classes().object(class));
            Ok(None)
        }
        Native::Equal => {
            let other = call.args.first().copied().unwrap_or(Value::NIL);
            stack.push(bool_value(call.receiver == other));
            Ok(None)
        }
        Native::NilP => {
            stack.push(bool_value(call.receiver == Value::NIL));
            Ok(None)
        }

        // -- #15's core library ------------------------------------------------
        // Raw storage and allocation. Everything else about these classes is
        // Ruby, in `core/*.rb`.
        Native::ArraySize => {
            let handle = expect_array(scope, call.receiver, "size")?;
            let length = array_len(scope, handle);
            stack.push(Value::fixnum(length as i64).expect("a length fits a fixnum"));
            Ok(None)
        }

        Native::ArrayIndex => {
            let handle = expect_array(scope, call.receiver, "[]")?;
            let length = array_len(scope, handle);
            // A Range, or a `(start, length)` pair, is slicing: it allocates a
            // new array and answers a different shape entirely. Reading only
            // the first argument and ignoring the second would answer `a[1, 3]`
            // with one element, which is wrong rather than missing.
            // Slices are answered here since #21: `a[start, length]` and
            // `a[range]` with Integer (or nil) ends, CRuby's `rb_ary_subseq`
            // and `rb_range_component_beg_len` rule for rule. A Float index is
            // truncated, as `Float#to_int` would. An index that needs a Ruby
            // `to_int` still refuses: the native cannot send it.
            let slice = match call.args.as_slice() {
                [start, count] => match (index_arg(*start), index_arg(*count)) {
                    (Some(start), Some(count)) => Some(subseq_bounds(start, count, length)),
                    _ => None,
                },
                [range] if is_range(scope, *range) => match range_parts(scope, *range) {
                    Some(parts) => Some(range_beg_len(scope, *range, parts, length, false)?),
                    None => None,
                },
                _ => {
                    let single = match call.args.as_slice() {
                        [index] => index_arg(*index),
                        _ => None,
                    };
                    match single {
                        Some(index) => {
                            let value = match resolve_index(index, length) {
                                Some(index) => array_get(scope, handle, index),
                                None => Value::NIL,
                            };
                            stack.push(value);
                            return Ok(None);
                        }
                        None => None,
                    }
                }
            };
            let Some(bounds) = slice else {
                return Err(Error::NoDispatch {
                    op: "Array#[]",
                    operands: "an index that is not an Integer, Float or Range",
                });
            };
            let value = match bounds {
                Some((start, count)) => {
                    let elements: Vec<Value> = (start..start + count)
                        .map(|index| array_get(scope, handle, index))
                        .collect();
                    new_array(scope, &elements)
                }
                None => Value::NIL,
            };
            stack.push(value);
            Ok(None)
        }

        Native::ArrayIndexSingle => {
            let handle = expect_array(scope, call.receiver, "at")?;
            let length = array_len(scope, handle);
            if call.args.len() != 1 {
                return Err(Error::raise(
                    "ArgumentError",
                    format!(
                        "wrong number of arguments (given {}, expected 1)",
                        call.args.len()
                    ),
                ));
            }
            let Some(index) = call.args.first().and_then(|&v| index_arg(v)) else {
                return Err(Error::NoDispatch {
                    op: "Array#[]",
                    operands: "an index that is not an Integer",
                });
            };
            let resolved = resolve_index(index, length);
            let value = match resolved {
                Some(index) => array_get(scope, handle, index),
                None => Value::NIL,
            };
            stack.push(value);
            Ok(None)
        }

        Native::ArrayStore => {
            let handle = expect_array(scope, call.receiver, "[]=")?;
            frozen_check(scope, call.receiver, "array")?;
            let length = array_len(scope, handle);
            // `a[i, n] = x` and `a[range] = x` splice, which is a different
            // operation on a different number of arguments. Three arguments
            // here is the splice form, not an index and a value.
            // The splice forms since #21 — `a[start, length] = v` and
            // `a[range] = v` — are CRuby's `rb_ary_splice`, errors included.
            let splice = match call.args.as_slice() {
                [start, count, value] => match (index_arg(*start), index_arg(*count)) {
                    (Some(start), Some(count)) => {
                        let at = if start < 0 {
                            start + length as i64
                        } else {
                            start
                        };
                        if at < 0 {
                            return Err(Error::raise(
                                "IndexError",
                                format!("index {start} too small for array; minimum: -{length}"),
                            ));
                        }
                        if count < 0 {
                            return Err(Error::raise(
                                "IndexError",
                                format!("negative length ({count})"),
                            ));
                        }
                        Some((at as usize, count as usize, *value))
                    }
                    _ => None,
                },
                [range, value] if is_range(scope, *range) => match range_parts(scope, *range) {
                    Some(parts) => {
                        let (start, count) = range_beg_len(scope, *range, parts, length, true)?
                            .expect("a splice range either raises or resolves");
                        Some((start, count, *value))
                    }
                    None => None,
                },
                _ => None,
            };
            if let Some((start, count, value)) = splice {
                // The replacement: an Array's elements, or the value itself as
                // one element. Something with its own `to_ary` would need it
                // called, which a native cannot.
                let replacement = match array_elements(scope, value) {
                    Some(elements) => elements,
                    None if defines_to_ary(scope, value) => {
                        return Err(Error::NoDispatch {
                            op: "Array#[]=",
                            operands: "a replacement with its own `to_ary`",
                        });
                    }
                    None => vec![value],
                };
                let value = scope.root(value);
                array_splice(scope, handle, start, count, &replacement);
                stack.push(scope.get(value));
                return Ok(None);
            }
            let assignment = match call.args.as_slice() {
                [index, value] => index_arg(*index).map(|index| (index, *value)),
                _ => None,
            };
            let Some((index, value)) = assignment else {
                return Err(Error::NoDispatch {
                    op: "Array#[]=",
                    operands: "an index that is not an Integer, Float or Range",
                });
            };
            // A negative index past the front is an IndexError in Ruby, not a
            // silent write at zero. One past the end is not an error at all:
            // the gap fills with nil, measured — `a = [1]; a[3] = 2` is
            // `[1, nil, nil, 2]`. `resolve_index` is the *read* rule, which
            // answers nil there, and was wrongly used here until #21.
            let at = if index < 0 {
                index + length as i64
            } else {
                index
            };
            if at < 0 {
                return Err(Error::raise(
                    "IndexError",
                    format!("index {index} too small for array; minimum: -{length}"),
                ));
            }
            let index = at as usize;
            array_set(scope, handle, index, value);
            stack.push(value);
            Ok(None)
        }

        Native::ArrayPush => {
            let handle = expect_array(scope, call.receiver, "push")?;
            frozen_check(scope, call.receiver, "array")?;
            for &value in &call.args {
                array_push(scope, handle, value);
            }
            // `push` and `<<` both answer the array itself, which is what makes
            // `a << 1 << 2` chain.
            stack.push(call.receiver);
            Ok(None)
        }

        Native::ArrayPop => {
            let handle = expect_array(scope, call.receiver, "pop")?;
            frozen_check(scope, call.receiver, "array")?;
            let length = array_len(scope, handle);
            // `pop(n)` answers a new *array* of the last n (#21), however many
            // there are: measured, `[1].pop(5)` is `[1]`.
            if let Some(&count) = call.args.first() {
                if call.args.len() > 1 {
                    return Err(Error::raise(
                        "ArgumentError",
                        format!(
                            "wrong number of arguments (given {}, expected 0..1)",
                            call.args.len()
                        ),
                    ));
                }
                let Some(count) = index_arg(count) else {
                    return Err(Error::NoDispatch {
                        op: "Array#pop",
                        operands: "a count that is not an Integer",
                    });
                };
                if count < 0 {
                    return Err(Error::raise("ArgumentError", "negative array size"));
                }
                let taken = (count as usize).min(length);
                let elements: Vec<Value> = (length - taken..length)
                    .map(|index| array_get(scope, handle, index))
                    .collect();
                let popped = new_array(scope, &elements);
                array_set_len(scope, handle, length - taken);
                stack.push(popped);
                return Ok(None);
            }
            if length == 0 {
                stack.push(Value::NIL);
                return Ok(None);
            }
            let value = array_get(scope, handle, length - 1);
            array_set_len(scope, handle, length - 1);
            stack.push(value);
            Ok(None)
        }

        Native::StringSize { bytes } => {
            let Some(payload) = string_bytes(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "String#length",
                    operands: "a receiver that is not a String",
                });
            };
            let length = if bytes {
                payload.len()
            } else {
                // Characters, by the receiver's encoding: an invalid UTF-8
                // byte is a character of its own, and an encoding whose
                // boundaries this VM does not know refuses rather than
                // counting bytes and calling them characters.
                let encoding =
                    string_encoding(scope, call.receiver).unwrap_or(crate::strings::UTF_8);
                match crate::strings::char_offsets(encoding, &payload) {
                    Some(offsets) => offsets.len() - 1,
                    None => return Err(unknown_encoding("`length`")),
                }
            };
            stack.push(Value::fixnum(length as i64).expect("a length fits a fixnum"));
            Ok(None)
        }

        Native::StringConcat => {
            let Some(mut left) = string_bytes(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "String#+",
                    operands: "a receiver that is not a String",
                });
            };
            let Some(right) = call.args.first().and_then(|&v| string_bytes(scope, v)) else {
                return Err(Error::raise(
                    "TypeError",
                    "no implicit conversion of nil into String",
                ));
            };
            let left_enc = string_encoding(scope, call.receiver).unwrap_or(crate::strings::UTF_8);
            let right_enc = call
                .args
                .first()
                .and_then(|&v| string_encoding(scope, v))
                .unwrap_or(crate::strings::UTF_8);
            let Some(encoding) = crate::strings::compatible((left_enc, &left), (right_enc, &right))
            else {
                return Err(incompatible_encodings(left_enc, right_enc));
            };
            left.extend_from_slice(&right);
            let value = string_bytes_in(scope, Builtin::String.id(), &left, encoding);
            stack.push(value);
            Ok(None)
        }

        Native::StringRepeat => {
            let Some(bytes) = string_bytes(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "String#*",
                    operands: "a receiver that is not a String",
                });
            };
            let Some(count) = call.args.first().and_then(|v| v.as_fixnum()) else {
                return Err(Error::raise(
                    "TypeError",
                    "no implicit conversion into Integer",
                ));
            };
            if count < 0 {
                return Err(Error::raise("ArgumentError", "negative argument"));
            }
            let encoding = string_encoding(scope, call.receiver).unwrap_or(crate::strings::UTF_8);
            let value = string_bytes_in(
                scope,
                Builtin::String.id(),
                &bytes.repeat(count as usize),
                encoding,
            );
            stack.push(value);
            Ok(None)
        }

        Native::Freeze => {
            if !call.receiver.is_immediate() {
                let handle = scope.root(call.receiver);
                scope.freeze(handle);
            }
            stack.push(call.receiver);
            Ok(None)
        }

        Native::FrozenP => {
            // Every immediate is frozen in Ruby: a fixnum, a symbol, `nil`,
            // `true` and `false` have nothing to mutate.
            let frozen = if call.receiver.is_immediate() {
                true
            } else {
                let handle = scope.root(call.receiver);
                scope.is_frozen(handle)
            };
            stack.push(bool_value(frozen));
            Ok(None)
        }

        Native::ObjectId => {
            // ponytail: an object's id is derived from its address, which is
            // unique and stable only because the collector does not move
            // objects. Phase 6's moving GC needs a side table keyed by the id
            // already handed out.
            //
            // The immediates are Ruby's own documented values where Ruby has
            // one, and the tagged word otherwise — which is unique per value
            // and equal for the same value, the two properties `object_id` is
            // asked for.
            use crate::value::Unpacked;
            let id = match call.receiver.unpack() {
                Unpacked::Fixnum(n) => n.checked_mul(2).and_then(|n| n.checked_add(1)),
                Unpacked::Nil => Some(8),
                Unpacked::True => Some(20),
                Unpacked::False => Some(0),
                Unpacked::Symbol(_) | Unpacked::Flonum(_) | Unpacked::Undef => {
                    i64::try_from(call.receiver.to_bits() >> 1).ok()
                }
                Unpacked::Heap(_) => {
                    let handle = scope.root(call.receiver);
                    // The address, shifted past the alignment bits that are
                    // zero on every cell.
                    i64::try_from(scope.address(handle) >> 4).ok()
                }
            };
            stack.push(id.and_then(Value::fixnum).unwrap_or(Value::NIL));
            Ok(None)
        }

        Native::Dup => {
            // A class's identity lives in the class table, not in its cell:
            // copying the cell would hand back an object that looks like the
            // class and shares its method table. `Class#dup` is real in Ruby
            // and is not this.
            if class_id_of(scope, call.receiver).is_some() {
                return Err(Error::Unknowable {
                    what: "`dup` on a class or module",
                    needs: "a copy of its method table, which is `Module#dup`",
                });
            }
            // A copy of a `Proc` must be `==` to the original — CRuby special-
            // cases it — and `Proc#==` here would be comparing six slots whose
            // equality is not the same question. Copying the cell and letting
            // `==` answer false would be a wrong answer, not a missing one.
            if is_builtin(scope, call.receiver, Builtin::Proc) {
                return Err(Error::Unknowable {
                    what: "`dup` on a Proc",
                    needs: "`Proc#==`, which compares bodies rather than identity",
                });
            }
            let value = dup_value(scope, call.receiver)?;
            stack.push(value);
            Ok(None)
        }

        Native::Allocate => {
            let Some(id) = class_id_of(scope, call.receiver) else {
                return Err(Error::raise(
                    "NoMethodError",
                    format!(
                        "undefined method 'allocate' for an instance of {}",
                        class_name(scope, call.receiver)
                    ),
                ));
            };
            if scope.classes().is_singleton(id) {
                return Err(Error::raise(
                    "TypeError",
                    "can't create instance of singleton class",
                ));
            }
            // `Array.allocate(1)` raises in Ruby: `allocate` never takes
            // arguments, on any class.
            if !call.args.is_empty() || !call.keywords.is_empty() {
                let given = call.args.len() + call.keywords.len();
                return Err(Error::raise(
                    "ArgumentError",
                    format!("wrong number of arguments (given {given}, expected 0)"),
                ));
            }
            let value = allocate_instance(scope, id)?;
            stack.push(value);
            Ok(None)
        }

        Native::NumOp(op) => {
            if call.args.len() != 1 {
                return Err(Error::raise(
                    "ArgumentError",
                    format!(
                        "wrong number of arguments (given {}, expected 1)",
                        call.args.len()
                    ),
                ));
            }
            match binop(scope, op, call.receiver, call.args[0]) {
                Ok(value) => {
                    stack.push(value);
                    Ok(None)
                }
                // Not a pair of numbers, so `Numeric`'s operator of the same
                // name decides: it asks the operand to `coerce`. Reached as
                // `super` from the class this primitive is defined on, and
                // under the operator's own name rather than the call's, which
                // an `alias_method :old_plus, :+` makes a different one.
                Err(Error::NoDispatch { .. }) => {
                    let owner = if call.receiver.as_flonum().is_some() {
                        Builtin::Float.id()
                    } else {
                        Builtin::Integer.id()
                    };
                    let forwarded = Pending {
                        cache: None,
                        name: crate::shared::symbols::intern(op.name()),
                        implicit_self: true,
                        public_only: false,
                        target: Target::Super { owner },
                        owner: None,
                        defined_as: None,
                        ..call
                    };
                    dispatch(scope, stack, frames, forwarded, proc_class, ids)
                }
                Err(other) => Err(other),
            }
        }
        Native::NumNeg => {
            if !call.args.is_empty() {
                return Err(Error::raise(
                    "ArgumentError",
                    format!(
                        "wrong number of arguments (given {}, expected 0)",
                        call.args.len()
                    ),
                ));
            }
            let value = negate(scope, call.receiver)?;
            stack.push(value);
            Ok(None)
        }

        Native::IntBits(op) => {
            // A bignum on either side goes through `BigInt`, which has all six
            // of these. Two's complement is what `&`, `|`, `^` and `~` mean in
            // Ruby for a negative operand, and it is what `num-bigint`
            // implements, so `~(2**70)` needs no sign fixup here.
            if crate::bignum::is_big(scope, call.receiver)
                || call
                    .args
                    .first()
                    .is_some_and(|v| crate::bignum::is_big(scope, *v))
            {
                let value = wide_bits(scope, op, call.receiver, call.args.first().copied())?;
                stack.push(value);
                return Ok(None);
            }
            let Some(left) = call.receiver.as_fixnum() else {
                return Err(Error::NoDispatch {
                    op: "Integer bit operation",
                    operands: "a receiver that is not an Integer",
                });
            };
            if op == crate::method::BitOp::Not {
                stack.push(fixnum_or_refuse(!left, "~")?);
                return Ok(None);
            }
            let Some(right) = call.args.first().and_then(|v| v.as_fixnum()) else {
                return Err(Error::raise(
                    "TypeError",
                    "no implicit conversion into Integer",
                ));
            };
            let answer = match op {
                crate::method::BitOp::And => left & right,
                crate::method::BitOp::Or => left | right,
                crate::method::BitOp::Xor => left ^ right,
                crate::method::BitOp::Not => unreachable!("handled above"),
                // A shift wider than the word, or one that would push a bit off
                // the top, is a bignum in Ruby. Refuse rather than wrap.
                crate::method::BitOp::Shl | crate::method::BitOp::Shr => {
                    let left_shift = op == crate::method::BitOp::Shl;
                    // `a >> -n` is `a << n`, and the other way round.
                    let (left_shift, distance) = if right < 0 {
                        (!left_shift, -right)
                    } else {
                        (left_shift, right)
                    };
                    if left_shift {
                        // A left shift is a multiplication by a power of two,
                        // and Ruby lets it grow: `1 << 70` is a bignum. Done in
                        // `BigInt` for any distance, because `checked_shl`
                        // refuses at exactly the boundary this slice removed.
                        let Ok(distance) = u32::try_from(distance) else {
                            return Err(Error::NoDispatch {
                                op: "Integer#<<",
                                operands: "a shift too large to allocate a result for",
                            });
                        };
                        let shifted = num_bigint::BigInt::from(left) << distance;
                        let value = crate::bignum::value(scope, &shifted);
                        stack.push(value);
                        return Ok(None);
                    } else if distance >= 64 {
                        // Shifting right off the end is the sign bit, forever.
                        if left < 0 { -1 } else { 0 }
                    } else {
                        left >> distance
                    }
                }
            };
            stack.push(fixnum_or_refuse(answer, "Integer bit operation")?);
            Ok(None)
        }

        Native::IntToSRadix => {
            let n = crate::bignum::read(scope, call.receiver)
                .or_else(|| call.receiver.as_fixnum().map(num_bigint::BigInt::from));
            let base = call.args.first().and_then(|v| v.as_fixnum());
            let (Some(n), Some(base @ 2..=36)) = (n, base) else {
                return Err(Error::NoDispatch {
                    op: "Integer#to_s",
                    operands: "a receiver that is not an Integer, or a radix outside 2..36",
                });
            };
            let digits = n.to_str_radix(base as u32);
            let class = class_handle(scope, Builtin::String);
            let value = string_alloc(scope, class, digits.as_bytes(), crate::strings::US_ASCII);
            stack.push(value);
            Ok(None)
        }

        Native::IntPow => {
            let Some(base) = call.receiver.as_fixnum() else {
                return Err(Error::NoDispatch {
                    op: "Integer#**",
                    operands: "a receiver that is not an Integer",
                });
            };
            let Some(exponent) = call.args.first().and_then(|v| v.as_fixnum()) else {
                return Err(Error::raise(
                    "TypeError",
                    "no implicit conversion into Integer",
                ));
            };
            if exponent < 0 {
                // `2 ** -1` is a Rational in Ruby, which this VM does not have.
                return Err(Error::NoDispatch {
                    op: "Integer#**",
                    operands: "a negative exponent, which is a Rational",
                });
            }
            // `2 ** 70` is the shape the whole bignum slice was filed for, so
            // the exponent loop is `BigInt`'s rather than a `checked_mul` chain
            // that refuses at 2^62. `pow` wants a `u32`; an exponent past that
            // asks for a number with more digits than the heap has bytes, and
            // CRuby warns and takes minutes rather than answering, so refusing
            // is the honest answer and not a shortcut.
            let Ok(exponent) = u32::try_from(exponent) else {
                return Err(Error::NoDispatch {
                    op: "Integer#**",
                    operands: "an exponent too large to allocate a result for",
                });
            };
            // CRuby 3.4 refuses a result past 16G bits up front, rather than
            // trying to allocate it. Measured: `100000000 ** 1000000000` is an
            // ArgumentError, `2 ** 40000000` an Integer.
            const LIMIT_BITS: u64 = 16 << 30;
            let base_bits = u64::from(64 - base.unsigned_abs().leading_zeros());
            if base_bits > 1 && base_bits.saturating_mul(u64::from(exponent)) > LIMIT_BITS {
                return Err(Error::raise("ArgumentError", "exponent is too large"));
            }
            let answer = num_bigint::BigInt::from(base).pow(exponent);
            let value = crate::bignum::value(scope, &answer);
            stack.push(value);
            Ok(None)
        }

        Native::SymbolName { length } => {
            let Some(id) = call.receiver.as_symbol() else {
                return Err(Error::NoDispatch {
                    op: "Symbol#to_s",
                    operands: "a receiver that is not a Symbol",
                });
            };
            let name = crate::shared::symbols::name(id).unwrap_or_default();
            let value = if length {
                // Characters, not bytes: `:"あab".length` is 3. A symbol's name
                // is always valid UTF-8, because it came from source.
                let characters = name.chars().count();
                Value::fixnum(characters as i64).expect("a name length fits a fixnum")
            } else {
                // US-ASCII for an ASCII name, UTF-8 otherwise — measured.
                let encoding = if name.is_ascii() {
                    crate::strings::US_ASCII
                } else {
                    crate::strings::UTF_8
                };
                string_bytes_in(scope, Builtin::String.id(), name.as_bytes(), encoding)
            };
            stack.push(value);
            Ok(None)
        }

        Native::FloatToS => {
            let Some(f) = call.receiver.as_flonum() else {
                return Err(Error::NoDispatch {
                    op: "Float#to_s",
                    operands: "a receiver that is not a Float",
                });
            };
            // US-ASCII, as every number's `to_s` is. Measured.
            let text = float_to_s(f);
            let value = string_bytes_in(
                scope,
                Builtin::String.id(),
                text.as_bytes(),
                crate::strings::US_ASCII,
            );
            stack.push(value);
            Ok(None)
        }

        Native::StringIndex => {
            let Some(payload) = string_bytes(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "String#[]",
                    operands: "a receiver that is not a String",
                });
            };
            let encoding = string_encoding(scope, call.receiver).unwrap_or(crate::strings::UTF_8);
            let Some(offsets) = crate::strings::char_offsets(encoding, &payload) else {
                return Err(unknown_encoding("`[]`"));
            };
            // `s[range]`, `s["sub"]` and `s[/re/]` are three more operations
            // with three more answers. Reading only the integer form and
            // ignoring the rest would answer them wrongly.
            let request = match call.args.as_slice() {
                [start] => start.as_fixnum().map(|start| (start, 1, false)),
                [start, count] => match (start.as_fixnum(), count.as_fixnum()) {
                    (Some(start), Some(count)) => Some((start, count, true)),
                    _ => None,
                },
                _ => None,
            };
            let Some((start, count, ranged)) = request else {
                return Err(Error::NoDispatch {
                    op: "String#[]",
                    operands: "an index that is not an Integer",
                });
            };
            let length = offsets.len() - 1;
            // `s[s.length]` is `""` for the two-argument form and `nil` for the
            // one-argument form, which is Ruby and is measured, not guessed.
            let start = if start < 0 {
                start + length as i64
            } else {
                start
            };
            if start < 0 || start > length as i64 || (!ranged && start == length as i64) {
                stack.push(Value::NIL);
                return Ok(None);
            }
            if count < 0 {
                stack.push(Value::NIL);
                return Ok(None);
            }
            let start = start as usize;
            let end = (start + count as usize).min(length);
            let slice = &payload[offsets[start]..offsets[end]];
            let value = string_bytes_in(scope, Builtin::String.id(), slice, encoding);
            stack.push(value);
            Ok(None)
        }

        Native::StringCompare => {
            let Some(left) = string_bytes(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "String#<=>",
                    operands: "a receiver that is not a String",
                });
            };
            // `<=>` answers nil for anything that is not a String, which is
            // what makes `Comparable` raise rather than guess.
            let Some(right) = call.args.first().and_then(|&v| string_bytes(scope, v)) else {
                stack.push(Value::NIL);
                return Ok(None);
            };
            // Bytes, which is what Ruby compares: `"a" <=> "b"` does not decode.
            // Equal bytes in encodings that cannot be compared order by
            // encoding index — CRuby's `rb_str_cmp`, measured.
            let left_enc = string_encoding(scope, call.receiver).unwrap_or(crate::strings::UTF_8);
            let right_enc = call
                .args
                .first()
                .and_then(|&v| string_encoding(scope, v))
                .unwrap_or(crate::strings::UTF_8);
            let answer = match left.cmp(&right) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal
                    if !crate::strings::comparable((left_enc, &left), (right_enc, &right)) =>
                {
                    if left_enc > right_enc {
                        1
                    } else {
                        -1
                    }
                }
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
            stack.push(Value::fixnum(answer).expect("-1, 0 and 1 are fixnums"));
            Ok(None)
        }

        Native::WriteString => {
            let Some(bytes) = call.args.first().and_then(|&v| string_bytes(scope, v)) else {
                return Err(Error::NoDispatch {
                    op: "__write__",
                    operands: "an argument that is not a String",
                });
            };
            use std::io::Write as _;
            // A closed or full stdout is a real error in Ruby (`EPIPE`), and IO
            // is phase 3. Until there is an `IOError` to raise, a failed write
            // is reported rather than swallowed. The second argument is the
            // descriptor: 2 is stderr, anything else stdout.
            let written = if call.args.get(1).and_then(|v| v.as_fixnum()) == Some(2) {
                std::io::stderr().write_all(&bytes)
            } else {
                std::io::stdout().write_all(&bytes)
            };
            written.map_err(|err| Error::raise("RuntimeError", format!("cannot write: {err}")))?;
            stack.push(Value::NIL);
            Ok(None)
        }

        Native::Strerror => {
            let Some(number) = call.args.first().and_then(|v| v.as_fixnum()) else {
                return Err(Error::NoDispatch {
                    op: "__strerror__",
                    operands: "an argument that is not an Integer",
                });
            };
            // `core/exception.rb` range-checks to a C `int` before calling, as
            // CRuby's `NUM2INT` does, so this never truncates a real errno.
            let number = i32::try_from(number).map_err(|_| Error::NoDispatch {
                op: "__strerror__",
                operands: "an Integer outside a C int",
            })?;
            let value = string_new(scope, &crate::errno::message(number));
            stack.push(value);
            Ok(None)
        }

        Native::ErrnoClass => {
            let Some(number) = call.args.first().and_then(|v| v.as_fixnum()) else {
                return Err(Error::NoDispatch {
                    op: "__errno_class__",
                    operands: "an argument that is not an Integer",
                });
            };
            // Resolved through the `Errno` module's constants by the table's
            // own names, so the answer is whatever class bootstrap put there.
            // 0 is in the table: measured, `SystemCallError.new(0)` is an
            // `Errno::NOERROR` whose message is the platform's "Success".
            let found = crate::errno::table().find(|&(_, known)| i64::from(known) == number);
            let errno_module = scope
                .classes()
                .const_get_here(
                    Builtin::Object.id(),
                    crate::shared::symbols::intern("Errno"),
                )
                .and_then(|value| class_id_of(scope, value));
            let class = match (found, errno_module) {
                (Some((name, _)), Some(module)) => scope
                    .classes()
                    .const_get_here(module, crate::shared::symbols::intern(name))
                    .unwrap_or(Value::NIL),
                _ => Value::NIL,
            };
            stack.push(class);
            Ok(None)
        }

        Native::SignalList => {
            let mut entries = Vec::new();
            for (name, number) in crate::signal::list() {
                let name = string_new(scope, name);
                let number = Value::fixnum(i64::from(number)).expect("a signal number is a fixnum");
                entries.push(new_array(scope, &[name, number]));
            }
            let limit = Value::fixnum(i64::from(crate::signal::LIMIT)).expect("NSIG is a fixnum");
            let pairs = new_array(scope, &entries);
            let value = new_array(scope, &[pairs, limit]);
            stack.push(value);
            Ok(None)
        }

        Native::BacktraceHere => {
            let below = &frames[..frames.len().saturating_sub(1)];
            let lines = backtrace_of(scope, below);
            let (_, triples) = backtrace_values(scope, &lines);
            stack.push(triples);
            Ok(None)
        }

        Native::Fiber(op) => fiber_native(scope, stack, frames, &call, op, ids),
        Native::Reflect(op) => reflect_native(scope, stack, &call, op),
        Native::Str(op) => str_native(scope, stack, &call, op),

        Native::RaiseNoMethod => {
            let name = call
                .args
                .first()
                .and_then(|v| v.as_symbol())
                .ok_or(Error::NoDispatch {
                    op: "method_missing",
                    operands: "a name that is not a Symbol",
                })?;
            let args = call.args.get(1).copied().unwrap_or(Value::NIL);
            let exception = no_method_error(scope, call.receiver, name);
            let exception = scope.root(exception);
            let object = scope.get(exception);
            ivar_set(scope, object, symbol("@args"), args)?;
            Ok(Some(Unwind::Exception(scope.get(exception))))
        }

        Native::StringIntern => {
            let Some(bytes) = string_bytes(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "String#to_sym",
                    operands: "a receiver that is not a String",
                });
            };
            // As `:"..."` does: a symbol whose bytes are not UTF-8 waits for
            // the Encoding slice, which keys the table by bytes.
            let Ok(name) = String::from_utf8(bytes) else {
                return Err(Error::Unknowable {
                    what: "`to_sym` on a String that is not UTF-8",
                    needs: "the Encoding slice (#19)",
                });
            };
            stack.push(Value::symbol(crate::shared::symbols::intern(&name)));
            Ok(None)
        }

        Native::DefineMethod { singleton } => {
            let target = if singleton {
                singleton_of(scope, call.receiver)?
            } else {
                class_id_of(scope, call.receiver).ok_or(Error::NoDispatch {
                    op: "define_method",
                    operands: "a receiver that is not a Module",
                })?
            };
            let Some(&name_value) = call.args.first() else {
                return Err(Error::raise(
                    "ArgumentError",
                    "wrong number of arguments (given 0, expected 1..2)",
                ));
            };
            if call.args.len() > 2 {
                return Err(Error::raise(
                    "ArgumentError",
                    format!(
                        "wrong number of arguments (given {}, expected 1..2)",
                        call.args.len()
                    ),
                ));
            }
            let Some(name) = method_name_of(scope, name_value) else {
                let shown = inspect(scope, name_value);
                return Err(Error::raise(
                    "TypeError",
                    format!("{shown} is not a symbol nor a string"),
                ));
            };
            // The body: a Proc argument, or the block. Measured messages for
            // neither and for the wrong kind.
            let body_value = match call.args.get(1) {
                Some(&given) => given,
                None if call.block != Value::NIL => call.block,
                None => {
                    return Err(Error::raise(
                        "ArgumentError",
                        "tried to create Proc object without a block",
                    ));
                }
            };
            if proc_body(scope, body_value).is_none() {
                let class = class_name(scope, body_value);
                if class == "Method" || class == "UnboundMethod" {
                    return Err(Error::NoDispatch {
                        op: "define_method",
                        operands: "a Method or UnboundMethod body, which is #27",
                    });
                }
                return Err(Error::raise(
                    "TypeError",
                    format!("wrong argument type {class} (expected Proc/Method/UnboundMethod)"),
                ));
            }
            let symbol = crate::shared::symbols::intern(&name);
            // `initialize` and its siblings are private wherever they are
            // defined; otherwise a call from the class's own body follows that
            // body's default — `private; define_method(:b) {}` is private.
            // Measured.
            let always_private = matches!(
                name.as_str(),
                "initialize"
                    | "initialize_copy"
                    | "initialize_clone"
                    | "initialize_dup"
                    | "respond_to_missing?"
            );
            let visibility = if always_private {
                Visibility::Private
            } else if !singleton
                && frames
                    .last()
                    .is_some_and(|frame| scope.classes().cref_class(frame.cref) == target)
            {
                frames
                    .last()
                    .map_or(Visibility::Public, |frame| frame.scope_default.visibility())
            } else {
                Visibility::Public
            };
            // A frozen module, or a frozen object's singleton: CRuby's
            // message names the frozen object, measured.
            let holder = if singleton {
                call.receiver
            } else {
                scope.classes().object(target)
            };
            if !holder.is_immediate() {
                let handle = scope.root(holder);
                if scope.is_frozen(handle) {
                    let class = class_name(scope, holder);
                    let shown = inspect(scope, holder);
                    return Err(Error::raise(
                        "FrozenError",
                        format!("can't modify frozen {class}: {shown}"),
                    ));
                }
            }
            // Under a bare `module_function` in the module's own body, the
            // method is a module function too: private here, public on the
            // singleton. Measured.
            let module_function = !singleton
                && frames.last().is_some_and(|frame| {
                    frame.scope_default == ScopeDefault::ModuleFunction
                        && scope.classes().cref_class(frame.cref) == target
                });
            let body = scope.definitions_mut().add(Definition::Proc(body_value));
            scope
                .classes_mut()
                .define_method_visibly(target, symbol, body, call.cref, visibility);
            stack.push(Value::symbol(symbol));
            if module_function {
                let meta = scope.singleton_class(target);
                scope.classes_mut().define_method_visibly(
                    meta,
                    symbol,
                    body,
                    call.cref,
                    Visibility::Public,
                );
                if let Some(unwind) =
                    fire_method_hook(scope, stack, frames, proc_class, ids, meta, "added", symbol)?
                {
                    return Ok(Some(unwind));
                }
            }
            fire_method_hook(
                scope, stack, frames, proc_class, ids, target, "added", symbol,
            )
        }

        Native::EvalBlock { module, exec } => {
            // No block: the body is a String, and this is string `eval`.
            let string_body = call.block == Value::NIL;
            // The class a string body's constants resolve in, read before
            // anything below makes a singleton class.
            let constant_class = class_of(scope, call.receiver);
            if string_body {
                if exec {
                    return Err(Error::raise("LocalJumpError", "no block given"));
                }
                if call.args.is_empty() || call.args.len() > 3 {
                    return Err(Error::raise(
                        "ArgumentError",
                        format!(
                            "wrong number of arguments (given {}, expected 1..3)",
                            call.args.len()
                        ),
                    ));
                }
            }
            if !string_body && !exec && !call.args.is_empty() {
                return Err(Error::raise(
                    "ArgumentError",
                    format!(
                        "wrong number of arguments (given {}, expected 0)",
                        call.args.len()
                    ),
                ));
            }
            // A Symbol or a number — a bignum and a heap Float included — cannot
            // have a singleton class.
            let numeric = class_of(scope, call.receiver).is_some_and(|class| {
                matches!(
                    scope.classes().repr(class),
                    Some(Builtin::Integer | Builtin::Float)
                )
            });
            let refuses_def = !module
                && (numeric
                    || (call.receiver.is_immediate()
                        && !matches!(call.receiver, Value::NIL | Value::TRUE | Value::FALSE)));
            let definee = if module {
                class_id_of(scope, call.receiver).ok_or(Error::NoDispatch {
                    op: "class_eval",
                    operands: "a receiver that is not a Module",
                })?
            } else if refuses_def {
                // An Integer cannot have a singleton class, so the eval node
                // refuses `def` (`refuses_def`); everything else in the body
                // runs against the class.
                class_of(scope, call.receiver).ok_or_else(|| no_class(call.receiver))?
            } else {
                singleton_of(scope, call.receiver)?
            };
            if string_body {
                let cref = frames.last().map_or(CrefId::ROOT, |frame| frame.cref);
                let cref = if module {
                    // A string `class_eval` is the class body reopened.
                    scope.classes_mut().push_cref(cref, definee)
                } else {
                    let constants = constant_class.unwrap_or(definee);
                    let cref = scope.classes_mut().push_constant_scope(cref, constants);
                    if refuses_def {
                        scope
                            .classes_mut()
                            .push_eval_cref_refusing_def(cref, definee)
                    } else {
                        scope.classes_mut().push_eval_cref(cref, definee)
                    }
                };
                let binding = capture_binding(scope, frames, frames.len().checked_sub(1))?;
                let handle = scope.root(binding);
                scope.set_slot(handle, BINDING_SELF, call.receiver);
                scope.set_slot(handle, BINDING_CREF, cref_value(cref));
                // A body of its own, so `def` starts public.
                scope.set_slot(handle, BINDING_VISIBILITY, Value::fixnum(0).expect("small"));
                let binding = scope.get(handle);
                return binding_eval(scope, stack, frames, &call, binding, ids);
            }
            let (receiver, block) = (call.receiver, call.block);
            let args = if exec {
                call.args.clone()
            } else {
                vec![receiver]
            };
            let keywords = if exec {
                call.keywords.clone()
            } else {
                Vec::new()
            };
            let inner = Pending {
                args,
                keywords,
                block: Value::NIL,
                block_is_literal: false,
                target: Target::Block(block),
                ..call
            };
            let role = ProcRole::Eval {
                receiver,
                definee,
                refuses_def,
            };
            // `break` in the block ends this call, whose frame is the block's
            // own: the id it is about to be given.
            set_break_target(scope, block, *ids + 1);
            push_proc_frame_as(scope, stack, frames, &inner, block, ids, role)?;
            Ok(None)
        }

        Native::ProcLocation => {
            let block = call.args.first().copied().unwrap_or(Value::NIL);
            let value = match proc_parts(scope, block) {
                Some((iseq, ..)) if iseq.first_line > 0 => match &iseq.path {
                    Some(path) => {
                        let path = string_new(scope, path);
                        let line = Value::fixnum(i64::from(iseq.first_line))
                            .expect("a line number is a fixnum");
                        new_array(scope, &[path, line])
                    }
                    None => Value::NIL,
                },
                _ => Value::NIL,
            };
            stack.push(value);
            Ok(None)
        }

        Native::Refuse { what, needs } => Err(Error::Unknowable { what, needs }),

        Native::NeedsThreads => Err(Error::Unknowable {
            what: "starting a `Thread`",
            needs: "`Thread` on the per-Ractor lock (#45)",
        }),

        Native::StderrTty => {
            use std::io::IsTerminal as _;
            stack.push(bool_value(std::io::stderr().is_terminal()));
            Ok(None)
        }

        Native::AbsolutePath => {
            let Some(bytes) = call.args.first().and_then(|&v| string_bytes(scope, v)) else {
                return Err(Error::NoDispatch {
                    op: "__absolute_path__",
                    operands: "an argument that is not a String",
                });
            };
            let path = String::from_utf8_lossy(&bytes).into_owned();
            let value = match std::fs::canonicalize(&path) {
                Ok(resolved) => string_new(scope, &resolved.to_string_lossy()),
                Err(_) => Value::NIL,
            };
            stack.push(value);
            Ok(None)
        }

        Native::FrameMethod { callee } => {
            // The caller's frame — a primitive pushes none of its own. A block
            // carries its method's owner and name, so it answers too.
            let value = match frames.last().map(|frame| (frame.owner, frame.defined_as)) {
                Some((Some(_), Some(called))) if callee => Value::symbol(called),
                Some((Some(owner), Some(called))) => {
                    // `__method__` is the name the body was *defined* under:
                    // through an alias that is the `def`'s own, which its
                    // `Iseq` is named after. A `define_method` body is a Proc
                    // whose `Iseq` is named after where it was written, so its
                    // name is the one it was defined as.
                    let body = scope
                        .classes_mut()
                        .lookup(owner, called)
                        .map(|method| method.body);
                    match body.and_then(|body| scope.definitions().get(body)) {
                        Some(Definition::Iseq(iseq)) => Value::symbol(symbol(&iseq.name)),
                        _ => Value::symbol(called),
                    }
                }
                _ => Value::NIL,
            };
            stack.push(value);
            Ok(None)
        }

        Native::Binding(op) => binding_native(scope, stack, frames, &call, op, ids),
        Native::LoadFile => load_file(scope, stack, frames, &call, ids),
        Native::RefusalBoundary => {
            let limit = call.args.first().and_then(|v| v.as_fixnum()).unwrap_or(0);
            if call.block == Value::NIL || limit <= 0 {
                return Err(Error::raise(
                    "ArgumentError",
                    "a refusal boundary takes a positive budget and a block",
                ));
            }
            let block = call.block;
            let inner = Pending {
                args: Vec::new(),
                keywords: Vec::new(),
                block: Value::NIL,
                block_is_literal: false,
                ..call
            };
            push_proc_frame(scope, stack, frames, &inner, block, ids)?;
            let last = frames.len() - 1;
            frames[last].boundary_limit = limit as u64;
            Ok(None)
        }
        Native::Getenv => {
            let value = match call.args.first().and_then(|&v| string_bytes(scope, v)) {
                Some(name) if !name.is_empty() && !name.contains(&b'=') && !name.contains(&0) => {
                    match std::env::var_os(std::ffi::OsString::from_vec(name)) {
                        Some(value) => os_string(scope, value),
                        None => Value::NIL,
                    }
                }
                _ => Value::NIL,
            };
            stack.push(value);
            Ok(None)
        }
        Native::Environ => {
            let mut values = Vec::new();
            for (name, value) in std::env::vars_os() {
                let name = os_string(scope, name);
                let name = scope.root(name);
                let value = os_string(scope, value);
                let value = scope.root(value);
                values.push((name, value));
            }
            let flat: Vec<Value> = values
                .into_iter()
                .flat_map(|(n, v)| [scope.get(n), scope.get(v)])
                .collect();
            let value = new_array(scope, &flat);
            stack.push(value);
            Ok(None)
        }
        Native::RubyConstants => {
            let pairs = [
                ("RUBY_VERSION", crate::LANGUAGE_VERSION.to_owned()),
                ("RUBY_ENGINE", crate::ENGINE.to_owned()),
                ("RUBY_ENGINE_VERSION", crate::ENGINE_VERSION.to_owned()),
                ("RUBY_PLATFORM", crate::platform()),
                ("RUBY_DESCRIPTION", crate::description()),
            ];
            let mut flat = Vec::new();
            for (name, value) in pairs {
                let name = string_new(scope, name);
                flat.push(scope.root(name));
                let value = string_new(scope, &value);
                flat.push(scope.root(value));
            }
            let flat: Vec<Value> = flat.into_iter().map(|h| scope.get(h)).collect();
            let value = new_array(scope, &flat);
            stack.push(value);
            Ok(None)
        }
        Native::Argv => {
            let (name, args) = scope.argv();
            let mut values = Vec::with_capacity(args.len() + 1);
            for text in std::iter::once(&name).chain(&args) {
                let value = string_new(scope, text);
                values.push(scope.root(value));
            }
            let values: Vec<Value> = values.into_iter().map(|h| scope.get(h)).collect();
            let value = new_array(scope, &values);
            stack.push(value);
            Ok(None)
        }
        Native::MarkPartial => {
            scope.mark_partial();
            stack.push(Value::NIL);
            Ok(None)
        }
        Native::FreezeGlobal => {
            for name in call.args.iter().filter_map(|v| v.as_symbol()) {
                scope.freeze_global(name);
            }
            stack.push(Value::NIL);
            Ok(None)
        }
        Native::Fs(op) => fs_native(scope, stack, &call, op),
        Native::Sys(op) => sys_native(scope, stack, &call, op),

        Native::FrameNesting => {
            let cref = frames.last().map_or(CrefId::ROOT, |frame| frame.cref);
            let scopes: Vec<Value> = scope
                .classes()
                .nesting(cref)
                .into_iter()
                .map(|id| scope.classes().object(id))
                .collect();
            let value = new_array(scope, &scopes);
            stack.push(value);
            Ok(None)
        }

        Native::FrameDir => {
            let path = frames.last().and_then(|frame| frame.iseq.path.clone());
            let from_eval = frames.last().is_some_and(|frame| frame.iseq.from_eval);
            let value = match path {
                // An `eval`'s file is a name, not a path to resolve: CRuby
                // answers its `dirname`, or nil when none was given.
                Some(path) if from_eval => {
                    if path.starts_with("(eval at ") {
                        Value::NIL
                    } else {
                        let parent = std::path::Path::new(&*path)
                            .parent()
                            .filter(|parent| !parent.as_os_str().is_empty())
                            .unwrap_or(std::path::Path::new("."));
                        string_new(scope, &parent.to_string_lossy())
                    }
                }
                Some(path) if !path.starts_with('<') && !path.starts_with('(') => {
                    let parent = std::path::Path::new(&*path)
                        .parent()
                        .filter(|parent| !parent.as_os_str().is_empty())
                        .unwrap_or(std::path::Path::new("."));
                    let resolved =
                        std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
                    string_new(scope, &resolved.to_string_lossy())
                }
                _ => Value::NIL,
            };
            stack.push(value);
            Ok(None)
        }

        Native::Sleep => {
            let seconds = match call.args.first() {
                Some(&value) => value
                    .as_fixnum()
                    .map(|n| n as f64)
                    .or_else(|| value.as_flonum()),
                None => None,
            };
            let Some(seconds) = seconds.filter(|s| s.is_finite() && *s >= 0.0) else {
                return Err(Error::NoDispatch {
                    op: "__sleep__",
                    operands: "a duration that is not a non-negative number",
                });
            };
            let start = std::time::Instant::now();
            std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
            let slept = start.elapsed().as_secs_f64().round() as i64;
            stack.push(Value::fixnum(slept).unwrap_or(Value::NIL));
            Ok(None)
        }

        Native::HashCombine => {
            let mut hasher = std::hash::DefaultHasher::new();
            for &value in &call.args {
                hash_value(scope, value, &mut hasher, 0)?;
            }
            // A fixnum, as `hash` always is: the same shift `HashValue` uses.
            let bits = std::hash::Hasher::finish(&hasher) >> 2;
            stack.push(Value::fixnum(bits as i64).unwrap_or(Value::NIL));
            Ok(None)
        }

        Native::HashValue => {
            let mut hasher = std::hash::DefaultHasher::new();
            hash_value(scope, call.receiver, &mut hasher, 0)?;
            // Ruby's `hash` is a fixnum, so the top bits go: a fixnum is 62
            // bits and the shift keeps the sign bit clear.
            let bits = std::hash::Hasher::finish(&hasher) >> 2;
            stack.push(Value::fixnum(bits as i64).unwrap_or(Value::NIL));
            Ok(None)
        }

        Native::Mixin { prepend } => {
            let Some(target) = class_id_of(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "Module#include",
                    operands: "a receiver that is not a Module",
                });
            };
            // `include A, B` mixes them in *right to left*, so that `A` ends up
            // nearer the class than `B`. Measured, not guessed: it is the order
            // `Module#ancestors` reports afterwards.
            if call.args.is_empty() {
                return Err(Error::raise(
                    "ArgumentError",
                    "wrong number of arguments (given 0, expected 1+)",
                ));
            }
            for &argument in call.args.iter().rev() {
                let Some(module) = class_id_of(scope, argument) else {
                    return Err(Error::raise(
                        "TypeError",
                        format!(
                            "wrong argument type {} (expected Module)",
                            class_name(scope, argument)
                        ),
                    ));
                };
                let how = if prepend {
                    scope.classes_mut().prepend(target, module)
                } else {
                    scope.classes_mut().include(target, module)
                };
                if let Err(err) = how {
                    return Err(match err {
                        crate::class::MixinError::NotAModule => Error::raise(
                            "TypeError",
                            format!(
                                "wrong argument type {} (expected Module)",
                                class_name(scope, argument)
                            ),
                        ),
                        crate::class::MixinError::Cyclic(_) => {
                            Error::raise("ArgumentError", format!("cyclic {err} detected"))
                        }
                    });
                }
            }
            // Ruby's `include` answers the receiver, which is what makes
            // `include Foo` usable as the last expression of a class body.
            stack.push(call.receiver);
            Ok(None)
        }

        Native::Extend => {
            // `extend` takes at least one module, same as `include`.
            if call.args.is_empty() {
                return Err(Error::raise(
                    "ArgumentError",
                    "wrong number of arguments (given 0, expected 1+)",
                ));
            }
            // Right to left, so `obj.extend(A, B)` leaves `A` nearer the
            // singleton than `B` — measured against CRuby, the same order
            // `include A, B` produces.
            for &argument in call.args.iter().rev() {
                // A Class is a Module in Ruby's hierarchy but not a legal
                // argument here: `obj.extend(String)` is a TypeError naming
                // Class, not a mixin of `String`'s methods.
                let module = match class_id_of(scope, argument) {
                    Some(id) if scope.classes().kind(id) == Kind::Module => id,
                    _ => {
                        return Err(Error::raise(
                            "TypeError",
                            format!(
                                "wrong argument type {} (expected Module)",
                                class_name(scope, argument)
                            ),
                        ));
                    }
                };
                // Allocating the singleton is the point, not a side effect:
                // `extend` is defined as an include into it. `singleton_of` is
                // what `def obj.foo` and `class << obj` already go through, so
                // an immediate is refused here with the same `TypeError` and
                // the same message rather than reaching the allocator.
                let meta = singleton_of(scope, call.receiver)?;
                if let Err(err) = scope.classes_mut().include(meta, module) {
                    return Err(match err {
                        crate::class::MixinError::NotAModule => Error::raise(
                            "TypeError",
                            format!(
                                "wrong argument type {} (expected Module)",
                                class_name(scope, argument)
                            ),
                        ),
                        crate::class::MixinError::Cyclic(_) => {
                            Error::raise("ArgumentError", format!("cyclic {err} detected"))
                        }
                    });
                }
            }
            // Ruby's `extend` answers the receiver, which is what makes
            // `obj.extend(M).foo` work.
            stack.push(call.receiver);
            Ok(None)
        }
        Native::Ancestors => {
            let Some(id) = class_id_of(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "Module#ancestors",
                    operands: "a receiver that is not a Module",
                });
            };
            let ids = scope.classes().ancestors(id);
            let objects: Vec<Value> = ids
                .into_iter()
                .map(|id| scope.classes().object(id))
                .collect();
            let value = new_array(scope, &objects);
            stack.push(value);
            Ok(None)
        }

        Native::Superclass => {
            let Some(id) = class_id_of(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "Class#superclass",
                    operands: "a receiver that is not a Class",
                });
            };
            // `BasicObject.superclass` is nil, and so is a module's — but a
            // module has no `superclass` method at all in Ruby, so saying nil
            // for one would be answering a call that should not have arrived.
            if scope.classes().kind(id) == Kind::Module {
                return Err(Error::raise(
                    "NoMethodError",
                    format!(
                        "undefined method 'superclass' for module {}",
                        scope.classes().name(id).unwrap_or("an anonymous module")
                    ),
                ));
            }
            let value = match scope.classes().superclass(id) {
                Some(parent) => scope.classes().object(parent),
                None => Value::NIL,
            };
            stack.push(value);
            Ok(None)
        }

        Native::SetVisibility(visibility) => {
            // `main`'s `public`/`private` are these same primitives, and at
            // the top level they act on `Object`.
            let id = class_id_of(scope, call.receiver).unwrap_or(Builtin::Object.id());
            // Bare: the visibility every `def` below it in *this* body gets.
            // It lives on the lexical scope, so reopening the class starts
            // public again — measured, and the reason `push_cref` making a
            // fresh node per body is load-bearing rather than incidental.
            if call.args.is_empty() {
                if let Some(frame) = frames.last_mut() {
                    // Replaces the default outright: after `module_function`, a
                    // bare `private` leaves a private instance method with no
                    // singleton copy. Measured on ruby 4.0.6 (#211).
                    frame.scope_default = ScopeDefault::from(visibility);
                }
                stack.push(Value::NIL);
                return Ok(None);
            }
            let mut narrowed = Vec::new();
            for &argument in &call.args {
                let Some(name) = method_name_of(scope, argument) else {
                    return Err(Error::raise("TypeError", "is not a symbol nor a string"));
                };
                let symbol = crate::shared::symbols::intern(&name);
                // Narrowing an *inherited* method defines it here, and Ruby
                // fires `method_added` for that definition — but not when the
                // method was already this class's own. The condition is what
                // makes `private :m` in the defining class stay quiet (#28).
                if !scope.classes().method_defined_here(id, symbol) {
                    narrowed.push(symbol);
                }
                if !scope.classes_mut().set_visibility(id, symbol, visibility) {
                    // Not `describe_receiver`: this wording quotes the name and
                    // that one does not — "for class 'E'" here against "for
                    // class E" in a `NoMethodError`. Both are measured on ruby
                    // 4.0.6 and ruby/spec asserts on each.
                    let kind = match scope.classes().kind(id) {
                        Kind::Module => "module",
                        Kind::Class => "class",
                    };
                    let owner = scope
                        .classes()
                        .name(id)
                        .map_or_else(|| "?".to_owned(), ToOwned::to_owned);
                    return Err(Error::raise(
                        "NameError",
                        format!("undefined method '{name}' for {kind} '{owner}'"),
                    ));
                }
            }
            // Ruby answers the arguments: one bare, several as an array. That
            // is what makes `private def m; end` work — `def` answers a symbol.
            let value = match call.args.as_slice() {
                [only] => *only,
                many => new_array(scope, many),
            };
            stack.push(value);
            // Last-pushed runs first, so reversed: the hooks run in argument
            // order.
            for &symbol in narrowed.iter().rev() {
                if let Some(unwind) =
                    fire_method_hook(scope, stack, frames, proc_class, ids, id, "added", symbol)?
                {
                    return Ok(Some(unwind));
                }
            }
            Ok(None)
        }

        Native::ModuleFunction => {
            let Some(id) = class_id_of(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "Module#module_function",
                    operands: "a receiver that is not a Module",
                });
            };
            if call.args.is_empty() {
                // The bare form is a mode, not an operation: it makes every
                // `def` below it in this body a module function. It shares one
                // field with bare `private` because a body is in exactly one
                // default, and it answers `nil` — not `self`, and not the
                // argument list the form below answers. Measured (#211).
                if let Some(frame) = frames.last_mut() {
                    frame.scope_default = ScopeDefault::ModuleFunction;
                }
                stack.push(Value::NIL);
                return Ok(None);
            }
            let singleton = singleton_of(scope, call.receiver)?;
            for &argument in &call.args {
                let Some(name) = method_name_of(scope, argument) else {
                    return Err(Error::raise("TypeError", "is not a symbol nor a string"));
                };
                let symbol = crate::shared::symbols::intern(&name);
                let Some(method) = scope.classes_mut().lookup(id, symbol) else {
                    let owner = scope
                        .classes()
                        .name(id)
                        .map_or_else(|| "?".to_owned(), ToOwned::to_owned);
                    return Err(Error::raise(
                        "NameError",
                        format!("undefined method '{name}' for module '{owner}'"),
                    ));
                };
                // Public on the singleton, private as an instance method.
                scope.classes_mut().define_method_visibly(
                    singleton,
                    symbol,
                    method.body,
                    method.cref,
                    Visibility::Public,
                );
                scope
                    .classes_mut()
                    .set_visibility(id, symbol, Visibility::Private);
            }
            let value = match call.args.as_slice() {
                [only] => *only,
                many => new_array(scope, many),
            };
            stack.push(value);
            // The public copy is a new singleton definition, and Ruby fires
            // `singleton_method_added` for it — measured in
            // `core/module/method_added_spec.rb`. Reversed so they run in
            // argument order.
            for &argument in call.args.iter().rev() {
                let Some(name) = method_name_of(scope, argument) else {
                    continue;
                };
                let symbol = crate::shared::symbols::intern(&name);
                if let Some(unwind) = fire_method_hook(
                    scope, stack, frames, proc_class, ids, singleton, "added", symbol,
                )? {
                    return Ok(Some(unwind));
                }
            }
            Ok(None)
        }

        Native::VisibilityDefined(visibility) => {
            let Some(id) = class_id_of(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "Module#private_method_defined?",
                    operands: "a receiver that is not a Module",
                });
            };
            let Some(name) = call.args.first().and_then(|&v| method_name_of(scope, v)) else {
                return Err(Error::raise("TypeError", "is not a symbol nor a string"));
            };
            let symbol = crate::shared::symbols::intern(&name);
            let found = lookup_inherited(scope, id, symbol, call.args.get(1));
            let matched = found.is_some_and(|method| method.visibility == visibility);
            stack.push(bool_value(matched));
            Ok(None)
        }

        // Class-variable reflection. The same table `@@a` uses, reached
        // through a receiver rather than through the frame's cref — so
        // `A.class_variable_get(:@@a)` asks about `A` wherever it is called
        // from, which is the difference between these and the instructions.
        Native::ClassVariable(op) => {
            let Some(id) = class_id_of(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "Module class-variable reflection",
                    operands: "a receiver that is not a Module",
                });
            };
            if matches!(op, CvarOp::Names) {
                // `class_variables(false)` is this module's own; the default is
                // own first, then the ancestors'. Measured: `B` under `A`
                // answers `[:@@b, :@@a]`.
                let inherit = !matches!(call.args.first(), Some(&Value::FALSE));
                let names = if inherit {
                    scope.classes().cvar_names(id)
                } else {
                    scope.classes().cvar_names_own(id)
                };
                let values: Vec<Value> = names.into_iter().map(Value::symbol).collect();
                let value = new_array(scope, &values);
                stack.push(value);
                return Ok(None);
            }
            let Some(name) = call.args.first().and_then(|&v| method_name_of(scope, v)) else {
                return Err(Error::raise("TypeError", "is not a symbol nor a string"));
            };
            // `A.class_variable_get(:z)` is a `NameError` about the *name*,
            // which is a different message from the one about a missing
            // variable. Measured.
            if !name.starts_with("@@") {
                return Err(Error::raise(
                    "NameError",
                    format!("'{name}' is not allowed as a class variable name"),
                ));
            }
            let symbol = crate::shared::symbols::intern(&name);
            match op {
                CvarOp::Names => unreachable!("answered above"),
                CvarOp::Defined => {
                    let held = scope.classes().cvar_defined(id, symbol);
                    stack.push(bool_value(held));
                }
                CvarOp::Get => match cvar_read(scope, id, symbol)? {
                    Some(value) => stack.push(value),
                    None => {
                        let where_ = scope
                            .classes()
                            .name(id)
                            .map_or_else(|| "an anonymous class".to_owned(), str::to_owned);
                        return Err(Error::raise(
                            "NameError",
                            format!("uninitialized class variable {name} in {where_}"),
                        ));
                    }
                },
                CvarOp::Set => {
                    // A frozen module takes no writes, class variables
                    // included.
                    frozen_check(scope, call.receiver, "class")?;
                    let value = call.args.get(1).copied().unwrap_or(Value::NIL);
                    scope.classes_mut().cvar_set(id, symbol, value);
                    // `class_variable_set` answers the value it wrote.
                    stack.push(value);
                }
            }
            Ok(None)
        }

        // The send-shaped `alias` and `undef`. Same two table operations as
        // `Insn::Alias` and `Insn::Undef`, but on the receiver rather than on
        // the frame's definee — which is the whole difference between the
        // statement and the method.
        Native::AliasMethod | Native::UndefMethod => {
            let aliasing = matches!(native, Native::AliasMethod);
            let op = if aliasing {
                "Module#alias_method"
            } else {
                "Module#undef_method"
            };
            let Some(id) = class_id_of(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op,
                    operands: "a receiver that is not a Module",
                });
            };
            module_frozen_check(scope, id)?;
            // `alias_method new, old` is one pair; `undef_method` takes any
            // number of names, each undefined in turn.
            let given = if aliasing {
                &call.args[..call.args.len().min(1)]
            } else {
                &call.args[..]
            };
            if aliasing && given.is_empty() {
                return Err(Error::raise("TypeError", "is not a symbol nor a string"));
            }
            let mut done = Vec::new();
            for &argument in given {
                let Some(name) = method_name_of(scope, argument) else {
                    return Err(Error::raise("TypeError", "is not a symbol nor a string"));
                };
                let name = crate::shared::symbols::intern(&name);
                let (ok, missing) = if aliasing {
                    let Some(old) = call.args.get(1).and_then(|&v| method_name_of(scope, v)) else {
                        return Err(Error::raise("TypeError", "is not a symbol nor a string"));
                    };
                    let old = crate::shared::symbols::intern(&old);
                    (scope.classes_mut().alias_method(id, name, old), old)
                } else {
                    (scope.classes_mut().undef_method(id, name), name)
                };
                if !ok {
                    return Err(Error::raise(
                        "NameError",
                        format!(
                            "undefined method '{}' for {}",
                            symbol_name(missing),
                            class_display_name(scope, id),
                        ),
                    ));
                }
                done.push(name);
            }
            // `alias_method` answers the new name; `undef_method` answers the
            // module. Measured.
            let answer = match done.first() {
                Some(&first) if aliasing => Value::symbol(first),
                _ => call.receiver,
            };
            stack.push(answer);
            // Reversed, so the hooks run in argument order.
            let event = if aliasing { "added" } else { "undefined" };
            for &name in done.iter().rev() {
                if let Some(unwind) =
                    fire_method_hook(scope, stack, frames, proc_class, ids, id, event, name)?
                {
                    return Ok(Some(unwind));
                }
            }
            Ok(None)
        }

        Native::MethodDefined => {
            let Some(id) = class_id_of(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "Module#method_defined?",
                    operands: "a receiver that is not a Module",
                });
            };
            let Some(name) = call.args.first().and_then(|&v| method_name_of(scope, v)) else {
                return Err(Error::raise("TypeError", "is not a symbol nor a string"));
            };
            // Public or protected, never private — measured on ruby 4.0.6.
            // The second argument is `inherit`, not mspec's `include_all`.
            let symbol = crate::shared::symbols::intern(&name);
            let found = lookup_inherited(scope, id, symbol, call.args.get(1));
            let matched = found.is_some_and(|method| method.visibility != Visibility::Private);
            stack.push(bool_value(matched));
            Ok(None)
        }

        Native::RespondTo => {
            let Some(name) = call.args.first().and_then(|&v| method_name_of(scope, v)) else {
                return Err(Error::raise("TypeError", "is not a symbol nor a string"));
            };
            // `class_of`, the same resolver `Target::Method` dispatch uses, so
            // a singleton class counts. `class_id_of` would answer "is the
            // receiver itself a module", which is a different question.
            let class = class_of(scope, call.receiver).ok_or_else(|| no_class(call.receiver))?;
            // Public only, unless `include_all` — the second argument, and here
            // it really is mspec's `include_all`, selecting on visibility,
            // rather than `method_defined?`'s `inherit`, which selects on the
            // ancestor chain. ruby/spec asks both with the same shape.
            let symbol = crate::shared::symbols::intern(&name);
            let include_all = call.args.get(1).is_some_and(|&v| v.is_truthy());
            let found = scope
                .classes_mut()
                .lookup(class, symbol)
                .is_some_and(|method| include_all || method.visibility == Visibility::Public);
            // Not in the table: a program's `respond_to_missing?` decides
            // (#28), as a frame whose answer comes back as a boolean.
            let asks = crate::shared::symbols::intern("respond_to_missing?");
            if !found
                && let Some(method) = scope.classes_mut().lookup(class, asks)
                && method.owner != Builtin::Kernel.id()
                && method.owner != Builtin::BasicObject.id()
            {
                let ask = Pending {
                    cache: None,
                    name: asks,
                    receiver: call.receiver,
                    args: vec![Value::symbol(symbol), bool_value(include_all)],
                    keywords: Vec::new(),
                    block: Value::NIL,
                    block_is_literal: false,
                    cref: call.cref,
                    implicit_self: true,
                    public_only: false,
                    target: Target::Method,
                    owner: None,
                    defined_as: None,
                };
                let depth = frames.len();
                if let Some(unwind) = dispatch(scope, stack, frames, ask, proc_class, ids)? {
                    return Ok(Some(unwind));
                }
                if frames.len() > depth {
                    let last = frames.len() - 1;
                    frames[last].booleanizes_value = true;
                } else if let Some(answer) = stack.pop() {
                    stack.push(bool_value(answer.is_truthy()));
                }
                return Ok(None);
            }
            stack.push(bool_value(found));
            Ok(None)
        }

        Native::PrivateConstant => {
            let Some(id) = class_id_of(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "Module#private_constant",
                    operands: "a receiver that is not a Module",
                });
            };
            // Every name is checked before any is marked, which is the order
            // Ruby uses: `private_constant :Known, :Missing` leaves `Known`
            // public. Measured.
            let mut names = Vec::with_capacity(call.args.len());
            for &argument in &call.args {
                let Some(text) = method_name_of(scope, argument) else {
                    return Err(Error::raise("TypeError", "is not a symbol nor a string"));
                };
                let symbol = crate::shared::symbols::intern(&text);
                if scope.classes().const_get_here(id, symbol).is_none() {
                    return Err(Error::raise(
                        "NameError",
                        format!(
                            "constant {}::{} not defined",
                            scope
                                .classes()
                                .name(id)
                                .unwrap_or("an anonymous module")
                                .to_owned(),
                            text
                        ),
                    ));
                }
                names.push(symbol);
            }
            for symbol in names {
                scope.classes_mut().mark_const_private(id, symbol);
            }
            // Ruby answers the module. Measured.
            stack.push(call.receiver);
            Ok(None)
        }

        Native::ModuleName => {
            let Some(id) = class_id_of(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "Module#name",
                    operands: "a receiver that is not a Module",
                });
            };
            let value = match scope.classes().name(id) {
                Some(name) => {
                    let name = name.to_owned();
                    string_new(scope, &name)
                }
                // An anonymous module's `name` is nil, and its `to_s` is not —
                // but `to_s` on one needs an object id in the text, which is
                // `#inspect`'s job and not this primitive's.
                None => Value::NIL,
            };
            stack.push(value);
            Ok(None)
        }

        Native::ArrayPlus => {
            let Some(mut left) = array_elements(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "+",
                    operands: "a receiver that is not an Array",
                });
            };
            let Some(right) = call.args.first().and_then(|&v| array_elements(scope, v)) else {
                return Err(Error::raise(
                    "TypeError",
                    "no implicit conversion into Array",
                ));
            };
            left.extend(right);
            let value = new_array(scope, &left);
            stack.push(value);
            Ok(None)
        }

        Native::Getter(slot) => {
            let handle = scope.root(call.receiver);
            let value = if (slot as u32) < scope.len(handle) {
                scope.slot(handle, slot as usize)
            } else {
                Value::NIL
            };
            stack.push(value);
            Ok(None)
        }

        Native::Setter(slot) => {
            let value = call.args.first().copied().unwrap_or(Value::NIL);
            let handle = scope.root(call.receiver);
            if (slot as u32) < scope.len(handle) {
                scope.set_slot(handle, slot as usize, value);
            }
            stack.push(value);
            Ok(None)
        }

        Native::IvarReader(name) => {
            let value = ivar_get(scope, call.receiver, name)?;
            stack.push(value);
            Ok(None)
        }

        Native::IvarWriter(name) => {
            let value = call.args.first().copied().unwrap_or(Value::NIL);
            ivar_set(scope, call.receiver, name, value)?;
            stack.push(value);
            Ok(None)
        }

        Native::AttrDefine { reader, writer } => {
            let Some(owner) = class_id_of(scope, call.receiver) else {
                return Err(Error::raise(
                    "NoMethodError",
                    format!(
                        "undefined method 'attr_accessor' for an instance of {}",
                        class_name(scope, call.receiver)
                    ),
                ));
            };
            // The methods these create take the body's current visibility, the
            // same as a `def` would: `private; attr_accessor :a` makes a
            // private reader (#161). The scope is the caller's, because this
            // native pushes no frame of its own.
            let cref = frames.last().map_or(CrefId::ROOT, |frame| frame.cref);
            let visibility = frames
                .last()
                .map_or(Visibility::Public, |frame| frame.scope_default.visibility());
            let mut defined: Vec<Value> = Vec::new();
            for &argument in &call.args {
                let Some(name) = attribute_name(scope, argument) else {
                    return Err(Error::raise(
                        "TypeError",
                        format!("{} is not a symbol nor a string", inspect(scope, argument)),
                    ));
                };
                let ivar = symbol(&format!("@{name}"));
                if reader {
                    let body = scope
                        .definitions_mut()
                        .add(Definition::Native(Native::IvarReader(ivar)));
                    let getter = symbol(&name);
                    scope
                        .classes_mut()
                        .define_method_visibly(owner, getter, body, cref, visibility);
                    defined.push(Value::symbol(getter));
                }
                if writer {
                    let body = scope
                        .definitions_mut()
                        .add(Definition::Native(Native::IvarWriter(ivar)));
                    let setter = symbol(&format!("{name}="));
                    scope
                        .classes_mut()
                        .define_method_visibly(owner, setter, body, cref, visibility);
                    defined.push(Value::symbol(setter));
                }
            }
            let value = new_array(scope, &defined);
            stack.push(value);
            // `method_added` for each, as a `def` fires it. Each hook is a
            // frame and the last pushed runs first, so they go on in reverse
            // to run in definition order. Measured.
            for name in defined.iter().rev().filter_map(|v| v.as_symbol()) {
                if let Some(unwind) =
                    fire_method_hook(scope, stack, frames, proc_class, ids, owner, "added", name)?
                {
                    return Ok(Some(unwind));
                }
            }
            Ok(None)
        }

        Native::InstanceVariable(op) => {
            let value = match op {
                IvarOp::Names => {
                    // `@__name__` is the core library's own state — a class's
                    // table id, a Hash's pairs — which CRuby keeps where Ruby
                    // cannot see it. `{}.instance_variables` is `[]` there, so
                    // it is here: the spelling is the marker.
                    let names: Vec<Value> = ivar_names(scope, call.receiver)
                        .into_iter()
                        .filter(|&name| !is_internal_ivar(name))
                        .map(Value::symbol)
                        .collect();
                    new_array(scope, &names)
                }
                _ => {
                    let argument = call.args.first().copied().unwrap_or(Value::NIL);
                    let Some(name) = attribute_name(scope, argument) else {
                        return Err(Error::raise(
                            "TypeError",
                            format!("{} is not a symbol nor a string", inspect(scope, argument)),
                        ));
                    };
                    // Ruby checks the spelling before the object: an argument
                    // that is not an ivar name is a `NameError` whether or not
                    // the receiver holds anything.
                    if !is_ivar_name(&name) {
                        return Err(Error::raise(
                            "NameError",
                            format!("'{name}' is not allowed as an instance variable name"),
                        ));
                    }
                    let ivar = symbol(&name);
                    match op {
                        IvarOp::Get => ivar_get(scope, call.receiver, ivar)?,
                        IvarOp::Defined => {
                            if ivar_defined(scope, call.receiver, ivar)? {
                                Value::TRUE
                            } else {
                                Value::FALSE
                            }
                        }
                        IvarOp::Set => {
                            let to = call.args.get(1).copied().unwrap_or(Value::NIL);
                            ivar_set(scope, call.receiver, ivar, to)?
                        }
                        IvarOp::Names => unreachable!("handled above"),
                    }
                }
            };
            stack.push(value);
            Ok(None)
        }

        Native::Raise => {
            // `raise Klass, msg` is `Klass.exception(msg)`, which is `new`. A
            // class whose `initialize` is Ruby — `Errno::ENOENT`, or any user
            // exception with its own — has to run it first, so this is `new`
            // with its frame told to raise the object when it leaves.
            if let Some(class) = ruby_initialized_exception(scope, &call.args) {
                let depth = frames.len();
                let new = Pending {
                    name: crate::shared::symbols::intern("new"),
                    receiver: class,
                    // The third argument is a backtrace, which is not the
                    // constructor's; this VM keeps none (PRD 0012).
                    args: call.args.get(1).copied().into_iter().collect(),
                    keywords: Vec::new(),
                    block: Value::NIL,
                    block_is_literal: false,
                    ..call
                };
                if let Some(unwind) =
                    native_call(scope, stack, frames, new, Native::New, proc_class, ids)?
                {
                    return Ok(Some(unwind));
                }
                if frames.len() > depth {
                    let last = frames.len() - 1;
                    frames[last].raises_receiver = true;
                    return Ok(None);
                }
                let exception = stack.pop().expect("`new` answered the exception");
                return Ok(Some(Unwind::Exception(exception)));
            }
            let exception = raise_argument(scope, frames, &call.args)?;
            let exception = scope.root(exception);
            // `raise Klass, msg, backtrace`: the third argument replaces the
            // backtrace this raise would otherwise take. A String is one line.
            if let Some(&given) = call.args.get(2) {
                let lines = if string_bytes(scope, given).is_some() {
                    new_array(scope, &[given])
                } else {
                    given
                };
                let object = scope.get(exception);
                ivar_set(scope, object, symbol(EXC_BACKTRACE), lines)?;
            }
            // `raise ..., cause: c` names the cause outright, nil included,
            // which is what keeps `attach_backtrace` from taking `$!`.
            let cause_key = crate::shared::symbols::intern("cause");
            if let Some(&(_, cause)) = call
                .keywords
                .iter()
                .find(|&&(key, _)| key.as_symbol() == Some(cause_key))
            {
                let object = scope.get(exception);
                ivar_set(scope, object, symbol(EXC_CAUSE), cause)?;
            }
            Ok(Some(Unwind::Exception(scope.get(exception))))
        }

        Native::Throw => {
            let Some(&tag) = call.args.first() else {
                return Err(Error::raise(
                    "ArgumentError",
                    "wrong number of arguments (given 0, expected 1..2)",
                ));
            };
            if call.args.len() > 2 {
                return Err(Error::raise(
                    "ArgumentError",
                    format!(
                        "wrong number of arguments (given {}, expected 1..2)",
                        call.args.len()
                    ),
                ));
            }
            let value = call.args.get(1).copied().unwrap_or(Value::NIL);
            // Ruby raises where the `throw` is, not where the search gives up,
            // and `UncaughtThrowError` is an ordinary rescuable exception — so
            // it is decided here rather than at the top of the unwind.
            if !frames.iter().any(|frame| frame.tag == Some(tag)) {
                let message = format!("uncaught throw {}", inspect(scope, tag));
                return Err(Error::raise("UncaughtThrowError", message));
            }
            Ok(Some(Unwind::Throw { tag, value }))
        }

        Native::Catch => {
            if call.block == Value::NIL {
                return Err(Error::raise("LocalJumpError", "no block given (yield)"));
            }
            // `catch` with no tag invents one. Ruby uses a fresh object, and
            // identity is the whole comparison, so a bare `Object` is exactly
            // enough.
            let tag = match call.args.first() {
                Some(&tag) => tag,
                None => {
                    let class = class_handle(scope, Builtin::Object);
                    let handle = alloc_ivar_object(scope, Some(class));
                    scope.get(handle)
                }
            };
            let block = call.block;
            let inner = Pending {
                cache: None,
                name: call.name,
                receiver: block,
                args: vec![tag],
                keywords: Vec::new(),
                block: Value::NIL,
                block_is_literal: false,
                cref: call.cref,
                implicit_self: false,
                public_only: false,
                target: Target::Block(block),
                owner: None,
                defined_as: None,
            };
            push_proc_frame(scope, stack, frames, &inner, block, ids)?;
            let last = frames.len() - 1;
            frames[last].tag = Some(tag);
            Ok(None)
        }

        // -- regexps ----------------------------------------------------
        //
        // Every matcher but `match?` sets `$~`, which is why they go through
        // one helper rather than each doing its own thing.
        Native::RegexpMatchOp | Native::StringMatchOp => {
            let (regexp, subject) = match native {
                Native::RegexpMatchOp => (call.receiver, first_arg(&call)),
                _ => (first_arg(&call), call.receiver),
            };
            // `"str" =~ "str"` is a TypeError in Ruby, not a comparison.
            if !is_regexp(scope, regexp) {
                return Err(Error::Raise {
                    class: "TypeError",
                    message: format!("type mismatch: {} given", class_name_of(scope, regexp)),
                });
            }
            let data = regexp_match_value(scope, regexp, subject)?;
            let answer = match match_parts(scope, data) {
                Some((_, text, groups)) => match groups.first().copied().flatten() {
                    Some((start, _)) => Value::fixnum(char_offset(&text, start))
                        .expect("a character offset fits a fixnum"),
                    None => Value::NIL,
                },
                None => Value::NIL,
            };
            stack.push(answer);
            Ok(None)
        }
        Native::RegexpMatch | Native::StringMatch => {
            let (regexp, subject) = match native {
                Native::RegexpMatch => (call.receiver, first_arg(&call)),
                _ => (first_arg(&call), call.receiver),
            };
            let data = regexp_match_from(scope, regexp, subject, nth_arg(&call, 1))?;
            // With a block, a match is yielded and the block's value is the
            // answer; no match yields nothing and answers nil. Measured. The
            // block's frame is pushed here rather than from a Ruby wrapper so
            // `$~` stays set in the caller's frame, where Ruby puts it.
            if call.block != Value::NIL && data != Value::NIL {
                let block = call.block;
                let inner = Pending {
                    cache: None,
                    name: call.name,
                    receiver: block,
                    args: vec![data],
                    keywords: Vec::new(),
                    block: Value::NIL,
                    block_is_literal: false,
                    cref: call.cref,
                    implicit_self: false,
                    public_only: false,
                    target: Target::Block(block),
                    owner: None,
                    defined_as: None,
                };
                push_proc_frame(scope, stack, frames, &inner, block, ids)?;
                return Ok(None);
            }
            stack.push(data);
            Ok(None)
        }
        Native::RegexpCaseEq => {
            // A Symbol is matched as its name; anything else that is not a
            // String is simply not a match — `===` never raises. Measured.
            let mut subject = first_arg(&call);
            if let Some(name) = subject.as_symbol() {
                let text = symbol_name(name);
                subject = string_new(scope, &text);
            } else if string_bytes(scope, subject).is_none() {
                scope.set_last_match(Value::NIL);
                stack.push(Value::FALSE);
                return Ok(None);
            }
            let data = regexp_match_value(scope, call.receiver, subject)?;
            stack.push(bool_value(data != Value::NIL));
            Ok(None)
        }
        Native::RegexpMatchP | Native::StringMatchP => {
            let (regexp, subject) = match native {
                Native::RegexpMatchP => (call.receiver, first_arg(&call)),
                _ => (first_arg(&call), call.receiver),
            };
            // `match?` is the one that does not touch `$~`, so the previous
            // match has to survive it.
            let saved = scope.last_match();
            let data = regexp_match_from(scope, regexp, subject, nth_arg(&call, 1))?;
            scope.set_last_match(saved);
            stack.push(bool_value(data != Value::NIL));
            Ok(None)
        }
        Native::RegexpSource | Native::RegexpOptions => {
            let slot = if matches!(native, Native::RegexpSource) {
                crate::regexp::REGEXP_SOURCE
            } else {
                crate::regexp::REGEXP_OPTIONS
            };
            if !is_regexp(scope, call.receiver) {
                return Err(Error::NoDispatch {
                    op: "source",
                    operands: "a receiver that is not a Regexp",
                });
            }
            let mut nested = scope.nested();
            let handle = nested.root(call.receiver);
            let value = nested.slot(handle, slot);
            stack.push(value);
            Ok(None)
        }
        Native::RegexpToS { inspect } => {
            let Some(program) = regexp_program(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "to_s",
                    operands: "a receiver that is not a Regexp",
                });
            };
            let text = if inspect {
                // `%r{/foo/bar}.inspect` is `/\/foo\/bar/`: a slash that the
                // source did not escape has to be escaped now, or the result
                // does not read back as the same literal.
                format!(
                    "/{}/{}",
                    escape_slashes(program.source()),
                    program.flags().to_letters()
                )
            } else {
                // `(?mix-mix:source)`: every flag named, on one side or the
                // other, which is what makes two `to_s` strings comparable.
                let on = program.flags().to_letters();
                let off: String = "mix".chars().filter(|c| !on.contains(*c)).collect();
                let dash = if off.is_empty() { "" } else { "-" };
                format!("(?{on}{dash}{off}:{})", program.source())
            };
            let value = string_new(scope, &text);
            stack.push(value);
            Ok(None)
        }

        // -- match data -------------------------------------------------
        Native::MatchIndex
        | Native::MatchToA { .. }
        | Native::MatchAround { .. }
        | Native::MatchEdge { .. }
        | Native::MatchNames
        | Native::MatchSize => {
            let Some((regexp, text, groups)) = match_parts(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "[]",
                    operands: "a receiver that is not a MatchData",
                });
            };
            let value = match_answer(scope, native, &call, regexp, &text, &groups)?;
            stack.push(value);
            Ok(None)
        }
    }
}

/// What `raise` was handed, as an exception object.
///
/// Ruby's five shapes, and each one is in the corpus:
///
/// ```ruby
/// raise                        # re-raise what this rescue caught
/// raise "boom"                 # RuntimeError with that message
/// raise TypeError              # TypeError, message is the class name
/// raise TypeError, "boom"      # both
/// raise TypeError.new("boom")  # an instance, passed through
/// ```
/// The exception class `raise`'s first argument names, when that class's
/// `initialize` is written in Ruby below `Exception` — the same test
/// `Native::New` uses to decide it cannot build the object itself.
fn ruby_initialized_exception(scope: &mut HandleScope<'_>, args: &[Value]) -> Option<Value> {
    let &first = args.first()?;
    let id = class_id_of(scope, first)?;
    if !is_exception_class(scope, id) {
        return None;
    }
    let initialize = crate::shared::symbols::intern("initialize");
    scope
        .classes()
        .lookup_uncached(id, initialize)
        .is_some_and(|method| method.owner != Builtin::Exception.id())
        .then_some(first)
}

fn raise_argument(
    scope: &mut HandleScope<'_>,
    frames: &[Call],
    args: &[Value],
) -> Result<Value, Error> {
    let Some(&first) = args.first() else {
        // A bare `raise` inside a `rescue` re-raises. Outside one, Ruby raises
        // a `RuntimeError` whose message is empty — measured, not guessed.
        return Ok(match frames.last().and_then(|frame| frame.rescued) {
            Some(exception) => exception,
            None => exception_new(scope, "RuntimeError", ""),
        });
    };

    // An instance is passed straight through, which is what makes
    // `raise e` inside a `rescue` keep the original object.
    if is_exception(scope, first) {
        return Ok(first);
    }

    // A String is a RuntimeError with that message — alone. A String with a
    // second argument is a TypeError, measured: `raise "m", ["bt"]` is not a
    // message and a backtrace, and is not taken as one.
    if let Some(text) = string_bytes(scope, first) {
        if args.len() > 1 {
            return Err(Error::raise("TypeError", "exception class/object expected"));
        }
        let message = String::from_utf8_lossy(&text).into_owned();
        return Ok(exception_new(scope, "RuntimeError", &message));
    }

    // Anything else has to be an exception class.
    let Some(id) = class_id_of(scope, first) else {
        return Err(Error::raise("TypeError", "exception class/object expected"));
    };
    let message = match args.get(1) {
        Some(&second) => match string_bytes(scope, second) {
            Some(text) => String::from_utf8_lossy(&text).into_owned(),
            None => inspect(scope, second),
        },
        // `raise ArgumentError` reads "ArgumentError" back out of `message`.
        None => scope
            .classes()
            .name(id)
            .map_or_else(String::new, str::to_owned),
    };
    let class = scope.classes().object(id);
    Ok(exception_of(scope, class, &message))
}

/// Whether `value`'s class defines `#to_ary`, which is what decides whether
/// Ruby would spread it across a block's parameters — or might answer it
/// through its own `respond_to_missing?`, which only running it can tell. The
/// callers refuse either way rather than bind the object whole.
fn defines_to_ary(scope: &mut HandleScope<'_>, value: Value) -> bool {
    let Some(class) = class_of(scope, value) else {
        return false;
    };
    let name = crate::shared::symbols::intern("to_ary");
    if scope.classes_mut().lookup(class, name).is_some() {
        return true;
    }
    let missing = crate::shared::symbols::intern("respond_to_missing?");
    scope
        .classes_mut()
        .lookup(class, missing)
        .is_some_and(|method| method.owner != Builtin::Kernel.id())
}

/// Whether the class `id` names is `Exception` or below it.
fn is_exception_class(scope: &mut HandleScope<'_>, id: ClassId) -> bool {
    scope
        .classes()
        .ancestors(id)
        .contains(&Builtin::Exception.id())
}

/// Whether `value` is an instance of `Exception` or one of its descendants.
fn is_exception(scope: &mut HandleScope<'_>, value: Value) -> bool {
    class_of(scope, value).is_some_and(|id| {
        scope
            .classes()
            .ancestors(id)
            .contains(&Builtin::Exception.id())
    })
}

/// A `String`'s bytes, or `None` when the value is not one.
fn string_bytes(scope: &mut HandleScope<'_>, value: Value) -> Option<Vec<u8>> {
    if value.is_immediate() {
        return None;
    }
    // The class's representation, so a subclass of `String` is a string here.
    if !is_string(scope, value) {
        return None;
    }
    let handle = scope.root(value);
    Some(crate::strings::bytes(scope, handle))
}

/// `lambda { }` given a block: the same body, marked as a lambda.
fn relambda<'h>(
    scope: &mut HandleScope<'h>,
    proc_class: Handle<'h>,
    block: Value,
) -> Result<Value, Error> {
    let Some((iseq, env, receiver, captured, _, cref)) = proc_parts(scope, block) else {
        return Err(Error::NoDispatch {
            op: "lambda",
            operands: "a block that is not a Proc",
        });
    };
    // The new `Proc` is a lambda, so its `return` becomes local — the frame
    // homes to itself when it is pushed. The old home is carried anyway so a
    // `Proc` that is re-lambda'd twice does not lose where it came from.
    let (home, _) = proc_links(scope, block);
    Ok(make_proc(
        scope, proc_class, &iseq, env, receiver, captured, true, cref, home,
    ))
}

/// Register the primitives on the bootstrap classes.
///
/// Called from `bootstrap`, so a heap that has classes also has the handful of
/// methods that make a `Proc` callable. Everything else is `core/*.rb` and
/// [#15](https://github.com/ar4mirez/spinel/issues/15).
pub fn install_primitives(scope: &mut HandleScope<'_>) {
    let table: &[(Builtin, &[&str], Native)] = &[
        (
            Builtin::Proc,
            &["call", "()", "[]", "yield", "==="],
            Native::Call,
        ),
        (Builtin::Proc, &["lambda?"], Native::IsLambda),
        (Builtin::Proc, &["arity"], Native::Arity),
        (
            Builtin::Kernel,
            &["send", "__send__"],
            Native::Send { public_only: false },
        ),
        (
            Builtin::Kernel,
            &["public_send"],
            Native::Send { public_only: true },
        ),
        (
            Builtin::Kernel,
            &["proc"],
            Native::MakeProc { lambda: false },
        ),
        (
            Builtin::Kernel,
            &["lambda"],
            Native::MakeProc { lambda: true },
        ),
        (Builtin::Kernel, &["block_given?"], Native::BlockGiven),
        (Builtin::Kernel, &["class"], Native::ClassOf),
        (Builtin::Class, &["new"], Native::New),
        (Builtin::Array, &["+"], Native::ArrayPlus),
        (Builtin::Kernel, &["raise", "fail"], Native::Raise),
        (Builtin::Kernel, &["throw"], Native::Throw),
        (Builtin::Kernel, &["catch"], Native::Catch),
        (Builtin::Regexp, &["=~"], Native::RegexpMatchOp),
        (Builtin::Regexp, &["match"], Native::RegexpMatch),
        (Builtin::Regexp, &["match?"], Native::RegexpMatchP),
        (Builtin::Regexp, &["__case_eq__"], Native::RegexpCaseEq),
        (Builtin::Regexp, &["source"], Native::RegexpSource),
        (Builtin::Regexp, &["options"], Native::RegexpOptions),
        (
            Builtin::Regexp,
            &["to_s"],
            Native::RegexpToS { inspect: false },
        ),
        (
            Builtin::Regexp,
            &["inspect"],
            Native::RegexpToS { inspect: true },
        ),
        (Builtin::String, &["=~"], Native::StringMatchOp),
        (Builtin::String, &["match"], Native::StringMatch),
        (Builtin::String, &["match?"], Native::StringMatchP),
        (Builtin::MatchData, &["[]"], Native::MatchIndex),
        (
            Builtin::MatchData,
            &["to_a"],
            Native::MatchToA { captures: false },
        ),
        (
            Builtin::MatchData,
            &["captures"],
            Native::MatchToA { captures: true },
        ),
        (
            Builtin::MatchData,
            &["pre_match"],
            Native::MatchAround { post: false },
        ),
        (
            Builtin::MatchData,
            &["post_match"],
            Native::MatchAround { post: true },
        ),
        (
            Builtin::MatchData,
            &["begin"],
            Native::MatchEdge { end: false },
        ),
        (
            Builtin::MatchData,
            &["end"],
            Native::MatchEdge { end: true },
        ),
        (Builtin::MatchData, &["size", "length"], Native::MatchSize),
        (Builtin::MatchData, &["names"], Native::MatchNames),
        (
            Builtin::Module,
            &["attr_reader"],
            Native::AttrDefine {
                reader: true,
                writer: false,
            },
        ),
        (
            Builtin::Module,
            &["attr_writer"],
            Native::AttrDefine {
                reader: false,
                writer: true,
            },
        ),
        (
            Builtin::Module,
            &["attr_accessor"],
            Native::AttrDefine {
                reader: true,
                writer: true,
            },
        ),
        (
            Builtin::Kernel,
            &["instance_variable_get"],
            Native::InstanceVariable(IvarOp::Get),
        ),
        (
            Builtin::Kernel,
            &["instance_variable_set"],
            Native::InstanceVariable(IvarOp::Set),
        ),
        (
            Builtin::Kernel,
            &["instance_variable_defined?"],
            Native::InstanceVariable(IvarOp::Defined),
        ),
        (
            Builtin::Kernel,
            &["instance_variables"],
            Native::InstanceVariable(IvarOp::Names),
        ),
        // `regexp` and `string` are the pattern and the subject the match was
        // made against, in the slots `match_data_new` wrote them to. Reading a
        // slot is what `Getter` is; `core/match_data.rb` builds `==` on them.
        (
            Builtin::MatchData,
            &["regexp"],
            Native::Getter(crate::regexp::MATCH_REGEXP as u16),
        ),
        (
            Builtin::MatchData,
            &["string"],
            Native::Getter(crate::regexp::MATCH_SUBJECT as u16),
        ),
        (Builtin::Kernel, &["equal?"], Native::Equal),
        // CRuby defines `equal?` and `__id__` on BasicObject, and `==` and `!`
        // there are identity in C, untouched by an `equal?` override. The
        // `__identical__` spelling is that C identity, for `core/basic_object.rb`.
        (
            Builtin::BasicObject,
            &["equal?", "__identical__"],
            Native::Equal,
        ),
        (Builtin::BasicObject, &["__id__"], Native::ObjectId),
        (Builtin::Kernel, &["nil?"], Native::NilP),
        // #15. Raw storage and allocation; the rest of these classes is Ruby.
        (Builtin::Array, &["[]", "slice"], Native::ArrayIndex),
        (Builtin::Array, &["at"], Native::ArrayIndexSingle),
        (Builtin::Array, &["[]="], Native::ArrayStore),
        (Builtin::Array, &["size", "length"], Native::ArraySize),
        (
            Builtin::Array,
            &["push", "append", "__append__"],
            Native::ArrayPush,
        ),
        (Builtin::Array, &["pop"], Native::ArrayPop),
        (
            Builtin::String,
            &["length", "size"],
            Native::StringSize { bytes: false },
        ),
        (
            Builtin::String,
            &["bytesize"],
            Native::StringSize { bytes: true },
        ),
        (Builtin::String, &["+"], Native::StringConcat),
        (Builtin::String, &["*"], Native::StringRepeat),
        (Builtin::Class, &["allocate"], Native::Allocate),
        (Builtin::Kernel, &["__dup__"], Native::Dup),
        (Builtin::Kernel, &["freeze"], Native::Freeze),
        (Builtin::Kernel, &["frozen?"], Native::FrozenP),
        (Builtin::Kernel, &["object_id", "__id__"], Native::ObjectId),
        (Builtin::Integer, &["&"], Native::IntBits(BitOp::And)),
        (Builtin::Integer, &["|"], Native::IntBits(BitOp::Or)),
        (Builtin::Integer, &["^"], Native::IntBits(BitOp::Xor)),
        (Builtin::Integer, &["<<"], Native::IntBits(BitOp::Shl)),
        (Builtin::Integer, &[">>"], Native::IntBits(BitOp::Shr)),
        (Builtin::Integer, &["~"], Native::IntBits(BitOp::Not)),
        (Builtin::Integer, &["**"], Native::IntPow),
        (Builtin::Integer, &["__to_s_radix__"], Native::IntToSRadix),
        (
            Builtin::Symbol,
            &["to_s", "id2name"],
            Native::SymbolName { length: false },
        ),
        (
            Builtin::Symbol,
            &["length", "size"],
            Native::SymbolName { length: true },
        ),
        (Builtin::Module, &["__name__"], Native::ModuleName),
        (
            Builtin::Module,
            &["private_constant"],
            Native::PrivateConstant,
        ),
        (Builtin::Kernel, &["hash"], Native::HashValue),
        (
            Builtin::Module,
            &["__include__"],
            Native::Mixin { prepend: false },
        ),
        (
            Builtin::Module,
            &["__prepend__"],
            Native::Mixin { prepend: true },
        ),
        // The splices under `include`, `prepend` and `extend`, which are Ruby
        // in `core/module.rb` and `core/kernel.rb` so their hooks run.
        (Builtin::Kernel, &["__extend__"], Native::Extend),
        (Builtin::Kernel, &["respond_to?"], Native::RespondTo),
        (
            Builtin::Module,
            &["private"],
            Native::SetVisibility(Visibility::Private),
        ),
        (
            Builtin::Module,
            &["public"],
            Native::SetVisibility(Visibility::Public),
        ),
        (
            Builtin::Module,
            &["protected"],
            Native::SetVisibility(Visibility::Protected),
        ),
        (
            Builtin::Module,
            &["private_method_defined?"],
            Native::VisibilityDefined(Visibility::Private),
        ),
        (
            Builtin::Module,
            &["public_method_defined?"],
            Native::VisibilityDefined(Visibility::Public),
        ),
        (
            Builtin::Module,
            &["protected_method_defined?"],
            Native::VisibilityDefined(Visibility::Protected),
        ),
        (
            Builtin::Module,
            &["module_function"],
            Native::ModuleFunction,
        ),
        (Builtin::Module, &["ancestors"], Native::Ancestors),
        (Builtin::Module, &["method_defined?"], Native::MethodDefined),
        (Builtin::Module, &["alias_method"], Native::AliasMethod),
        (
            Builtin::Module,
            &["class_variables"],
            Native::ClassVariable(CvarOp::Names),
        ),
        (
            Builtin::Module,
            &["class_variable_get"],
            Native::ClassVariable(CvarOp::Get),
        ),
        (
            Builtin::Module,
            &["class_variable_set"],
            Native::ClassVariable(CvarOp::Set),
        ),
        (
            Builtin::Module,
            &["class_variable_defined?"],
            Native::ClassVariable(CvarOp::Defined),
        ),
        (Builtin::Module, &["undef_method"], Native::UndefMethod),
        (Builtin::Class, &["superclass"], Native::Superclass),
        (Builtin::Kernel, &["__write__"], Native::WriteString),
        (Builtin::Kernel, &["__strerror__"], Native::Strerror),
        (Builtin::Kernel, &["__errno_class__"], Native::ErrnoClass),
        (Builtin::Kernel, &["__signal_list__"], Native::SignalList),
        (
            Builtin::Kernel,
            &["__backtrace_here__"],
            Native::BacktraceHere,
        ),
        (Builtin::Kernel, &["__stderr_tty__"], Native::StderrTty),
        (
            Builtin::Kernel,
            &["__needs_threads__"],
            Native::NeedsThreads,
        ),
        (
            Builtin::Kernel,
            &["__needs_process__"],
            Native::Refuse {
                what: "starting or ending a process",
                needs: "`Process` (#43)",
            },
        ),
        (
            Builtin::Kernel,
            &["__needs_time__"],
            Native::Refuse {
                what: "`Time` arithmetic and calendars",
                needs: "`Time` (#32)",
            },
        ),
        (
            Builtin::Kernel,
            &["__needs_kernel_clone__"],
            Native::Refuse {
                what: "`clone` of an object with singleton methods",
                needs: "`Kernel#clone` (#201)",
            },
        ),
        (
            Builtin::Kernel,
            &["__needs_io__"],
            Native::Refuse {
                what: "a `File` opened for writing",
                needs: "`IO` and `File` (#41)",
            },
        ),
        (Builtin::Kernel, &["__hash_combine__"], Native::HashCombine),
        (
            Builtin::Kernel,
            &["__fiber_new__"],
            Native::Fiber(FiberOp::New),
        ),
        (
            Builtin::Kernel,
            &["__fiber_resume__"],
            Native::Fiber(FiberOp::Resume),
        ),
        (
            Builtin::Kernel,
            &["__fiber_yield__"],
            Native::Fiber(FiberOp::Yield),
        ),
        (
            Builtin::Kernel,
            &["__fiber_raise__"],
            Native::Fiber(FiberOp::Raise),
        ),
        (
            Builtin::Kernel,
            &["__fiber_kill__"],
            Native::Fiber(FiberOp::Kill),
        ),
        (
            Builtin::Kernel,
            &["__fiber_transfer__"],
            Native::Fiber(FiberOp::Transfer),
        ),
        (
            Builtin::Kernel,
            &["__fiber_current__"],
            Native::Fiber(FiberOp::Current),
        ),
        (
            Builtin::Kernel,
            &["__fiber_alive__"],
            Native::Fiber(FiberOp::Alive),
        ),
        (
            Builtin::Kernel,
            &["__fiber_status__"],
            Native::Fiber(FiberOp::Status),
        ),
        (
            Builtin::Kernel,
            &["__proc_location__"],
            Native::ProcLocation,
        ),
        (
            Builtin::Module,
            &["define_method"],
            Native::DefineMethod { singleton: false },
        ),
        (
            Builtin::Kernel,
            &["define_singleton_method"],
            Native::DefineMethod { singleton: true },
        ),
        (
            Builtin::BasicObject,
            &["instance_eval"],
            Native::EvalBlock {
                module: false,
                exec: false,
            },
        ),
        (
            Builtin::BasicObject,
            &["instance_exec"],
            Native::EvalBlock {
                module: false,
                exec: true,
            },
        ),
        (
            Builtin::Module,
            &["class_eval", "module_eval"],
            Native::EvalBlock {
                module: true,
                exec: false,
            },
        ),
        (
            Builtin::Module,
            &["class_exec", "module_exec"],
            Native::EvalBlock {
                module: true,
                exec: true,
            },
        ),
        (
            Builtin::Kernel,
            &["__reflect_const_lookup__"],
            Native::Reflect(ReflectOp::ConstLookup),
        ),
        (
            Builtin::Kernel,
            &["__reflect_const_set__"],
            Native::Reflect(ReflectOp::ConstSet),
        ),
        (
            Builtin::Kernel,
            &["__reflect_const_names__"],
            Native::Reflect(ReflectOp::ConstNames),
        ),
        (
            Builtin::Kernel,
            &["__reflect_const_remove__"],
            Native::Reflect(ReflectOp::ConstRemove),
        ),
        (
            Builtin::Kernel,
            &["__reflect_const_public__"],
            Native::Reflect(ReflectOp::ConstPublic),
        ),
        (
            Builtin::Kernel,
            &["__reflect_method_names__"],
            Native::Reflect(ReflectOp::MethodNames),
        ),
        (
            Builtin::Kernel,
            &["__reflect_class_of__"],
            Native::Reflect(ReflectOp::ClassOf),
        ),
        (
            Builtin::Kernel,
            &["__reflect_singleton_class__"],
            Native::Reflect(ReflectOp::SingletonClass),
        ),
        (
            Builtin::Kernel,
            &["__reflect_remove_method__"],
            Native::Reflect(ReflectOp::RemoveMethod),
        ),
        (
            Builtin::Kernel,
            &["__reflect_is_singleton__"],
            Native::Reflect(ReflectOp::IsSingleton),
        ),
        (
            Builtin::Kernel,
            &["__reflect_module_kind__"],
            Native::Reflect(ReflectOp::ModuleKind),
        ),
        (
            Builtin::Kernel,
            &["__reflect_attached__"],
            Native::Reflect(ReflectOp::Attached),
        ),
        (
            Builtin::Kernel,
            &["__method__"],
            Native::FrameMethod { callee: false },
        ),
        (
            Builtin::Kernel,
            &["__callee__"],
            Native::FrameMethod { callee: true },
        ),
        (Builtin::Kernel, &["__dir__"], Native::FrameDir),
        (
            Builtin::Kernel,
            &["binding"],
            Native::Binding(BindingOp::Capture { caller: false }),
        ),
        (
            Builtin::Kernel,
            &["__caller_binding__"],
            Native::Binding(BindingOp::Capture { caller: true }),
        ),
        // A `Binding`'s own primitives, on Kernel because `Binding` is a class
        // `core/binding.rb` defines; each checks its receiver is one.
        (
            Builtin::Kernel,
            &["__binding_eval__"],
            Native::Binding(BindingOp::Eval),
        ),
        (
            Builtin::Kernel,
            &["__binding_get__"],
            Native::Binding(BindingOp::Get),
        ),
        (
            Builtin::Kernel,
            &["__binding_set__"],
            Native::Binding(BindingOp::Set),
        ),
        (
            Builtin::Kernel,
            &["__binding_names__"],
            Native::Binding(BindingOp::Names),
        ),
        (
            Builtin::Kernel,
            &["__binding_receiver__"],
            Native::Binding(BindingOp::Receiver),
        ),
        (
            Builtin::Kernel,
            &["__binding_location__"],
            Native::Binding(BindingOp::Location),
        ),
        (
            Builtin::Kernel,
            &["__binding_receiver_set__"],
            Native::Binding(BindingOp::SetReceiver),
        ),
        (Builtin::Kernel, &["__load_file__"], Native::LoadFile),
        (
            Builtin::Kernel,
            &["__refusal_boundary__"],
            Native::RefusalBoundary,
        ),
        (
            Builtin::Kernel,
            &["__freeze_global__"],
            Native::FreezeGlobal,
        ),
        (Builtin::Kernel, &["__argv__"], Native::Argv),
        (Builtin::Kernel, &["__mark_partial__"], Native::MarkPartial),
        (
            Builtin::Kernel,
            &["__ruby_constants__"],
            Native::RubyConstants,
        ),
        (Builtin::Kernel, &["__environ__"], Native::Environ),
        (Builtin::Kernel, &["__getenv__"], Native::Getenv),
        (Builtin::Kernel, &["__fs_kind__"], Native::Fs(FsOp::Kind)),
        (
            Builtin::Kernel,
            &["__fs_realpath__"],
            Native::Fs(FsOp::Realpath),
        ),
        (
            Builtin::Kernel,
            &["__fs_getcwd__"],
            Native::Fs(FsOp::Getcwd),
        ),
        (Builtin::Kernel, &["__fs_chdir__"], Native::Fs(FsOp::Chdir)),
        (Builtin::Kernel, &["__fs_read__"], Native::Fs(FsOp::Read)),
        (
            Builtin::Kernel,
            &["__fs_children__"],
            Native::Fs(FsOp::Children),
        ),
        (
            Builtin::Kernel,
            &["__fs_isatty__"],
            Native::Fs(FsOp::Isatty),
        ),
        (
            Builtin::Kernel,
            &["__fs_access__"],
            Native::Fs(FsOp::Access),
        ),
        (Builtin::Kernel, &["__sys_ids__"], Native::Sys(SysOp::Ids)),
        (
            Builtin::Kernel,
            &["__sys_clock__"],
            Native::Sys(SysOp::Clock),
        ),
        (
            Builtin::Kernel,
            &["__sys_clock_ids__"],
            Native::Sys(SysOp::ClockIds),
        ),
        (
            Builtin::Kernel,
            &["__sys_process_constants__"],
            Native::Sys(SysOp::ProcessConstants),
        ),
        (
            Builtin::Kernel,
            &["__fs_constants__"],
            Native::Fs(FsOp::Constants),
        ),
        (
            Builtin::Kernel,
            &["__module_nesting__"],
            Native::FrameNesting,
        ),
        // `main`'s `public` and `private`, aliased onto its singleton by
        // `core/object.rb` so a bare one reaches the top-level frame.
        (
            Builtin::Kernel,
            &["__main_public__"],
            Native::SetVisibility(Visibility::Public),
        ),
        (
            Builtin::Kernel,
            &["__main_private__"],
            Native::SetVisibility(Visibility::Private),
        ),
        (Builtin::Kernel, &["__sleep__"], Native::Sleep),
        (Builtin::String, &["to_sym", "intern"], Native::StringIntern),
        // #19's byte and encoding primitives; `core/string.rb` and
        // `core/encoding.rb` are the methods.
        (
            Builtin::String,
            &["__encoding_index__"],
            Native::Str(StrOp::EncodingIndex),
        ),
        (
            Builtin::String,
            &["__force_encoding__"],
            Native::Str(StrOp::ForceEncoding),
        ),
        (Builtin::String, &["__splice__"], Native::Str(StrOp::Splice)),
        (
            Builtin::String,
            &["__byte_index__"],
            Native::Str(StrOp::ByteIndex),
        ),
        (
            Builtin::String,
            &["__byte_rindex__"],
            Native::Str(StrOp::ByteRindex),
        ),
        (
            Builtin::String,
            &["__case_map__"],
            Native::Str(StrOp::CaseMap),
        ),
        (Builtin::String, &["succ", "next"], Native::Str(StrOp::Succ)),
        (
            Builtin::String,
            &["__transcode__"],
            Native::Str(StrOp::Transcode),
        ),
        (
            Builtin::Float,
            &["__format__"],
            Native::Str(StrOp::FloatFormat),
        ),
        (Builtin::Float, &["__bits__"], Native::Str(StrOp::FloatBits)),
        (
            Builtin::Integer,
            &["__float_from_bits__"],
            Native::Str(StrOp::FloatFromBits),
        ),
        (
            Builtin::Kernel,
            &["__needs_pointers__"],
            Native::Str(StrOp::NeedsPointers),
        ),
        (
            Builtin::Kernel,
            &["__needs_char_table__"],
            Native::Str(StrOp::NeedsCharTable),
        ),
        (
            Builtin::String,
            &["__getbyte__"],
            Native::Str(StrOp::GetByte),
        ),
        (
            Builtin::String,
            &["__setbyte__"],
            Native::Str(StrOp::SetByte),
        ),
        (
            Builtin::String,
            &["__byteslice__"],
            Native::Str(StrOp::ByteSlice),
        ),
        (Builtin::String, &["__bytes__"], Native::Str(StrOp::Bytes)),
        (
            Builtin::String,
            &["valid_encoding?"],
            Native::Str(StrOp::ValidEncoding),
        ),
        (
            Builtin::String,
            &["ascii_only?"],
            Native::Str(StrOp::AsciiOnly),
        ),
        (
            Builtin::String,
            &["__char_offsets__"],
            Native::Str(StrOp::CharOffsets),
        ),
        (
            Builtin::String,
            &["__compatible__"],
            Native::Str(StrOp::Compatible),
        ),
        (
            Builtin::Kernel,
            &["__encoding_install__"],
            Native::Str(StrOp::EncodingInstall),
        ),
        (
            Builtin::BasicObject,
            &["__raise_no_method__"],
            Native::RaiseNoMethod,
        ),
        // `__send__` is BasicObject's, where a blank slate still has it.
        (
            Builtin::BasicObject,
            &["__send__"],
            Native::Send { public_only: false },
        ),
        (
            Builtin::Kernel,
            &["__absolute_path__"],
            Native::AbsolutePath,
        ),
        (Builtin::Float, &["to_s"], Native::FloatToS),
        (Builtin::String, &["__index__"], Native::StringIndex),
        (Builtin::String, &["<=>"], Native::StringCompare),
    ];
    for (builtin, names, native) in table {
        let body = scope.definitions_mut().add(Definition::Native(*native));
        for name in *names {
            let symbol = crate::shared::symbols::intern(name);
            scope
                .classes_mut()
                .define_method(builtin.id(), symbol, body);
        }
    }

    // The operators as methods (#239). `Insn::BinOp` and `Insn::Neg` answer a
    // pair of numbers without dispatching; these are the same functions under
    // the names a `send`, a `respond_to?` or an `inject(:+)` looks up. Sealed,
    // so the instructions can tell when a program has put something else there.
    for (which, class) in [Builtin::Integer.id(), Builtin::Float.id()]
        .into_iter()
        .enumerate()
    {
        for op in [
            BinOp::Add,
            BinOp::Sub,
            BinOp::Mul,
            BinOp::Div,
            BinOp::Mod,
            BinOp::Lt,
            BinOp::Le,
            BinOp::Gt,
            BinOp::Ge,
        ] {
            let body = scope
                .definitions_mut()
                .add(Definition::Native(Native::NumOp(op)));
            let symbol = crate::shared::symbols::intern(op.name());
            scope.classes_mut().define_method(class, symbol, body);
            scope
                .classes_mut()
                .seal_operator(which, class, op as usize, symbol, body);
        }
        let body = scope
            .definitions_mut()
            .add(Definition::Native(Native::NumNeg));
        let symbol = crate::shared::symbols::intern("-@");
        scope.classes_mut().define_method(class, symbol, body);
        scope
            .classes_mut()
            .seal_operator(which, class, NEG_GUARD, symbol, body);
    }

    // The module functions (#161). `Kernel.private_instance_methods(false)` on
    // ruby 4.0.6 is where this list comes from, not from taste: each is a
    // method a program calls receiverless and Ruby refuses on a receiver, so
    // `defined?(Object.print)` is `nil` rather than `"method"`.
    //
    // The Ruby-side half of the same list — `loop`, `p`, `print`, `puts` — is
    // in `core/kernel.rb`, next to the definitions, where it reads as Ruby.
    let kernel = Builtin::Kernel.id();
    let kernel_object = scope.classes().object(kernel);
    let singleton = singleton_of(scope, kernel_object)
        .expect("the Kernel module object takes a singleton class");
    for name in [
        "block_given?",
        "catch",
        "fail",
        "lambda",
        "proc",
        "raise",
        "throw",
    ] {
        let symbol = crate::shared::symbols::intern(name);
        // `module_function`, by hand: a public copy on `Kernel`'s singleton and
        // a private instance method. `Kernel.raise` answers and
        // `Object.new.raise` does not, which is the pair `defined?` asks about.
        if let Some(method) = scope.classes_mut().lookup(kernel, symbol) {
            scope.classes_mut().define_method_visibly(
                singleton,
                symbol,
                method.body,
                method.cref,
                Visibility::Public,
            );
        }
        scope
            .classes_mut()
            .set_visibility(kernel, symbol, Visibility::Private);
    }
}

fn jump(pc: usize, displacement: i32) -> usize {
    // The displacement counts from the instruction after the jump, and `pc` has
    // already been advanced past it.
    (pc as isize + displacement as isize) as usize
}

fn bool_value(b: bool) -> Value {
    if b { Value::TRUE } else { Value::FALSE }
}

fn class_handle<'h>(scope: &mut HandleScope<'h>, builtin: Builtin) -> Handle<'h> {
    let object = scope.classes().object(builtin.id());
    scope.root(object)
}

/// Turn a literal *description* into a value in this heap.
///
/// A string literal allocates every time it is evaluated, which is Ruby: two
/// evaluations of the same `"a"` are different objects unless the file is
/// frozen-string-literal.
fn materialise<'h>(
    scope: &mut HandleScope<'h>,
    literal: &Literal,
    string_class: Handle<'h>,
) -> Result<Value, Error> {
    match literal {
        Literal::Float(f) => Ok(Value::flonum(*f).expect("checked at compile time")),
        Literal::BoxedFloat(_) => Err(Error::NoDispatch {
            op: "Float",
            operands: "a float outside flonum range",
        }),
        Literal::BigInt(digits) => {
            let n = num_bigint::BigInt::parse_bytes(digits.as_bytes(), 10)
                .expect("the compiler normalised the literal to base 10");
            Ok(crate::bignum::value(scope, &n))
        }
        Literal::Regexp { source, options } => regexp_literal(scope, source, *options),
        Literal::Str(bytes, encoding) | Literal::FrozenStr(bytes, encoding) => {
            let encoding = *encoding;
            // A frozen literal is interned by content, CRuby's fstring: every
            // evaluation of every site with these bytes answers one object.
            if matches!(literal, Literal::FrozenStr(..))
                && let Some(interned) = scope.regexps().fstring(bytes, encoding)
            {
                return Ok(interned);
            }
            u32::try_from(bytes.len()).map_err(|_| Error::NoDispatch {
                op: "String",
                operands: "a literal larger than 4 GiB",
            })?;
            let value = string_alloc(scope, string_class, bytes, encoding);
            let handle = scope.root(value);
            if matches!(literal, Literal::FrozenStr(..)) {
                scope.freeze(handle);
                let value = scope.get(handle);
                scope.regexps_mut().intern_fstring(bytes, encoding, value);
            }
            Ok(scope.get(handle))
        }
    }
}

// ---------------------------------------------------------------------------
// Operators
// ---------------------------------------------------------------------------

/// A numeric operand, once its tag has been read.
#[derive(Clone, Copy)]
enum Num {
    Int(i64),
    Float(f64),
}

fn num(value: Value) -> Option<Num> {
    value
        .as_fixnum()
        .map(Num::Int)
        .or_else(|| value.as_flonum().map(Num::Float))
}

/// [`Classes::seal_operator`]'s number for `-@`: one past the last [`BinOp`].
///
/// [`Classes::seal_operator`]: crate::class::Classes::seal_operator
const NEG_GUARD: usize = BinOp::Ge as usize + 1;

/// Whether an operator instruction may answer `left`'s operator itself.
///
/// It may unless `left` is a number whose class no longer has the VM's own
/// method under that name — `class Integer; def +(o) = 42; end` makes `1 + 1`
/// 42, measured, so the instruction has to send (#239).
///
/// Code in `core/*.rb` is exempt. It stands where CRuby has C, and C adds two
/// integers without asking `Integer#+`: `[1, 2].each_with_index` still counts
/// from zero after that redefinition.
#[inline]
fn operator_fast_path(scope: &mut HandleScope<'_>, left: Value, op: usize, frame: &Call) -> bool {
    let which = if left.as_fixnum().is_some() {
        0
    } else if left.as_flonum().is_some() {
        1
    } else if crate::bignum::is_big(scope, left) {
        0
    } else {
        return true;
    };
    scope.classes_mut().operator_is_native(which, op)
        || frame
            .iseq
            .path
            .as_deref()
            .is_some_and(|path| path.starts_with("<internal:"))
}

fn binop(
    scope: &mut HandleScope<'_>,
    op: BinOp,
    left: Value,
    right: Value,
) -> Result<Value, Error> {
    match op {
        BinOp::Eq => return Ok(bool_value(ruby_eq(scope, left, right)?)),
        BinOp::Neq => return Ok(bool_value(!ruby_eq(scope, left, right)?)),
        _ => {}
    }

    // An `Integer` past the fixnum range is a heap cell, so it never unpacks as
    // a number. Routed before `num` rather than inside it because building the
    // `BigInt` needs the heap, and because a pair of fixnums must not pay for
    // the check on every `+` in the corpus.
    if crate::bignum::is_big(scope, left) || crate::bignum::is_big(scope, right) {
        return wide_op(scope, op, left, right);
    }

    let (Some(left), Some(right)) = (num(left), num(right)) else {
        return Err(Error::NoDispatch {
            op: op.name(),
            operands: "operands that are not both numbers",
        });
    };

    match (left, right) {
        (Num::Int(a), Num::Int(b)) => integer_op(scope, op, a, b),
        // Ruby promotes to Float when either side is one.
        (a, b) => float_op(op, as_float(a), as_float(b)),
    }
}

/// `1 == 1.0`, exactly — the mixed arm of [`ruby_eq`].
///
/// Not `i as f64 == f`: past 2^53 the cast rounds, so `2**54 + 1 == (2**54).to_f`
/// came out true. A float equals an integer only when it is finite, has no
/// fractional part, and names that same integer. Every `f64` whose magnitude
/// reaches 2^63 is already larger than any fixnum, so the comparison is
/// decidable without widening either side to a bignum.
fn int_eq_float(i: i64, f: f64) -> bool {
    // 2^63, the first magnitude an `i64` cannot hold. Exact as an `f64`.
    const OUT_OF_RANGE: f64 = 9_223_372_036_854_775_808.0;
    if !f.is_finite() || f.fract() != 0.0 || f <= -OUT_OF_RANGE || f >= OUT_OF_RANGE {
        return false;
    }
    f as i64 == i
}

/// A `BigInt` against an `f64`, exactly — the wide twin of [`int_eq_float`],
/// and an ordering rather than just equality because `<` and `>` need it too.
///
/// `None` is NaN, which is unordered against every integer. Nothing else
/// answers `None`: `f.trunc()` of a finite float is integer-valued, so its
/// conversion to a `BigInt` cannot fail.
///
/// The float is never widened to a `BigInt`'s precision and the integer is
/// never narrowed to an `f64`. Narrowing is what made `2**70 + 1 > (2**70).to_f`
/// false: both sides land on the same `f64` past 2^53, and Ruby answers `true`
/// because it compares them exactly. So the float is split at its decimal
/// point, the two integer halves are compared as integers, and the fraction
/// only breaks a tie.
fn big_cmp_float(a: &num_bigint::BigInt, f: f64) -> Option<std::cmp::Ordering> {
    use num_traits::FromPrimitive;
    use std::cmp::Ordering;

    if f.is_nan() {
        return None;
    }
    if f == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if f == f64::NEG_INFINITY {
        return Some(Ordering::Greater);
    }
    let whole = num_bigint::BigInt::from_f64(f.trunc())?;
    Some(match a.cmp(&whole) {
        // Equal integer parts, so the float's fraction decides: `2.5` is
        // greater than `2` and `-2.5` is less than `-2`, because `f64` keeps
        // the sign on the fraction as well as on the whole.
        Ordering::Equal => match f.fract().partial_cmp(&0.0) {
            Some(Ordering::Greater) => Ordering::Less,
            Some(Ordering::Less) => Ordering::Greater,
            _ => Ordering::Equal,
        },
        other => other,
    })
}

/// One of the six comparison operators, answered from an [`Ordering`].
///
/// `None` for an operator that is not a comparison, so a caller can use this
/// as the test for "is this op one of them" as well as for the answer.
fn cmp_op(op: BinOp, ord: std::cmp::Ordering) -> Option<Value> {
    use std::cmp::Ordering;
    Some(bool_value(match op {
        BinOp::Eq => ord == Ordering::Equal,
        BinOp::Neq => ord != Ordering::Equal,
        BinOp::Lt => ord == Ordering::Less,
        BinOp::Le => ord != Ordering::Greater,
        BinOp::Gt => ord == Ordering::Greater,
        BinOp::Ge => ord != Ordering::Less,
        _ => return None,
    }))
}

fn as_float(n: Num) -> f64 {
    match n {
        Num::Int(i) => i as f64,
        Num::Float(f) => f,
    }
}

/// The same operators with at least one operand wider than a fixnum.
///
/// A `Float` on either side still wins, exactly as it does for two fixnums:
/// `(2**70) / 2.0` is a Float. The integer answers go back through
/// [`crate::bignum::value`], so one that has come back inside the fixnum range
/// is an immediate again — `(2**70) - (2**70)` is `0`, the same value the
/// literal `0` is. Measured.
fn wide_op(
    scope: &mut HandleScope<'_>,
    op: BinOp,
    left: Value,
    right: Value,
) -> Result<Value, Error> {
    use num_traits::ToPrimitive;

    let pair = (
        crate::bignum::read(scope, left),
        crate::bignum::read(scope, right),
    );
    let (Some(a), Some(b)) = pair else {
        // One side is a Float, or not a number at all.
        //
        // A *comparison* answers exactly. Widening the integer is what made
        // `2**70 + 1 > (2**70).to_f` false and `2**70 + 1 == (2**70).to_f`
        // true: past 2^53 the two sides are the same `f64`, and Ruby says
        // they are a different number. NaN answers `None` here and falls
        // through to the widening path below, which is where every
        // comparison against it is already false.
        let exact = match (&pair.0, &pair.1) {
            (Some(a), None) => right.as_flonum().and_then(|f| big_cmp_float(a, f)),
            (None, Some(b)) => left
                .as_flonum()
                .and_then(|f| big_cmp_float(b, f))
                .map(std::cmp::Ordering::reverse),
            _ => None,
        };
        if let Some(value) = exact.and_then(|ord| cmp_op(op, ord)) {
            return Ok(value);
        }
        // Arithmetic promotes to Float instead, and *that* conversion can
        // lose precision — which is Ruby's answer too: `(2**70 + 1) / 2.0`
        // and `(2**70) / 2.0` are the same Float in Ruby as well.
        let widen = |v: Value, scope: &mut HandleScope<'_>| -> Option<f64> {
            if let Some(f) = v.as_flonum() {
                return Some(f);
            }
            crate::bignum::read(scope, v).and_then(|n| n.to_f64())
        };
        let a = widen(left, scope);
        let b = widen(right, scope);
        let (Some(a), Some(b)) = (a, b) else {
            return Err(Error::NoDispatch {
                op: op.name(),
                operands: "operands that are not both numbers",
            });
        };
        return float_op(op, a, b);
    };

    let zero = num_bigint::BigInt::from(0);
    match op {
        BinOp::Add => Ok(crate::bignum::value(scope, &(a + b))),
        BinOp::Sub => Ok(crate::bignum::value(scope, &(a - b))),
        BinOp::Mul => Ok(crate::bignum::value(scope, &(a * b))),
        // `num-bigint`'s `/` and `%` truncate the way Rust's do; Ruby floors.
        // The same correction `floor_div` and `floor_mod` make for fixnums, and
        // the reason neither of those is simply `a / b`.
        BinOp::Div => {
            if b == zero {
                return Err(Error::raise("ZeroDivisionError", "divided by 0"));
            }
            let quotient = &a / &b;
            let exact = &quotient * &b == a;
            let negative = (a < zero) != (b < zero);
            let floored = if exact || !negative {
                quotient
            } else {
                quotient - 1
            };
            Ok(crate::bignum::value(scope, &floored))
        }
        BinOp::Mod => {
            if b == zero {
                return Err(Error::raise("ZeroDivisionError", "divided by 0"));
            }
            let remainder = &a % &b;
            let cross = remainder != zero && ((remainder < zero) != (b < zero));
            let floored = if cross { remainder + b } else { remainder };
            Ok(crate::bignum::value(scope, &floored))
        }
        BinOp::Lt => Ok(bool_value(a < b)),
        BinOp::Le => Ok(bool_value(a <= b)),
        BinOp::Gt => Ok(bool_value(a > b)),
        BinOp::Ge => Ok(bool_value(a >= b)),
        BinOp::Eq => Ok(bool_value(a == b)),
        BinOp::Neq => Ok(bool_value(a != b)),
    }
}

/// `&`, `|`, `^`, `~`, `<<` and `>>` with a bignum on either side.
///
/// Separate from [`wide_op`] because these are `Native`s rather than `BinOp`
/// instructions: Ruby's bit operators have no fast-path opcode, so they arrive
/// through method dispatch with the receiver in `call.receiver`.
fn wide_bits(
    scope: &mut HandleScope<'_>,
    op: BitOp,
    receiver: Value,
    argument: Option<Value>,
) -> Result<Value, Error> {
    use num_traits::ToPrimitive;

    let Some(left) = crate::bignum::read(scope, receiver) else {
        return Err(Error::NoDispatch {
            op: "Integer bit operation",
            operands: "a receiver that is not an Integer",
        });
    };
    if op == BitOp::Not {
        return Ok(crate::bignum::value(scope, &!left));
    }
    let Some(right) = argument.and_then(|v| crate::bignum::read(scope, v)) else {
        return Err(Error::raise(
            "TypeError",
            "no implicit conversion into Integer",
        ));
    };
    let answer = match op {
        BitOp::And => left & right,
        BitOp::Or => left | right,
        BitOp::Xor => left ^ right,
        BitOp::Not => unreachable!("handled above"),
        BitOp::Shl | BitOp::Shr => {
            // `a >> -n` is `a << n`, and the other way round.
            let left_shift = (op == BitOp::Shl) == (right >= num_bigint::BigInt::from(0));
            let Some(distance) = right.magnitude().to_u32() else {
                return Err(Error::NoDispatch {
                    op: "Integer shift",
                    operands: "a shift too large to allocate a result for",
                });
            };
            if left_shift {
                left << distance
            } else {
                // An arithmetic shift: `num-bigint`'s `>>` already carries the
                // sign, so a negative bignum shifted far enough lands on -1
                // rather than 0, which is Ruby's answer.
                left >> distance
            }
        }
    };
    Ok(crate::bignum::value(scope, &answer))
}

fn integer_op(scope: &mut HandleScope<'_>, op: BinOp, a: i64, b: i64) -> Result<Value, Error> {
    let value = match op {
        BinOp::Add => a.checked_add(b),
        BinOp::Sub => a.checked_sub(b),
        BinOp::Mul => a.checked_mul(b),
        // Ruby's `/` and `%` floor; Rust's truncate. They agree only while the
        // signs do, and `-7 / 2` is `-4` in Ruby and `-3` in Rust.
        BinOp::Div => {
            if b == 0 {
                return Err(Error::raise("ZeroDivisionError", "divided by 0"));
            }
            floor_div(a, b)
        }
        BinOp::Mod => {
            if b == 0 {
                return Err(Error::raise("ZeroDivisionError", "divided by 0"));
            }
            floor_mod(a, b)
        }
        BinOp::Lt => return Ok(bool_value(a < b)),
        BinOp::Le => return Ok(bool_value(a <= b)),
        BinOp::Gt => return Ok(bool_value(a > b)),
        BinOp::Ge => return Ok(bool_value(a >= b)),
        BinOp::Eq | BinOp::Neq => unreachable!("handled before the numeric path"),
    };
    // An `Integer` that leaves the fixnum range promotes, which is why this is
    // the one arm that can allocate. A `checked_*` answering `None` above is
    // the overflow signal, and the operands are redone as `BigInt` rather than
    // a wrapped `i64` being repaired: `i64::MIN / -1` overflows with no wrapped
    // value worth having.
    match value.and_then(Value::fixnum) {
        Some(fits) => Ok(fits),
        None => {
            let (a, b) = (
                Value::fixnum(a).expect("a came from a fixnum"),
                Value::fixnum(b).expect("b came from a fixnum"),
            );
            wide_op(scope, op, a, b)
        }
    }
}

fn floor_div(a: i64, b: i64) -> Option<i64> {
    let quotient = a.checked_div(b)?;
    if a % b != 0 && ((a < 0) != (b < 0)) {
        quotient.checked_sub(1)
    } else {
        Some(quotient)
    }
}

fn floor_mod(a: i64, b: i64) -> Option<i64> {
    let remainder = a.checked_rem(b)?;
    if remainder != 0 && ((remainder < 0) != (b < 0)) {
        remainder.checked_add(b)
    } else {
        Some(remainder)
    }
}

fn float_op(op: BinOp, a: f64, b: f64) -> Result<Value, Error> {
    let float = |f: f64| {
        Value::flonum(f).ok_or(Error::NoDispatch {
            op: op.name(),
            operands: "a result outside flonum range",
        })
    };
    match op {
        BinOp::Add => float(a + b),
        BinOp::Sub => float(a - b),
        BinOp::Mul => float(a * b),
        BinOp::Div => float(a / b),
        // `%` by zero raises where `/` by zero answers Infinity — measured:
        // `4.2 / 0` is Infinity, `4.2 % 0` and `4.2 % 0.0` are both
        // ZeroDivisionError. `integer_op` has always checked this; the float
        // path did not, and produced a NaN it then could not represent.
        BinOp::Mod => {
            if b == 0.0 {
                return Err(Error::raise("ZeroDivisionError", "divided by 0"));
            }
            // CRuby's `flodivmod`: `fmod`, which is exact, then the divisor's
            // sign. `a - b * floor(a / b)` loses the remainder once `a / b`
            // passes 2^53 — `2**70 % 3.0` came out 0.0 rather than 1.0.
            let modulus = a % b;
            float(if b * modulus < 0.0 {
                modulus + b
            } else {
                modulus
            })
        }
        BinOp::Lt => Ok(bool_value(a < b)),
        BinOp::Le => Ok(bool_value(a <= b)),
        BinOp::Gt => Ok(bool_value(a > b)),
        BinOp::Ge => Ok(bool_value(a >= b)),
        BinOp::Eq | BinOp::Neq => unreachable!("handled before the numeric path"),
    }
}

fn negate(scope: &mut HandleScope<'_>, value: Value) -> Result<Value, Error> {
    let fail = || Error::NoDispatch {
        op: "-@",
        operands: "an operand that is not a number",
    };
    // `-(2**70)`, and also `-4611686018427387904`: Prism gives a negative
    // literal past the fixnum floor as a negation of a positive one, so the
    // operand is already a heap cell by the time this runs.
    if let Some(n) = crate::bignum::read(scope, value) {
        return Ok(crate::bignum::value(scope, &-n));
    }
    match num(value).ok_or_else(fail)? {
        Num::Int(i) => i.checked_neg().and_then(Value::fixnum).ok_or_else(fail),
        Num::Float(f) => Value::flonum(-f).ok_or_else(fail),
    }
}

/// Ruby `==`, for the types this slice can produce.
///
/// The same function [`Insn::BinOp`] uses, exported because `spec/harness`
/// compares a matcher's two sides with it — so the harness cannot pass an
/// example the VM would fail.
pub fn ruby_eq(scope: &mut HandleScope<'_>, left: Value, right: Value) -> Result<bool, Error> {
    ruby_eq_in(scope, left, right, &mut Vec::new())
}

/// [`ruby_eq`], with the pairs of arrays it is part-way through comparing: a
/// pair met again is equal so far, which is how `Array#==` ends on an array
/// that contains itself rather than recursing until the Rust stack runs out.
fn ruby_eq_in(
    scope: &mut HandleScope<'_>,
    left: Value,
    right: Value,
    comparing: &mut Vec<(Value, Value)>,
) -> Result<bool, Error> {
    // Bitwise equality is exactly Ruby's `equal?` for immediates, which is why
    // #6 excluded NaN and -0.0 from the flonum range. It settles most pairs —
    // but not for an object whose class may define its own `==`: identical
    // operands still dispatch then, because `def ==(o) = false` is honoured
    // even for `o1 == o1`. Measured; `kernel/case_compare_spec.rb` checks it.
    if left == right && (left.is_immediate() || heap_kind(scope, left).is_some()) {
        return Ok(true);
    }
    // `1 == 1.0` is true in Ruby even though the words differ. Two *integers*
    // still compare as integers: past 2^53 two different `i64`s round to the
    // same `f64`, which made `2**54 == 2**54 + 1` true and — because `Hash`
    // looks a key up with `==` — `{2**54 => 1}[2**54 + 1]` answer `1`. The
    // bignum arm below says the same thing for the wider type; this is the
    // fixnum half of it, and `binop` already splits the pair this way for
    // every other operator.
    if let (Some(a), Some(b)) = (num(left), num(right)) {
        return Ok(match (a, b) {
            (Num::Int(x), Num::Int(y)) => x == y,
            (Num::Int(i), Num::Float(f)) | (Num::Float(f), Num::Int(i)) => int_eq_float(i, f),
            (Num::Float(x), Num::Float(y)) => x == y,
        });
    }
    // A heap `Integer` is not immediate, so the bitwise test above missed it.
    // Compared as integers rather than as floats: past 2^53 two different
    // bignums round to the same `f64`, and `2**70 == 2**70 + 1` would be true.
    if crate::bignum::is_big(scope, left) || crate::bignum::is_big(scope, right) {
        let pair = (
            crate::bignum::read(scope, left),
            crate::bignum::read(scope, right),
        );
        match (&pair.0, &pair.1) {
            (Some(a), Some(b)) => return Ok(a == b),
            // A Float against a bignum, compared exactly for the same reason
            // `int_eq_float` compares a Float against a fixnum exactly. Both
            // orders reach this: with the bignum on the right the numeric
            // fast path above skipped the pair, and with it on the left the
            // identity rule below would have answered `false`.
            (Some(a), None) => {
                if let Some(f) = right.as_flonum() {
                    return Ok(big_cmp_float(a, f) == Some(std::cmp::Ordering::Equal));
                }
            }
            (None, Some(b)) => {
                if let Some(f) = left.as_flonum() {
                    return Ok(big_cmp_float(b, f) == Some(std::cmp::Ordering::Equal));
                }
            }
            (None, None) => {}
        }
    }
    // Ruby dispatches `a == b` on `a`, so the *left* operand decides. Every
    // immediate's `==` is identity once the numeric case above is out of the
    // way: `nil == false`, `:a == 1` and `1 == "1"` are all simply false.
    if left.is_immediate() {
        return Ok(false);
    }

    match heap_kind(scope, left) {
        Some(HeapKind::Str) => {
            if heap_kind(scope, right) != Some(HeapKind::Str) {
                return Ok(false);
            }
            let (a, b) = (scope.root(left), scope.root(right));
            let (a_bytes, b_bytes) = (
                crate::strings::bytes(scope, a),
                crate::strings::bytes(scope, b),
            );
            let (a_enc, b_enc) = (
                crate::strings::encoding(scope, a),
                crate::strings::encoding(scope, b),
            );
            Ok(a_bytes == b_bytes
                && crate::strings::comparable((a_enc, &a_bytes), (b_enc, &b_bytes)))
        }
        Some(HeapKind::Array) => {
            if heap_kind(scope, right) != Some(HeapKind::Array) {
                return Ok(false);
            }
            let (a, b) = (scope.root(left), scope.root(right));
            if array_len(scope, a) != array_len(scope, b) {
                return Ok(false);
            }
            if comparing.contains(&(left, right)) {
                return Ok(true);
            }
            comparing.push((left, right));
            for index in 0..array_len(scope, a) {
                let (x, y) = (array_get(scope, a, index), array_get(scope, b, index));
                if !ruby_eq_in(scope, x, y, comparing)? {
                    comparing.pop();
                    return Ok(false);
                }
            }
            comparing.pop();
            Ok(true)
        }
        // A class object, or anything else whose `==` is a method that does not
        // exist yet. Refusing keeps a spec blocked rather than passing it for
        // the wrong reason.
        None => Err(Error::NoDispatch {
            op: "==",
            operands: "an object whose class has no methods yet",
        }),
    }
}

/// `===`, which is `==` for every type this slice has and is deliberately *not*
/// assumed to be for the ones it does not.
fn case_eq(scope: &mut HandleScope<'_>, condition: Value, subject: Value) -> Result<bool, Error> {
    // `when /re/` is `Regexp#===`, which matches and sets `$~` rather than
    // comparing. The first `when` condition in this VM that is not `==`.
    if is_regexp(scope, condition) {
        return Ok(regexp_match_value(scope, condition, subject)? != Value::NIL);
    }
    if !condition.is_immediate() && heap_kind(scope, condition).is_none() {
        // A Range, Class, Regexp, or Proc in `when` position means something
        // other than `==`, and getting it wrong would pass a spec for the wrong
        // reason.
        return Err(Error::NoDispatch {
            op: "===",
            operands: "a `when` condition that is not a value",
        });
    }
    ruby_eq(scope, condition, subject)
}

/// The heap classes this slice can reason about.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HeapKind {
    Str,
    Array,
}

fn heap_kind(scope: &mut HandleScope<'_>, value: Value) -> Option<HeapKind> {
    if value.is_immediate() {
        return None;
    }
    // The class's *representation*, not the class object. One table read where
    // this was two comparisons, and it accepts a subclass — `MyString == "a"`
    // is true in Ruby, and was a `NoDispatch` while this asked whether the
    // class object was exactly `String`'s. `bench/method_cache.rs` is the check
    // that the receiver question did not get more expensive; this runs on every
    // `==`.
    match class_of(scope, value).and_then(|id| scope.classes().repr(id)) {
        Some(Builtin::String) => Some(HeapKind::Str),
        Some(Builtin::Array) => Some(HeapKind::Array),
        _ => None,
    }
}

/// `Object#inspect`, for the types this slice has.
///
/// Not a Ruby method — `core/*.rb` owns that in
/// [#15](https://github.com/ar4mirez/spinel/issues/15). This is what a *report*
/// prints: a spec failure that says `expected [1, 2], got [1]` is worth more
/// than one that says two values differed.
#[must_use]
pub fn inspect(scope: &mut HandleScope<'_>, value: Value) -> String {
    use crate::value::Unpacked;
    match value.unpack() {
        Unpacked::Nil => "nil".to_owned(),
        Unpacked::True => "true".to_owned(),
        Unpacked::False => "false".to_owned(),
        Unpacked::Undef => "undefined".to_owned(),
        Unpacked::Fixnum(n) => n.to_string(),
        // Ruby prints a float with a fractional part always: `1.0`, not `1`.
        Unpacked::Flonum(f) => {
            if f.fract() == 0.0 && f.is_finite() {
                format!("{f:.1}")
            } else {
                f.to_string()
            }
        }
        Unpacked::Symbol(id) => match crate::shared::symbols::name(id) {
            Some(name) => format!(":{name}"),
            None => format!(":<symbol {}>", id.0),
        },
        // A heap `Integer` renders as its digits, the same as a fixnum: the
        // boundary is invisible from Ruby, so it must be invisible here too.
        // Checked before `heap_kind`, which knows the `String`/`Array`/`Hash`
        // cells and would call this one an anonymous object.
        Unpacked::Heap(_) if crate::bignum::is_big(scope, value) => {
            match crate::bignum::read(scope, value) {
                Some(n) => n.to_string(),
                None => unreachable!("`is_big` just said it was one"),
            }
        }
        Unpacked::Heap(_) => match heap_kind(scope, value) {
            Some(HeapKind::Str) => {
                let handle = scope.root(value);
                let bytes = crate::strings::bytes(scope, handle);
                format!("{:?}", String::from_utf8_lossy(&bytes))
            }
            Some(HeapKind::Array) => {
                let handle = scope.root(value);
                let items: Vec<String> = (0..array_len(scope, handle))
                    .map(|index| {
                        let item = array_get(scope, handle, index);
                        inspect(scope, item)
                    })
                    .collect();
                format!("[{}]", items.join(", "))
            }
            None => {
                let handle = scope.root(value);
                match scope.class_id_of(handle) {
                    // `Module#inspect` is the module's name, which is how a
                    // class reads in a spec's failure message and in `p C`.
                    Some(id) => match scope.classes().name(id) {
                        Some(name) => name.to_owned(),
                        None => format!("#<Class:0x{:x}>", id.index()),
                    },
                    None => "#<object>".to_owned(),
                }
            }
        },
    }
}

// -- regexps ---------------------------------------------------------------
//
// A `Regexp` object is three slots: an index into the heap's compiled table,
// the source as a `String`, and the options as a fixnum. A `MatchData` is
// three more: the regexp, the subject, and an `Array` of byte offsets, two per
// group, with `nil` where a group took no part.
//
// Byte offsets in the array, character offsets out of `#begin` and `#end`,
// because the subject is right there to convert with and storing both would
// mean keeping them in step.

/// What a refusal from the regex engine means to the VM.
fn regexp_error(error: &spinel_regex::Error) -> Error {
    match error {
        spinel_regex::Error::Syntax(message) => Error::Raise {
            class: "RegexpError",
            message: message.clone(),
        },
        // Never a wrong answer: the harness reports the example blocked.
        spinel_regex::Error::Unsupported(what) => Error::Unknowable {
            what,
            needs: "the rest of the Onigmo dialect",
        },
        spinel_regex::Error::Budget => Error::Budget,
    }
}

/// A regexp literal, compiled once and cached.
///
/// Ruby answers the *same* object every time a literal without interpolation is
/// evaluated — `rs[0].should.equal?(rs[1])` in `regexp_spec.rb` — so the cache
/// is a correctness requirement rather than an optimisation.
fn regexp_literal(scope: &mut HandleScope<'_>, source: &str, options: i64) -> Result<Value, Error> {
    if let Some(cached) = scope.regexps().cached(source, options) {
        return Ok(cached);
    }
    let value = regexp_new(scope, source, options)?;
    scope.regexps_mut().cache(source, options, value);
    Ok(value)
}

/// Compile `source` and wrap it in a `Regexp` object.
/// `Regexp.new(source, options)`.
///
/// The literal path is [`Insn::NewRegexp`]; this is the same compile with its
/// argument arriving as a value, which is the half `core/regexp.rb` said was
/// missing. The object is **not** frozen, unlike a literal — measured.
fn regexp_new_from(scope: &mut HandleScope<'_>, call: &Pending) -> Result<Value, Error> {
    let Some(first) = call.args.first().copied() else {
        return Err(Error::raise(
            "ArgumentError",
            "wrong number of arguments (given 0, expected 1..3)",
        ));
    };
    // `Regexp.new(/a/i)` takes the pattern's own source *and* its options, and
    // ignores a second argument. Measured.
    let from_regexp = is_regexp(scope, first);
    let (source, mut options) = if from_regexp {
        let mut nested = scope.nested();
        let handle = nested.root(first);
        let text = nested.slot(handle, crate::regexp::REGEXP_SOURCE);
        let opts = nested.slot(handle, crate::regexp::REGEXP_OPTIONS);
        drop(nested);
        let opts = match opts.unpack() {
            crate::value::Unpacked::Fixnum(n) => n,
            _ => 0,
        };
        (string_text(scope, text), opts)
    } else {
        (string_text(scope, first), 0)
    };
    let Some(source) = source else {
        return Err(Error::raise(
            "TypeError",
            "no implicit conversion into String",
        ));
    };
    if !from_regexp {
        // Ruby takes an integer of flags, and treats any other truthy value as
        // `IGNORECASE` — `Regexp.new("a", true).options` is 1. Measured.
        options = match call.args.get(1).copied() {
            None | Some(Value::NIL) | Some(Value::FALSE) => 0,
            Some(v) => match v.unpack() {
                crate::value::Unpacked::Fixnum(n) => n,
                _ => spinel_regex::Flags::IGNORECASE,
            },
        };
    }
    regexp_build(scope, &source, options, false)
}

fn regexp_new(scope: &mut HandleScope<'_>, source: &str, options: i64) -> Result<Value, Error> {
    regexp_build(scope, source, options, true)
}

/// The pattern behind both constructors.
///
/// A literal is frozen from birth, which is what makes `Regexp#initialize` on
/// one a `FrozenError`; `Regexp.new("a").frozen?` is `false`. Measured, and the
/// only difference between the two.
fn regexp_build(
    scope: &mut HandleScope<'_>,
    source: &str,
    options: i64,
    frozen: bool,
) -> Result<Value, Error> {
    let index = scope
        .regexps_mut()
        .add(source, options)
        .map_err(|e| regexp_error(&e))?;
    let text = string_new(scope, source);
    let class = class_handle(scope, Builtin::Regexp);
    let (payload, slots) = crate::regexp::regexp_shape();
    let mut nested = scope.nested();
    let text = nested.root(text);
    let handle = nested.alloc(Some(class), payload, slots);
    let index = Value::fixnum(i64::try_from(index).unwrap_or(i64::MAX))
        .expect("a regexp table index fits a fixnum");
    nested.set_slot(handle, crate::regexp::REGEXP_INDEX, index);
    let text = nested.get(text);
    nested.set_slot(handle, crate::regexp::REGEXP_SOURCE, text);
    if frozen {
        nested.freeze(handle);
    }
    nested.set_slot(
        handle,
        crate::regexp::REGEXP_OPTIONS,
        Value::fixnum(options).expect("regexp options fit a fixnum"),
    );
    Ok(nested.get(handle))
}

/// Whether `value` has `builtin`'s **representation** — its own instances, and
/// a subclass's.
///
/// The question a primitive asks about its receiver. Comparing class objects
/// instead made `MyArray#size` a "receiver that is not an Array" while
/// `MyArray.new.class` correctly answered `MyArray`, which is the pair of wrong
/// answers #199 exists to remove.
fn is_builtin(scope: &mut HandleScope<'_>, value: Value, builtin: Builtin) -> bool {
    class_of(scope, value).is_some_and(|id| scope.classes().repr(id) == Some(builtin))
}

fn is_regexp(scope: &mut HandleScope<'_>, value: Value) -> bool {
    is_builtin(scope, value, Builtin::Regexp)
}

/// The compiled pattern behind a `Regexp` object.
fn regexp_program(
    scope: &mut HandleScope<'_>,
    value: Value,
) -> Option<std::sync::Arc<spinel_regex::Regex>> {
    if !is_regexp(scope, value) {
        return None;
    }
    let mut nested = scope.nested();
    let handle = nested.root(value);
    let index = nested
        .slot(handle, crate::regexp::REGEXP_INDEX)
        .as_fixnum()?;
    let index = usize::try_from(index).ok()?;
    nested.regexps().get(index).map(std::sync::Arc::clone)
}

/// A `String` object's text, when it is one and it is UTF-8.
fn string_text(scope: &mut HandleScope<'_>, value: Value) -> Option<String> {
    let bytes = string_bytes(scope, value)?;
    String::from_utf8(bytes).ok()
}

/// Match `regexp` against `subject`, answer a `MatchData` or nil, and set `$~`.
fn regexp_match_value(
    scope: &mut HandleScope<'_>,
    regexp: Value,
    subject: Value,
) -> Result<Value, Error> {
    regexp_match_from(scope, regexp, subject, Value::NIL)
}

/// The same, from a starting position.
///
/// `pos` is in *characters* and may be negative, counting back from the end —
/// `Regexp#match` and `String#match` both take one, and both spell it that way.
fn regexp_match_from(
    scope: &mut HandleScope<'_>,
    regexp: Value,
    subject: Value,
    pos: Value,
) -> Result<Value, Error> {
    let Some(program) = regexp_program(scope, regexp) else {
        return Err(Error::NoDispatch {
            op: "=~",
            operands: "a receiver that is not a Regexp",
        });
    };
    // `/re/ =~ nil` is nil in Ruby, not a TypeError.
    if subject == Value::NIL {
        scope.set_last_match(Value::NIL);
        return Ok(Value::NIL);
    }
    // A UTF-16 or UTF-32 subject cannot meet an ASCII-based pattern at all:
    // CRuby's CompatibilityError, naming the regexp's encoding — US-ASCII for
    // an ASCII source, UTF-8 otherwise, until regexps carry their own (#33).
    if let Some(encoding) = string_encoding(scope, subject)
        && !crate::strings::ascii_compatible(encoding)
    {
        let source = {
            let mut nested = scope.nested();
            let handle = nested.root(regexp);
            nested.slot(handle, crate::regexp::REGEXP_SOURCE)
        };
        let ascii = string_bytes(scope, source).is_some_and(|bytes| bytes.is_ascii());
        return Err(Error::raise(
            "Encoding::CompatibilityError",
            format!(
                "incompatible encoding regexp match ({} regexp with {} string)",
                if ascii { "US-ASCII" } else { "UTF-8" },
                crate::strings::name(encoding)
            ),
        ));
    }
    // A String whose bytes are not valid in its own encoding cannot be
    // matched, and says so; a valid one that is not UTF-8 is a real subject
    // the regex engine cannot read yet (#19, #33).
    if let (Some(bytes), Some(encoding)) = (
        string_bytes(scope, subject),
        string_encoding(scope, subject),
    ) && std::str::from_utf8(&bytes).is_err()
    {
        if crate::strings::valid(encoding, &bytes) != Some(true) {
            return Err(Error::raise(
                "ArgumentError",
                format!(
                    "invalid byte sequence in {}",
                    crate::strings::name(encoding)
                ),
            ));
        }
        return Err(Error::Unknowable {
            what: "matching a regexp against a string that is not UTF-8 text",
            needs: "a regex engine that reads bytes in the string's encoding (#33)",
        });
    }
    let Some(text) = string_text(scope, subject) else {
        return Err(Error::Raise {
            class: "TypeError",
            message: format!(
                "no implicit conversion of {} into String",
                class_name_of(scope, subject)
            ),
        });
    };

    // A position past either end of the subject is no match at all, rather
    // than an error.
    let start = match pos {
        Value::NIL => 0,
        _ => {
            let Some(n) = pos.as_fixnum() else {
                return Err(Error::Raise {
                    class: "TypeError",
                    message: format!(
                        "no implicit conversion of {} into Integer",
                        class_name_of(scope, pos)
                    ),
                });
            };
            let chars = text.chars().count() as i64;
            let from = if n < 0 { chars + n } else { n };
            if from < 0 || from > chars {
                scope.set_last_match(Value::NIL);
                return Ok(Value::NIL);
            }
            byte_offset(&text, from as usize)
        }
    };

    let found = program
        .find_at(&text, start)
        .map_err(|e| regexp_error(&e))?;
    let Some(caps) = found else {
        scope.set_last_match(Value::NIL);
        return Ok(Value::NIL);
    };

    let mut offsets = Vec::with_capacity(caps.len() * 2);
    for group in 0..caps.len() {
        match caps.group(group) {
            Some((start, end)) => {
                offsets.push(
                    Value::fixnum(i64::try_from(start).unwrap_or(i64::MAX))
                        .expect("an offset fits a fixnum"),
                );
                offsets.push(
                    Value::fixnum(i64::try_from(end).unwrap_or(i64::MAX))
                        .expect("an offset fits a fixnum"),
                );
            }
            None => {
                offsets.push(Value::NIL);
                offsets.push(Value::NIL);
            }
        }
    }

    let data = match_data_new(scope, regexp, subject, &offsets);
    scope.set_last_match(data);
    Ok(data)
}

fn match_data_new(
    scope: &mut HandleScope<'_>,
    regexp: Value,
    subject: Value,
    offsets: &[Value],
) -> Value {
    // The subject is kept as a frozen copy unless it is frozen already, as
    // CRuby's `rb_str_new_frozen` does: measured, `md.string` is frozen and is
    // not the caller's String, so mutating that String later cannot change
    // what the match reports.
    let subject = match string_bytes(scope, subject) {
        Some(bytes) => {
            let original = scope.root(subject);
            if scope.is_frozen(original) {
                subject
            } else {
                let encoding = string_encoding(scope, subject).unwrap_or(crate::strings::UTF_8);
                let copy = string_bytes_in(scope, Builtin::String.id(), &bytes, encoding);
                let copy = scope.root(copy);
                scope.freeze(copy);
                scope.get(copy)
            }
        }
        None => subject,
    };
    let array = new_array(scope, offsets);
    let class = class_handle(scope, Builtin::MatchData);
    let (payload, slots) = crate::regexp::match_shape();
    let mut nested = scope.nested();
    let array = nested.root(array);
    let handle = nested.alloc(Some(class), payload, slots);
    nested.set_slot(handle, crate::regexp::MATCH_REGEXP, regexp);
    nested.set_slot(handle, crate::regexp::MATCH_SUBJECT, subject);
    let array = nested.get(array);
    nested.set_slot(handle, crate::regexp::MATCH_OFFSETS, array);
    nested.get(handle)
}

/// A `MatchData`'s three parts: its regexp, its subject, and the byte range
/// each group covered.
type MatchParts = (Value, Subject, Vec<Option<(usize, usize)>>);

/// A match's subject: its bytes, and the encoding every piece of it keeps —
/// `$&`, `pre_match`, a group — as CRuby's do (#19).
struct Subject {
    bytes: Vec<u8>,
    encoding: u8,
}

impl Subject {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Bytes `start..end` as a new String in the subject's encoding.
    fn part(&self, scope: &mut HandleScope<'_>, start: usize, end: usize) -> Value {
        string_bytes_in(
            scope,
            Builtin::String.id(),
            &self.bytes[start..end],
            self.encoding,
        )
    }
}

/// A `MatchData`'s three parts: its regexp, its subject text, and its offsets.
fn match_parts(scope: &mut HandleScope<'_>, value: Value) -> Option<MatchParts> {
    if !is_builtin(scope, value, Builtin::MatchData) {
        return None;
    }
    let (regexp, subject, offsets) = {
        let mut nested = scope.nested();
        let handle = nested.root(value);
        (
            nested.slot(handle, crate::regexp::MATCH_REGEXP),
            nested.slot(handle, crate::regexp::MATCH_SUBJECT),
            nested.slot(handle, crate::regexp::MATCH_OFFSETS),
        )
    };
    let text = Subject {
        bytes: string_bytes(scope, subject)?,
        encoding: string_encoding(scope, subject)?,
    };
    let raw = array_elements(scope, offsets)?;
    let groups = raw
        .chunks(2)
        .map(
            |pair| match (pair.first()?.as_fixnum(), pair.get(1)?.as_fixnum()) {
                (Some(start), Some(end)) => Some((start as usize, end as usize)),
                _ => None,
            },
        )
        .collect();
    Some((regexp, text, groups))
}

/// One of `$~`, `$&`, `` $` ``, `$'`, `$1`..`$n`, read off the last match.
fn last_match_part(scope: &mut HandleScope<'_>, which: &MatchRef) -> Result<Value, Error> {
    let data = scope.last_match();
    if matches!(which, MatchRef::Data) {
        return Ok(data);
    }
    if data == Value::NIL {
        return Ok(Value::NIL);
    }
    let Some((_, text, groups)) = match_parts(scope, data) else {
        return Ok(Value::NIL);
    };
    let slice = match_slice(which, text.len(), &groups);
    Ok(match slice {
        Some((start, end)) => text.part(scope, start, end),
        None => Value::NIL,
    })
}

/// Which stretch of the subject a regexp special names, or `None` where it has
/// no value — an unmatched group, or a `$&` on a match that did not happen.
///
/// Shared by [`Insn::LastMatch`] and [`Insn::DefinedMatch`] so the two cannot
/// drift: `defined?($1)` is exactly "`$1` would answer a string", which is not
/// the same as "the pattern has a group 1".
fn match_slice(
    which: &MatchRef,
    text_len: usize,
    groups: &[Option<(usize, usize)>],
) -> Option<(usize, usize)> {
    let whole = groups.first().copied().flatten();
    match which {
        MatchRef::Data => whole,
        MatchRef::Whole => whole,
        MatchRef::Pre => whole.map(|(start, _)| (0, start)),
        MatchRef::Post => whole.map(|(_, end)| (end, text_len)),
        // `$10` is group 10 when the pattern has one, and nil otherwise.
        MatchRef::Group(n) => groups.get(*n as usize).copied().flatten(),
        // `$+` is the last group that *participated*: `"a" =~ /(a)(b)?/`
        // answers `"a"` and not `nil`, so this skips trailing unmatched groups
        // rather than taking the highest number. Group 0 is the whole match and
        // is not a group for this purpose.
        MatchRef::LastGroup => groups.iter().skip(1).rev().find_map(|g| *g),
    }
}

/// Whether `value` is a `MatchData`, the only thing `$~ = ` accepts besides nil.
fn is_match_data(scope: &mut HandleScope<'_>, value: Value) -> bool {
    class_of(scope, value) == Some(Builtin::MatchData.id())
}

/// `defined?($&)`, `defined?($1)`, `defined?($+)`.
///
/// `"global-variable"` where the ref has a value and `nil` where it does not.
/// `$~` never reaches here: it is `"global-variable"` whether or not anything
/// matched, so the compiler answers it with a constant.
fn defined_match(scope: &mut HandleScope<'_>, which: &MatchRef) -> bool {
    let data = scope.last_match();
    if data == Value::NIL {
        return false;
    }
    let Some((_, text, groups)) = match_parts(scope, data) else {
        return false;
    };
    match_slice(which, text.len(), &groups).is_some()
}

/// Ruby reports match offsets in characters of the subject's encoding; the
/// engine works in bytes.
fn char_offset(text: &Subject, byte: usize) -> i64 {
    let prefix = &text.bytes[..byte.min(text.len())];
    let count = crate::strings::char_offsets(text.encoding, prefix)
        .map_or(prefix.len(), |offsets| offsets.len() - 1);
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// The first argument of a call, or nil when it was given none.
fn first_arg(call: &Pending) -> Value {
    nth_arg(call, 0)
}

fn nth_arg(call: &Pending, n: usize) -> Value {
    call.args.get(n).copied().unwrap_or(Value::NIL)
}

/// Ruby counts positions in characters; the engine works in bytes.
fn byte_offset(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i)
}

/// Escape any `/` the source left bare, for `Regexp#inspect`.
fn escape_slashes(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut escaped = false;
    for c in source.chars() {
        if c == '/' && !escaped {
            out.push('\\');
        }
        escaped = c == '\\' && !escaped;
        out.push(c);
    }
    out
}

/// The `MatchData` readers, which all work off the same three parts.
fn match_answer(
    scope: &mut HandleScope<'_>,
    native: Native,
    call: &Pending,
    regexp: Value,
    text: &Subject,
    groups: &[Option<(usize, usize)>],
) -> Result<Value, Error> {
    let whole = groups.first().copied().flatten();
    match native {
        Native::MatchNames => {
            let Some(program) = regexp_program(scope, regexp) else {
                return Err(Error::NoDispatch {
                    op: "names",
                    operands: "a MatchData whose regexp is gone",
                });
            };
            // One name may label several groups, and Ruby lists it once.
            let mut names: Vec<String> = Vec::new();
            for (name, _) in program.names() {
                if !names.iter().any(|seen| seen == name) {
                    names.push(name.clone());
                }
            }
            let names: Vec<Value> = names
                .into_iter()
                .map(|name| string_new(scope, &name))
                .collect();
            Ok(new_array(scope, &names))
        }

        Native::MatchSize => Ok(
            Value::fixnum(i64::try_from(groups.len()).unwrap_or(i64::MAX))
                .expect("a group count fits a fixnum"),
        ),

        Native::MatchAround { post } => {
            let slice = match (whole, post) {
                (Some((_, end)), true) => (end, text.len()),
                (Some((start, _)), false) => (0, start),
                (None, _) => return Ok(Value::NIL),
            };
            Ok(text.part(scope, slice.0, slice.1))
        }

        Native::MatchEdge { end } => {
            let index = match_group_index(scope, call, regexp)?;
            // `#begin` is stricter than `#[]`: a group number the pattern does
            // not have is an IndexError rather than nil.
            if index >= groups.len() {
                return Err(Error::Raise {
                    class: "IndexError",
                    message: format!("index {index} out of matches"),
                });
            }
            Ok(match groups.get(index).copied().flatten() {
                Some((start, stop)) => {
                    let byte = if end { stop } else { start };
                    Value::fixnum(char_offset(text, byte)).expect("an offset fits a fixnum")
                }
                None => Value::NIL,
            })
        }

        Native::MatchToA { captures } => {
            // `to_a` leads with the whole match; `captures` does not.
            let skip = usize::from(captures);
            let elements: Vec<Value> = groups
                .iter()
                .skip(skip)
                .map(|group| match group {
                    Some((start, end)) => text.part(scope, *start, *end),
                    None => Value::NIL,
                })
                .collect();
            Ok(new_array(scope, &elements))
        }

        Native::MatchIndex => {
            // `m[start, length]` slices the group list, the way `Array#[]`
            // does, rather than naming one group.
            if call.args.len() == 2 {
                let (Some(start), Some(length)) =
                    (first_arg(call).as_fixnum(), nth_arg(call, 1).as_fixnum())
                else {
                    return Err(Error::Raise {
                        class: "TypeError",
                        message: "no implicit conversion into Integer".to_owned(),
                    });
                };
                let count = groups.len() as i64;
                let from = if start < 0 { count + start } else { start };
                if from < 0 || from > count || length < 0 {
                    return Ok(Value::NIL);
                }
                let upto = (from + length).min(count);
                let elements: Vec<Value> = groups[from as usize..upto as usize]
                    .iter()
                    .map(|group| match group {
                        Some((s, e)) => text.part(scope, *s, *e),
                        None => Value::NIL,
                    })
                    .collect();
                return Ok(new_array(scope, &elements));
            }
            let index = match_group_index(scope, call, regexp)?;
            Ok(match groups.get(index).copied().flatten() {
                Some((start, end)) => text.part(scope, start, end),
                None => Value::NIL,
            })
        }

        _ => Err(Error::NoDispatch {
            op: "MatchData",
            operands: "a reader this slice does not have",
        }),
    }
}

/// The group a `MatchData` reader was asked for: a number, or a capture name.
fn match_group_index(
    scope: &mut HandleScope<'_>,
    call: &Pending,
    regexp: Value,
) -> Result<usize, Error> {
    let key = first_arg(call);
    if let Some(n) = key.as_fixnum() {
        // A negative index counts back from the end, as everywhere else.
        return usize::try_from(n).map_err(|_| Error::Raise {
            class: "IndexError",
            message: format!("index {n} out of matches"),
        });
    }
    // `m[:name]` and `m["name"]` both work in Ruby.
    let name = match key.as_symbol() {
        Some(id) => Some(symbol_name(id)),
        None => string_text(scope, key),
    };
    let Some(name) = name else {
        return Err(Error::Raise {
            class: "TypeError",
            message: format!(
                "no implicit conversion of {} into Integer",
                class_name_of(scope, key)
            ),
        });
    };
    let Some(program) = regexp_program(scope, regexp) else {
        return Err(Error::NoDispatch {
            op: "[]",
            operands: "a MatchData whose regexp is gone",
        });
    };
    let candidates = program.groups_named(&name);
    if candidates.is_empty() {
        return Err(Error::Raise {
            class: "IndexError",
            message: format!("undefined group name reference: {name}"),
        });
    }
    // One name may label several groups. Ruby answers the *farthest* one that
    // took part — "returns the last match when multiple named matches exist
    // with the same name" — so the search runs from the back.
    let matched = match_parts(scope, call.receiver)
        .map(|(_, _, groups)| groups)
        .and_then(|groups| {
            candidates
                .iter()
                .rev()
                .copied()
                .find(|&g| groups.get(g).copied().flatten().is_some())
        });
    Ok(matched.unwrap_or_else(|| candidates.last().copied().expect("just checked non-empty")))
}

// ---------------------------------------------------------------------------
// Bindings and string `eval` (#38)
// ---------------------------------------------------------------------------

/// A `Binding`'s slots. `BODY` names an `Iseq` whose `locals` and `outer`
/// name every environment from `ENV` outward — the caller's own at capture,
/// and the last `eval` that declared a local after it, so a later `eval`
/// through the same binding sees it.
const BINDING_BODY: usize = 0;
const BINDING_ENV: usize = 1;
const BINDING_SELF: usize = 2;
const BINDING_CREF: usize = 3;
const BINDING_BLOCK: usize = 4;
/// The frame a `return` inside an `eval` leaves: the captured frame's.
const BINDING_HOME: usize = 5;
const BINDING_FILE: usize = 6;
const BINDING_LINE: usize = 7;
/// The method the frame was running, for `__method__` and `super` in an
/// `eval`: its owner as a class object, and the name it was called by.
const BINDING_OWNER: usize = 8;
const BINDING_METHOD: usize = 9;
/// What a `def` in the string defaults to: the frame's `private` or
/// `module_function`, as a block would inherit it.
const BINDING_VISIBILITY: usize = 10;
const BINDING_SLOTS: u32 = 11;

/// The class `core/binding.rb` defines.
fn binding_class(scope: &mut HandleScope<'_>) -> Option<Value> {
    let object = Builtin::Object.id();
    let symbol = crate::shared::symbols::intern("Binding");
    scope.classes().const_get_here(object, symbol)
}

fn is_binding(scope: &mut HandleScope<'_>, value: Value) -> bool {
    let Some(class) = binding_class(scope).and_then(|c| class_id_of(scope, c)) else {
        return false;
    };
    if value.is_immediate() || class_of(scope, value) != Some(class) {
        return false;
    }
    let handle = scope.root(value);
    scope.payload(handle) == Payload::Slots && scope.len(handle) == BINDING_SLOTS
}

/// The local names of each environment from a binding's outward, innermost
/// first.
fn binding_chain(scope: &mut HandleScope<'_>, binding: Value) -> Vec<Vec<Box<str>>> {
    let handle = scope.root(binding);
    let body = scope.slot(handle, BINDING_BODY);
    match scope.definitions().get(body) {
        Some(Definition::Iseq(iseq)) => {
            let mut chain = vec![iseq.locals.clone()];
            chain.extend(iseq.outer.iter().cloned());
            chain
        }
        _ => Vec::new(),
    }
}

/// `(slot, depth)` of `name` in a binding, innermost wins.
fn binding_find(chain: &[Vec<Box<str>>], name: &str) -> Option<(usize, u16)> {
    chain.iter().enumerate().find_map(|(depth, names)| {
        let slot = names.iter().position(|n| &**n == name)?;
        Some((slot, depth as u16))
    })
}

/// Point a binding at a new innermost environment, named by `iseq`.
fn binding_push(scope: &mut HandleScope<'_>, binding: Value, iseq: &Arc<Iseq>, env: Value) {
    let body = scope
        .definitions_mut()
        .intern_iseq(iseq, Arc::as_ptr(iseq) as usize);
    let (binding, env) = (scope.root(binding), scope.root(env));
    let env = scope.get(env);
    scope.set_slot(binding, BINDING_BODY, body);
    scope.set_slot(binding, BINDING_ENV, env);
}

fn binding_native(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    call: &Pending,
    op: BindingOp,
    ids: &mut u64,
) -> Result<Option<Unwind>, Error> {
    if let BindingOp::Capture { caller } = op {
        let index = frames.len().checked_sub(if caller { 2 } else { 1 });
        let binding = capture_binding(scope, frames, index)?;
        stack.push(binding);
        return Ok(None);
    }
    binding_op(scope, stack, frames, call, op, ids)
}

/// A `Binding` of `frames[index]`: its locals, `self`, lexical scope, block,
/// and where it is.
fn capture_binding(
    scope: &mut HandleScope<'_>,
    frames: &[Call],
    index: Option<usize>,
) -> Result<Value, Error> {
    let Some(frame) = index.and_then(|index| frames.get(index)) else {
        return Err(Error::NoDispatch {
            op: "binding",
            operands: "no Ruby frame to capture",
        });
    };
    let line = frame.iseq.line_at(frame.pc.saturating_sub(1)).unwrap_or(0);
    let path = frame.iseq.path.clone();
    let (iseq, env, receiver, cref, block, home) = (
        Arc::clone(&frame.iseq),
        frame.env,
        frame.receiver,
        frame.cref,
        frame.block,
        frame.home,
    );
    let owner = frame
        .owner
        .map_or(Value::NIL, |owner| scope.classes().object(owner));
    let method = frame.defined_as.map_or(Value::NIL, Value::symbol);
    let visibility = match frame.scope_default {
        ScopeDefault::Public => 0,
        ScopeDefault::Private => 1,
        ScopeDefault::Protected => 2,
        ScopeDefault::ModuleFunction => 3,
    };
    let Some(class) = binding_class(scope) else {
        return Err(Error::Unknowable {
            what: "`binding` before `core/binding.rb` loads",
            needs: "the core library",
        });
    };
    let (env, receiver, block) = (scope.root(env), scope.root(receiver), scope.root(block));
    let class = scope.root(class);
    let file = match path {
        Some(path) => string_new(scope, &path),
        None => Value::NIL,
    };
    let file = scope.root(file);
    let handle = scope.alloc(Some(class), Payload::Slots, BINDING_SLOTS);
    let body = scope
        .definitions_mut()
        .intern_iseq(&iseq, Arc::as_ptr(&iseq) as usize);
    let values = [
        (BINDING_BODY, body),
        (BINDING_ENV, scope.get(env)),
        (BINDING_SELF, scope.get(receiver)),
        (BINDING_CREF, cref_value(cref)),
        (BINDING_BLOCK, scope.get(block)),
        (
            BINDING_HOME,
            Value::fixnum(home as i64).expect("a frame id fits in a fixnum"),
        ),
        (BINDING_FILE, scope.get(file)),
        (BINDING_OWNER, owner),
        (BINDING_METHOD, method),
        (
            BINDING_VISIBILITY,
            Value::fixnum(visibility).expect("small"),
        ),
        (
            BINDING_LINE,
            Value::fixnum(i64::from(line)).expect("a line number is a fixnum"),
        ),
    ];
    for (slot, value) in values {
        scope.set_slot(handle, slot, value);
    }
    Ok(scope.get(handle))
}

fn binding_op(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    call: &Pending,
    op: BindingOp,
    ids: &mut u64,
) -> Result<Option<Unwind>, Error> {
    let binding = call.receiver;
    if !is_binding(scope, binding) {
        return Err(Error::NoDispatch {
            op: "Binding",
            operands: "a receiver that is not a Binding",
        });
    }
    let handle = scope.root(binding);
    let symbol_arg = |index: usize| -> Option<String> {
        call.args
            .get(index)
            .and_then(|v| v.as_symbol())
            .and_then(crate::shared::symbols::name)
    };
    match op {
        BindingOp::Capture { .. } => unreachable!("answered above"),
        BindingOp::Receiver => {
            stack.push(scope.slot(handle, BINDING_SELF));
            Ok(None)
        }
        BindingOp::SetReceiver => {
            let receiver = call.args.first().copied().unwrap_or(Value::NIL);
            scope.set_slot(handle, BINDING_SELF, receiver);
            stack.push(binding);
            Ok(None)
        }
        BindingOp::Location => {
            let file = scope.slot(handle, BINDING_FILE);
            let line = scope.slot(handle, BINDING_LINE);
            let value = new_array(scope, &[file, line]);
            stack.push(value);
            Ok(None)
        }
        BindingOp::Names => {
            let chain = binding_chain(scope, binding);
            let mut seen: Vec<&str> = Vec::new();
            for name in chain.iter().flatten() {
                if spellable_local(name) && !seen.contains(&&**name) {
                    seen.push(name);
                }
            }
            let names: Vec<Value> = seen.iter().map(|n| Value::symbol(symbol(n))).collect();
            let value = new_array(scope, &names);
            stack.push(value);
            Ok(None)
        }
        BindingOp::Get => {
            let Some(name) = symbol_arg(0) else {
                return Err(Error::NoDispatch {
                    op: "local_variable_get",
                    operands: "a name that is not a Symbol",
                });
            };
            let chain = binding_chain(scope, binding);
            let value = match binding_find(&chain, &name) {
                Some((slot, depth)) => {
                    let env = scope.slot(handle, BINDING_ENV);
                    let env = env_outer(scope, env, depth);
                    let value = env_get(scope, env, slot);
                    new_array(scope, &[value])
                }
                None => Value::NIL,
            };
            stack.push(value);
            Ok(None)
        }
        BindingOp::Set => {
            let (Some(name), Some(&value)) = (symbol_arg(0), call.args.get(1)) else {
                return Err(Error::NoDispatch {
                    op: "local_variable_set",
                    operands: "a name that is not a Symbol",
                });
            };
            let chain = binding_chain(scope, binding);
            let env = scope.slot(handle, BINDING_ENV);
            if let Some((slot, depth)) = binding_find(&chain, &name) {
                let env = env_outer(scope, env, depth);
                env_set(scope, env, slot, value);
            } else {
                // A new local lives in an environment of its own inside the
                // captured one, so the frame it came from never sees it —
                // which is Ruby's rule — and a later `eval` through this
                // binding does.
                let body = scope.slot(handle, BINDING_BODY);
                let (label, level) = match scope.definitions().get(body) {
                    Some(Definition::Iseq(iseq)) => (iseq.name.clone(), iseq.block_level),
                    _ => ("<main>".into(), 0),
                };
                let iseq = Arc::new(Iseq {
                    name: label,
                    block_level: level,
                    locals: vec![name.into_boxed_str()],
                    outer: chain,
                    ..Iseq::default()
                });
                let value = scope.root(value);
                let layer = env_alloc(scope, env, 1);
                let value = scope.get(value);
                env_set(scope, layer, 0, value);
                binding_push(scope, binding, &iseq, layer);
            }
            stack.push(value);
            Ok(None)
        }
        BindingOp::Eval => binding_eval(scope, stack, frames, call, binding, ids),
    }
}

/// Whether a slot name is one Ruby can see: the compiler's own temporaries
/// start with `%`, and a numbered or `it` parameter is not a local.
fn spellable_local(name: &str) -> bool {
    let numbered = name.len() == 2 && name.starts_with('_') && name.as_bytes()[1].is_ascii_digit();
    !(name.starts_with('%') || numbered || name == "it")
}

fn binding_eval(
    scope: &mut HandleScope<'_>,
    stack: &[Value],
    frames: &mut Vec<Call>,
    call: &Pending,
    binding: Value,
    ids: &mut u64,
) -> Result<Option<Unwind>, Error> {
    let Some(parser) = scope.parser() else {
        return Err(Error::Unknowable {
            what: "string `eval`",
            needs: "a parser, which `spinel_core::boot` installs",
        });
    };
    // `Kernel#eval` and `Binding#eval` convert their arguments in Ruby first;
    // `instance_eval` and `class_eval` are natives, and calling `to_str` or
    // `to_int` from one is a frame it cannot sequence.
    // ponytail: a String or Integer only, here; the upgrade is a Ruby
    // `instance_eval` around a native that captures its caller's frame.
    let convertible = |scope: &mut HandleScope<'_>, index: usize, integer: bool| {
        call.args.get(index).is_none_or(|&v| {
            v == Value::NIL
                || if integer {
                    v.as_fixnum().is_some()
                } else {
                    is_string(scope, v)
                }
        })
    };
    if !convertible(scope, 0, false)
        || !convertible(scope, 1, false)
        || !convertible(scope, 2, true)
    {
        return Err(Error::Unknowable {
            what: "`instance_eval` or `class_eval` with an argument to convert",
            needs: "a Ruby `instance_eval` around the native",
        });
    }
    let source = call.args.first().and_then(|&v| string_bytes(scope, v));
    let Some(source) = source else {
        return Err(Error::NoDispatch {
            op: "eval",
            operands: "source that is not a String",
        });
    };
    let path = match call.args.get(1).and_then(|&v| string_bytes(scope, v)) {
        Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        // CRuby names an `eval` after where it was written.
        None => {
            let handle = scope.root(binding);
            let file = scope.slot(handle, BINDING_FILE);
            let line = scope.slot(handle, BINDING_LINE).as_fixnum().unwrap_or(0);
            match string_bytes(scope, file) {
                Some(file) => format!("(eval at {}:{line})", String::from_utf8_lossy(&file)),
                None => "(eval)".to_owned(),
            }
        }
    };
    let line = call.args.get(2).and_then(|v| v.as_fixnum()).unwrap_or(1);
    let handle = scope.root(binding);
    let in_method = scope.slot(handle, BINDING_OWNER) != Value::NIL;
    let program = match parser(&path, &source, line, in_method) {
        Ok(mut program) => {
            // With no magic comment, the string's own encoding is the source
            // encoding its literals take.
            let encoding = call.args.first().and_then(|&v| string_encoding(scope, v));
            if let Some(map) = Arc::get_mut(&mut program.source)
                && map.encoding.is_none()
                && let Some(encoding) = encoding.filter(|&e| e != crate::strings::UTF_8)
            {
                map.encoding = Some(crate::strings::name(encoding).into());
            }
            program
        }
        Err(crate::heap::ParseFailure::Syntax(message)) => {
            return Err(Error::raise("SyntaxError", message));
        }
        Err(crate::heap::ParseFailure::Unsupported) => {
            return Err(Error::Unknowable {
                what: "an `eval` string the parser cannot lower",
                needs: "the lowering to cover it",
            });
        }
    };
    let chain = binding_chain(scope, binding);
    // A backtrace names an `eval`'s frame after the body it runs in.
    let (name, level) = {
        let handle = scope.root(binding);
        let body = scope.slot(handle, BINDING_BODY);
        match scope.definitions().get(body) {
            Some(Definition::Iseq(iseq)) => (iseq.name.clone(), iseq.block_level),
            _ => ("<main>".into(), 0),
        }
    };
    let iseq = match crate::compile::eval(&program, chain, &name, level) {
        Ok(iseq) => Arc::new(iseq),
        Err(unsupported) => {
            return Err(Error::Unknowable {
                what: unsupported.node,
                needs: "the compiler to lower it inside an `eval`",
            });
        }
    };
    let handle = scope.root(binding);
    let outer = scope.slot(handle, BINDING_ENV);
    let receiver = scope.slot(handle, BINDING_SELF);
    let cref = cref_from(scope.slot(handle, BINDING_CREF));
    let block = scope.slot(handle, BINDING_BLOCK);
    let home = scope
        .slot(handle, BINDING_HOME)
        .as_fixnum()
        .map_or(0, |id| id as u64);
    // `super` and `__method__` inside the string answer for the method the
    // binding was taken in.
    let owner = scope.slot(handle, BINDING_OWNER);
    let owner = class_id_of(scope, owner);
    let defined_as = scope.slot(handle, BINDING_METHOD).as_symbol();
    let pending = Pending {
        cache: None,
        receiver,
        name: call.name,
        args: Vec::new(),
        keywords: Vec::new(),
        block,
        block_is_literal: false,
        cref,
        implicit_self: false,
        public_only: false,
        target: Target::Method,
        owner,
        defined_as,
    };
    *ids += 1;
    let scope_default = match scope.slot(handle, BINDING_VISIBILITY).as_fixnum() {
        Some(1) => ScopeDefault::Private,
        Some(2) => ScopeDefault::Protected,
        Some(3) => ScopeDefault::ModuleFunction,
        _ => ScopeDefault::Public,
    };
    let links = Links {
        id: *ids,
        home,
        breaks: 0,
        scope_default,
    };
    push_frame(
        scope,
        stack,
        frames,
        &pending,
        &iseq,
        outer,
        Binding::Loose,
        links,
    )?;
    // A local the string declared stays visible to the next `eval` through
    // this binding.
    if iseq.locals.iter().any(|name| spellable_local(name)) {
        let env = frames.last().expect("just pushed").env;
        binding_push(scope, binding, &iseq, env);
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Loading files and the file system (#39)
// ---------------------------------------------------------------------------

/// A path argument as the OS takes it.
fn path_arg(scope: &mut HandleScope<'_>, call: &Pending, index: usize) -> Result<PathBuf, Error> {
    let Some(bytes) = call.args.get(index).and_then(|&v| string_bytes(scope, v)) else {
        return Err(Error::NoDispatch {
            op: "a file system call",
            operands: "a path that is not a String",
        });
    };
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

/// An `io::Error` as the errno the Ruby side raises `SystemCallError` with.
fn errno_value(error: &std::io::Error) -> Value {
    let errno = error.raw_os_error().unwrap_or(libc::EIO);
    Value::fixnum(i64::from(errno)).expect("an errno is a fixnum")
}

fn fs_native(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    call: &Pending,
    op: FsOp,
) -> Result<Option<Unwind>, Error> {
    let value = match op {
        FsOp::Kind => {
            let path = path_arg(scope, call, 0)?;
            let follow = call.args.get(1).is_some_and(|v| v.is_truthy());
            let metadata = if follow {
                std::fs::metadata(&path)
            } else {
                std::fs::symlink_metadata(&path)
            };
            match metadata {
                Ok(m) if m.file_type().is_symlink() => Value::symbol(symbol("link")),
                Ok(m) if m.is_file() => Value::symbol(symbol("file")),
                Ok(m) if m.is_dir() => Value::symbol(symbol("directory")),
                Ok(_) => Value::symbol(symbol("other")),
                Err(_) => Value::NIL,
            }
        }
        FsOp::Realpath => {
            let path = path_arg(scope, call, 0)?;
            match std::fs::canonicalize(&path) {
                Ok(resolved) => os_string(scope, resolved.into_os_string()),
                Err(error) => errno_value(&error),
            }
        }
        FsOp::Access => {
            let path = path_arg(scope, call, 0)?;
            let mode = call.args.get(1).and_then(|v| v.as_fixnum()).unwrap_or(0);
            let allowed = std::ffi::CString::new(path.into_os_string().into_vec())
                .ok()
                .zip(i32::try_from(mode).ok())
                // SAFETY: `path` is a NUL-terminated string that outlives the
                // call, and `access` only reads it.
                .is_some_and(|(path, mode)| unsafe { libc::access(path.as_ptr(), mode) } == 0);
            bool_value(allowed)
        }
        FsOp::Isatty => {
            use std::io::IsTerminal as _;
            // The three standard streams: the only descriptors an `IO` holds
            // until #41 opens files.
            let tty = match call.args.first().and_then(|v| v.as_fixnum()) {
                Some(0) => std::io::stdin().is_terminal(),
                Some(1) => std::io::stdout().is_terminal(),
                Some(2) => std::io::stderr().is_terminal(),
                _ => false,
            };
            bool_value(tty)
        }
        FsOp::Getcwd => match std::env::current_dir() {
            Ok(dir) => os_string(scope, dir.into_os_string()),
            Err(error) => errno_value(&error),
        },
        FsOp::Chdir => {
            let path = path_arg(scope, call, 0)?;
            // ponytail: the working directory is the process's, shared by every
            // heap in it. One Ractor runs Ruby today; per-Ractor directories are
            // CRuby's rule too, so this stays process-wide when #117 lands.
            match std::env::set_current_dir(&path) {
                Ok(()) => Value::TRUE,
                Err(error) => errno_value(&error),
            }
        }
        FsOp::Constants => {
            let common: &[(&str, i32)] = &[
                ("RDONLY", libc::O_RDONLY),
                ("WRONLY", libc::O_WRONLY),
                ("RDWR", libc::O_RDWR),
                ("APPEND", libc::O_APPEND),
                ("CREAT", libc::O_CREAT),
                ("EXCL", libc::O_EXCL),
                ("NONBLOCK", libc::O_NONBLOCK),
                ("TRUNC", libc::O_TRUNC),
                ("NOCTTY", libc::O_NOCTTY),
                ("SYNC", libc::O_SYNC),
                ("DSYNC", libc::O_DSYNC),
                ("NOFOLLOW", libc::O_NOFOLLOW),
                ("LOCK_SH", libc::LOCK_SH),
                ("LOCK_EX", libc::LOCK_EX),
                ("LOCK_NB", libc::LOCK_NB),
                ("LOCK_UN", libc::LOCK_UN),
            ];
            #[cfg(target_os = "linux")]
            let platform: &[(&str, i32)] = &[
                ("DIRECT", libc::O_DIRECT),
                ("NOATIME", libc::O_NOATIME),
                ("RSYNC", libc::O_RSYNC),
                ("TMPFILE", libc::O_TMPFILE),
            ];
            #[cfg(not(target_os = "linux"))]
            let platform: &[(&str, i32)] = &[];
            let mut out = Vec::with_capacity(common.len() + platform.len());
            for &(name, value) in common.iter().chain(platform) {
                let name = string_new(scope, name);
                let name = scope.root(name);
                let value = Value::fixnum(i64::from(value)).expect("a flag is a fixnum");
                let name = scope.get(name);
                let pair = new_array(scope, &[name, value]);
                out.push(scope.root(pair));
            }
            let out: Vec<Value> = out.into_iter().map(|h| scope.get(h)).collect();
            new_array(scope, &out)
        }
        FsOp::Children => {
            let path = path_arg(scope, call, 0)?;
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    let mut names: Vec<std::ffi::OsString> = entries
                        .filter_map(|e| e.ok().map(|e| e.file_name()))
                        .collect();
                    names.sort();
                    let mut values = Vec::with_capacity(names.len());
                    for name in names {
                        let value = os_string(scope, name);
                        values.push(scope.root(value));
                    }
                    let values: Vec<Value> = values.into_iter().map(|h| scope.get(h)).collect();
                    new_array(scope, &values)
                }
                Err(error) => errno_value(&error),
            }
        }
        FsOp::Read => {
            let path = path_arg(scope, call, 0)?;
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let class = class_handle(scope, Builtin::String);
                    string_alloc(scope, class, &bytes, crate::strings::BINARY)
                }
                Err(error) => errno_value(&error),
            }
        }
    };
    stack.push(value);
    Ok(None)
}

/// A path the OS handed back, as a UTF-8 String.
fn os_string(scope: &mut HandleScope<'_>, text: std::ffi::OsString) -> Value {
    let class = class_handle(scope, Builtin::String);
    string_alloc(scope, class, &text.into_vec(), crate::strings::UTF_8)
}

/// `__load_file__(path, wrap, self)`: run a file's top level in a frame of
/// its own, with `self` as `main` unless a wrapped `load` made another.
/// The frame's value arrives when it leaves; a file that cannot be read is
/// a `LoadError`.
fn load_file(
    scope: &mut HandleScope<'_>,
    stack: &[Value],
    frames: &mut Vec<Call>,
    call: &Pending,
    ids: &mut u64,
) -> Result<Option<Unwind>, Error> {
    let Some(parser) = scope.parser() else {
        return Err(Error::Unknowable {
            what: "loading a file",
            needs: "a parser, which `spinel_core::boot` installs",
        });
    };
    let path = path_arg(scope, call, 0)?;
    let source = match std::fs::read(&path) {
        Ok(source) => source,
        Err(_) => {
            return Err(Error::raise(
                "LoadError",
                format!("cannot load such file -- {}", path.display()),
            ));
        }
    };
    let name = path.to_string_lossy().into_owned();
    let program = match parser(&name, &source, 1, false) {
        Ok(program) => program,
        Err(crate::heap::ParseFailure::Syntax(message)) => {
            return Err(Error::raise("SyntaxError", message));
        }
        Err(crate::heap::ParseFailure::Unsupported) => {
            return Err(Error::Unknowable {
                what: "a file the parser cannot lower",
                needs: "the lowering to cover it",
            });
        }
    };
    let iseq = match crate::compile::program_as(&program, "<top (required)>") {
        Ok(iseq) => Arc::new(iseq),
        Err(unsupported) => {
            return Err(Error::Unknowable {
                what: unsupported.node,
                needs: "the compiler to lower it in a required file",
            });
        }
    };
    // `load(file, true)`: the file's constants and methods land in an
    // anonymous module rather than at the top level.
    let wrap = call
        .args
        .get(1)
        .copied()
        .and_then(|module| class_id_of(scope, module));
    let cref = match wrap {
        Some(module) => scope.classes_mut().push_cref(CrefId::ROOT, module),
        None => CrefId::ROOT,
    };
    let receiver = match call.args.get(2) {
        Some(&receiver) if receiver != Value::NIL => receiver,
        _ => scope.main(),
    };
    let pending = Pending {
        cache: None,
        receiver,
        name: call.name,
        args: Vec::new(),
        keywords: Vec::new(),
        block: Value::NIL,
        block_is_literal: false,
        cref,
        implicit_self: false,
        public_only: false,
        target: Target::Method,
        owner: None,
        defined_as: None,
    };
    *ids += 1;
    let links = Links {
        id: *ids,
        home: *ids,
        breaks: 0,
        // A `def` at a file's top level is private, as in the main script.
        scope_default: ScopeDefault::Private,
    };
    push_frame(
        scope,
        stack,
        frames,
        &pending,
        &iseq,
        Value::NIL,
        Binding::Strict,
        links,
    )?;
    Ok(None)
}

// ---------------------------------------------------------------------------
// The process's identity and clocks (#145)
// ---------------------------------------------------------------------------

fn sys_native(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    call: &Pending,
    op: SysOp,
) -> Result<Option<Unwind>, Error> {
    let int = |n: i64| Value::fixnum(n).expect("an id or a clock reading is a fixnum");
    let value = match op {
        SysOp::Ids => {
            // SAFETY: these six take no arguments, cannot fail, and only read
            // the calling process's credentials.
            let ids = unsafe {
                [
                    i64::from(libc::getpid()),
                    i64::from(libc::getppid()),
                    i64::from(libc::getuid()),
                    i64::from(libc::geteuid()),
                    i64::from(libc::getgid()),
                    i64::from(libc::getegid()),
                ]
            };
            let values: Vec<Value> = ids.into_iter().map(int).collect();
            new_array(scope, &values)
        }
        SysOp::Clock => {
            let Some(id) = call
                .args
                .first()
                .and_then(|v| v.as_fixnum())
                .and_then(|id| libc::clockid_t::try_from(id).ok())
            else {
                return Err(Error::NoDispatch {
                    op: "__sys_clock__",
                    operands: "a clock id that is not an Integer",
                });
            };
            let mut now = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: `now` is a valid, writable `timespec` for the duration
            // of the call, and an unknown clock id is reported as EINVAL.
            let status = unsafe { libc::clock_gettime(id, &raw mut now) };
            if status == 0 {
                // `time_t` and `c_long` are `i64` on the 64-bit targets CI
                // builds and narrower elsewhere, so the conversion is kept.
                #[allow(clippy::useless_conversion)]
                let (seconds, nanoseconds) = (i64::from(now.tv_sec), i64::from(now.tv_nsec));
                new_array(scope, &[int(seconds), int(nanoseconds)])
            } else {
                let errno = std::io::Error::last_os_error();
                errno_value(&errno)
            }
        }
        SysOp::ProcessConstants => {
            // The rlimit resources are an `int` on some libcs and an unsigned
            // enum on glibc; the limits are `rlim_t`. Every value is small or
            // `RLIM_INFINITY`, which fits a fixnum's 62 bits only as a bignum,
            // so the values go through `bignum::value`.
            // `mut` only where the Linux-only pair below is added.
            #[allow(clippy::unnecessary_cast)]
            #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
            let mut pairs: Vec<(&str, i128)> = vec![
                ("WNOHANG", libc::WNOHANG as i128),
                ("WUNTRACED", libc::WUNTRACED as i128),
                ("PRIO_PROCESS", libc::PRIO_PROCESS as i128),
                ("PRIO_PGRP", libc::PRIO_PGRP as i128),
                ("PRIO_USER", libc::PRIO_USER as i128),
                ("RLIMIT_CPU", libc::RLIMIT_CPU as i128),
                ("RLIMIT_FSIZE", libc::RLIMIT_FSIZE as i128),
                ("RLIMIT_DATA", libc::RLIMIT_DATA as i128),
                ("RLIMIT_STACK", libc::RLIMIT_STACK as i128),
                ("RLIMIT_CORE", libc::RLIMIT_CORE as i128),
                ("RLIMIT_RSS", libc::RLIMIT_RSS as i128),
                ("RLIMIT_NPROC", libc::RLIMIT_NPROC as i128),
                ("RLIMIT_NOFILE", libc::RLIMIT_NOFILE as i128),
                ("RLIMIT_MEMLOCK", libc::RLIMIT_MEMLOCK as i128),
                ("RLIMIT_AS", libc::RLIMIT_AS as i128),
                ("RLIM_INFINITY", libc::RLIM_INFINITY as i128),
            ];
            #[cfg(target_os = "linux")]
            pairs.extend([
                ("RLIM_SAVED_MAX", libc::RLIM_SAVED_MAX as i128),
                ("RLIM_SAVED_CUR", libc::RLIM_SAVED_CUR as i128),
            ]);
            let mut out = Vec::with_capacity(pairs.len());
            for (name, value) in pairs {
                let name = string_new(scope, name);
                let name = scope.root(name);
                let value = crate::bignum::value(scope, &num_bigint::BigInt::from(value));
                let value = scope.root(value);
                let (name, value) = (scope.get(name), scope.get(value));
                let pair = new_array(scope, &[name, value]);
                out.push(scope.root(pair));
            }
            let out: Vec<Value> = out.into_iter().map(|h| scope.get(h)).collect();
            new_array(scope, &out)
        }
        SysOp::ClockIds => {
            let ids: &[(&str, libc::clockid_t)] = &[
                ("CLOCK_REALTIME", libc::CLOCK_REALTIME),
                ("CLOCK_MONOTONIC", libc::CLOCK_MONOTONIC),
                ("CLOCK_PROCESS_CPUTIME_ID", libc::CLOCK_PROCESS_CPUTIME_ID),
                ("CLOCK_THREAD_CPUTIME_ID", libc::CLOCK_THREAD_CPUTIME_ID),
            ];
            let mut pairs = Vec::with_capacity(ids.len());
            for &(name, id) in ids {
                let name = string_new(scope, name);
                let name = scope.root(name);
                let name = scope.get(name);
                let pair = new_array(scope, &[name, int(i64::from(id))]);
                pairs.push(scope.root(pair));
            }
            let pairs: Vec<Value> = pairs.into_iter().map(|h| scope.get(h)).collect();
            new_array(scope, &pairs)
        }
    };
    stack.push(value);
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::Iseq;

    /// Build an `Iseq` without the parser, so miri can run it.
    ///
    /// `tests/eval.rs` and `tests/bytecode.rs` cover far more, and both are
    /// skipped under miri because `spinel-parse` calls into Prism and Prism is
    /// C. This keeps the part miri is actually for — the heap's pointer
    /// arithmetic, reached here through a string literal, an array, and a slot
    /// read — inside the job's reach.
    fn iseq(insns: Vec<Insn>, literals: Vec<Literal>, max_stack: usize) -> Iseq {
        Iseq {
            name: "<test>".into(),
            insns,
            literals,
            symbols: vec!["a_symbol".into()],
            locals: vec!["slot".into()],
            max_stack,
            ..Iseq::default()
        }
    }

    /// The property the two-slot representation exists for: growth replaces the
    /// storage object and leaves the `Array` itself where it was, so `a << 1`
    /// is visible through every reference to `a`.
    #[test]
    fn growing_an_array_keeps_its_identity() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        scope.bootstrap();

        let array = new_array(&mut scope, &[]);
        let handle = scope.root(array);
        assert_eq!(array_len(&mut scope, handle), 0);

        // Past the first capacity, so storage is reallocated at least twice.
        for n in 0..40 {
            let value = Value::fixnum(n).expect("small");
            array_push(&mut scope, handle, value);
        }
        assert_eq!(array_len(&mut scope, handle), 40);
        // Same `Value`: the caller's reference still names this array.
        assert_eq!(scope.get(handle), array, "growth must not move the Array");
        for n in 0..40 {
            assert_eq!(
                array_get(&mut scope, handle, n as usize),
                Value::fixnum(n).expect("small"),
                "element {n} survived the copies"
            );
        }
    }

    /// `Array#[]` past the end is `nil`, not whatever the storage object still
    /// holds from before a `pop`.
    #[test]
    fn reading_past_the_length_is_nil_not_stale_storage() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        scope.bootstrap();

        let array = new_array(&mut scope, &[Value::fixnum(7).expect("small")]);
        let handle = scope.root(array);
        array_set_len(&mut scope, handle, 0);
        assert_eq!(array_get(&mut scope, handle, 0), Value::NIL);
    }

    /// Every value has a class, including the immediates. Before #15 four of
    /// them did not, and `nil.to_s` had nowhere to dispatch.
    #[test]
    fn every_immediate_has_a_class() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        scope.bootstrap();
        for (value, expected) in [
            (Value::NIL, Builtin::NilClass),
            (Value::TRUE, Builtin::TrueClass),
            (Value::FALSE, Builtin::FalseClass),
            (Value::fixnum(1).expect("small"), Builtin::Integer),
            (Value::flonum(1.5).expect("flonum"), Builtin::Float),
        ] {
            assert_eq!(
                class_of(&mut scope, value),
                Some(expected.id()),
                "{expected:?} is the class of {value:?}"
            );
        }
    }

    /// Measured against CRuby: the plain form inside `[1e-4, 1e15)` and the
    /// exponent form outside it, with a two-digit signed exponent.
    #[test]
    fn float_to_s_matches_rubys_shape() {
        for (value, expected) in [
            (1.0, "1.0"),
            (-0.0, "-0.0"),
            (0.0001, "0.0001"),
            (0.00001, "1.0e-05"),
            (1e14, "100000000000000.0"),
            (1e15, "1.0e+15"),
            (1e20, "1.0e+20"),
            (-1e15, "-1.0e+15"),
        ] {
            assert_eq!(float_to_s(value), expected, "{value}");
        }
    }

    #[test]
    fn the_interpreter_allocates_and_reads_under_miri() {
        let iseq = iseq(
            vec![
                // ["hi", :a_symbol] stored in a local, then read back.
                Insn::PushLit(0),
                Insn::PushSym(0),
                Insn::NewArray(2),
                Insn::SetLocal(0, 0),
                Insn::GetLocal(0, 0),
                Insn::Leave,
            ],
            vec![Literal::Str(Box::from(&b"hi"[..]), crate::strings::UTF_8)],
            3,
        );

        let mut heap = Heap::new();
        let mut frame = Frame::new(1);
        let mut scope = heap.scope();
        scope.bootstrap();
        let value = eval_in(&mut scope, &mut frame, &iseq).expect("should run");
        assert_eq!(inspect(&mut scope, value), "[\"hi\", :a_symbol]");
    }

    #[test]
    fn a_collection_mid_run_does_not_lose_the_stack() {
        // Everything the loop allocates is rooted in the scope it was handed, so
        // a collection between two allocations cannot free a value the stack is
        // still holding. Forcing one is the only way to check that claim.
        let iseq = iseq(
            vec![
                Insn::PushLit(0),
                Insn::PushLit(0),
                Insn::NewArray(2),
                Insn::Leave,
            ],
            vec![Literal::Str(
                Box::from(&b"survivor"[..]),
                crate::strings::UTF_8,
            )],
            3,
        );

        let mut heap = Heap::new();
        let mut frame = Frame::new(1);
        let mut scope = heap.scope();
        scope.bootstrap();
        scope.collect();
        let value = eval_in(&mut scope, &mut frame, &iseq).expect("should run");
        scope.collect();
        assert_eq!(inspect(&mut scope, value), "[\"survivor\", \"survivor\"]");
    }

    #[test]
    fn an_arity_error_carries_rubys_own_message() {
        // R9: ruby/spec asserts on this string, so it is measured against what
        // CRuby prints rather than invented here.
        //
        //   -> { m(1, 2) }.should raise_error(ArgumentError,
        //     "wrong number of arguments (given 2, expected 1)")
        let fixed = ParamSpec {
            required: vec![0],
            ..ParamSpec::default()
        };
        assert_eq!(
            message(&fixed, 2),
            "wrong number of arguments (given 2, expected 1)"
        );

        let splat = ParamSpec {
            required: vec![0, 1],
            rest: Some(2),
            ..ParamSpec::default()
        };
        assert_eq!(
            message(&splat, 1),
            "wrong number of arguments (given 1, expected 2+)"
        );

        let optional = ParamSpec {
            required: vec![0],
            optional: vec![crate::bytecode::Optional { slot: 1 }],
            ..ParamSpec::default()
        };
        assert_eq!(
            message(&optional, 3),
            "wrong number of arguments (given 3, expected 1..2)"
        );

        // A count inside the range is not an error at all.
        assert!(check_arity(&optional, 2).is_ok());
        assert!(check_arity(&splat, 9).is_ok());
    }

    /// The text `check_arity` puts in the raise, for the assertions above.
    fn message(spec: &ParamSpec, given: usize) -> String {
        match check_arity(spec, given) {
            Err(Error::Raise { message, .. }) => message,
            other => panic!("expected an ArgumentError, got {other:?}"),
        }
    }

    #[test]
    fn a_block_spreads_a_lone_array_only_when_it_has_room() {
        // The rule most of `block_spec.rb` is a table of: `{ |a| }` takes the
        // Array whole, `{ |a, b| }` spreads it, `{ |*a| }` wraps it.
        let one = ParamSpec {
            required: vec![0],
            ..ParamSpec::default()
        };
        let two = ParamSpec {
            required: vec![0, 1],
            ..ParamSpec::default()
        };
        let splat = ParamSpec {
            rest: Some(0),
            ..ParamSpec::default()
        };
        let trailing_comma = ParamSpec {
            required: vec![0],
            rest: Some(1),
            ..ParamSpec::default()
        };
        let one_optional = ParamSpec {
            optional: vec![crate::bytecode::Optional { slot: 0 }],
            ..ParamSpec::default()
        };
        assert!(!spreads(&one));
        assert!(spreads(&two));
        assert!(!spreads(&splat));
        assert!(spreads(&trailing_comma));
        assert!(!spreads(&one_optional));
    }

    #[test]
    fn a_call_pushes_a_frame_under_miri() {
        // The half of #11 miri is for: a frame's locals are a heap object, and
        // a call writes another heap object's slots through the binder. Built
        // by hand because `spinel-parse` calls into Prism and Prism is C.
        let callee = Arc::new(Iseq {
            name: "callee".into(),
            insns: vec![Insn::GetLocal(0, 0), Insn::Leave],
            locals: vec!["a".into()],
            max_stack: 1,
            params: ParamSpec {
                required: vec![0],
                ..ParamSpec::default()
            },
            scope_barrier: true,
            ..Iseq::default()
        });
        let caller = Iseq {
            name: "<test>".into(),
            insns: vec![Insn::PushSelf, Insn::PushInt(7), Insn::Send(0), Insn::Leave],
            symbols: vec!["callee".into()],
            call_sites: vec![crate::bytecode::CallSite {
                name: 0,
                argc: 1,
                splats: Vec::new(),
                keywords: Vec::new(),
                block: crate::bytecode::BlockRef::None,
                implicit_self: true,
                kwsplat: false,
            }],
            max_stack: 3,
            ..Iseq::default()
        };

        let mut heap = Heap::new();
        let mut frame = Frame::new(0);
        let mut scope = heap.scope();
        scope.bootstrap();
        let name = crate::shared::symbols::intern("callee");
        let body = scope
            .definitions_mut()
            .intern_iseq(&callee, Arc::as_ptr(&callee) as usize);
        scope
            .classes_mut()
            .define_method(Builtin::Object.id(), name, body);

        let value = eval_in(&mut scope, &mut frame, &caller).expect("should run");
        assert_eq!(inspect(&mut scope, value), "7");
    }

    #[test]
    fn arithmetic_that_leaves_the_fast_path_refuses() {
        for (op, left, right) in [
            (BinOp::Add, Value::TRUE, Value::fixnum(1).unwrap()),
            (BinOp::Lt, Value::NIL, Value::NIL),
        ] {
            let mut heap = Heap::new();
            let mut scope = heap.scope();
            scope.bootstrap();
            assert!(
                matches!(
                    binop(&mut scope, op, left, right),
                    Err(Error::NoDispatch { .. })
                ),
                "{op:?} should refuse rather than guess"
            );
        }
    }
}
