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

use std::sync::Arc;

use crate::bytecode::{
    BinOp, BlockRef, CallSite, CatchKind, ClassDef, ConstScope, DefKind, Insn, Iseq, Literal,
    ParamSpec,
};
use crate::class::Builtin;
use crate::class::{ClassId, CrefId, Kind};
use crate::heap::{Handle, HandleScope, Heap, Payload};
use crate::method::{Definition, Native};
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
    /// A method this heap has never been given.
    ///
    /// Deliberately *not* [`Error::Raise`], even though Ruby's answer for a
    /// missing method is a `NoMethodError` a `rescue` can catch. The two are
    /// indistinguishable from inside, and treating this one as catchable is
    /// actively worse than reporting it: `core/array/reject_spec.rb` writes
    ///
    /// ```ruby
    /// begin
    ///   a.reject! { |x| raise StandardError if x == 3 }
    /// rescue StandardError
    /// end
    /// a.should == [1, 3, 4]
    /// ```
    ///
    /// and with a catchable `NoMethodError` the `rescue` swallows "Spinel has
    /// no `reject!`", the example carries on down a branch Ruby never takes,
    /// and the comparison fails for a reason that has nothing to do with the
    /// behaviour under test. Before this slice nothing could catch anything, so
    /// such an example was reported blocked; it still is.
    ///
    /// The wording matches [`Error::Raise`]'s so the blocked-reason ranking
    /// that chooses the next slice reads the same as it did.
    NoSuchMethod { name: String, class: String },
    /// A loop that ran past its budget. Guards the harness against a spec that
    /// depends on a construct the compiler silently made non-terminating.
    Budget,
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
            Error::NoSuchMethod { name, class } => write!(
                f,
                "would raise NoMethodError: undefined method '{name}' for an instance of {class}"
            ),
            Error::Budget => write!(f, "ran past the instruction budget"),
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
const BUDGET: u64 = 50_000_000;

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
    /// This frame's own locals.
    env: Value,
    receiver: Value,
    /// The block this frame was called with, as a `Proc` or `nil`. Reached by
    /// `yield` and by `block_given?`, and never by a slot, so an anonymous
    /// block costs nothing.
    block: Value,
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
    /// Reasons parked by an `ensure` in this frame, innermost last.
    ///
    /// A `Vec` rather than one slot because an `ensure` body can contain
    /// another `begin`/`ensure`, and the outer reason has to survive the inner
    /// one running to completion.
    parked: Vec<Parked>,
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
        // `main`. A plain `Object` with no slots, as Ruby has it: `def` at the
        // top level lands on its class, and a receiverless call finds it there.
        let object = class_handle(scope, Builtin::Object);
        let handle = scope.alloc(Some(object), Payload::Slots, 0);
        frame.receiver = scope.get(handle);
    }

    // Rooted once rather than per allocation: `alloc` needs a handle to the
    // class, and taking one inside the loop would grow the root stack by an
    // entry per literal.
    let string_class = class_handle(scope, Builtin::String);
    let array_class = class_handle(scope, Builtin::Array);
    let proc_class = class_handle(scope, Builtin::Proc);

    let mut stack: Vec<Value> = Vec::with_capacity(iseq.max_stack);
    let mut frames: Vec<Call> = vec![Call {
        iseq: Arc::new(iseq.clone()),
        symbols: iseq.link(),
        env,
        receiver: frame.receiver,
        cref: frame.cref,
        block: Value::NIL,
        pc: 0,
        base: 0,
        keeps_receiver: false,
        // The outermost frame is its own `return` target and has no `break`
        // target: `return` at the top level ends the script, and `break` there
        // has no call to end.
        id: 1,
        home: 1,
        breaks: 0,
        tag: None,
        rescued: None,
        parked: Vec::new(),
    }];
    let mut budget = BUDGET;
    // Frame ids, handed out in order. Zero means "no such frame", which is what
    // a body with nowhere to `break` to carries.
    let mut ids: u64 = 1;

    let result = loop {
        budget = budget.checked_sub(1).ok_or(Error::Budget)?;
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

                Insn::Pop => {
                    stack.pop();
                }
                Insn::Dup => {
                    let value = *stack.last().expect("dup on an empty stack");
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

                Insn::BinOp(op) => {
                    let right = stack.pop().expect("binop on an empty stack");
                    let left = stack.pop().expect("binop on an empty stack");
                    match binop(scope, op, left, right) {
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
                                name: crate::shared::symbols::intern(op.name()),
                                receiver: left,
                                args: vec![right],
                                keywords: Vec::new(),
                                block: Value::NIL,
                                block_is_literal: false,
                                cref: frames[top].cref,
                                target: Target::Method,
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
                    stack.push(negate(value)?);
                }
                Insn::Not => {
                    let value = stack.pop().expect("not on an empty stack");
                    stack.push(bool_value(!value.is_truthy()));
                }

                Insn::NewArray(count) => {
                    let at = stack.len() - count as usize;
                    let handle = scope.alloc(Some(array_class), Payload::Slots, count);
                    for (index, value) in stack.drain(at..).enumerate() {
                        scope.set_slot(handle, index, value);
                    }
                    stack.push(scope.get(handle));
                }

                Insn::CaseEq => {
                    let condition = stack.pop().expect("caseeq on an empty stack");
                    let subject = stack.pop().expect("caseeq on an empty stack");
                    // `when c` asks `c === subject`, receiver first. For every type
                    // this slice has, `===` is `==`; for a Range, Class, Regexp, or
                    // Proc it is not, and those say so rather than guessing.
                    stack.push(bool_value(case_eq(scope, condition, subject)?));
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
                    let owner = scope.classes().cref_class(cref);
                    define_method_on(scope, owner, symbol, iseq, cref);
                    stack.push(Value::symbol(symbol));
                }

                Insn::DefineSingleton(index) => {
                    let (name, child) = frames[top].iseq.definitions[index as usize];
                    let iseq = Arc::clone(&frames[top].iseq.children[child as usize]);
                    let symbol = frames[top].symbols[name as usize];
                    let cref = frames[top].cref;
                    let receiver = stack.pop().expect("a receiver to define on");
                    let owner = singleton_of(scope, receiver)?;
                    // Ruby calls `singleton_method_added` on the receiver here.
                    // Spinel does not, and an object that defines the hook would
                    // silently never see it — so it refuses rather than defining
                    // the method and reporting a state the program did not reach.
                    // The check costs a method lookup only when one is written,
                    // and the hooks themselves belong to #28's reflection slice.
                    let hook = crate::shared::symbols::intern("singleton_method_added");
                    if scope.classes_mut().lookup(owner, hook).is_some() {
                        return Err(Error::Unknowable {
                            what: "`singleton_method_added`, which this definition would fire",
                            needs: "the definition hooks (#28)",
                        });
                    }
                    define_method_on(scope, owner, symbol, iseq, cref);
                    stack.push(Value::symbol(symbol));
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
                    // Assignment is an expression, and its value is what was
                    // assigned — not the module it landed on.
                    stack.push(value);
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
                    let value = defined_answer(scope, string_class, found.map(|_| "method"))?;
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
                    open_class(scope, &mut stack, &mut frames, def, &iseq, &mut ids)?;
                }

                Insn::Send(index) => {
                    let iseq = Arc::clone(&frames[top].iseq);
                    let site = &iseq.call_sites[index as usize];
                    let call = pop_call(scope, &mut stack, site, &frames[top], proc_class, true)?;
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

                Insn::JumpUnlessUndef(displacement) => {
                    if stack.pop().expect("jump on an empty stack") != Value::UNDEF {
                        frames[top].pc = jump(frames[top].pc, displacement);
                    }
                }

                Insn::Leave => {
                    let value = stack.pop().unwrap_or(Value::NIL);
                    let done = frames.pop().expect("a frame to leave");
                    stack.truncate(done.base);
                    if frames.is_empty() {
                        return Ok(Step::Done(value));
                    }
                    // `Class#new` left the object below the base; `initialize`'s own
                    // value is dropped.
                    if !done.keeps_receiver {
                        stack.push(value);
                    }
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

                Insn::EnterEnsure => {
                    let value = stack.pop().unwrap_or(Value::NIL);
                    frames[top].parked.push(Parked::Value(value));
                }

                Insn::LeaveEnsure => match frames[top].parked.pop().expect("an ensure to leave") {
                    Parked::Value(value) => stack.push(value),
                    Parked::Unwind(unwind) => return Ok(Step::Unwind(unwind)),
                },
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
            // report a missing feature as an exception a spec handled.
            Err(other) => return Err(other),
        };

        if let Some(value) = unwind_to_handler(scope, &mut stack, &mut frames, unwind)? {
            break value;
        }
    };

    Ok(result)
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
    unwind: Unwind,
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
                        stack.push(exception);
                    }
                    _ => unreachable!("a rescue entry only accepts an exception"),
                },
                CatchKind::Ensure => frames[top].parked.push(Parked::Unwind(unwind)),
            }
            frames[top].pc = entry.target as usize;
            return Ok(None);
        }

        // Nothing in this frame wanted it. The frame is done either way, and
        // its own `ensure`s have already run — they are entries in the table
        // that was just searched.
        let done = frames.pop().expect("a frame to unwind out of");
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
                return Ok(Some(value));
            }
            if !done.keeps_receiver {
                stack.push(value);
            }
            return Ok(None);
        }
        if frames.is_empty() {
            return Err(escaped(scope, unwind));
        }
    }
}

/// What an unwind that left the outermost frame is, as a reportable error.
fn escaped(scope: &mut HandleScope<'_>, unwind: Unwind) -> Error {
    match unwind {
        Unwind::Exception(exception) => Error::Uncaught {
            class: class_name_of(scope, exception),
            message: exception_message(scope, exception),
        },
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
}

/// One call, assembled from the stack and not yet dispatched.
struct Pending {
    name: SymbolId,
    receiver: Value,
    args: Vec<Value>,
    keywords: Vec<(SymbolId, Value)>,
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

    let mut keywords = Vec::with_capacity(site.keywords.len());
    for &name in site.keywords.iter().rev() {
        let value = stack.pop().expect("keyword on an empty stack");
        keywords.push((frame.symbols[name as usize], value));
    }
    keywords.reverse();

    let at = stack.len() - site.argc as usize;
    let mut args: Vec<Value> = stack.drain(at..).collect();
    if !site.splats.is_empty() {
        args = expand_splats(scope, args, &site.splats);
    }

    let receiver = if has_receiver {
        stack.pop().expect("send on an empty stack")
    } else {
        frame.receiver
    };

    Ok(Pending {
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
        target: Target::Method,
    })
}

/// `f(a, *b)`: splice the elements of the splatted arguments into the list.
///
/// Only the positions the call site marked. Expanding every array instead would
/// make `f(a, *b)` with an array `a` pass `a`'s elements as separate arguments,
/// which is a wrong answer rather than a missing feature.
///
// ponytail: a non-Array splat should call `to_a` first. Nothing has `to_a`
// until #15, and passing the value through is what an object with no `to_a`
// does anyway.
fn expand_splats(scope: &mut HandleScope<'_>, args: Vec<Value>, splats: &[u16]) -> Vec<Value> {
    let mut out = Vec::with_capacity(args.len());
    for (index, arg) in args.into_iter().enumerate() {
        match splats
            .contains(&(index as u16))
            .then(|| array_elements(scope, arg))
        {
            Some(Some(elements)) => out.extend(elements),
            _ => out.push(arg),
        }
    }
    out
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
        Target::Method => {
            let class = class_of(scope, call.receiver).ok_or_else(|| no_class(call.receiver))?;
            let found = scope.classes_mut().lookup(class, call.name);
            // R8: an unknown method raises rather than answering `nil`. The
            // harness reports a statement that merely evaluates as a passing
            // effect, so a `nil` here would turn every matcher this VM does not
            // implement into a spec that passes without asserting anything.
            let Some(method) = found else {
                return Err(Error::NoSuchMethod {
                    name: symbol_name(call.name).to_string(),
                    class: scope
                        .classes()
                        .name(class)
                        .unwrap_or("an anonymous class")
                        .to_owned(),
                });
            };
            match scope.definitions().get(method.body).cloned() {
                Some(Definition::Iseq(iseq)) => {
                    // The body resolves constants against the scope its `def`
                    // was written in, which is not the caller's and need not be
                    // reachable from `owner`'s ancestors.
                    let call = Pending {
                        cref: method.cref,
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
                Some(Definition::Native(native)) => {
                    native_call(scope, stack, frames, call, native, proc_class, ids)
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
    let Some((iseq, env, receiver, captured, lambda, cref)) = proc_parts(scope, block) else {
        return Err(Error::NoDispatch {
            op: "call",
            operands: "a receiver that is not a Proc",
        });
    };
    let call = Pending {
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
        target: Target::Method,
    };
    // A block takes both links from the `Proc`: `return` leaves the method it
    // was *written* in, `break` ends the call it was *passed* to. A lambda is a
    // method as far as `return` is concerned, so it homes to itself.
    let (home, breaks) = proc_links(scope, block);
    *ids += 1;
    let links = Links {
        id: *ids,
        home: if lambda { *ids } else { home },
        breaks,
    };
    let binding = if lambda {
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
    bind(scope, env, &iseq.params, &symbols, call, binding)?;
    frames.push(Call {
        iseq: Arc::clone(iseq),
        symbols,
        env,
        receiver: call.receiver,
        cref: call.cref,
        block: call.block,
        pc: 0,
        base: stack.len(),
        keeps_receiver: false,
        id: links.id,
        home: links.home,
        breaks: links.breaks,
        tag: None,
        rescued: None,
        parked: Vec::new(),
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
                needs: "the binder can call Ruby, which re-entrant primitives bring with fibers (#40)",
            });
        }
    }

    if lambda {
        check_arity(spec, args.len())?;
    }

    let required = spec.required as usize;
    let post = spec.post as usize;
    let optional = spec.optional.len();

    // Required from the left, post-required from the right, optionals from
    // whatever is left in between. Ruby's order, and the reason `post` is
    // counted separately rather than added to `required`.
    let available = args.len();
    let leading = required.min(available);
    for (slot, value) in args.iter().take(leading).enumerate() {
        env_set(scope, env, slot, *value);
    }
    // Ruby pads a block's missing required parameters with `nil`; a lambda
    // never gets here, because `check_arity` refused first.
    for slot in leading..required {
        env_set(scope, env, slot, Value::NIL);
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

    let post_base = required + optional + usize::from(spec.rest.is_some());
    for index in 0..post {
        let value = if index < trailing {
            args[post_from + index]
        } else {
            Value::NIL
        };
        env_set(scope, env, post_base + index, value);
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
    let places = spec.required as usize + spec.optional.len() + spec.post as usize;
    places > 1 || (spec.rest.is_some() && places > 0)
}

fn bind_keywords(
    scope: &mut HandleScope<'_>,
    env: Value,
    spec: &ParamSpec,
    symbols: &[SymbolId],
    call: &Pending,
) -> Result<(), Error> {
    // A callee that declares no keywords at all does not *reject* them: Ruby
    // packs them into a trailing positional Hash. There is no Hash, so this is
    // not yet dispatchable — and saying `ArgumentError: unknown keyword` here
    // would claim Ruby raises where Ruby does not, which is the one thing the
    // blocked report must never do.
    if spec.keywords.is_empty() && !call.keywords.is_empty() {
        return Err(Error::NoDispatch {
            op: "a keyword argument",
            operands: "a method with no keyword parameters, which needs a Hash",
        });
    }

    for keyword in &spec.keywords {
        let name = symbols[keyword.name as usize];
        let supplied = call.keywords.iter().find(|(k, _)| *k == name);
        match (supplied, keyword.required) {
            (Some((_, value)), _) => env_set(scope, env, keyword.slot as usize, *value),
            (None, true) => {
                return Err(Error::raise(
                    "ArgumentError",
                    format!("missing keyword: :{}", symbol_name(name)),
                ));
            }
            (None, false) => env_set(scope, env, keyword.slot as usize, Value::UNDEF),
        }
    }
    // An unknown keyword is an error in Ruby unless the method collects them,
    // and `**kw` is not compiled, so anything left over is unknown.
    for (name, _) in &call.keywords {
        if !spec
            .keywords
            .iter()
            .any(|k| symbols[k.name as usize] == *name)
        {
            return Err(Error::raise(
                "ArgumentError",
                format!("unknown keyword: :{}", symbol_name(*name)),
            ));
        }
    }
    Ok(())
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

/// An exception is two slots: what it says, and where it came from.
///
/// Slots rather than instance variables because instance variables are
/// [#151](https://github.com/ar4mirez/spinel/issues/151)'s shape tree and do not
/// exist yet — the same reason a `Proc` is slots. When they do, this becomes
/// two ivars and the accessors below become ordinary Ruby.
const EXC_MESSAGE: usize = 0;
/// Always `nil`. A real backtrace needs source positions the compiler does not
/// record, and `[]` would be a plausible-but-wrong answer rather than an absent
/// one — see the non-goals in PRD 0012.
const EXC_BACKTRACE: usize = 1;
const EXC_SLOTS: u32 = 2;

/// A Ruby `String` holding `text`.
fn string_new(scope: &mut HandleScope<'_>, text: &str) -> Value {
    let class = class_handle(scope, Builtin::String);
    let len = u32::try_from(text.len()).unwrap_or(u32::MAX);
    let handle = scope.alloc(Some(class), Payload::Bytes, len);
    scope
        .bytes_mut(handle)
        .copy_from_slice(&text.as_bytes()[..len as usize]);
    scope.get(handle)
}

/// An instance of `class`, an already-resolved exception class object.
fn exception_of(scope: &mut HandleScope<'_>, class: Value, message: &str) -> Value {
    // The message is allocated first and stays rooted in the scope, so the
    // allocation below cannot collect it out from under the slot write.
    let text = string_new(scope, message);
    let class = scope.root(class);
    let handle = scope.alloc(Some(class), Payload::Slots, EXC_SLOTS);
    scope.set_slot(handle, EXC_MESSAGE, text);
    scope.set_slot(handle, EXC_BACKTRACE, Value::NIL);
    scope.get(handle)
}

/// An instance of the exception class `class` names at the top level.
///
/// This is what turns every [`Error::Raise`] the VM has emitted since #11 into
/// something a `rescue` can catch. The class name and the message text were
/// already correct — measured against CRuby where ruby/spec asserts on them —
/// so nothing here has to rediscover Ruby's wording.
fn exception_new(scope: &mut HandleScope<'_>, class: &str, message: &str) -> Value {
    let symbol = crate::shared::symbols::intern(class);
    let object = scope
        .classes()
        .const_get_here(Builtin::Object.id(), symbol)
        .expect("every class the VM raises is bootstrapped");
    exception_of(scope, object, message)
}

/// An exception's message, for a report. Empty when it is not one.
fn exception_message(scope: &mut HandleScope<'_>, exception: Value) -> String {
    if exception.is_immediate() {
        return String::new();
    }
    let handle = scope.root(exception);
    if scope.payload(handle) != Payload::Slots || scope.len(handle) < EXC_SLOTS {
        return String::new();
    }
    let text = scope.slot(handle, EXC_MESSAGE);
    if text.is_immediate() {
        return String::new();
    }
    let text = scope.root(text);
    if scope.payload(text) != Payload::Bytes {
        return String::new();
    }
    String::from_utf8_lossy(scope.bytes(text)).into_owned()
}

/// The name of a value's class, for a report.
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
    let class = scope.class(handle)?;
    (class == scope.classes().object(Builtin::Proc.id())).then(|| scope.slot(handle, PROC_BODY))
}

/// Everything needed to call a `Proc`.
fn proc_parts(
    scope: &mut HandleScope<'_>,
    value: Value,
) -> Option<(Arc<Iseq>, Value, Value, Value, bool, CrefId)> {
    let body = proc_body(scope, value)?;
    let iseq = match scope.definitions().get(body)? {
        Definition::Iseq(iseq) => Arc::clone(iseq),
        Definition::Native(_) => return None,
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
fn open_class(
    scope: &mut HandleScope<'_>,
    stack: &mut Vec<Value>,
    frames: &mut Vec<Call>,
    def: &ClassDef,
    iseq: &Arc<Iseq>,
    ids: &mut u64,
) -> Result<(), Error> {
    let top = frames.len() - 1;
    let outer = frames[top].cref;

    let id = match def.kind {
        DefKind::Singleton => {
            let object = stack.pop().expect("an object to open the singleton of");
            singleton_of(scope, object)?
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
                false => scope.classes().cref_class(outer),
            };
            let name = frames[top].symbols[def.name as usize];
            define_or_reopen(scope, cbase, name, kind, superclass)?
        }
    };

    let cref = scope.classes_mut().push_cref(outer, id);
    let receiver = scope.classes().object(id);
    let body = Arc::clone(&iseq.children[def.body as usize]);
    let env = env_alloc(scope, Value::NIL, body.locals.len());
    let symbols = body.link();
    *ids += 1;
    let links = Links {
        id: *ids,
        home: *ids,
        breaks: 0,
    };
    frames.push(Call {
        iseq: body,
        symbols,
        env,
        receiver,
        cref,
        // A class body is not called with a block, so `yield` inside one is a
        // `LocalJumpError` — which is Ruby.
        block: Value::NIL,
        pc: 0,
        base: stack.len(),
        keeps_receiver: false,
        // A class body is its own `return` target, like a method: `return` in
        // one is a LocalJumpError in Ruby, and homing it here keeps the
        // unwinder from walking out into the enclosing method instead.
        id: links.id,
        home: links.id,
        breaks: 0,
        tag: None,
        rescued: None,
        parked: Vec::new(),
    });
    Ok(())
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
fn define_or_reopen(
    scope: &mut HandleScope<'_>,
    cbase: ClassId,
    name: SymbolId,
    kind: DefKind,
    superclass: Option<Value>,
) -> Result<ClassId, Error> {
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
        return Ok(id);
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
    Ok(id)
}

/// `Module#name`: `"A::B"` inside `A`, and `"B"` at the top level.
fn qualified_name(scope: &mut HandleScope<'_>, cbase: ClassId, name: SymbolId) -> String {
    let leaf = symbol_name(name);
    match scope.classes().name(cbase) {
        Some(outer) if cbase != Builtin::Object.id() => format!("{outer}::{leaf}"),
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
fn class_name(scope: &mut HandleScope<'_>, value: Value) -> String {
    class_of(scope, value)
        .and_then(|id| scope.classes().name(id).map(str::to_string))
        .unwrap_or_else(|| inspect(scope, value))
}

/// Define a method, remembering the scope its `def` was written in.
fn define_method_on(
    scope: &mut HandleScope<'_>,
    owner: ClassId,
    name: SymbolId,
    iseq: Arc<Iseq>,
    cref: CrefId,
) {
    let body = scope
        .definitions_mut()
        .intern_iseq(&iseq, Arc::as_ptr(&iseq) as usize);
    scope
        .classes_mut()
        .define_method_in(owner, name, body, cref);
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
    if receiver.is_immediate() {
        // Ruby's text exactly: no receiver in it, and no article. `nil`, `true`,
        // and `false` are *not* here — they answer `NilClass` and friends, which
        // this VM will have once `core/*.rb` does (#15); until then they fall
        // through to `class_of` and report themselves undispatchable.
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
        // a `NameError` would name.
        ConstScope::Lexical => Ok(scope.classes().cref_class(cref)),
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
            Error::raise(
                "NameError",
                format!("uninitialized constant {owner}::{constant}"),
            )
        }
    }
}

/// `defined?`'s answer for a *name*: the string, or a report that this heap
/// cannot tell "undefined" from "never loaded".
///
/// Ruby answers `nil` for a name that is not defined. Spinel runs each example
/// in a fresh heap holding only the bootstrap classes, so a miss means either
/// "genuinely undefined" — Ruby's `nil` — or "defined in a file `require` has
/// not landed to load", and nothing here can tell them apart. Answering `nil`
/// would pass `defined?(SomeFixture).should be_nil` while the VM had simply
/// never heard of the fixture: a wrong answer wearing a passing spec.
///
/// So a miss is [`Error::NoDispatch`] — *not yet knowable* rather than *no* —
/// which `spec/harness` reports as blocked. It becomes `nil` on its own when
/// [#39](https://github.com/ar4mirez/spinel/issues/39) can load the file that
/// would have defined the name. This is R8 of PRD 0011 one layer down: an
/// unknown name raises rather than answering `nil`.
fn defined_answer<'h>(
    scope: &mut HandleScope<'h>,
    string_class: Handle<'h>,
    answer: Option<&str>,
) -> Result<Value, Error> {
    let Some(answer) = answer else {
        return Err(Error::Unknowable {
            what: "`defined?` of a name this heap has never seen",
            needs: "`require` can load the file that would define it (#39)",
        });
    };
    Ok(defined_word(scope, string_class, Some(answer)))
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
    let handle = scope.alloc(Some(string_class), Payload::Bytes, answer.len() as u32);
    scope.bytes_mut(handle).copy_from_slice(answer.as_bytes());
    scope.get(handle)
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
        // NilClass, TrueClass, FalseClass and Float are not in the bootstrap
        // set; #13 creates them with the rest of the constant table.
        Unpacked::Nil | Unpacked::True | Unpacked::False | Unpacked::Flonum(_) => None,
        Unpacked::Undef => None,
        Unpacked::Heap(_) => {
            let handle = scope.root(value);
            scope.class_of(handle)
        }
    }
}

fn symbol_name(id: SymbolId) -> String {
    crate::shared::symbols::name(id).unwrap_or_else(|| format!("<symbol {}>", id.0))
}

/// The elements of an `Array`, or `None` if the value is not one.
fn array_elements(scope: &mut HandleScope<'_>, value: Value) -> Option<Vec<Value>> {
    if heap_kind(scope, value) != Some(HeapKind::Array) {
        return None;
    }
    let handle = scope.root(value);
    Some(
        (0..scope.len(handle) as usize)
            .map(|index| scope.slot(handle, index))
            .collect(),
    )
}

fn new_array(scope: &mut HandleScope<'_>, elements: &[Value]) -> Value {
    let class = class_handle(scope, Builtin::Array);
    let handle = scope.alloc(Some(class), Payload::Slots, elements.len() as u32);
    for (index, value) in elements.iter().enumerate() {
        scope.set_slot(handle, index, *value);
    }
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
        Native::Send => {
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
                // `send` forwards whatever block it was given, and how that
                // block was written travels with it.
                block_is_literal: call.block_is_literal,
                name,
                receiver: call.receiver,
                args,
                keywords: call.keywords,
                block: call.block,
                cref: call.cref,
                target: Target::Method,
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
            // A bootstrap class other than `Object` has a representation this
            // cannot build: a `Proc` is six slots, a `String` is bytes, and a
            // bare zero-slot object wearing their class is a value every
            // primitive on them would then misread — `Proc.new` used to reach
            // `Proc#lambda?` and index past the end of the object. `Object` and
            // `BasicObject` really are plain, so they are allowed.
            // An exception class is allocatable: `raise ArgumentError.new("x")`
            // and `rescue Klass => e` are everywhere in the corpus, and the
            // representation is two slots this module owns rather than one
            // `core/*.rb` has yet to define.
            if is_exception_class(scope, id) {
                // ...unless CRuby gives it an `initialize` of its own, which
                // Spinel does not have. `SignalException.new(:NOSIG)` raises
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
                let message = match call.args.first() {
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
            let plain = matches!(
                Builtin::ALL.get(id.index()),
                None | Some(Builtin::Object | Builtin::BasicObject)
            );
            if !plain {
                return Err(Error::Unknowable {
                    what: "`new` on a built-in class",
                    needs: "`core/*.rb` defines how one is allocated (#15)",
                });
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
            let class = scope.classes().object(id);
            let class = scope.root(class);
            // ponytail: no instance slots, because instance variables are
            // #151's shape tree. A zero-slot object is enough to have an
            // identity, a class, and singleton methods, which is what this
            // slice's specs ask of it.
            let handle = scope.alloc(Some(class), Payload::Slots, 0);
            let object = scope.get(handle);
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

        Native::Raise => {
            let exception = raise_argument(scope, frames, &call.args)?;
            Ok(Some(Unwind::Exception(exception)))
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
                    let handle = scope.alloc(Some(class), Payload::Slots, 0);
                    scope.get(handle)
                }
            };
            let block = call.block;
            let inner = Pending {
                name: call.name,
                receiver: block,
                args: vec![tag],
                keywords: Vec::new(),
                block: Value::NIL,
                block_is_literal: false,
                cref: call.cref,
                target: Target::Block(block),
            };
            push_proc_frame(scope, stack, frames, &inner, block, ids)?;
            let last = frames.len() - 1;
            frames[last].tag = Some(tag);
            Ok(None)
        }

        Native::ExceptionMessage => {
            let message = exception_message(scope, call.receiver);
            let value = string_new(scope, &message);
            stack.push(value);
            Ok(None)
        }

        Native::ExceptionBacktrace => {
            stack.push(Value::NIL);
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

    // A String is a RuntimeError with that message.
    if let Some(text) = string_bytes(scope, first) {
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
/// Ruby would spread it across a block's parameters.
fn defines_to_ary(scope: &mut HandleScope<'_>, value: Value) -> bool {
    let Some(class) = class_of(scope, value) else {
        return false;
    };
    let name = crate::shared::symbols::intern("to_ary");
    scope.classes_mut().lookup(class, name).is_some()
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
    let handle = scope.root(value);
    if scope.payload(handle) != Payload::Bytes {
        return None;
    }
    let class = scope.class(handle)?;
    (class == scope.classes().object(Builtin::String.id())).then(|| scope.bytes(handle).to_vec())
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
            &["send", "__send__", "public_send"],
            Native::Send,
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
        (
            Builtin::Exception,
            &["message", "to_s"],
            Native::ExceptionMessage,
        ),
        (
            Builtin::Exception,
            &["backtrace"],
            Native::ExceptionBacktrace,
        ),
        (Builtin::Kernel, &["equal?"], Native::Equal),
        (Builtin::Kernel, &["nil?"], Native::NilP),
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
        Literal::BigInt(_) => Err(Error::NoDispatch {
            op: "Integer",
            operands: "a value wider than a fixnum",
        }),
        Literal::Str(bytes) => {
            let len = u32::try_from(bytes.len()).map_err(|_| Error::NoDispatch {
                op: "String",
                operands: "a literal larger than 4 GiB",
            })?;
            let handle = scope.alloc(Some(string_class), Payload::Bytes, len);
            scope.bytes_mut(handle).copy_from_slice(bytes);
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

    let (Some(left), Some(right)) = (num(left), num(right)) else {
        return Err(Error::NoDispatch {
            op: op.name(),
            operands: "operands that are not both numbers",
        });
    };

    match (left, right) {
        (Num::Int(a), Num::Int(b)) => integer_op(op, a, b),
        // Ruby promotes to Float when either side is one.
        (a, b) => float_op(op, as_float(a), as_float(b)),
    }
}

fn as_float(n: Num) -> f64 {
    match n {
        Num::Int(i) => i as f64,
        Num::Float(f) => f,
    }
}

fn integer_op(op: BinOp, a: i64, b: i64) -> Result<Value, Error> {
    let overflow = || Error::NoDispatch {
        op: op.name(),
        operands: "integers that overflow a fixnum",
    };
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
    // An Integer that leaves fixnum range promotes to a bignum, and there is no
    // bignum. Refusing is right; wrapping would be a wrong answer.
    value.and_then(Value::fixnum).ok_or_else(overflow)
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
        BinOp::Mod => float(a - b * (a / b).floor()),
        BinOp::Lt => Ok(bool_value(a < b)),
        BinOp::Le => Ok(bool_value(a <= b)),
        BinOp::Gt => Ok(bool_value(a > b)),
        BinOp::Ge => Ok(bool_value(a >= b)),
        BinOp::Eq | BinOp::Neq => unreachable!("handled before the numeric path"),
    }
}

fn negate(value: Value) -> Result<Value, Error> {
    let fail = || Error::NoDispatch {
        op: "-@",
        operands: "an operand that is not a number",
    };
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
    // Bitwise equality is exactly Ruby's `equal?` for immediates, which is why
    // #6 excluded NaN and -0.0 from the flonum range. It settles most pairs.
    if left == right {
        return Ok(true);
    }
    // `1 == 1.0` is true in Ruby even though the words differ.
    if let (Some(a), Some(b)) = (num(left), num(right)) {
        return Ok(as_float(a) == as_float(b));
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
            Ok(scope.bytes(a) == scope.bytes(b))
        }
        Some(HeapKind::Array) => {
            if heap_kind(scope, right) != Some(HeapKind::Array) {
                return Ok(false);
            }
            let (a, b) = (scope.root(left), scope.root(right));
            if scope.len(a) != scope.len(b) {
                return Ok(false);
            }
            for index in 0..scope.len(a) as usize {
                let (x, y) = (scope.slot(a, index), scope.slot(b, index));
                if !ruby_eq(scope, x, y)? {
                    return Ok(false);
                }
            }
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
    // A nested scope so the handle pops immediately. This runs once per `==`,
    // and a loop that compares a thousand times would otherwise leave a
    // thousand roots behind until the whole evaluation ended.
    let mut nested = scope.nested();
    let handle = nested.root(value);
    let class = nested.class(handle)?;
    let classes = nested.classes();
    if class == classes.object(Builtin::String.id()) {
        Some(HeapKind::Str)
    } else if class == classes.object(Builtin::Array.id()) {
        Some(HeapKind::Array)
    } else {
        None
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
        Unpacked::Heap(_) => match heap_kind(scope, value) {
            Some(HeapKind::Str) => {
                let handle = scope.root(value);
                format!("{:?}", String::from_utf8_lossy(scope.bytes(handle)))
            }
            Some(HeapKind::Array) => {
                let handle = scope.root(value);
                let items: Vec<String> = (0..scope.len(handle) as usize)
                    .map(|index| {
                        let item = scope.slot(handle, index);
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
            vec![Literal::Str(Box::from(&b"hi"[..]))],
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
            vec![Literal::Str(Box::from(&b"survivor"[..]))],
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
            required: 1,
            ..ParamSpec::default()
        };
        assert_eq!(
            message(&fixed, 2),
            "wrong number of arguments (given 2, expected 1)"
        );

        let splat = ParamSpec {
            required: 2,
            rest: Some(2),
            ..ParamSpec::default()
        };
        assert_eq!(
            message(&splat, 1),
            "wrong number of arguments (given 1, expected 2+)"
        );

        let optional = ParamSpec {
            required: 1,
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
            required: 1,
            ..ParamSpec::default()
        };
        let two = ParamSpec {
            required: 2,
            ..ParamSpec::default()
        };
        let splat = ParamSpec {
            rest: Some(0),
            ..ParamSpec::default()
        };
        let trailing_comma = ParamSpec {
            required: 1,
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
                required: 1,
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
