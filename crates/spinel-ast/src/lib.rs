//! Spinel's own Ruby syntax tree.
//!
//! Every crate above the parser consumes this tree and never a Prism node, so a
//! hand-written parser can replace Prism later without touching anything else.
//! See `docs/architecture.md`.
//!
//! # Shape
//!
//! One [`Expr`] type carries a [`Span`] plus an [`ExprKind`]. Everything is an
//! expression, because in Ruby everything is: `if`, `def`, and `class` all have
//! values. Statement lists are plain `Vec<Expr>`; there is no statements node.
//!
//! Anything a diagnostic can point at is spanned, and spanned the same way: a
//! `span` field beside a `kind`. [`Expr`], [`Target`], [`HashEntry`], and each
//! parameter follow that rule, because Ruby aims a warning at each of them —
//! `assigned but unused variable`, `key :a is duplicated`, `duplicated argument
//! name`. Learn `.span` once and it is everywhere.
//!
//! # Relationship to Prism
//!
//! Prism has 151 node types. This tree has fewer, because Prism distinguishes
//! cases that differ only in spelling and that the bytecode compiler treats
//! identically — `@a ||= 1` and `A ||= 1` are one [`ExprKind::Assign`] here,
//! separated by their [`Target`]. [`prism_map`] records where every one of the
//! 151 lands, and a test there fails if that table ever goes stale.
//!
//! Folds never discard meaning. `until` keeps its flag rather than becoming
//! `while !`, and an integer literal keeps its base, because those are choices a
//! reader made, not punctuation.
//!
//! # What this tree is not
//!
//! It is a semantic tree, not a lossless syntax tree. Keyword and delimiter
//! positions — `then`, `do` versus `{}`, the parentheses in `def foo()`, comments
//! — are not nodes here; only the [`Span`] survives. That is enough for the
//! compiler, for `spinel lint`, and for diagnostics. `spinel fmt` (phase 7) needs
//! byte-exact trivia and will want a token layer beside this tree rather than
//! more fields on it.

#![forbid(unsafe_code)]

pub mod prism_map;

// ---------------------------------------------------------------------------
// Spans and names
// ---------------------------------------------------------------------------

/// A half-open byte range `[start, end)` into the source file.
///
/// Byte offsets, not character offsets: Ruby source is bytes, and a file may be
/// in any encoding. Line and column are derived on demand by the diagnostic
/// printer, which is the only thing that needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    // ponytail: u32 caps a source file at 4 GiB and keeps `Expr` small. Widen to
    // u64 if anyone ever reports a larger Ruby file.
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// A span covering both operands. Used to give a folded node a sensible
    /// extent when it spans several Prism nodes.
    pub fn to(self, other: Span) -> Self {
        Self::new(self.start.min(other.start), self.end.max(other.end))
    }

    pub const fn len(self) -> u32 {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// An identifier: a method, variable, constant, or parameter name.
///
// ponytail: `Box<str>` now, an interned symbol id once `spinel-vm` owns the
// symbol table. Named so that swap touches this line and the constructors only.
pub type Name = Box<str>;

/// Literal string content. Ruby strings are byte strings, not UTF-8: `"\xFF"`
/// is a valid one-byte String, so this cannot be `String`.
pub type Bytes = Box<[u8]>;

// ---------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------

/// A whole parsed file.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub span: Span,
    /// Local variables the parser assigned to the top-level scope, in slot order.
    pub locals: Vec<Name>,
    pub body: Vec<Expr>,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// Any Ruby expression: a [`Span`] plus its [`ExprKind`].
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub span: Span,
    pub kind: ExprKind,
}

impl Expr {
    pub fn new(span: Span, kind: ExprKind) -> Self {
        Self { span, kind }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    // -- atoms ------------------------------------------------------------
    Nil,
    True,
    False,
    /// `self`
    SelfExpr,
    /// `__FILE__`
    SourceFile(Bytes),
    /// `__LINE__`
    SourceLine,
    /// `__ENCODING__`
    SourceEncoding,
    /// A node the parser could not build. Kept so that one syntax error does
    /// not cost the rest of the tree, which `spinel parse` and the language
    /// server both need.
    Missing,

    // -- numbers ----------------------------------------------------------
    Int(IntLit),
    Float(f64),
    /// `3r`, `1/3r`
    Rational(Box<Rational>),
    /// `2i` — the inner expression is the real numeric literal.
    Imaginary(Box<Expr>),

    // -- strings ----------------------------------------------------------
    /// `"a"`, `"a#{b}c"`, and heredocs. A literal with no interpolation is a
    /// single [`StrPart::Bytes`].
    Str(Box<StrLit>),
    /// `` `ls` ``
    XStr(Box<StrLit>),
    /// `:a`, `:"a#{b}"`
    Sym(Box<StrLit>),
    /// `/a/`, `/a#{b}/`
    Regexp(Box<RegexpLit>),
    /// A regexp in condition position, which matches against `$_`: `if /a/`.
    MatchLastLine(Box<RegexpLit>),

    // -- collections ------------------------------------------------------
    Array(Vec<Expr>),
    Hash(Box<HashLit>),
    /// `a..b`, `a...b`, and beginless or endless forms.
    Range(Box<RangeLit>),

    // -- variables --------------------------------------------------------
    Var(VarRef),
    /// `A::B`, `::A`
    ConstPath(Box<ConstPath>),
    /// Every form of assignment, including `+=`, `||=`, `&&=`, and `a, b = ...`.
    Assign(Box<Assign>),

    // -- calls ------------------------------------------------------------
    Call(Box<Call>),
    /// `super`, `super(...)`
    Super(Box<Super>),
    Yield(Box<Yield>),
    /// `-> {}`, `lambda {}` written with the arrow.
    Lambda(Box<Block>),
    /// `...` in argument position.
    ForwardingArgs,
    /// `*a` in an array literal or argument list. A bare `*` has no expression.
    Splat(Option<Box<Expr>>),
    /// The elided value in `{x:}` or `foo(x:)`; the inner expression is what
    /// Ruby fills in.
    Implicit(Box<Expr>),

    // -- control flow -----------------------------------------------------
    If(Box<If>),
    While(Box<While>),
    For(Box<For>),
    Case(Box<Case>),
    /// `a && b`, `a || b`
    Logical(Box<Logical>),
    /// `if a..b` in condition position.
    FlipFlop(Box<FlipFlop>),
    Defined(Box<Expr>),

    // -- jumps ------------------------------------------------------------
    Break(Option<Box<Expr>>),
    Next(Option<Box<Expr>>),
    Return(Option<Box<Expr>>),
    Redo,
    Retry,

    // -- exceptions -------------------------------------------------------
    Begin(Box<Begin>),
    /// `a rescue b`
    RescueMod(Box<RescueMod>),

    // -- definitions ------------------------------------------------------
    Def(Box<Def>),
    Class(Box<Class>),
    Module(Box<Module>),
    /// `class << self`
    SingletonClass(Box<SingletonClass>),
    Alias(Box<Alias>),
    Undef(Vec<Expr>),

    // -- structure --------------------------------------------------------
    /// `(a; b)`. Kept rather than flattened because parens with several
    /// statements cannot be re-derived from precedence.
    Parens(Vec<Expr>),
    /// `BEGIN {}` / `END {}`
    Exec(Box<Exec>),
    /// The `# shareable_constant_value` magic comment, applied to the write it
    /// precedes.
    ShareableConstant(Box<ShareableConstant>),

    // -- pattern matching -------------------------------------------------
    /// `a in p` (`raises: false`) and `a => p` (`raises: true`).
    MatchPattern(Box<MatchPattern>),
    /// `/(?<a>.)/ =~ s`, which writes the named captures into locals.
    MatchWrite(Box<MatchWrite>),
    ArrayPattern(Box<ArrayPattern>),
    FindPattern(Box<FindPattern>),
    HashPattern(Box<HashPattern>),
    /// `in a | b`
    AltPattern(Box<AltPattern>),
    /// `in [x] => y`
    CapturePattern(Box<CapturePattern>),
    /// `in ^a`, `in ^(expr)`
    Pin(Box<Expr>),
}

// ---------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntLit {
    /// Kept so `spinel fmt` reprints `0xff` as written.
    pub base: IntBase,
    pub value: IntValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntBase {
    Binary,
    Octal,
    Decimal,
    Hexadecimal,
}

/// Ruby integers are arbitrary precision. Literals that fit are kept as `i64`
/// so the common path costs no allocation and no re-parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntValue {
    Small(i64),
    /// Digits in `IntLit::base`, without separators or prefix.
    Big(Box<str>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rational {
    pub base: IntBase,
    pub numerator: IntValue,
    pub denominator: IntValue,
}

// ---------------------------------------------------------------------------
// Strings, symbols, regexps
// ---------------------------------------------------------------------------

/// A string, symbol, or backtick literal, interpolated or not.
#[derive(Debug, Clone, PartialEq)]
pub struct StrLit {
    pub parts: Vec<StrPart>,
    pub encoding: ForcedEncoding,
    /// `Some(true)` under `# frozen_string_literal: true`, `Some(false)` under
    /// an explicit `+"..."`, `None` when the file says nothing.
    pub frozen: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    /// Literal content, already unescaped.
    Bytes(Bytes),
    /// `#{...}` and the `#@a` / `#$a` shorthands, which hold one expression.
    Interp(Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegexpLit {
    pub parts: Vec<StrPart>,
    pub flags: RegexpFlags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RegexpFlags {
    /// `i`
    pub ignore_case: bool,
    /// `x`
    pub extended: bool,
    /// `m`
    pub multi_line: bool,
    /// `o`
    pub once: bool,
    /// `n`, `e`, `s`, `u`
    pub encoding: RegexpEncoding,
    pub forced: ForcedEncoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegexpEncoding {
    #[default]
    None,
    /// `e`
    EucJp,
    /// `n`
    Ascii8Bit,
    /// `s`
    Windows31J,
    /// `u`
    Utf8,
}

/// An encoding the parser pinned onto a literal because of its escapes, rather
/// than one the source declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForcedEncoding {
    #[default]
    None,
    Utf8,
    Binary,
    UsAscii,
}

// ---------------------------------------------------------------------------
// Collections
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct HashLit {
    pub entries: Vec<HashEntry>,
    /// `false` for the braceless keyword hash in `foo(a: 1)`.
    pub braces: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HashEntry {
    pub span: Span,
    pub kind: HashEntryKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HashEntryKind {
    Pair {
        key: Expr,
        value: Expr,
    },
    /// `**h`; a bare `**nil` in a call has no expression.
    Splat(Option<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RangeLit {
    pub left: Option<Expr>,
    pub right: Option<Expr>,
    /// `...` rather than `..`
    pub exclude_end: bool,
}

// ---------------------------------------------------------------------------
// Variables and assignment
// ---------------------------------------------------------------------------

/// A variable read. The kind is what makes `@a` and `A` differ; folding them
/// into one variant keeps the compiler's assignment path single.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarRef {
    /// `depth` is how many enclosing scopes up the slot lives.
    Local { name: Name, depth: u32 },
    /// `@a`
    Instance(Name),
    /// `@@a`
    Class(Name),
    /// `$a`
    Global(Name),
    /// `A`
    Const(Name),
    /// The implicit block parameter `it`.
    It,
    /// `$~`, `$&`, `` $` ``, `$'`, `$+`
    BackRef(Name),
    /// `$1`, `$2`, ...
    NumberedRef(u32),
}

/// `A::B`. `parent` is `None` for the top-level form `::A`; `name` is `None`
/// only in a tree recovered from a syntax error.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstPath {
    pub parent: Option<Expr>,
    pub name: Option<Name>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assign {
    pub target: Target,
    pub op: AssignOp,
    pub value: Expr,
}

/// Which assignment this is. The target keeps its own kind, so `@a ||= 1` and
/// `a[0] ||= 1` stay distinguishable even though both are `Or` here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignOp {
    /// `=`
    Assign,
    /// `&&=`
    And,
    /// `||=`
    Or,
    /// `+=`, `<<=`, ... The name is the binary operator, not the writer method.
    Binary(Name),
}

/// An assignment target. Spanned like [`Expr`] because Ruby's own warnings
/// point at one: `assigned but unused variable - a`.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub span: Span,
    pub kind: TargetKind,
}

impl Target {
    pub fn new(span: Span, kind: TargetKind) -> Self {
        Self { span, kind }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TargetKind {
    Var(VarRef),
    ConstPath(Box<ConstPath>),
    /// `a.b = `, `a&.b = `
    Call(Box<CallTarget>),
    /// `a[b] = `
    Index(Box<IndexTarget>),
    /// `a, b = `, and the nested `(a, b), c = ` form.
    Multi(Box<MultiTarget>),
    /// `*a` in a multiple assignment. `None` is the anonymous `*`, which is
    /// also how a trailing comma (`a, = xs`) is written.
    Splat(Option<Box<Target>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallTarget {
    pub receiver: Expr,
    pub name: Name,
    /// `&.` rather than `.`
    pub safe_nav: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexTarget {
    pub receiver: Expr,
    pub args: Vec<Expr>,
    pub block: Option<BlockArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MultiTarget {
    pub lefts: Vec<Target>,
    /// The splat, if any. Everything after it lands in `rights`.
    pub rest: Option<Target>,
    pub rights: Vec<Target>,
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// `None` for a receiverless call, which is an implicit `self` send.
    pub receiver: Option<Expr>,
    pub name: Name,
    pub name_span: Span,
    /// Splats and the keyword hash appear here as [`ExprKind::Splat`] and
    /// [`ExprKind::Hash`], in source order, because argument order is what
    /// Ruby evaluates.
    pub args: Vec<Expr>,
    pub block: Option<BlockArg>,
    pub flags: CallFlags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CallFlags {
    /// `a&.b`
    pub safe_nav: bool,
    /// A bare name that could have been a local variable, such as `foo` rather
    /// than `foo()`. `defined?` and `method_missing` both need to know.
    pub variable_call: bool,
    /// A call that reaches a private method legally, such as `self.foo = 1`.
    pub ignore_visibility: bool,
    /// The call had parentheses. `foo()` and `foo` differ to `super`.
    pub has_parens: bool,
    /// The call was written as an assignment: `a.b = v`, `a[k] = v`, and the
    /// parenthesised `a.b=(v)`. Such a call evaluates to the value assigned,
    /// never to what the writer method returned — `def b=(*) = 1` still makes
    /// `(a.b = "x")` answer `"x"`. `a.send(:b=, "x")` is not one of these and
    /// does answer 1, which is why this is a flag from the parser rather than
    /// a test on the method name.
    pub attribute_write: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockArg {
    /// A literal `{ }` or `do end`.
    Block(Box<Block>),
    /// `&blk`. `None` is the anonymous `&`.
    Pass(Option<Box<Expr>>),
}

/// A block body and its parameters. Shared by `{ }`, `do end`, and `-> {}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub params: Params,
    pub body: Vec<Expr>,
    pub locals: Vec<Name>,
}

/// `super` with arguments, or bare `super`, which forwards the current
/// method's arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Super {
    /// `None` is bare `super`; `Some(vec![])` is `super()`.
    pub args: Option<Vec<Expr>>,
    pub block: Option<BlockArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Yield {
    pub args: Vec<Expr>,
    pub has_parens: bool,
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// How a method or block declares its parameters.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Params {
    /// No parameter list at all: `def foo`, `{ }`.
    #[default]
    None,
    Explicit(Box<ParamList>),
    /// `_1`, `_2`, ... The count is the highest one used.
    Numbered(u8),
    /// The block used `it`.
    It,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamList {
    pub span: Span,
    pub required: Vec<RequiredParam>,
    pub optional: Vec<OptionalParam>,
    pub rest: Option<RestParam>,
    /// Required parameters that follow the splat: `def f(a, *b, c)`.
    pub posts: Vec<RequiredParam>,
    pub keywords: Vec<KeywordParam>,
    pub keyword_rest: Option<KeywordRestParam>,
    pub block: Option<BlockParam>,
    /// Block-locals: the `b` in `{ |a; b| }`.
    pub locals: Vec<BlockLocal>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockLocal {
    pub span: Span,
    pub name: Name,
}

// Every parameter carries its own span, because Ruby reports
// `duplicated argument name` against one parameter and not the whole list.

#[derive(Debug, Clone, PartialEq)]
pub struct RequiredParam {
    pub span: Span,
    pub kind: RequiredParamKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RequiredParamKind {
    Named(Name),
    /// `{ |(a, b)| }`
    Destructure(Box<MultiTarget>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OptionalParam {
    pub span: Span,
    pub name: Name,
    pub default: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RestParam {
    pub span: Span,
    /// `None` is the anonymous `*`.
    pub name: Option<Name>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeywordParam {
    pub span: Span,
    pub name: Name,
    /// `None` makes the keyword required: `def f(a:)`.
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeywordRestParam {
    pub span: Span,
    pub kind: KeywordRestKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeywordRestKind {
    /// `**kw`, or the anonymous `**`.
    Named(Option<Name>),
    /// `**nil`: the method accepts no keywords at all.
    Forbidden,
    /// `...`, which forwards positional arguments, keywords, and the block.
    Forwarding,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockParam {
    pub span: Span,
    /// `None` is the anonymous `&`.
    pub name: Option<Name>,
}

// ---------------------------------------------------------------------------
// Control flow
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct If {
    pub predicate: Expr,
    pub then_body: Vec<Expr>,
    /// `elsif` is one [`ExprKind::If`] inside this list.
    pub else_body: Option<Vec<Expr>>,
    /// `unless` rather than `if`. Kept rather than negating the predicate so
    /// the formatter can reprint the source.
    pub unless: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct While {
    pub predicate: Expr,
    pub body: Vec<Expr>,
    /// `until` rather than `while`.
    pub until: bool,
    /// `begin ... end while c`, which runs the body once before testing.
    pub post: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct For {
    pub index: Target,
    pub iterable: Expr,
    pub body: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    /// `None` for `case` with no subject.
    pub predicate: Option<Expr>,
    pub branches: CaseBranches,
    pub else_body: Option<Vec<Expr>>,
}

/// A `case` uses `when` or `in`, never both.
#[derive(Debug, Clone, PartialEq)]
pub enum CaseBranches {
    When(Vec<WhenClause>),
    In(Vec<InClause>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhenClause {
    pub span: Span,
    pub conditions: Vec<Expr>,
    pub body: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InClause {
    pub span: Span,
    pub pattern: Expr,
    /// `in a if b`. Prism folds this into the pattern; lowering lifts it back
    /// out so consumers never meet an `if` where a pattern belongs.
    pub guard: Option<Guard>,
    pub body: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Guard {
    If(Expr),
    Unless(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Logical {
    pub op: LogicalOp,
    pub left: Expr,
    pub right: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOp {
    /// `&&`, `and`
    And,
    /// `||`, `or`
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlipFlop {
    pub left: Option<Expr>,
    pub right: Option<Expr>,
    pub exclude_end: bool,
}

// ---------------------------------------------------------------------------
// Exceptions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Begin {
    pub body: Vec<Expr>,
    pub rescues: Vec<Rescue>,
    /// The `else`, which runs when no exception was raised.
    pub else_body: Option<Vec<Expr>>,
    pub ensure_body: Option<Vec<Expr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rescue {
    pub span: Span,
    /// Empty means `rescue` with no class list, which catches `StandardError`.
    pub exceptions: Vec<Expr>,
    /// The `e` in `rescue => e`.
    pub reference: Option<Target>,
    pub body: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RescueMod {
    pub value: Expr,
    pub rescue_value: Expr,
}

// ---------------------------------------------------------------------------
// Definitions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Def {
    pub name: Name,
    pub name_span: Span,
    /// `def self.foo`, `def obj.foo`.
    pub receiver: Option<Expr>,
    pub params: Params,
    pub body: Vec<Expr>,
    pub locals: Vec<Name>,
    /// `def foo = expr`
    pub endless: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Class {
    /// A [`TargetKind::Var`] holding a [`VarRef::Const`], or a [`TargetKind::ConstPath`].
    pub path: Target,
    pub superclass: Option<Expr>,
    pub body: Vec<Expr>,
    pub locals: Vec<Name>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub path: Target,
    pub body: Vec<Expr>,
    pub locals: Vec<Name>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SingletonClass {
    pub expression: Expr,
    pub body: Vec<Expr>,
    pub locals: Vec<Name>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alias {
    pub new_name: Expr,
    pub old_name: Expr,
    /// `alias $new $old` rather than `alias new old`.
    pub global: bool,
}

// ---------------------------------------------------------------------------
// Structure
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Exec {
    pub kind: ExecKind,
    pub body: Vec<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecKind {
    /// `BEGIN {}`
    Pre,
    /// `END {}`
    Post,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShareableConstant {
    pub mode: ShareableMode,
    /// The constant write the comment applies to.
    pub write: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareableMode {
    Literal,
    ExperimentalEverything,
    ExperimentalCopy,
}

// ---------------------------------------------------------------------------
// Pattern matching
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct MatchPattern {
    pub value: Expr,
    pub pattern: Expr,
    /// `a => p` raises `NoMatchingPatternError` on failure; `a in p` returns
    /// false.
    pub raises: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchWrite {
    /// The `=~` call itself.
    pub call: Expr,
    /// The locals the named captures write into.
    pub targets: Vec<Target>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArrayPattern {
    /// `Foo[a, b]`
    pub constant: Option<Expr>,
    pub requireds: Vec<Expr>,
    pub rest: Option<Expr>,
    pub posts: Vec<Expr>,
}

/// `in [*, x, *]`
#[derive(Debug, Clone, PartialEq)]
pub struct FindPattern {
    pub constant: Option<Expr>,
    pub left: Expr,
    pub requireds: Vec<Expr>,
    pub right: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HashPattern {
    pub constant: Option<Expr>,
    pub elements: Vec<HashEntry>,
    pub rest: Option<PatternRest>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternRest {
    /// `**rest`, or a bare `**`.
    Splat(Option<Expr>),
    /// `**nil`: no other keys may be present.
    Forbidden,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AltPattern {
    pub left: Expr,
    pub right: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CapturePattern {
    pub value: Expr,
    pub target: Target,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_to_covers_both() {
        assert_eq!(Span::new(2, 4).to(Span::new(9, 11)), Span::new(2, 11));
        assert_eq!(Span::new(9, 11).to(Span::new(2, 4)), Span::new(2, 11));
    }

    #[test]
    fn expr_stays_small() {
        // Every payload past two words is boxed. If this grows, a `Vec<Expr>`
        // of anything gets proportionally more expensive to walk.
        assert!(
            size_of::<Expr>() <= 40,
            "Expr is {} bytes; box the payload that grew",
            size_of::<Expr>()
        );
    }

    fn sp() -> Span {
        Span::new(0, 0)
    }
    fn e(kind: ExprKind) -> Expr {
        Expr::new(sp(), kind)
    }
    fn n(s: &str) -> Name {
        s.into()
    }
    fn local(s: &str) -> Expr {
        e(ExprKind::Var(VarRef::Local {
            name: n(s),
            depth: 0,
        }))
    }

    /// Builds, by hand, the tree for:
    ///
    /// ```ruby
    /// def greet(name, *rest, greeting: "hi", &blk)
    ///   @seen ||= {}
    ///   @seen[name] = "#{greeting}, #{name}"
    /// end
    /// ```
    ///
    /// The point is that it compiles: a tree of types nobody has instantiated
    /// can be missing a `Box` or a field and nobody finds out until the
    /// lowering is half written.
    #[test]
    fn a_real_method_is_expressible() {
        let params = ParamList {
            span: sp(),
            required: vec![RequiredParam {
                span: sp(),
                kind: RequiredParamKind::Named(n("name")),
            }],
            optional: vec![],
            rest: Some(RestParam {
                span: sp(),
                name: Some(n("rest")),
            }),
            posts: vec![],
            keywords: vec![KeywordParam {
                span: sp(),
                name: n("greeting"),
                default: Some(e(ExprKind::Str(Box::new(StrLit {
                    parts: vec![StrPart::Bytes(Box::from(&b"hi"[..]))],
                    encoding: ForcedEncoding::None,
                    frozen: None,
                })))),
            }],
            keyword_rest: None,
            block: Some(BlockParam {
                span: sp(),
                name: Some(n("blk")),
            }),
            locals: vec![],
        };

        // @seen ||= {}
        let memo = e(ExprKind::Assign(Box::new(Assign {
            target: Target::new(sp(), TargetKind::Var(VarRef::Instance(n("seen")))),
            op: AssignOp::Or,
            value: e(ExprKind::Hash(Box::new(HashLit {
                entries: vec![],
                braces: true,
            }))),
        })));

        // @seen[name] = "#{greeting}, #{name}"
        let store = e(ExprKind::Assign(Box::new(Assign {
            target: Target::new(
                sp(),
                TargetKind::Index(Box::new(IndexTarget {
                    receiver: e(ExprKind::Var(VarRef::Instance(n("seen")))),
                    args: vec![local("name")],
                    block: None,
                })),
            ),
            op: AssignOp::Assign,
            value: e(ExprKind::Str(Box::new(StrLit {
                parts: vec![
                    StrPart::Interp(vec![local("greeting")]),
                    StrPart::Bytes(Box::from(&b", "[..])),
                    StrPart::Interp(vec![local("name")]),
                ],
                encoding: ForcedEncoding::None,
                frozen: None,
            }))),
        })));

        let def = Def {
            name: n("greet"),
            name_span: sp(),
            receiver: None,
            params: Params::Explicit(Box::new(params)),
            body: vec![memo, store],
            locals: vec![n("name"), n("rest"), n("greeting"), n("blk")],
            endless: false,
        };

        let program = Program {
            span: sp(),
            locals: vec![],
            body: vec![e(ExprKind::Def(Box::new(def)))],
        };

        let ExprKind::Def(def) = &program.body[0].kind else {
            panic!("expected a def")
        };
        assert_eq!(def.body.len(), 2);
        assert_eq!(def.locals.len(), 4);
        assert!(matches!(def.params, Params::Explicit(_)));
        // Debug is what `spinel parse` prints in issue #3.
        assert!(format!("{program:?}").contains("greet"));
    }

    #[test]
    fn string_content_is_bytes_not_utf8() {
        // `"\xFF"` is a valid one-byte Ruby String, so this must compile.
        let s = StrLit {
            parts: vec![StrPart::Bytes(Box::new([0xFF]))],
            encoding: ForcedEncoding::Binary,
            frozen: None,
        };
        assert_eq!(s.parts.len(), 1);
    }
}
