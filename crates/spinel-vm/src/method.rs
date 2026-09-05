//! What a method *is*: the table behind [`Method::body`].
//!
//! [#8](https://github.com/ar4mirez/spinel/issues/8) left `Method::body` as an
//! opaque [`Value`] because there was no bytecode yet. This is what it points
//! at: an index into a per-heap table whose entries are either an [`Iseq`] or
//! one of the handful of operations Ruby cannot define in Ruby.
//!
//! # Why a fixnum id and not a heap object
//!
//! A heap object would need a payload kind that can hold an `Arc<Iseq>` and a
//! finaliser to drop one, and the heap has neither — [`Payload`] is slots or
//! bytes, and the collector sweeps without running destructors. A definition id
//! is a fixnum, so the collector never has to trace a method body at all, and
//! the table it indexes is per-heap and dropped with the heap.
//!
//! [`Method::body`]: crate::class::Method::body
//! [`Payload`]: crate::heap::Payload

use std::sync::Arc;

use crate::bytecode::Iseq;
use crate::value::Value;

/// An operation the VM performs itself.
///
/// engine.md's rule for what becomes a primitive is "raw memory, allocation,
/// encoding tables, syscalls, **dispatch**, and anything the JIT needs as an
/// intrinsic". Every entry here is dispatch: calling a block, forwarding a call
/// under another name, or reading a `Proc`'s own shape. The rest of `Kernel`
/// is Ruby and waits for
/// [#15](https://github.com/ar4mirez/spinel/issues/15).
///
/// An enum rather than a function pointer because two of these — [`Native::Call`]
/// and [`Native::Send`] — do not *return* a value, they push a frame, and a
/// function that could do that would need the whole interpreter as an argument.
/// The loop matches on this instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Native {
    /// `Proc#call`, and its aliases `()`, `[]`, and `yield`. Pushes a frame.
    Call,
    /// `Object#send`, `__send__`, `public_send`. Re-dispatches under the name
    /// in the first argument. Pushes a frame when the target is Ruby.
    Send,
    /// `Kernel#proc`, `Kernel#lambda`, `Proc.new`. Returns the block it was
    /// passed; `lambda` also marks it one.
    MakeProc { lambda: bool },
    /// `Proc#lambda?`
    IsLambda,
    /// `Proc#arity`
    Arity,
    /// `Kernel#block_given?`
    BlockGiven,
    /// `Object#class`
    ClassOf,
    /// `Class#new`: allocate, then run `initialize` if there is one.
    ///
    /// A primitive because it is allocation and dispatch, which `docs/engine.md`
    /// reserves for Rust. Everything else `Class` will answer is `core/*.rb`'s.
    New,
    /// `Object#equal?` — identity, which for this VM is `Value` equality.
    Equal,
    /// `Array#+`: a new array with the two joined. Allocation, so Rust.
    ArrayPlus,
    /// Read slot `n` of the receiver. What `attr_reader` becomes when
    /// `core/*.rb` can ask for one.
    Getter(u16),
    /// Write slot `n` of the receiver, answering the value written.
    Setter(u16),
    /// `Kernel#raise` and `Kernel#fail`. Does not return a value: it hands the
    /// interpreter an unwind, which is why it lives here and not in Ruby.
    Raise,
    /// `Kernel#throw`. The other primitive that unwinds.
    Throw,
    /// `Kernel#catch`. Pushes a frame for the block and marks it as the
    /// boundary a matching `throw` stops at.
    Catch,
    /// `Exception#message` and `#to_s`.
    ExceptionMessage,
    /// `Exception#backtrace`. Always `nil` — see PRD 0012's non-goals.
    ExceptionBacktrace,
    /// `Object#frozen?`, `Object#nil?`, `Object#!`. Cheap predicates the target
    /// specs reach for while checking something else.
    NilP,
}

/// A method body.
#[derive(Debug, Clone)]
pub enum Definition {
    /// Compiled Ruby. `Arc` because the same body is reachable from the class
    /// table and from the `Iseq` that defined it, and because phase 3 shares
    /// bytecode between Ractors.
    Iseq(Arc<Iseq>),
    Native(Native),
}

/// One heap's method bodies, indexed by the fixnum in [`Method::body`].
///
/// Append-only within a heap: redefining a method points the class table at a
/// new entry rather than mutating one, so a frame already running the old body
/// keeps running it. That is Ruby's rule — redefining a method mid-call does
/// not rewrite the call in flight.
///
/// [`Method::body`]: crate::class::Method::body
#[derive(Debug, Default)]
pub struct Definitions {
    entries: Vec<Definition>,
    /// Body id per `Arc<Iseq>` address, so evaluating a block literal in a loop
    /// interns one definition rather than one per iteration. The `Iseq` is kept
    /// alive by the `Iseq` that owns it as a child, which outlives the frame
    /// that could look it up.
    interned: std::collections::HashMap<usize, Value>,
}

impl Definitions {
    #[must_use]
    pub fn new() -> Definitions {
        Definitions {
            entries: Vec::new(),
            interned: std::collections::HashMap::new(),
        }
    }

    /// The body id for a compiled `Iseq`, added once per distinct `Iseq`.
    ///
    /// `key` is the `Arc`'s address. Without the memo, `10.times { }` would add
    /// a definition per iteration and the table would grow with the loop.
    ///
    /// An address is only a safe key because the entry it points at holds a
    /// clone of the same `Arc`: the `Iseq` cannot be dropped while the table
    /// remembers it, so its address cannot be reused by a different one. That
    /// is an invariant of `add` below, not a coincidence — a memo that stored
    /// the id without keeping the `Arc` would eventually answer with the wrong
    /// method body.
    pub fn intern_iseq(&mut self, iseq: &Arc<Iseq>, key: usize) -> Value {
        if let Some(&body) = self.interned.get(&key) {
            return body;
        }
        let body = self.add(Definition::Iseq(Arc::clone(iseq)));
        self.interned.insert(key, body);
        body
    }

    /// Add a definition and return the [`Value`] that names it.
    ///
    /// # Panics
    ///
    /// If a heap ever holds more definitions than a fixnum can index, which is
    /// 2^62 of them.
    pub fn add(&mut self, definition: Definition) -> Value {
        let id = self.entries.len();
        self.entries.push(definition);
        Value::fixnum(id as i64).expect("a definition id fits a fixnum")
    }

    #[must_use]
    pub fn get(&self, body: Value) -> Option<&Definition> {
        let id = body.as_fixnum()?;
        self.entries.get(usize::try_from(id).ok()?)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_definition_id_round_trips_and_is_not_a_heap_value() {
        let mut defs = Definitions::new();
        let body = defs.add(Definition::Native(Native::Arity));
        // The whole reason for a fixnum body: the collector never traces it.
        assert!(body.is_immediate());
        assert!(matches!(
            defs.get(body),
            Some(Definition::Native(Native::Arity))
        ));
    }

    #[test]
    fn redefining_leaves_the_old_body_reachable() {
        // A frame already running the old body holds its id, and that id must
        // keep resolving after the class table has moved on.
        let mut defs = Definitions::new();
        let old = defs.add(Definition::Native(Native::Arity));
        let new = defs.add(Definition::Native(Native::IsLambda));
        assert_ne!(old, new);
        assert!(matches!(
            defs.get(old),
            Some(Definition::Native(Native::Arity))
        ));
    }
}
