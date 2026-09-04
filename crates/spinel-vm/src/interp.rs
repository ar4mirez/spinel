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

use crate::bytecode::{BinOp, BlockRef, CallSite, Insn, Iseq, Literal, ParamSpec};
use crate::class::Builtin;
use crate::class::ClassId;
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
    /// A loop that ran past its budget. Guards the harness against a spec that
    /// depends on a construct the compiler silently made non-terminating.
    Budget,
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
            Error::Budget => write!(f, "ran past the instruction budget"),
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
}

impl Frame {
    /// A frame with room for `slots` locals, all `nil`.
    #[must_use]
    pub fn new(slots: usize) -> Frame {
        Frame {
            env: Value::NIL,
            slots,
            receiver: Value::NIL,
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
    /// The block a `Proc` frame runs with is the one its *defining* frame had,
    /// which is what makes `yield` inside a block reach the method's block.
    pc: usize,
    /// Where this frame's operands start in the shared value stack.
    base: usize,
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
        block: Value::NIL,
        pc: 0,
        base: 0,
    }];
    let mut budget = BUDGET;

    let result = loop {
        budget = budget.checked_sub(1).ok_or(Error::Budget)?;
        let top = frames.len() - 1;
        let insn = frames[top].iseq.insns[frames[top].pc];
        frames[top].pc += 1;

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
                stack.push(binop(scope, op, left, right)?);
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
                );
                stack.push(value);
            }

            Insn::DefineMethod(index) => {
                let (name, child) = frames[top].iseq.definitions[index as usize];
                let iseq = Arc::clone(&frames[top].iseq.children[child as usize]);
                let symbol = frames[top].symbols[name as usize];
                let receiver = stack.pop().expect("definemethod on an empty stack");
                define_method(scope, receiver, symbol, iseq)?;
                stack.push(Value::symbol(symbol));
            }

            Insn::Send(index) => {
                let iseq = Arc::clone(&frames[top].iseq);
                let site = &iseq.call_sites[index as usize];
                let call = pop_call(scope, &mut stack, site, &frames[top], proc_class, true)?;
                dispatch(scope, &mut stack, &mut frames, call, proc_class)?;
            }

            Insn::Yield(index) => {
                let iseq = Arc::clone(&frames[top].iseq);
                let site = &iseq.call_sites[index as usize];
                // The block is a field of the frame rather than a slot, so an
                // anonymous block costs nothing and `yield` needs no name.
                let block = frames[top].block;
                let mut call = pop_call(scope, &mut stack, site, &frames[top], proc_class, false)?;
                if block == Value::NIL {
                    return Err(Error::raise("LocalJumpError", "no block given (yield)"));
                }
                call.receiver = block;
                call.target = Target::Block(block);
                dispatch(scope, &mut stack, &mut frames, call, proc_class)?;
            }

            Insn::JumpUnlessUndef(displacement) => {
                if stack.pop().expect("jump on an empty stack") != Value::UNDEF {
                    frames[top].pc = jump(frames[top].pc, displacement);
                }
            }

            Insn::Leave | Insn::Return => {
                let value = stack.pop().unwrap_or(Value::NIL);
                let done = frames.pop().expect("a frame to leave");
                stack.truncate(done.base);
                if frames.is_empty() {
                    break value;
                }
                stack.push(value);
            }
        }
    };

    Ok(result)
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
const PROC_SLOTS: u32 = 5;

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
) -> Result<(), Error> {
    match call.target {
        Target::Block(block) => push_proc_frame(scope, stack, frames, &call, block),
        Target::Method => {
            let class = class_of(scope, call.receiver).ok_or_else(|| no_class(call.receiver))?;
            let found = scope.classes_mut().lookup(class, call.name);
            // R8: an unknown method raises rather than answering `nil`. The
            // harness reports a statement that merely evaluates as a passing
            // effect, so a `nil` here would turn every matcher this VM does not
            // implement into a spec that passes without asserting anything.
            let Some(method) = found else {
                return Err(Error::raise(
                    "NoMethodError",
                    format!(
                        "undefined method '{}' for an instance of {}",
                        symbol_name(call.name),
                        scope.classes().name(class).unwrap_or("an anonymous class"),
                    ),
                ));
            };
            match scope.definitions().get(method.body).cloned() {
                Some(Definition::Iseq(iseq)) => {
                    push_frame(scope, stack, frames, &call, &iseq, Value::NIL, false)
                }
                Some(Definition::Native(native)) => {
                    native_call(scope, stack, frames, call, native, proc_class)
                }
                None => unreachable!("a method body that is not in the definition table"),
            }
        }
    }
}

/// Call a `Proc`: its own body, its captured environment, its own `self`.
fn push_proc_frame(
    scope: &mut HandleScope<'_>,
    stack: &[Value],
    frames: &mut Vec<Call>,
    call: &Pending,
    block: Value,
) -> Result<(), Error> {
    let Some((iseq, env, receiver, captured, lambda)) = proc_parts(scope, block) else {
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
        target: Target::Method,
    };
    push_frame(scope, stack, frames, &call, &iseq, env, lambda)
}

/// Bind the arguments and push the frame.
fn push_frame(
    scope: &mut HandleScope<'_>,
    stack: &[Value],
    frames: &mut Vec<Call>,
    call: &Pending,
    iseq: &Arc<Iseq>,
    outer: Value,
    lambda: bool,
) -> Result<(), Error> {
    let env = env_alloc(scope, outer, iseq.locals.len());
    let symbols = iseq.link();
    bind(scope, env, &iseq.params, &symbols, call, lambda)?;
    frames.push(Call {
        iseq: Arc::clone(iseq),
        symbols,
        env,
        receiver: call.receiver,
        block: call.block,
        pc: 0,
        base: stack.len(),
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
    lambda: bool,
) -> Result<(), Error> {
    let mut args = call.args.clone();

    // A block with room for more than one value spreads a single Array across
    // its parameters; `{ |a| }` and `{ |*a| }` do not. This is most of what
    // `block_spec.rb` checks.
    if !lambda && args.len() == 1 && spreads(spec) {
        if let Some(elements) = array_elements(scope, args[0]) {
            args = elements;
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
    scope.get(handle)
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
) -> Option<(Arc<Iseq>, Value, Value, Value, bool)> {
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
    ))
}

/// `def` at the top level defines a private method on `Object`, which is where
/// Ruby puts it and what makes `def foo; end; foo` work.
fn define_method(
    scope: &mut HandleScope<'_>,
    receiver: Value,
    name: SymbolId,
    iseq: Arc<Iseq>,
) -> Result<(), Error> {
    let owner = if receiver == Value::NIL {
        Builtin::Object.id()
    } else {
        class_of(scope, receiver).ok_or_else(|| no_class(receiver))?
    };
    let body = scope
        .definitions_mut()
        .intern_iseq(&iseq, Arc::as_ptr(&iseq) as usize);
    scope.classes_mut().define_method(owner, name, body);
    Ok(())
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
) -> Result<(), Error> {
    match native {
        // Two that push a frame rather than returning a value, which is why
        // `Native` is an enum the loop matches and not a function pointer.
        Native::Call => push_proc_frame(scope, stack, frames, &call, call.receiver),
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
                name,
                receiver: call.receiver,
                args,
                keywords: call.keywords,
                block: call.block,
                target: Target::Method,
            };
            dispatch(scope, stack, frames, forwarded, proc_class)
        }

        Native::MakeProc { lambda } => {
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
            Ok(())
        }
        Native::IsLambda => {
            let handle = scope.root(call.receiver);
            let lambda = scope.slot(handle, PROC_LAMBDA);
            stack.push(lambda);
            Ok(())
        }
        Native::Arity => {
            let Some((iseq, ..)) = proc_parts(scope, call.receiver) else {
                return Err(Error::NoDispatch {
                    op: "arity",
                    operands: "a receiver that is not a Proc",
                });
            };
            stack.push(Value::fixnum(iseq.params.arity()).expect("an arity fits a fixnum"));
            Ok(())
        }
        Native::BlockGiven => {
            // The block of the frame that called `block_given?`, which is the
            // one still on top: a primitive does not push a frame.
            let block = frames.last().map_or(Value::NIL, |f| f.block);
            stack.push(bool_value(block != Value::NIL));
            Ok(())
        }
        Native::ClassOf => {
            let class = class_of(scope, call.receiver).ok_or_else(|| no_class(call.receiver))?;
            stack.push(scope.classes().object(class));
            Ok(())
        }
        Native::Equal => {
            let other = call.args.first().copied().unwrap_or(Value::NIL);
            stack.push(bool_value(call.receiver == other));
            Ok(())
        }
        Native::NilP => {
            stack.push(bool_value(call.receiver == Value::NIL));
            Ok(())
        }
    }
}

/// `lambda { }` given a block: the same body, marked as a lambda.
fn relambda<'h>(
    scope: &mut HandleScope<'h>,
    proc_class: Handle<'h>,
    block: Value,
) -> Result<Value, Error> {
    let Some((iseq, env, receiver, captured, _)) = proc_parts(scope, block) else {
        return Err(Error::NoDispatch {
            op: "lambda",
            operands: "a block that is not a Proc",
        });
    };
    Ok(make_proc(
        scope, proc_class, &iseq, env, receiver, captured, true,
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
            None => "#<object>".to_owned(),
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
