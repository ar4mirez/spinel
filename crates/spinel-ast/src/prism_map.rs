//! Where every Prism node lands in this tree.
//!
//! The issue behind this crate asks that every Prism node kind have a
//! `spinel_ast` counterpart *or a documented reason it is folded into another*.
//! This table is that document, in a form a test can check: [`PRISM_NODES`] has
//! one row per node in Prism [`PRISM_VERSION`], and the tests below fail if a
//! row is missing, duplicated, or blank.
//!
//! It is deliberately data and not a `match` on Prism types: `spinel-ast` must
//! not depend on `prism` (see `docs/architecture.md`), so the snapshot of
//! upstream's node set lives here as strings. The lowering in `spinel-parse`
//! consumes this list to prove it handles every node.
//!
//! ## What the table does not cover
//!
//! Two kinds of Prism field are dropped on purpose, and neither is a fold,
//! because nothing is lost:
//!
//! - **Cached predicates.** `ArgumentsNodeFlags::CONTAINS_SPLAT`,
//!   `ArrayNodeFlags::CONTAINS_SPLAT`, `KeywordHashNodeFlags::SYMBOL_KEYS`,
//!   `ParenthesesNodeFlags::MULTIPLE_STATEMENTS`, and
//!   `ParameterFlags::REPEATED_PARAMETER` each restate something a walk of the
//!   children answers. Prism caches them for its C consumers; storing them here
//!   would be a second copy that can disagree with the tree.
//! - **Derived names.** The write-node families carry both a `read_name` and a
//!   `write_name` (`b` and `b=`, `[]` and `[]=`). `Target` keeps the read name
//!   and the compiler appends `=`, which is the rule Prism itself applies.
//!   `ClassNode` and `ModuleNode` likewise carry a `name` that is the last
//!   segment of their `constant_path`.
//!
//! Source positions beyond [`Span`](crate::Span) — `then`, `do`, the parens in
//! `def foo()` — are dropped too. See the crate docs on what this tree is not.
//!
//! ## Upgrading Prism
//!
//! Diff `config.yml` against [`PRISM_VERSION`], add or remove rows here, then
//! bump the constant. `node_count_matches_prism` fails until you do.

/// The Prism release [`PRISM_NODES`] was taken from.
pub const PRISM_VERSION: &str = "1.9.0";

/// How many node kinds that release defines.
pub const PRISM_NODE_COUNT: usize = 151;

/// One Prism node kind and its home in this tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeMapping {
    /// The Prism node name, as spelled in upstream's `config.yml`.
    pub prism: &'static str,
    /// The `spinel_ast` type, variant, or field it becomes.
    pub spinel: &'static str,
    /// Why it is not one-to-one. Empty when it is.
    pub note: &'static str,
}

impl NodeMapping {
    /// Whether this node folded into a shape that also serves other nodes.
    pub const fn is_folded(&self) -> bool {
        !self.note.is_empty()
    }
}

/// Every Prism node kind, sorted by name.
pub const PRISM_NODES: &[NodeMapping] = &[
    NodeMapping {
        prism: "AliasGlobalVariableNode",
        spinel: "ExprKind::Alias",
        note: "Folded: `global: true` distinguishes it from the method form.",
    },
    NodeMapping {
        prism: "AliasMethodNode",
        spinel: "ExprKind::Alias",
        note: "Folded: `global: false`.",
    },
    NodeMapping {
        prism: "AlternationPatternNode",
        spinel: "ExprKind::AltPattern",
        note: "",
    },
    NodeMapping {
        prism: "AndNode",
        spinel: "ExprKind::Logical",
        note: "Folded: `LogicalOp::And`.",
    },
    NodeMapping {
        prism: "ArgumentsNode",
        spinel: "Call::args",
        note: "Folded: a bare `Vec<Expr>`; the wrapper carries nothing the list does not.",
    },
    NodeMapping {
        prism: "ArrayNode",
        spinel: "ExprKind::Array",
        note: "",
    },
    NodeMapping {
        prism: "ArrayPatternNode",
        spinel: "ExprKind::ArrayPattern",
        note: "",
    },
    NodeMapping {
        prism: "AssocNode",
        spinel: "HashEntryKind::Pair",
        note: "",
    },
    NodeMapping {
        prism: "AssocSplatNode",
        spinel: "HashEntryKind::Splat",
        note: "",
    },
    NodeMapping {
        prism: "BackReferenceReadNode",
        spinel: "VarRef::BackRef",
        note: "",
    },
    NodeMapping {
        prism: "BeginNode",
        spinel: "ExprKind::Begin",
        note: "",
    },
    NodeMapping {
        prism: "BlockArgumentNode",
        spinel: "BlockArg::Pass",
        note: "",
    },
    NodeMapping {
        prism: "BlockLocalVariableNode",
        spinel: "ParamList::locals",
        note: "Folded: carried as a field of its parent, which is the only place it can appear.",
    },
    NodeMapping {
        prism: "BlockNode",
        spinel: "BlockArg::Block",
        note: "",
    },
    NodeMapping {
        prism: "BlockParameterNode",
        spinel: "BlockParam",
        note: "",
    },
    NodeMapping {
        prism: "BlockParametersNode",
        spinel: "ParamList",
        note: "Folded: same shape as `ParametersNode` plus block-locals, which `ParamList` already has.",
    },
    NodeMapping {
        prism: "BreakNode",
        spinel: "ExprKind::Break",
        note: "",
    },
    NodeMapping {
        prism: "CallAndWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Call` + `AssignOp::And`.",
    },
    NodeMapping {
        prism: "CallNode",
        spinel: "ExprKind::Call",
        note: "",
    },
    NodeMapping {
        prism: "CallOperatorWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Call` + `AssignOp::Binary`.",
    },
    NodeMapping {
        prism: "CallOrWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Call` + `AssignOp::Or`.",
    },
    NodeMapping {
        prism: "CallTargetNode",
        spinel: "TargetKind::Call",
        note: "",
    },
    NodeMapping {
        prism: "CapturePatternNode",
        spinel: "ExprKind::CapturePattern",
        note: "",
    },
    NodeMapping {
        prism: "CaseMatchNode",
        spinel: "ExprKind::Case",
        note: "Folded: `CaseBranches::In`.",
    },
    NodeMapping {
        prism: "CaseNode",
        spinel: "ExprKind::Case",
        note: "Folded: `CaseBranches::When`.",
    },
    NodeMapping {
        prism: "ClassNode",
        spinel: "ExprKind::Class",
        note: "",
    },
    NodeMapping {
        prism: "ClassVariableAndWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Class)` + `AssignOp::And`. Folded: the variable kind lives in `Target`, so every compound assignment shares one path.",
    },
    NodeMapping {
        prism: "ClassVariableOperatorWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Class)` + `AssignOp::Binary`.",
    },
    NodeMapping {
        prism: "ClassVariableOrWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Class)` + `AssignOp::Or`.",
    },
    NodeMapping {
        prism: "ClassVariableReadNode",
        spinel: "VarRef::Class",
        note: "",
    },
    NodeMapping {
        prism: "ClassVariableTargetNode",
        spinel: "TargetKind::Var(VarRef::Class)",
        note: "",
    },
    NodeMapping {
        prism: "ClassVariableWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Class)` + `AssignOp::Assign`.",
    },
    NodeMapping {
        prism: "ConstantAndWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Const)` + `AssignOp::And`. Folded: the variable kind lives in `Target`, so every compound assignment shares one path.",
    },
    NodeMapping {
        prism: "ConstantOperatorWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Const)` + `AssignOp::Binary`.",
    },
    NodeMapping {
        prism: "ConstantOrWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Const)` + `AssignOp::Or`.",
    },
    NodeMapping {
        prism: "ConstantPathAndWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::ConstPath` + `AssignOp::And`.",
    },
    NodeMapping {
        prism: "ConstantPathNode",
        spinel: "ExprKind::ConstPath",
        note: "",
    },
    NodeMapping {
        prism: "ConstantPathOperatorWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::ConstPath` + `AssignOp::Binary`.",
    },
    NodeMapping {
        prism: "ConstantPathOrWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::ConstPath` + `AssignOp::Or`.",
    },
    NodeMapping {
        prism: "ConstantPathTargetNode",
        spinel: "TargetKind::ConstPath",
        note: "",
    },
    NodeMapping {
        prism: "ConstantPathWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::ConstPath` + `AssignOp::Assign`.",
    },
    NodeMapping {
        prism: "ConstantReadNode",
        spinel: "VarRef::Const",
        note: "",
    },
    NodeMapping {
        prism: "ConstantTargetNode",
        spinel: "TargetKind::Var(VarRef::Const)",
        note: "",
    },
    NodeMapping {
        prism: "ConstantWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Const)` + `AssignOp::Assign`.",
    },
    NodeMapping {
        prism: "DefNode",
        spinel: "ExprKind::Def",
        note: "",
    },
    NodeMapping {
        prism: "DefinedNode",
        spinel: "ExprKind::Defined",
        note: "",
    },
    NodeMapping {
        prism: "ElseNode",
        spinel: "If::else_body",
        note: "Folded: an `Option<Vec<Expr>>` on `If`, `Case`, and `Begin`; the keyword adds nothing.",
    },
    NodeMapping {
        prism: "EmbeddedStatementsNode",
        spinel: "StrPart::Interp",
        note: "",
    },
    NodeMapping {
        prism: "EmbeddedVariableNode",
        spinel: "StrPart::Interp",
        note: "Folded: `#@a` is `#{@a}` with one expression.",
    },
    NodeMapping {
        prism: "EnsureNode",
        spinel: "Begin::ensure_body",
        note: "Folded: carried as a field of its parent, which is the only place it can appear.",
    },
    NodeMapping {
        prism: "FalseNode",
        spinel: "ExprKind::False",
        note: "",
    },
    NodeMapping {
        prism: "FindPatternNode",
        spinel: "ExprKind::FindPattern",
        note: "",
    },
    NodeMapping {
        prism: "FlipFlopNode",
        spinel: "ExprKind::FlipFlop",
        note: "",
    },
    NodeMapping {
        prism: "FloatNode",
        spinel: "ExprKind::Float",
        note: "",
    },
    NodeMapping {
        prism: "ForNode",
        spinel: "ExprKind::For",
        note: "",
    },
    NodeMapping {
        prism: "ForwardingArgumentsNode",
        spinel: "ExprKind::ForwardingArgs",
        note: "",
    },
    NodeMapping {
        prism: "ForwardingParameterNode",
        spinel: "KeywordRestKind::Forwarding",
        note: "Folded: Prism puts `...` in the keyword-rest slot; so does this.",
    },
    NodeMapping {
        prism: "ForwardingSuperNode",
        spinel: "ExprKind::Super",
        note: "Folded: `args: None` is bare `super`, `Some(vec![])` is `super()`.",
    },
    NodeMapping {
        prism: "GlobalVariableAndWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Global)` + `AssignOp::And`. Folded: the variable kind lives in `Target`, so every compound assignment shares one path.",
    },
    NodeMapping {
        prism: "GlobalVariableOperatorWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Global)` + `AssignOp::Binary`.",
    },
    NodeMapping {
        prism: "GlobalVariableOrWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Global)` + `AssignOp::Or`.",
    },
    NodeMapping {
        prism: "GlobalVariableReadNode",
        spinel: "VarRef::Global",
        note: "",
    },
    NodeMapping {
        prism: "GlobalVariableTargetNode",
        spinel: "TargetKind::Var(VarRef::Global)",
        note: "",
    },
    NodeMapping {
        prism: "GlobalVariableWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Global)` + `AssignOp::Assign`.",
    },
    NodeMapping {
        prism: "HashNode",
        spinel: "ExprKind::Hash",
        note: "Folded: `braces: true`.",
    },
    NodeMapping {
        prism: "HashPatternNode",
        spinel: "ExprKind::HashPattern",
        note: "",
    },
    NodeMapping {
        prism: "IfNode",
        spinel: "ExprKind::If",
        note: "Folded: `unless: false`. `elsif` is a nested `If` in `else_body`.",
    },
    NodeMapping {
        prism: "ImaginaryNode",
        spinel: "ExprKind::Imaginary",
        note: "",
    },
    NodeMapping {
        prism: "ImplicitNode",
        spinel: "ExprKind::Implicit",
        note: "",
    },
    NodeMapping {
        prism: "ImplicitRestNode",
        spinel: "TargetKind::Splat(None)",
        note: "Folded: `a, = xs` binds exactly as `a, * = xs`.",
    },
    NodeMapping {
        prism: "InNode",
        spinel: "InClause",
        note: "",
    },
    NodeMapping {
        prism: "IndexAndWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Index` + `AssignOp::And`.",
    },
    NodeMapping {
        prism: "IndexOperatorWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Index` + `AssignOp::Binary`.",
    },
    NodeMapping {
        prism: "IndexOrWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Index` + `AssignOp::Or`.",
    },
    NodeMapping {
        prism: "IndexTargetNode",
        spinel: "TargetKind::Index",
        note: "",
    },
    NodeMapping {
        prism: "InstanceVariableAndWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Instance)` + `AssignOp::And`. Folded: the variable kind lives in `Target`, so every compound assignment shares one path.",
    },
    NodeMapping {
        prism: "InstanceVariableOperatorWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Instance)` + `AssignOp::Binary`.",
    },
    NodeMapping {
        prism: "InstanceVariableOrWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Instance)` + `AssignOp::Or`.",
    },
    NodeMapping {
        prism: "InstanceVariableReadNode",
        spinel: "VarRef::Instance",
        note: "",
    },
    NodeMapping {
        prism: "InstanceVariableTargetNode",
        spinel: "TargetKind::Var(VarRef::Instance)",
        note: "",
    },
    NodeMapping {
        prism: "InstanceVariableWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Instance)` + `AssignOp::Assign`.",
    },
    NodeMapping {
        prism: "IntegerNode",
        spinel: "ExprKind::Int",
        note: "",
    },
    NodeMapping {
        prism: "InterpolatedMatchLastLineNode",
        spinel: "ExprKind::MatchLastLine",
        note: "Folded: a literal with no interpolation is one `StrPart::Bytes`.",
    },
    NodeMapping {
        prism: "InterpolatedRegularExpressionNode",
        spinel: "ExprKind::Regexp",
        note: "Folded: a literal with no interpolation is one `StrPart::Bytes`.",
    },
    NodeMapping {
        prism: "InterpolatedStringNode",
        spinel: "ExprKind::Str",
        note: "Folded: a literal with no interpolation is one `StrPart::Bytes`.",
    },
    NodeMapping {
        prism: "InterpolatedSymbolNode",
        spinel: "ExprKind::Sym",
        note: "Folded: a literal with no interpolation is one `StrPart::Bytes`.",
    },
    NodeMapping {
        prism: "InterpolatedXStringNode",
        spinel: "ExprKind::XStr",
        note: "Folded: a literal with no interpolation is one `StrPart::Bytes`.",
    },
    NodeMapping {
        prism: "ItLocalVariableReadNode",
        spinel: "VarRef::It",
        note: "",
    },
    NodeMapping {
        prism: "ItParametersNode",
        spinel: "Params::It",
        note: "",
    },
    NodeMapping {
        prism: "KeywordHashNode",
        spinel: "ExprKind::Hash",
        note: "Folded: `braces: false`.",
    },
    NodeMapping {
        prism: "KeywordRestParameterNode",
        spinel: "KeywordRestKind::Named",
        note: "",
    },
    NodeMapping {
        prism: "LambdaNode",
        spinel: "ExprKind::Lambda",
        note: "",
    },
    NodeMapping {
        prism: "LocalVariableAndWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Local)` + `AssignOp::And`. Folded: the variable kind lives in `Target`, so every compound assignment shares one path.",
    },
    NodeMapping {
        prism: "LocalVariableOperatorWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Local)` + `AssignOp::Binary`.",
    },
    NodeMapping {
        prism: "LocalVariableOrWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Local)` + `AssignOp::Or`.",
    },
    NodeMapping {
        prism: "LocalVariableReadNode",
        spinel: "VarRef::Local",
        note: "",
    },
    NodeMapping {
        prism: "LocalVariableTargetNode",
        spinel: "TargetKind::Var(VarRef::Local)",
        note: "",
    },
    NodeMapping {
        prism: "LocalVariableWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Var(VarRef::Local)` + `AssignOp::Assign`.",
    },
    NodeMapping {
        prism: "MatchLastLineNode",
        spinel: "ExprKind::MatchLastLine",
        note: "",
    },
    NodeMapping {
        prism: "MatchPredicateNode",
        spinel: "ExprKind::MatchPattern",
        note: "Folded: `raises: false` is `in`.",
    },
    NodeMapping {
        prism: "MatchRequiredNode",
        spinel: "ExprKind::MatchPattern",
        note: "Folded: `raises: true` is `=>`.",
    },
    NodeMapping {
        prism: "MatchWriteNode",
        spinel: "ExprKind::MatchWrite",
        note: "",
    },
    NodeMapping {
        prism: "MissingNode",
        spinel: "ExprKind::Missing",
        note: "",
    },
    NodeMapping {
        prism: "ModuleNode",
        spinel: "ExprKind::Module",
        note: "",
    },
    NodeMapping {
        prism: "MultiTargetNode",
        spinel: "TargetKind::Multi",
        note: "",
    },
    NodeMapping {
        prism: "MultiWriteNode",
        spinel: "ExprKind::Assign",
        note: "Folded: `TargetKind::Multi` + `AssignOp::Assign`.",
    },
    NodeMapping {
        prism: "NextNode",
        spinel: "ExprKind::Next",
        note: "",
    },
    NodeMapping {
        prism: "NilNode",
        spinel: "ExprKind::Nil",
        note: "",
    },
    NodeMapping {
        prism: "NoKeywordsParameterNode",
        spinel: "KeywordRestKind::Forbidden",
        note: "Folded: `PatternRest::Forbidden` in a hash pattern, which is the other place `**nil` is legal.",
    },
    NodeMapping {
        prism: "NumberedParametersNode",
        spinel: "Params::Numbered",
        note: "",
    },
    NodeMapping {
        prism: "NumberedReferenceReadNode",
        spinel: "VarRef::NumberedRef",
        note: "",
    },
    NodeMapping {
        prism: "OptionalKeywordParameterNode",
        spinel: "KeywordParam",
        note: "Folded: `default: Some(_)`.",
    },
    NodeMapping {
        prism: "OptionalParameterNode",
        spinel: "OptionalParam",
        note: "",
    },
    NodeMapping {
        prism: "OrNode",
        spinel: "ExprKind::Logical",
        note: "Folded: `LogicalOp::Or`.",
    },
    NodeMapping {
        prism: "ParametersNode",
        spinel: "ParamList",
        note: "",
    },
    NodeMapping {
        prism: "ParenthesesNode",
        spinel: "ExprKind::Parens",
        note: "",
    },
    NodeMapping {
        prism: "PinnedExpressionNode",
        spinel: "ExprKind::Pin",
        note: "",
    },
    NodeMapping {
        prism: "PinnedVariableNode",
        spinel: "ExprKind::Pin",
        note: "Folded: a pinned variable is a pinned expression whose inner node is a read.",
    },
    NodeMapping {
        prism: "PostExecutionNode",
        spinel: "ExprKind::Exec",
        note: "Folded: `ExecKind::Post`.",
    },
    NodeMapping {
        prism: "PreExecutionNode",
        spinel: "ExprKind::Exec",
        note: "Folded: `ExecKind::Pre`.",
    },
    NodeMapping {
        prism: "ProgramNode",
        spinel: "Program",
        note: "",
    },
    NodeMapping {
        prism: "RangeNode",
        spinel: "ExprKind::Range",
        note: "",
    },
    NodeMapping {
        prism: "RationalNode",
        spinel: "ExprKind::Rational",
        note: "",
    },
    NodeMapping {
        prism: "RedoNode",
        spinel: "ExprKind::Redo",
        note: "",
    },
    NodeMapping {
        prism: "RegularExpressionNode",
        spinel: "ExprKind::Regexp",
        note: "",
    },
    NodeMapping {
        prism: "RequiredKeywordParameterNode",
        spinel: "KeywordParam",
        note: "Folded: `default: None`.",
    },
    NodeMapping {
        prism: "RequiredParameterNode",
        spinel: "RequiredParamKind::Named",
        note: "",
    },
    NodeMapping {
        prism: "RescueModifierNode",
        spinel: "ExprKind::RescueMod",
        note: "",
    },
    NodeMapping {
        prism: "RescueNode",
        spinel: "Rescue",
        note: "Folded: the `subsequent` chain flattens into `Begin::rescues`.",
    },
    NodeMapping {
        prism: "RestParameterNode",
        spinel: "RestParam",
        note: "",
    },
    NodeMapping {
        prism: "RetryNode",
        spinel: "ExprKind::Retry",
        note: "",
    },
    NodeMapping {
        prism: "ReturnNode",
        spinel: "ExprKind::Return",
        note: "",
    },
    NodeMapping {
        prism: "SelfNode",
        spinel: "ExprKind::SelfExpr",
        note: "",
    },
    NodeMapping {
        prism: "ShareableConstantNode",
        spinel: "ExprKind::ShareableConstant",
        note: "",
    },
    NodeMapping {
        prism: "SingletonClassNode",
        spinel: "ExprKind::SingletonClass",
        note: "",
    },
    NodeMapping {
        prism: "SourceEncodingNode",
        spinel: "ExprKind::SourceEncoding",
        note: "",
    },
    NodeMapping {
        prism: "SourceFileNode",
        spinel: "ExprKind::SourceFile",
        note: "",
    },
    NodeMapping {
        prism: "SourceLineNode",
        spinel: "ExprKind::SourceLine",
        note: "",
    },
    NodeMapping {
        prism: "SplatNode",
        spinel: "ExprKind::Splat",
        note: "",
    },
    NodeMapping {
        prism: "StatementsNode",
        spinel: "Vec<Expr>",
        note: "Folded: statement lists are plain vectors; there is no wrapper node.",
    },
    NodeMapping {
        prism: "StringNode",
        spinel: "ExprKind::Str",
        note: "",
    },
    NodeMapping {
        prism: "SuperNode",
        spinel: "ExprKind::Super",
        note: "",
    },
    NodeMapping {
        prism: "SymbolNode",
        spinel: "ExprKind::Sym",
        note: "",
    },
    NodeMapping {
        prism: "TrueNode",
        spinel: "ExprKind::True",
        note: "",
    },
    NodeMapping {
        prism: "UndefNode",
        spinel: "ExprKind::Undef",
        note: "",
    },
    NodeMapping {
        prism: "UnlessNode",
        spinel: "ExprKind::If",
        note: "Folded: `unless: true`. The flag is kept so `spinel fmt` reprints the keyword.",
    },
    NodeMapping {
        prism: "UntilNode",
        spinel: "ExprKind::While",
        note: "Folded: `until: true`, not a negated predicate, for the same reason.",
    },
    NodeMapping {
        prism: "WhenNode",
        spinel: "WhenClause",
        note: "",
    },
    NodeMapping {
        prism: "WhileNode",
        spinel: "ExprKind::While",
        note: "",
    },
    NodeMapping {
        prism: "XStringNode",
        spinel: "ExprKind::XStr",
        note: "",
    },
    NodeMapping {
        prism: "YieldNode",
        spinel: "ExprKind::Yield",
        note: "",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_count_matches_prism() {
        assert_eq!(
            PRISM_NODES.len(),
            PRISM_NODE_COUNT,
            "table has {} rows but Prism {PRISM_VERSION} defines {PRISM_NODE_COUNT} nodes",
            PRISM_NODES.len()
        );
    }

    #[test]
    fn every_node_has_a_home() {
        for m in PRISM_NODES {
            assert!(!m.prism.is_empty(), "a row has no Prism name");
            assert!(
                !m.spinel.is_empty(),
                "{} has no spinel_ast counterpart; give it one or document the fold",
                m.prism
            );
            assert!(
                m.prism.ends_with("Node"),
                "{} is not a Prism node name",
                m.prism
            );
        }
    }

    #[test]
    fn names_are_unique_and_sorted() {
        let names: Vec<_> = PRISM_NODES.iter().map(|m| m.prism).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(
            names, sorted,
            "keep the table sorted so diffs stay readable"
        );
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "a Prism node is listed twice");
    }

    #[test]
    fn folds_are_the_minority() {
        // Not a rule, a tripwire: if most of the tree became notes, the fold
        // went too far and the AST stopped resembling Ruby.
        let folded = PRISM_NODES.iter().filter(|m| m.is_folded()).count();
        assert!(
            folded < PRISM_NODES.len() / 2,
            "{folded} of {PRISM_NODE_COUNT} nodes folded"
        );
    }
}
