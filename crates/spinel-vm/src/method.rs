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
use crate::value::{SymbolId, Value};

/// Which of `Object`'s four instance-variable reflection methods is being run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IvarOp {
    Get,
    Set,
    Defined,
    /// `Object#instance_variables`, in the order the object acquired them.
    Names,
}

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
    MakeProc {
        lambda: bool,
    },
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
    /// Read slot `n` of the receiver. A fixed slot, so only for the built-ins
    /// whose representation *is* fixed: `MatchData`'s regexp and subject.
    Getter(u16),
    /// Write slot `n` of the receiver, answering the value written.
    Setter(u16),
    /// Read the receiver's instance variable, by name. What `attr_reader`
    /// defines.
    ///
    /// By name and not by slot, because which slot an instance variable lands
    /// at is the object's shape's business: an `attr_reader` built on slot 0
    /// would be right for a class with one ivar and wrong for its second. #15
    /// left `attr_accessor` out rather than ship that.
    IvarReader(SymbolId),
    /// Write the receiver's instance variable, by name, answering the value
    /// written. What `attr_writer` defines.
    IvarWriter(SymbolId),
    /// `Module#attr_reader`, `#attr_writer`, `#attr_accessor`. Defines an
    /// [`Native::IvarReader`], an [`Native::IvarWriter`], or both per name, and
    /// answers the array of symbols it defined — which is what Ruby 3.0 does.
    AttrDefine {
        reader: bool,
        writer: bool,
    },
    /// `Object#instance_variable_get`, `#instance_variable_set`,
    /// `#instance_variable_defined?` and `#instance_variables`.
    InstanceVariable(IvarOp),
    /// `Kernel#raise` and `Kernel#fail`. Does not return a value: it hands the
    /// interpreter an unwind, which is why it lives here and not in Ruby.
    Raise,
    /// `Kernel#throw`. The other primitive that unwinds.
    Throw,
    /// `Kernel#catch`. Pushes a frame for the block and marks it as the
    /// boundary a matching `throw` stops at.
    Catch,
    /// `Regexp#=~` — the character offset the match began at, or nil.
    RegexpMatchOp,
    /// `Regexp#match` — a `MatchData`, or nil.
    RegexpMatch,
    /// `Regexp#match?` — a boolean, and the one matcher that leaves `$~` alone.
    RegexpMatchP,
    /// `Regexp#===`, which is what `when /re/` runs.
    RegexpCaseEq,
    /// `Regexp#source`
    RegexpSource,
    /// `Regexp#options`
    RegexpOptions,
    /// `Regexp#to_s` (`(?-mix:foo)`) and `#inspect` (`/foo/`), which differ.
    RegexpToS {
        inspect: bool,
    },
    /// `String#=~`, `String#match`, `String#match?` — the same three matchers
    /// with the operands the other way round.
    StringMatchOp,
    StringMatch,
    StringMatchP,
    /// `MatchData#[]`, by group number or by capture name.
    MatchIndex,
    /// `MatchData#to_a` and `#captures`, which differ only in whether the whole
    /// match is the first element.
    MatchToA {
        captures: bool,
    },
    /// `MatchData#pre_match` and `#post_match`.
    MatchAround {
        post: bool,
    },
    /// `MatchData#begin` and `#end`, in characters.
    MatchEdge {
        end: bool,
    },
    /// `MatchData#size` and `#length`.
    MatchSize,
    /// `MatchData#names` — the capture names the pattern declares, in group
    /// order and without repeats. What `MatchData#inspect` branches on.
    MatchNames,
    /// `Object#frozen?`, `Object#nil?`, `Object#!`. Cheap predicates the target
    /// specs reach for while checking something else.
    NilP,

    // -- #15's core library. Each one is raw memory or allocation; everything
    // -- else about these classes is Ruby, in `core/*.rb`.
    /// `Array#[]` — reads a raw slot run.
    ArrayIndex,
    /// `Array#[]=` — writes one, and reallocates storage past the end.
    ArrayStore,
    /// `Array#size` — reads the length slot.
    ArraySize,
    /// `Array#push` — writes a raw slot, reallocating storage when full.
    ArrayPush,
    /// `Array#pop` — writes the length back.
    ArrayPop,
    /// `String#length` and `#size` (characters), and `#bytesize` (bytes).
    /// One primitive, because both read the same byte payload's length; they
    /// differ only in whether the bytes are decoded first.
    StringSize {
        bytes: bool,
    },
    /// `String#+` — allocates a byte payload.
    StringConcat,
    /// `String#*` — allocates a byte payload.
    StringRepeat,
    /// `Class#allocate` — allocation, and the shape is per class.
    Allocate,
    /// `Object#dup` — allocates a copy of a cell. `Array` overrides it in Ruby,
    /// because a shallow copy of an `Array` would share its storage object.
    Dup,
    /// `Object#freeze` — sets a header flag bit.
    Freeze,
    /// `Object#frozen?` — reads it.
    FrozenP,
    /// `Object#object_id` — the object's address.
    ObjectId,
    /// `Integer#<<`, `#>>`, `#&`, `#|`, `#^`, `#~` — fixnum bit patterns, which
    /// the JIT wants as intrinsics.
    IntBits(BitOp),
    /// `Integer#**` — repeated multiplication with an overflow check, so the
    /// answer is a refusal rather than a wrapped one.
    IntPow,
    /// `Symbol#to_s`, `#name`, `#length` — reads the shared symbol table.
    SymbolName {
        length: bool,
    },
    /// `Module#name`, `Module#to_s` — reads the class table.
    ModuleName,
    /// `Object#hash` — a fixnum that is equal whenever `==` is.
    ///
    /// Content for a `String` and an `Array`, identity for everything else,
    /// which is Ruby's own default. A primitive because it reads raw bytes and
    /// raw slots, and because a `Hash` keyed on it wants it as an intrinsic.
    HashValue,
    /// `Module#include` and `Module#prepend` — splices a module into the
    /// ancestor chain, which is a write to the class table.
    ///
    /// Without it `core/comparable.rb` is unreachable: `include Comparable` is
    /// how every mixin in Ruby is used.
    Mixin {
        prepend: bool,
    },
    /// `Kernel#respond_to?` — a lookup from the receiver's *dispatch* class.
    ///
    /// Not `self.class.method_defined?`, which is what `core/kernel.rb` had:
    /// `Object#class` skips the singleton, by design, so that spelling could
    /// never see a `def obj.foo` or an `extend`ed module. The question
    /// `respond_to?` asks is the one dispatch asks, so it starts where dispatch
    /// starts.
    RespondTo,
    /// `Object#extend` — an `include` into the receiver's singleton class,
    /// which is the whole of what Ruby's `extend` is.
    ///
    /// A primitive rather than `singleton_class.include(m)` in Ruby, because
    /// `Module#include` is private in Ruby and the singleton class of an
    /// ordinary object is allocated by the *table*, not by anything `core/*.rb`
    /// can reach.
    Extend,
    /// `Module#ancestors` — the linearised chain, which only the class table
    /// knows. `is_a?`, `kind_of?`, `Module#===` and `Module#<` are Ruby on it.
    Ancestors,
    /// `Class#superclass` — one step up the same chain.
    Superclass,
    /// `Module#method_defined?` — a method-table lookup.
    MethodDefined,
    /// `Float#to_s` — the shortest decimal that reads back as the same float,
    /// which is an algorithm (Ruby uses `dtoa`) and not a formatting rule.
    FloatToS,
    /// `String#[]` — allocates a substring out of a byte payload.
    StringIndex,
    /// `String#<=>` — compares two byte payloads.
    StringCompare,
    /// Writes a `String`'s bytes to stdout. A syscall, so Rust.
    ///
    /// Installed as `Kernel#__write__`, which is not a Ruby method name.
    /// `docs/engine.md` spells a primitive `Primitive.write(...)`; there is no
    /// `Primitive` module yet, and inventing one for a single entry would be a
    /// module to name, bootstrap and document before anything needed it.
    /// `puts`, `print` and `p` are Ruby on top of this.
    WriteString,
}

/// The bitwise operators on `Integer`, which share one primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitOp {
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Not,
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
