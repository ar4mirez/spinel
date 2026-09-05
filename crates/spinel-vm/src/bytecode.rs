//! The bytecode: what the compiler emits and the interpreter runs.
//!
//! # Why an enum and not a byte buffer
//!
//! The name is "bytecode" and the representation is `Vec<Insn>`. A packed byte
//! buffer would buy density and cost a decode step, and the decode step is
//! exactly the `match` an enum already does. [`Insn`] is `Copy` and 16 bytes,
//! so the array is no larger than a naive encoding would have been, and phase
//! 6's Cranelift lowering reads the enum either way.
//!
//! What that trades away is that the on-disk form is a *serialisation* of the
//! enum rather than the enum's own bytes. Phase 3's bytecode cache and
//! `core.image` pay that once.
//!
//! # Position independence
//!
//! An [`Iseq`] can be written by one process and read by another, cached on
//! disk, or shared between Ractors. Three rules keep that true:
//!
//! - **Jumps are relative.** A displacement counts from the instruction *after*
//!   the jump, so an `Iseq` never needs relocating.
//! - **Symbols are names.** The pool in [`Iseq::symbols`] holds `Box<str>`, and
//!   an instruction that means a symbol holds an index into it. [`Iseq::link`]
//!   turns the pool into [`SymbolId`]s against whatever the process has interned
//!   so far, so two processes agree about symbols without agreeing about the
//!   order they first saw them in.
//! - **Literals are descriptions.** A [`Literal`] is never a [`Value`]: a
//!   `Value` may be a pointer into one particular heap. The interpreter
//!   materialises a literal into the heap it is running on, which is also what
//!   makes a string literal a fresh object per evaluation, as Ruby requires.
//!
//! [`Value`]: crate::Value

use std::sync::Arc;

use crate::shared::symbols;
use crate::value::SymbolId;

/// One instruction.
///
/// Kept `Copy` and small; `debug_assert` in [`Iseq::new`] holds the size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Insn {
    // -- pushing ----------------------------------------------------------
    PushNil,
    PushTrue,
    PushFalse,
    /// `self`. There is one receiver per frame until [#11][] gives frames a
    /// caller.
    ///
    /// [#11]: https://github.com/ar4mirez/spinel/issues/11
    PushSelf,
    /// An integer that fits a fixnum. Larger ones go through the literal pool
    /// once bignums exist.
    PushInt(i64),
    /// Index into [`Iseq::literals`].
    PushLit(u32),
    /// Index into [`Iseq::symbols`].
    PushSym(u32),

    // -- stack ------------------------------------------------------------
    Pop,
    Dup,

    // -- locals -----------------------------------------------------------
    /// `(slot, depth)`. `depth` is how many lexical scopes up the slot lives:
    /// `0` is this frame, and anything higher walks the environment chain a
    /// block captured. Carried on the instruction rather than split into a
    /// second opcode because the interpreter's walk is a loop that runs zero
    /// times in the common case.
    GetLocal(u16, u16),
    /// Pops. Assignment is an expression in Ruby, so the compiler emits `Dup`
    /// first when the value is wanted — the same split YARV makes.
    SetLocal(u16, u16),

    // -- control flow -----------------------------------------------------
    /// Relative to the instruction after this one.
    Jump(i32),
    /// Pops; jumps when the value is falsy.
    JumpUnless(i32),
    /// Pops; jumps when the value is truthy.
    JumpIf(i32),
    /// Peeks; jumps when the value is falsy, leaving it on the stack. `&&`.
    JumpUnlessKeep(i32),
    /// Peeks; jumps when the value is truthy, leaving it on the stack. `||`.
    JumpIfKeep(i32),
    /// Pops; jumps unless the value is the "no argument was supplied" marker.
    ///
    /// How a default is run for exactly the parameters a call left out. The
    /// binder writes [`Value::UNDEF`] into those slots and the body opens with
    /// one guarded default per optional and keyword, which handles both in the
    /// same shape — an entry offset per parameter would not, because keyword
    /// defaults are independent rather than a fall-through chain.
    ///
    /// [`Value::UNDEF`]: crate::Value::UNDEF
    JumpUnlessUndef(i32),

    // -- operators --------------------------------------------------------
    /// One of Ruby's specialised binary operators. See [`BinOp`] for the
    /// fast-path-with-a-send-behind-it story.
    BinOp(BinOp),
    /// `-@`
    Neg,
    /// `!`
    Not,

    // -- aggregates -------------------------------------------------------
    /// Pops `n` values into a new `Array`.
    NewArray(u32),

    // -- case/when --------------------------------------------------------
    /// Pops the `when` condition and the subject beneath it, pushes
    /// `condition === subject`. Note the order: `when` calls `===` **on the
    /// condition**, so `when Integer` asks `Integer === x`, not the reverse.
    CaseEq,

    // -- calls -------------------------------------------------------------
    /// Index into [`Iseq::call_sites`]. The receiver and then the arguments are
    /// on the stack, deepest first; the site says how many and what shape.
    ///
    /// A call site is a table entry rather than an instruction operand because
    /// engine.md's inline caches are a per-heap side table keyed by call-site
    /// id — bytecode is shared between Ractors and a cache cannot be. Emitting
    /// the id now means that slice adds a table, not an instruction format.
    Send(u32),
    /// Index into [`Iseq::call_sites`]; calls the current frame's block. The
    /// name in the site is unused, the arguments are not.
    Yield(u32),
    /// Index into [`Iseq::children`]: makes a `Proc` capturing this frame's
    /// environment. The flag is set for `->`, which is a lambda.
    MakeProc(u32, bool),
    /// Index into [`Iseq::definitions`]. Defines the method and pushes its name
    /// as a symbol, which is what `def` evaluates to.
    DefineMethod(u32),

    // -- exit -------------------------------------------------------------
    /// Return the top of the stack from this frame.
    Leave,
    /// Explicit `return`. Distinct from `Leave` because a `return` inside a
    /// block leaves the enclosing *method*, and telling the two apart is what
    /// lets a lambda return locally while a proc's `return` reports itself as
    /// [#12](https://github.com/ar4mirez/spinel/issues/12)'s work rather than
    /// silently returning from the wrong frame.
    Return,
}

/// The binary operators the compiler emits directly.
///
/// These are method calls in Ruby, and compiling them as calls needs
/// [#11][issue-11]'s calling convention. Emitting them as instructions is what
/// YARV does too (`opt_plus`, `opt_lt`, …): a fast path for the immediate types,
/// with a real send behind it when the fast path does not apply.
///
/// The send behind it does not exist yet. Until #11, an operand the fast path
/// does not cover is [`Error::NoDispatch`][nd] — *not yet dispatchable*, rather
/// than wrong — and the spec that needed it stays blocked.
///
/// [issue-11]: https://github.com/ar4mirez/spinel/issues/11
/// [nd]: crate::interp::Error::NoDispatch
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    /// The operator as Ruby spells it, for diagnostics and for the send #11
    /// puts behind the fast path.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Eq => "==",
            BinOp::Neq => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
        }
    }

    /// The operator for a method name, if it is one this slice specialises.
    #[must_use]
    pub fn from_name(name: &str) -> Option<BinOp> {
        Some(match name {
            "+" => BinOp::Add,
            "-" => BinOp::Sub,
            "*" => BinOp::Mul,
            "/" => BinOp::Div,
            "%" => BinOp::Mod,
            "==" => BinOp::Eq,
            "!=" => BinOp::Neq,
            "<" => BinOp::Lt,
            "<=" => BinOp::Le,
            ">" => BinOp::Gt,
            ">=" => BinOp::Ge,
            _ => return None,
        })
    }
}

/// A literal, described rather than built.
///
/// Never a [`Value`](crate::Value): a `Value` can be a pointer into one heap,
/// and an `Iseq` outlives and outranges any particular heap.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// Outside fixnum range, so it needs the heap. Kept as digits until
    /// bignums exist.
    BigInt(Box<str>),
    /// Outside flonum range — NaN, the infinities, `-0.0`, the extremes — so it
    /// needs the heap.
    BoxedFloat(f64),
    /// A float that fits an immediate. Held here rather than in the instruction
    /// because `Insn` stays `Copy` and small either way, and this keeps one
    /// materialisation path.
    Float(f64),
    /// Ruby strings are byte strings, not UTF-8.
    Str(Box<[u8]>),
}

/// One compiled unit: a script, and later a method or a block body.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Iseq {
    /// What a backtrace calls this. `"<main>"` for a script.
    pub name: Box<str>,
    pub insns: Vec<Insn>,
    pub literals: Vec<Literal>,
    /// Symbols **by name**. See the module docs on position independence.
    pub symbols: Vec<Box<str>>,
    /// Slot index to name. `Binding#local_variable_get` and `spinel`'s
    /// diagnostics both read this; the interpreter only needs the length.
    pub locals: Vec<Box<str>>,
    /// The deepest the value stack gets, computed by the compiler.
    pub max_stack: usize,
    /// How this `Iseq` takes arguments. `default()` for a script.
    pub params: ParamSpec,
    /// Nested bodies: block literals and method bodies, indexed by
    /// [`Insn::MakeProc`] and [`Insn::DefineMethod`].
    ///
    /// Held inline rather than in a registry so an `Iseq` stays one
    /// self-contained, position-independent unit that phase 3 can cache whole.
    pub children: Vec<Arc<Iseq>>,
    pub call_sites: Vec<CallSite>,
    /// `(symbol index, child index)` for each [`Insn::DefineMethod`].
    pub definitions: Vec<(u32, u32)>,
    /// A block body reads its enclosing frame's locals; a method body starts a
    /// new scope and must not. A `GetLocal` walking past this would be a
    /// compiler bug the interpreter cannot see, so the interpreter refuses.
    pub scope_barrier: bool,
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

/// What a call site passes as a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockRef {
    #[default]
    None,
    /// A literal `{ }` or `do end`: an index into [`Iseq::children`].
    Literal(u32),
    /// `&blk`. The value is on the stack above the arguments.
    Pass,
}

/// Everything a call needs that does not fit in an instruction.
///
/// The caller describes what it is *sending*. It never inspects the callee's
/// parameters, which is what makes an ordinary call, `send`, `yield`, and
/// `Proc#call` one code path with a different receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    /// Index into [`Iseq::symbols`].
    pub name: u32,
    /// Positional arguments pushed. A splat contributes its array as one value.
    pub argc: u16,
    /// Which of the `argc` pushed values are splats to expand, by position.
    ///
    /// Positions rather than a flag: `f(a, *b)` where `a` is *also* an array
    /// must expand only `b`, and a call-wide flag cannot tell them apart. The
    /// list is almost always empty and never long.
    pub splats: Vec<u16>,
    /// Keyword names passed, in the order their values were pushed. Indexes
    /// [`Iseq::symbols`]. Keyword values sit above the positional arguments.
    pub keywords: Vec<u32>,
    pub block: BlockRef,
    /// A receiverless call — an implicit `self` send. Visibility checks and
    /// `super` both need to know.
    pub implicit_self: bool,
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// One optional parameter.
///
/// Only a slot: the default is code at the top of the body, guarded by
/// [`Insn::JumpUnlessUndef`], so the binder never has to run anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Optional {
    pub slot: u16,
}

/// A declared keyword parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keyword {
    /// Index into [`Iseq::symbols`].
    pub name: u32,
    pub slot: u16,
    /// `def f(a:)` — a call that omits it raises rather than defaulting.
    pub required: bool,
}

/// How an `Iseq` takes its arguments.
///
/// Slots rather than names: the binder writes locals, and the compiler has
/// already decided which slot each parameter is. They are allocated in the
/// order the binder fills them — required, optional, rest, post, keywords,
/// block — so [`Iseq::locals`] reads in that order too.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParamSpec {
    /// Leading required parameters, in slots `0..required`.
    pub required: u16,
    pub optional: Vec<Optional>,
    /// `*rest`. `Some` even for an anonymous `*`, which still collects.
    pub rest: Option<u16>,
    /// Required parameters *after* the splat: `def f(a, *b, c)`. They are bound
    /// from the right, which is why they are counted separately rather than
    /// added to `required`.
    pub post: u16,
    pub keywords: Vec<Keyword>,
    /// `&blk`. The block reaches the frame either way; this is the slot that
    /// also names it as a `Proc`.
    pub block: Option<u16>,
}

impl ParamSpec {
    /// How many slots the parameters occupy, which is where the first
    /// non-parameter local starts.
    #[must_use]
    pub fn slots(&self) -> usize {
        self.required as usize
            + self.optional.len()
            + usize::from(self.rest.is_some())
            + self.post as usize
            + self.keywords.len()
            + usize::from(self.block.is_some())
    }

    /// The fewest positional arguments a lambda-arity call accepts.
    #[must_use]
    pub fn min_positional(&self) -> usize {
        self.required as usize + self.post as usize
    }

    /// The most accepted, or `None` when a splat makes it unbounded.
    #[must_use]
    pub fn max_positional(&self) -> Option<usize> {
        if self.rest.is_some() {
            None
        } else {
            Some(self.min_positional() + self.optional.len())
        }
    }

    /// Whether the shape is a single plain parameter list with no optional,
    /// splat, post, or keyword part — the common case, and the one a proc does
    /// *not* destructure a lone array argument for.
    #[must_use]
    pub fn is_simple(&self) -> bool {
        self.optional.is_empty()
            && self.rest.is_none()
            && self.post == 0
            && self.keywords.is_empty()
    }

    /// Ruby's `Proc#arity`: negative when the count is not fixed, and then the
    /// magnitude is one more than the required count.
    #[must_use]
    pub fn arity(&self) -> i64 {
        let required = self.min_positional() as i64;
        if self.rest.is_some() || !self.optional.is_empty() {
            -(required + 1)
        } else {
            required
        }
    }
}

impl Iseq {
    /// The symbol ids for this `Iseq`'s pool, interned into the process table.
    ///
    /// This is the "relinked on load" half of position independence: the pool
    /// holds names, and a name means the same symbol in every process, whereas
    /// an id only means something relative to one table's history.
    ///
    /// Called once before a run rather than per instruction, so the hot path
    /// indexes a `Vec` instead of taking the table's lock.
    #[must_use]
    pub fn link(&self) -> Vec<SymbolId> {
        self.symbols
            .iter()
            .map(|name| symbols::intern(name))
            .collect()
    }

    /// Total instructions, which is also the one-past-the-end jump target.
    #[must_use]
    pub fn len(&self) -> usize {
        self.insns.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.insns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_insn_stays_small_enough_to_be_dense() {
        // The claim in the module docs that an enum costs no more than a packed
        // encoding rests on this. If it grows, the trade needs re-arguing.
        assert!(size_of::<Insn>() <= 16, "{}", size_of::<Insn>());
    }

    #[test]
    fn linking_maps_the_pool_by_name_not_by_position() {
        // Two iseqs that list the same symbol in different positions must link
        // to the same id: that is the whole property.
        let first = Iseq {
            name: "a".into(),
            insns: vec![Insn::PushSym(0), Insn::Leave],
            literals: Vec::new(),
            symbols: vec!["shared_name".into(), "other_name".into()],
            locals: Vec::new(),
            max_stack: 1,
            ..Iseq::default()
        };
        let second = Iseq {
            symbols: vec!["other_name".into(), "shared_name".into()],
            ..first.clone()
        };
        assert_eq!(first.link()[0], second.link()[1]);
        assert_eq!(first.link()[1], second.link()[0]);
    }

    #[test]
    fn operator_names_round_trip() {
        for op in [
            BinOp::Add,
            BinOp::Sub,
            BinOp::Mul,
            BinOp::Div,
            BinOp::Mod,
            BinOp::Eq,
            BinOp::Neq,
            BinOp::Lt,
            BinOp::Le,
            BinOp::Gt,
            BinOp::Ge,
        ] {
            assert_eq!(BinOp::from_name(op.name()), Some(op));
        }
    }

    #[test]
    fn a_name_that_is_not_specialised_is_not_an_operator() {
        // `<<` is deliberately absent: it mutates, and the growable Array it
        // would need is #15's.
        assert_eq!(BinOp::from_name("<<"), None);
        assert_eq!(BinOp::from_name("each"), None);
    }
}
