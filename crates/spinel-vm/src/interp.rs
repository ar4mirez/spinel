//! The interpreter loop.
//!
//! Non-recursive from the first commit, because engine.md requires it: a
//! Ruby-to-Ruby call pushes a frame and continues the same loop rather than
//! recursing on the Rust stack, which is what fibers and Ruby's own recursion
//! limits depend on. There is exactly one frame until
//! [#11](https://github.com/ar4mirez/spinel/issues/11) gives it a caller, but
//! the shape is the one that scales.
//!
//! # Rooting
//!
//! Every value the loop puts on its stack is either an immediate or an object
//! allocated through the [`HandleScope`] it was handed, so the collector can see
//! all of them for as long as the scope lives.
//!
//! ponytail: that also means an object allocated inside a loop is not reclaimed
//! until the whole evaluation ends — the scope only pops on drop. The real fix
//! is the interpreter's value stack and frames as root sources in `Heap::mark`,
//! which [#7](https://github.com/ar4mirez/spinel/issues/7) already shaped
//! `shade` to accept. It belongs with #11, where a frame stops being a local
//! variable of this function and becomes a thing with a lifetime.

use crate::bytecode::{BinOp, Insn, Iseq, Literal};
use crate::class::Builtin;
use crate::heap::{Handle, HandleScope, Heap, Payload};
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
    Raise { class: &'static str },
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
            Error::Raise { class } => write!(f, "would raise {class}"),
            Error::Budget => write!(f, "ran past the instruction budget"),
        }
    }
}

impl std::error::Error for Error {}

/// How many instructions one evaluation may run before it is assumed stuck.
///
/// Generous enough that no ruby/spec example approaches it, small enough that a
/// non-terminating loop is a reported failure rather than a hung run.
const BUDGET: u64 = 50_000_000;

/// One call frame: the locals of a scope.
///
/// The harness keeps one across an example's statements, which is how `a = 1` in
/// the first line is visible to `a.should == 1` in the last without the VM
/// knowing what a matcher is.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Slot-indexed. Ruby reads a declared-but-unassigned local as `nil`, which
    /// is exactly what a frame of `nil`s gives for free.
    locals: Vec<Value>,
    /// ponytail: top-level `self` is the `main` object, which needs
    /// `Object.new` and a constant table — #13's. Nothing this slice compiles
    /// can observe the difference; `PushSelf` exists so #11 does not add an
    /// instruction, not because it works.
    receiver: Value,
}

impl Frame {
    /// A frame with `slots` locals, all `nil`.
    #[must_use]
    pub fn new(slots: usize) -> Frame {
        Frame {
            locals: vec![Value::NIL; slots],
            receiver: Value::NIL,
        }
    }

    /// Grow to hold at least `slots` locals, keeping the ones already set.
    ///
    /// The harness compiles an example's statements separately against one
    /// shared slot map, and a later statement may be the first to mention a
    /// local; the frame follows rather than being rebuilt.
    pub fn reserve(&mut self, slots: usize) {
        if self.locals.len() < slots {
            self.locals.resize(slots, Value::NIL);
        }
    }

    #[must_use]
    pub fn local(&self, slot: usize) -> Option<Value> {
        self.locals.get(slot).copied()
    }
}

/// Compile-and-run's other half: run `iseq` in a fresh frame on `heap`.
pub fn eval(heap: &mut Heap, iseq: &Iseq) -> Result<Value, Error> {
    let mut frame = Frame::new(iseq.locals.len());
    let mut scope = heap.scope();
    eval_in(&mut scope, &mut frame, iseq)
}

/// Run `iseq` in `frame`, which may already hold locals from an earlier run.
pub fn eval_in(
    scope: &mut HandleScope<'_>,
    frame: &mut Frame,
    iseq: &Iseq,
) -> Result<Value, Error> {
    frame.reserve(iseq.locals.len());
    let symbols = iseq.link();

    // Rooted once rather than per allocation: `alloc` needs a handle to the
    // class, and taking one inside the loop would grow the root stack by an
    // entry per literal.
    let string_class = class_handle(scope, Builtin::String);
    let array_class = class_handle(scope, Builtin::Array);

    let mut stack: Vec<Value> = Vec::with_capacity(iseq.max_stack);
    let mut pc: usize = 0;
    let mut budget = BUDGET;

    loop {
        budget = budget.checked_sub(1).ok_or(Error::Budget)?;
        let insn = iseq.insns[pc];
        pc += 1;

        match insn {
            Insn::PushNil => stack.push(Value::NIL),
            Insn::PushTrue => stack.push(Value::TRUE),
            Insn::PushFalse => stack.push(Value::FALSE),
            Insn::PushSelf => stack.push(frame.receiver),
            Insn::PushInt(n) => {
                stack.push(Value::fixnum(n).ok_or(Error::NoDispatch {
                    op: "Integer",
                    operands: "a value wider than a fixnum",
                })?);
            }
            Insn::PushLit(index) => {
                let value = materialise(scope, &iseq.literals[index as usize], string_class)?;
                stack.push(value);
            }
            Insn::PushSym(index) => stack.push(Value::symbol(symbols[index as usize])),

            Insn::Pop => {
                stack.pop();
            }
            Insn::Dup => {
                let top = *stack.last().expect("dup on an empty stack");
                stack.push(top);
            }

            Insn::GetLocal(slot) => {
                stack.push(frame.locals[slot as usize]);
            }
            Insn::SetLocal(slot) => {
                frame.locals[slot as usize] = stack.pop().expect("setlocal on an empty stack");
            }

            Insn::Jump(displacement) => pc = jump(pc, displacement),
            Insn::JumpUnless(displacement) => {
                if !stack.pop().expect("jump on an empty stack").is_truthy() {
                    pc = jump(pc, displacement);
                }
            }
            Insn::JumpIf(displacement) => {
                if stack.pop().expect("jump on an empty stack").is_truthy() {
                    pc = jump(pc, displacement);
                }
            }
            Insn::JumpUnlessKeep(displacement) => {
                if !stack.last().expect("jump on an empty stack").is_truthy() {
                    pc = jump(pc, displacement);
                }
            }
            Insn::JumpIfKeep(displacement) => {
                if stack.last().expect("jump on an empty stack").is_truthy() {
                    pc = jump(pc, displacement);
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

            Insn::Leave => return Ok(stack.pop().unwrap_or(Value::NIL)),
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
                return Err(Error::Raise {
                    class: "ZeroDivisionError",
                });
            }
            floor_div(a, b)
        }
        BinOp::Mod => {
            if b == 0 {
                return Err(Error::Raise {
                    class: "ZeroDivisionError",
                });
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
                Insn::SetLocal(0),
                Insn::GetLocal(0),
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
