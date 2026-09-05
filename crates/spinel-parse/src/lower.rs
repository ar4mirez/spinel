//! Prism's tree in, [`spinel_ast`] out.
//!
//! One [`Lower`] per file. The entry point is [`program`].
//!
//! # Rules this file follows
//!
//! - **Exhaustive.** [`Lower::expr`] matches every variant of Prism's `Node`.
//!   A Prism upgrade that adds a node breaks the build here, which is the point.
//! - **Never panics.** A file this cannot lower produces a diagnostic and an
//!   [`ExprKind::Missing`] hole, so `spinel parse` over a corpus reports rather
//!   than aborts.
//! - **Folds keep meaning.** Where 31 Prism assignment nodes become one
//!   [`Assign`], the distinction moves into [`Target`] and [`AssignOp`], it does
//!   not evaporate. `spinel_ast::prism_map` is the ledger.

use ruby_prism as pm;
use spinel_ast::*;

use crate::{Diagnostic, Origin};

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Lower a whole file. Returns the tree and any bug found lowering it.
pub(crate) fn program(node: &pm::ProgramNode<'_>) -> (Program, Vec<Diagnostic>) {
    let mut lower = Lower::default();
    let statements = node.statements();
    let program = Program {
        span: span_of(&node.as_node().location()),
        locals: constants(&node.locals()),
        body: lower.stmts(Some(&statements)),
    };
    (program, lower.errors)
}

#[derive(Default)]
struct Lower {
    errors: Vec<Diagnostic>,
}

// ---------------------------------------------------------------------------
// Small conversions
// ---------------------------------------------------------------------------

/// Prism offsets are `usize`; spans are `u32`.
///
// ponytail: a source file past 4 GiB clamps rather than wrapping. Ruby files
// that large do not exist; if one ever does, widen `Span` rather than patch here.
pub(crate) fn span_of(loc: &pm::Location<'_>) -> Span {
    let clamp = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    Span::new(clamp(loc.start_offset()), clamp(loc.end_offset()))
}

/// An identifier. Ruby identifiers are bytes in the source encoding; the ones
/// that are not UTF-8 are pathological, and lossy conversion keeps them
/// printable instead of losing the whole file.
fn ident(id: &pm::ConstantId<'_>) -> Name {
    String::from_utf8_lossy(id.as_slice()).into()
}

fn constants(list: &pm::ConstantList<'_>) -> Vec<Name> {
    list.iter().map(|c| ident(&c)).collect()
}

fn bytes(slice: &[u8]) -> Bytes {
    Bytes::from(slice)
}

/// `node.as_foo_node()` on the variant that was just matched.
macro_rules! get {
    ($node:expr, $as:ident) => {
        $node.$as().expect("variant was just matched")
    };
}

impl Lower {
    /// Record a lowering bug. Returns `Missing` so the caller can carry on and
    /// report every problem in a file rather than only the first.
    fn internal(&mut self, span: Span, what: &str) -> ExprKind {
        self.bug(
            span,
            format!("unhandled node: {what} in expression position"),
        );
        ExprKind::Missing
    }

    // -----------------------------------------------------------------------
    // Statement lists
    // -----------------------------------------------------------------------

    fn stmts(&mut self, node: Option<&pm::StatementsNode<'_>>) -> Vec<Expr> {
        node.map(|n| n.body().iter().map(|c| self.expr(&c)).collect())
            .unwrap_or_default()
    }

    /// A `def`, `class`, `module`, or block body. Prism puts either a
    /// `StatementsNode` or a `BeginNode` here, the latter when the body has a
    /// `rescue`/`ensure` without its own `begin`.
    fn body(&mut self, node: Option<pm::Node<'_>>) -> Vec<Expr> {
        match node {
            None => Vec::new(),
            Some(pm::Node::StatementsNode { .. }) => {
                let n = get!(node.as_ref().expect("matched"), as_statements_node);
                self.stmts(Some(&n))
            }
            Some(other) => vec![self.expr(&other)],
        }
    }

    fn else_body(&mut self, node: Option<pm::ElseNode<'_>>) -> Option<Vec<Expr>> {
        node.map(|n| self.stmts(n.statements().as_ref()))
    }

    fn args(&mut self, node: Option<&pm::ArgumentsNode<'_>>) -> Vec<Expr> {
        node.map(|n| n.arguments().iter().map(|a| self.expr(&a)).collect())
            .unwrap_or_default()
    }

    fn exprs(&mut self, list: &pm::NodeList<'_>) -> Vec<Expr> {
        list.iter().map(|n| self.expr(&n)).collect()
    }
}

// ---------------------------------------------------------------------------
// The assignment fold
//
// 31 Prism nodes become one `Assign`. These macros are why that is 31 short
// arms below and not 31 near-identical blocks: the families differ only in
// which `VarRef` they build and which `AssignOp` they carry.
// ---------------------------------------------------------------------------

/// `@a = 1`, `@a ||= 1`, `@a &&= 1` and their class/global/constant twins.
macro_rules! var_write {
    ($self:ident, $node:expr, $as:ident, $var:path, $op:expr) => {{
        let n = get!($node, $as);
        ExprKind::Assign(Box::new(Assign {
            target: Target::new(
                span_of(&n.name_loc()),
                TargetKind::Var($var(ident(&n.name()))),
            ),
            op: $op,
            value: $self.expr(&n.value()),
        }))
    }};
}

/// `@a += 1` and twins: same shape, but the operator is a method name.
macro_rules! var_op_write {
    ($self:ident, $node:expr, $as:ident, $var:path) => {{
        let n = get!($node, $as);
        ExprKind::Assign(Box::new(Assign {
            target: Target::new(
                span_of(&n.name_loc()),
                TargetKind::Var($var(ident(&n.name()))),
            ),
            op: AssignOp::Binary(ident(&n.binary_operator())),
            value: $self.expr(&n.value()),
        }))
    }};
}

/// `A::B = 1` and twins. The target is a whole constant path.
macro_rules! const_path_write {
    ($self:ident, $node:expr, $as:ident, $op:expr) => {{
        let n = get!($node, $as);
        let target = n.target();
        ExprKind::Assign(Box::new(Assign {
            target: $self.const_path_target(&target),
            op: $op,
            value: $self.expr(&n.value()),
        }))
    }};
}

/// `a.b ||= 1` and twins. Prism keeps both `read_name` (`b`) and `write_name`
/// (`b=`); `Target` keeps the read name and the compiler appends `=`, which is
/// the rule Prism itself applies.
macro_rules! call_write {
    ($self:ident, $node:expr, $as:ident, $op:expr) => {{
        let n = get!($node, $as);
        let receiver = n.receiver().map(|r| $self.expr(&r));
        let span = span_of(&n.as_node().location());
        ExprKind::Assign(Box::new(Assign {
            target: Target::new(
                n.message_loc().as_ref().map_or(span, span_of),
                TargetKind::Call(Box::new(CallTarget {
                    // A receiver-less `.b ||=` is not expressible in Ruby; if the
                    // parser ever recovers into one, `self` is the honest reading.
                    receiver: receiver.unwrap_or_else(|| Expr::new(span, ExprKind::SelfExpr)),
                    name: ident(&n.read_name()),
                    safe_nav: n.is_safe_navigation(),
                })),
            ),
            op: $op,
            value: $self.expr(&n.value()),
        }))
    }};
}

/// `a[0] ||= 1` and twins.
macro_rules! index_write {
    ($self:ident, $node:expr, $as:ident, $op:expr) => {{
        let n = get!($node, $as);
        let span = span_of(&n.as_node().location());
        let receiver = n.receiver().map(|r| $self.expr(&r));
        let args = $self.args(n.arguments().as_ref());
        let block = n.block().map(|b| $self.block_pass(&b));
        ExprKind::Assign(Box::new(Assign {
            target: Target::new(
                span,
                TargetKind::Index(Box::new(IndexTarget {
                    receiver: receiver.unwrap_or_else(|| Expr::new(span, ExprKind::SelfExpr)),
                    args,
                    block,
                })),
            ),
            op: $op,
            value: $self.expr(&n.value()),
        }))
    }};
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

impl Lower {
    fn expr(&mut self, node: &pm::Node<'_>) -> Expr {
        let span = span_of(&node.location());
        Expr::new(span, self.kind(node, span))
    }

    fn boxed(&mut self, node: &pm::Node<'_>) -> Box<Expr> {
        Box::new(self.expr(node))
    }

    #[allow(clippy::too_many_lines)] // One arm per Prism node; splitting it would only hide the map.
    fn kind(&mut self, node: &pm::Node<'_>, span: Span) -> ExprKind {
        match node {
            // -- atoms ------------------------------------------------------
            pm::Node::NilNode { .. } => ExprKind::Nil,
            pm::Node::TrueNode { .. } => ExprKind::True,
            pm::Node::FalseNode { .. } => ExprKind::False,
            pm::Node::SelfNode { .. } => ExprKind::SelfExpr,
            pm::Node::SourceLineNode { .. } => ExprKind::SourceLine,
            pm::Node::SourceEncodingNode { .. } => ExprKind::SourceEncoding,
            pm::Node::MissingNode { .. } => ExprKind::Missing,
            pm::Node::RedoNode { .. } => ExprKind::Redo,
            pm::Node::RetryNode { .. } => ExprKind::Retry,
            pm::Node::ForwardingArgumentsNode { .. } => ExprKind::ForwardingArgs,
            pm::Node::SourceFileNode { .. } => {
                ExprKind::SourceFile(bytes(get!(node, as_source_file_node).filepath()))
            }

            // -- numbers ----------------------------------------------------
            pm::Node::IntegerNode { .. } => {
                let n = get!(node, as_integer_node);
                let base = int_base(n.is_binary(), n.is_octal(), n.is_hexadecimal());
                ExprKind::Int(IntLit {
                    base,
                    value: int_value(&n.value(), base),
                })
            }
            pm::Node::FloatNode { .. } => ExprKind::Float(get!(node, as_float_node).value()),
            pm::Node::RationalNode { .. } => {
                let n = get!(node, as_rational_node);
                let base = int_base(n.is_binary(), n.is_octal(), n.is_hexadecimal());
                ExprKind::Rational(Box::new(Rational {
                    base,
                    numerator: int_value(&n.numerator(), base),
                    denominator: int_value(&n.denominator(), base),
                }))
            }
            pm::Node::ImaginaryNode { .. } => {
                ExprKind::Imaginary(self.boxed(&get!(node, as_imaginary_node).numeric()))
            }

            // -- strings, symbols, regexps ----------------------------------
            pm::Node::StringNode { .. } => {
                let n = get!(node, as_string_node);
                ExprKind::Str(Box::new(StrLit {
                    parts: vec![StrPart::Bytes(bytes(n.unescaped()))],
                    encoding: forced(
                        n.is_forced_utf8_encoding(),
                        n.is_forced_binary_encoding(),
                        false,
                    ),
                    frozen: frozen(n.is_frozen(), n.is_mutable()),
                }))
            }
            pm::Node::InterpolatedStringNode { .. } => {
                let n = get!(node, as_interpolated_string_node);
                let parts = self.str_parts(&n.parts());
                ExprKind::Str(Box::new(StrLit {
                    parts,
                    encoding: ForcedEncoding::None,
                    frozen: frozen(n.is_frozen(), n.is_mutable()),
                }))
            }
            pm::Node::XStringNode { .. } => {
                let n = get!(node, as_x_string_node);
                ExprKind::XStr(Box::new(StrLit {
                    parts: vec![StrPart::Bytes(bytes(n.unescaped()))],
                    encoding: forced(
                        n.is_forced_utf8_encoding(),
                        n.is_forced_binary_encoding(),
                        false,
                    ),
                    frozen: None,
                }))
            }
            pm::Node::InterpolatedXStringNode { .. } => {
                let n = get!(node, as_interpolated_x_string_node);
                let parts = self.str_parts(&n.parts());
                ExprKind::XStr(Box::new(StrLit {
                    parts,
                    encoding: ForcedEncoding::None,
                    frozen: None,
                }))
            }
            pm::Node::SymbolNode { .. } => {
                let n = get!(node, as_symbol_node);
                ExprKind::Sym(Box::new(StrLit {
                    parts: vec![StrPart::Bytes(bytes(n.unescaped()))],
                    encoding: forced(
                        n.is_forced_utf8_encoding(),
                        n.is_forced_binary_encoding(),
                        n.is_forced_us_ascii_encoding(),
                    ),
                    frozen: None,
                }))
            }
            pm::Node::InterpolatedSymbolNode { .. } => {
                let n = get!(node, as_interpolated_symbol_node);
                let parts = self.str_parts(&n.parts());
                ExprKind::Sym(Box::new(StrLit {
                    parts,
                    encoding: ForcedEncoding::None,
                    frozen: None,
                }))
            }
            pm::Node::RegularExpressionNode { .. } => {
                let n = get!(node, as_regular_expression_node);
                ExprKind::Regexp(Box::new(RegexpLit {
                    parts: vec![StrPart::Bytes(bytes(n.unescaped()))],
                    flags: regexp_flags_literal(&n),
                }))
            }
            pm::Node::InterpolatedRegularExpressionNode { .. } => {
                let n = get!(node, as_interpolated_regular_expression_node);
                let parts = self.str_parts(&n.parts());
                ExprKind::Regexp(Box::new(RegexpLit {
                    parts,
                    flags: regexp_flags_interpolated(&n),
                }))
            }
            pm::Node::MatchLastLineNode { .. } => {
                let n = get!(node, as_match_last_line_node);
                ExprKind::MatchLastLine(Box::new(RegexpLit {
                    parts: vec![StrPart::Bytes(bytes(n.unescaped()))],
                    flags: regexp_flags_last_line(&n),
                }))
            }
            pm::Node::InterpolatedMatchLastLineNode { .. } => {
                let n = get!(node, as_interpolated_match_last_line_node);
                let parts = self.str_parts(&n.parts());
                ExprKind::MatchLastLine(Box::new(RegexpLit {
                    parts,
                    flags: regexp_flags_interpolated_last_line(&n),
                }))
            }

            // -- collections ------------------------------------------------
            pm::Node::ArrayNode { .. } => {
                let elements = get!(node, as_array_node).elements();
                ExprKind::Array(self.exprs(&elements))
            }
            pm::Node::HashNode { .. } => {
                let elements = get!(node, as_hash_node).elements();
                ExprKind::Hash(Box::new(HashLit {
                    entries: self.hash_entries(&elements),
                    braces: true,
                }))
            }
            // `f(a: 1)` — a hash with no braces, in argument position.
            pm::Node::KeywordHashNode { .. } => {
                let elements = get!(node, as_keyword_hash_node).elements();
                ExprKind::Hash(Box::new(HashLit {
                    entries: self.hash_entries(&elements),
                    braces: false,
                }))
            }
            pm::Node::RangeNode { .. } => {
                let n = get!(node, as_range_node);
                let left = n.left().map(|l| self.expr(&l));
                let right = n.right().map(|r| self.expr(&r));
                ExprKind::Range(Box::new(RangeLit {
                    left,
                    right,
                    exclude_end: n.is_exclude_end(),
                }))
            }
            pm::Node::SplatNode { .. } => {
                let expression = get!(node, as_splat_node).expression();
                ExprKind::Splat(expression.map(|e| self.boxed(&e)))
            }
            pm::Node::ImplicitNode { .. } => {
                ExprKind::Implicit(self.boxed(&get!(node, as_implicit_node).value()))
            }

            // -- variable reads ---------------------------------------------
            pm::Node::LocalVariableReadNode { .. } => {
                let n = get!(node, as_local_variable_read_node);
                ExprKind::Var(VarRef::Local {
                    name: ident(&n.name()),
                    depth: n.depth(),
                })
            }
            pm::Node::InstanceVariableReadNode { .. } => ExprKind::Var(VarRef::Instance(ident(
                &get!(node, as_instance_variable_read_node).name(),
            ))),
            pm::Node::ClassVariableReadNode { .. } => ExprKind::Var(VarRef::Class(ident(
                &get!(node, as_class_variable_read_node).name(),
            ))),
            pm::Node::GlobalVariableReadNode { .. } => ExprKind::Var(VarRef::Global(ident(
                &get!(node, as_global_variable_read_node).name(),
            ))),
            pm::Node::ConstantReadNode { .. } => ExprKind::Var(VarRef::Const(ident(
                &get!(node, as_constant_read_node).name(),
            ))),
            pm::Node::ItLocalVariableReadNode { .. } => ExprKind::Var(VarRef::It),
            pm::Node::BackReferenceReadNode { .. } => ExprKind::Var(VarRef::BackRef(ident(
                &get!(node, as_back_reference_read_node).name(),
            ))),
            pm::Node::NumberedReferenceReadNode { .. } => ExprKind::Var(VarRef::NumberedRef(
                get!(node, as_numbered_reference_read_node).number(),
            )),
            pm::Node::ConstantPathNode { .. } => {
                let n = get!(node, as_constant_path_node);
                let parent = n.parent().map(|p| self.expr(&p));
                ExprKind::ConstPath(Box::new(ConstPath {
                    parent,
                    name: n.name().as_ref().map(ident),
                }))
            }

            // -- assignment: the 31-node fold -------------------------------
            pm::Node::LocalVariableWriteNode { .. } => {
                let n = get!(node, as_local_variable_write_node);
                self.local_assign(
                    &n.name(),
                    n.depth(),
                    &n.name_loc(),
                    AssignOp::Assign,
                    &n.value(),
                )
            }
            pm::Node::LocalVariableAndWriteNode { .. } => {
                let n = get!(node, as_local_variable_and_write_node);
                self.local_assign(
                    &n.name(),
                    n.depth(),
                    &n.name_loc(),
                    AssignOp::And,
                    &n.value(),
                )
            }
            pm::Node::LocalVariableOrWriteNode { .. } => {
                let n = get!(node, as_local_variable_or_write_node);
                self.local_assign(
                    &n.name(),
                    n.depth(),
                    &n.name_loc(),
                    AssignOp::Or,
                    &n.value(),
                )
            }
            pm::Node::LocalVariableOperatorWriteNode { .. } => {
                let n = get!(node, as_local_variable_operator_write_node);
                let op = AssignOp::Binary(ident(&n.binary_operator()));
                self.local_assign(&n.name(), n.depth(), &n.name_loc(), op, &n.value())
            }

            pm::Node::InstanceVariableWriteNode { .. } => {
                var_write!(
                    self,
                    node,
                    as_instance_variable_write_node,
                    VarRef::Instance,
                    AssignOp::Assign
                )
            }
            pm::Node::InstanceVariableAndWriteNode { .. } => {
                var_write!(
                    self,
                    node,
                    as_instance_variable_and_write_node,
                    VarRef::Instance,
                    AssignOp::And
                )
            }
            pm::Node::InstanceVariableOrWriteNode { .. } => {
                var_write!(
                    self,
                    node,
                    as_instance_variable_or_write_node,
                    VarRef::Instance,
                    AssignOp::Or
                )
            }
            pm::Node::InstanceVariableOperatorWriteNode { .. } => {
                var_op_write!(
                    self,
                    node,
                    as_instance_variable_operator_write_node,
                    VarRef::Instance
                )
            }

            pm::Node::ClassVariableWriteNode { .. } => {
                var_write!(
                    self,
                    node,
                    as_class_variable_write_node,
                    VarRef::Class,
                    AssignOp::Assign
                )
            }
            pm::Node::ClassVariableAndWriteNode { .. } => {
                var_write!(
                    self,
                    node,
                    as_class_variable_and_write_node,
                    VarRef::Class,
                    AssignOp::And
                )
            }
            pm::Node::ClassVariableOrWriteNode { .. } => {
                var_write!(
                    self,
                    node,
                    as_class_variable_or_write_node,
                    VarRef::Class,
                    AssignOp::Or
                )
            }
            pm::Node::ClassVariableOperatorWriteNode { .. } => {
                var_op_write!(
                    self,
                    node,
                    as_class_variable_operator_write_node,
                    VarRef::Class
                )
            }

            pm::Node::GlobalVariableWriteNode { .. } => {
                var_write!(
                    self,
                    node,
                    as_global_variable_write_node,
                    VarRef::Global,
                    AssignOp::Assign
                )
            }
            pm::Node::GlobalVariableAndWriteNode { .. } => {
                var_write!(
                    self,
                    node,
                    as_global_variable_and_write_node,
                    VarRef::Global,
                    AssignOp::And
                )
            }
            pm::Node::GlobalVariableOrWriteNode { .. } => {
                var_write!(
                    self,
                    node,
                    as_global_variable_or_write_node,
                    VarRef::Global,
                    AssignOp::Or
                )
            }
            pm::Node::GlobalVariableOperatorWriteNode { .. } => {
                var_op_write!(
                    self,
                    node,
                    as_global_variable_operator_write_node,
                    VarRef::Global
                )
            }

            pm::Node::ConstantWriteNode { .. } => {
                var_write!(
                    self,
                    node,
                    as_constant_write_node,
                    VarRef::Const,
                    AssignOp::Assign
                )
            }
            pm::Node::ConstantAndWriteNode { .. } => {
                var_write!(
                    self,
                    node,
                    as_constant_and_write_node,
                    VarRef::Const,
                    AssignOp::And
                )
            }
            pm::Node::ConstantOrWriteNode { .. } => {
                var_write!(
                    self,
                    node,
                    as_constant_or_write_node,
                    VarRef::Const,
                    AssignOp::Or
                )
            }
            pm::Node::ConstantOperatorWriteNode { .. } => {
                var_op_write!(self, node, as_constant_operator_write_node, VarRef::Const)
            }

            pm::Node::ConstantPathWriteNode { .. } => {
                const_path_write!(self, node, as_constant_path_write_node, AssignOp::Assign)
            }
            pm::Node::ConstantPathAndWriteNode { .. } => {
                const_path_write!(self, node, as_constant_path_and_write_node, AssignOp::And)
            }
            pm::Node::ConstantPathOrWriteNode { .. } => {
                const_path_write!(self, node, as_constant_path_or_write_node, AssignOp::Or)
            }
            pm::Node::ConstantPathOperatorWriteNode { .. } => {
                let n = get!(node, as_constant_path_operator_write_node);
                let target = n.target();
                let target = self.const_path_target(&target);
                ExprKind::Assign(Box::new(Assign {
                    target,
                    op: AssignOp::Binary(ident(&n.binary_operator())),
                    value: self.expr(&n.value()),
                }))
            }

            pm::Node::CallAndWriteNode { .. } => {
                call_write!(self, node, as_call_and_write_node, AssignOp::And)
            }
            pm::Node::CallOrWriteNode { .. } => {
                call_write!(self, node, as_call_or_write_node, AssignOp::Or)
            }
            pm::Node::CallOperatorWriteNode { .. } => {
                let op = AssignOp::Binary(ident(
                    &get!(node, as_call_operator_write_node).binary_operator(),
                ));
                call_write!(self, node, as_call_operator_write_node, op)
            }

            pm::Node::IndexAndWriteNode { .. } => {
                index_write!(self, node, as_index_and_write_node, AssignOp::And)
            }
            pm::Node::IndexOrWriteNode { .. } => {
                index_write!(self, node, as_index_or_write_node, AssignOp::Or)
            }
            pm::Node::IndexOperatorWriteNode { .. } => {
                let op = AssignOp::Binary(ident(
                    &get!(node, as_index_operator_write_node).binary_operator(),
                ));
                index_write!(self, node, as_index_operator_write_node, op)
            }

            pm::Node::MultiWriteNode { .. } => {
                let n = get!(node, as_multi_write_node);
                let lefts = n.lefts();
                let rest = n.rest();
                let rights = n.rights();
                let multi = self.multi_target(&lefts, rest.as_ref(), &rights);
                ExprKind::Assign(Box::new(Assign {
                    target: Target::new(span, TargetKind::Multi(Box::new(multi))),
                    op: AssignOp::Assign,
                    value: self.expr(&n.value()),
                }))
            }

            // -- calls ------------------------------------------------------
            pm::Node::CallNode { .. } => {
                let n = get!(node, as_call_node);
                let receiver = n.receiver().map(|r| self.expr(&r));
                let args = self.args(n.arguments().as_ref());
                let block = n.block().map(|b| self.block_arg(&b));
                ExprKind::Call(Box::new(Call {
                    receiver,
                    name: ident(&n.name()),
                    name_span: n.message_loc().as_ref().map_or(span, span_of),
                    args,
                    block,
                    flags: CallFlags {
                        safe_nav: n.is_safe_navigation(),
                        variable_call: n.is_variable_call(),
                        ignore_visibility: n.is_ignore_visibility(),
                        has_parens: n.opening_loc().is_some(),
                        attribute_write: n.is_attribute_write(),
                    },
                }))
            }
            pm::Node::SuperNode { .. } => {
                let n = get!(node, as_super_node);
                let args = self.args(n.arguments().as_ref());
                let block = n.block().map(|b| self.block_arg(&b));
                ExprKind::Super(Box::new(Super {
                    args: Some(args),
                    block,
                }))
            }
            // Bare `super`: passes the caller's arguments along. `args: None` is
            // what tells the compiler to do that, versus `super()`'s `Some(vec![])`.
            pm::Node::ForwardingSuperNode { .. } => {
                let block = get!(node, as_forwarding_super_node).block();
                let block = block.map(|b| BlockArg::Block(Box::new(self.block(&b))));
                ExprKind::Super(Box::new(Super { args: None, block }))
            }
            pm::Node::YieldNode { .. } => {
                let n = get!(node, as_yield_node);
                let args = self.args(n.arguments().as_ref());
                ExprKind::Yield(Box::new(Yield {
                    args,
                    has_parens: n.lparen_loc().is_some(),
                }))
            }
            pm::Node::LambdaNode { .. } => {
                let n = get!(node, as_lambda_node);
                let params = self.params(n.parameters());
                let body = self.body(n.body());
                ExprKind::Lambda(Box::new(Block {
                    params,
                    body,
                    locals: constants(&n.locals()),
                }))
            }
            pm::Node::DefinedNode { .. } => {
                ExprKind::Defined(self.boxed(&get!(node, as_defined_node).value()))
            }

            // -- control flow -----------------------------------------------
            pm::Node::IfNode { .. } => {
                let n = get!(node, as_if_node);
                let predicate = self.expr(&n.predicate());
                let then_body = self.stmts(n.statements().as_ref());
                // `subsequent` is an `ElseNode`, or another `IfNode` for `elsif`.
                let else_body = n.subsequent().map(|s| match s {
                    pm::Node::ElseNode { .. } => {
                        let e = get!(&s, as_else_node);
                        self.stmts(e.statements().as_ref())
                    }
                    other => vec![self.expr(&other)],
                });
                ExprKind::If(Box::new(If {
                    predicate,
                    then_body,
                    else_body,
                    unless: false,
                }))
            }
            pm::Node::UnlessNode { .. } => {
                let n = get!(node, as_unless_node);
                let predicate = self.expr(&n.predicate());
                let then_body = self.stmts(n.statements().as_ref());
                let else_body = self.else_body(n.else_clause());
                ExprKind::If(Box::new(If {
                    predicate,
                    then_body,
                    else_body,
                    unless: true,
                }))
            }
            pm::Node::WhileNode { .. } => {
                let n = get!(node, as_while_node);
                let predicate = self.expr(&n.predicate());
                let body = self.stmts(n.statements().as_ref());
                ExprKind::While(Box::new(While {
                    predicate,
                    body,
                    until: false,
                    post: n.is_begin_modifier(),
                }))
            }
            pm::Node::UntilNode { .. } => {
                let n = get!(node, as_until_node);
                let predicate = self.expr(&n.predicate());
                let body = self.stmts(n.statements().as_ref());
                ExprKind::While(Box::new(While {
                    predicate,
                    body,
                    until: true,
                    post: n.is_begin_modifier(),
                }))
            }
            pm::Node::ForNode { .. } => {
                let n = get!(node, as_for_node);
                let index_node = n.index();
                let index = self.target(&index_node);
                let iterable = self.expr(&n.collection());
                let body = self.stmts(n.statements().as_ref());
                ExprKind::For(Box::new(For {
                    index,
                    iterable,
                    body,
                }))
            }
            pm::Node::CaseNode { .. } => {
                let n = get!(node, as_case_node);
                let predicate = n.predicate().map(|p| self.expr(&p));
                let branches = n
                    .conditions()
                    .iter()
                    .map(|c| {
                        let w = get!(&c, as_when_node);
                        let conditions = w.conditions();
                        WhenClause {
                            span: span_of(&c.location()),
                            conditions: self.exprs(&conditions),
                            body: self.stmts(w.statements().as_ref()),
                        }
                    })
                    .collect();
                let else_body = self.else_body(n.else_clause());
                ExprKind::Case(Box::new(Case {
                    predicate,
                    branches: CaseBranches::When(branches),
                    else_body,
                }))
            }
            pm::Node::CaseMatchNode { .. } => {
                let n = get!(node, as_case_match_node);
                let predicate = n.predicate().map(|p| self.expr(&p));
                let branches = n.conditions().iter().map(|c| self.in_clause(&c)).collect();
                let else_body = self.else_body(n.else_clause());
                ExprKind::Case(Box::new(Case {
                    predicate,
                    branches: CaseBranches::In(branches),
                    else_body,
                }))
            }
            pm::Node::AndNode { .. } => {
                let n = get!(node, as_and_node);
                let left = self.expr(&n.left());
                let right = self.expr(&n.right());
                ExprKind::Logical(Box::new(Logical {
                    op: LogicalOp::And,
                    left,
                    right,
                }))
            }
            pm::Node::OrNode { .. } => {
                let n = get!(node, as_or_node);
                let left = self.expr(&n.left());
                let right = self.expr(&n.right());
                ExprKind::Logical(Box::new(Logical {
                    op: LogicalOp::Or,
                    left,
                    right,
                }))
            }
            pm::Node::FlipFlopNode { .. } => {
                let n = get!(node, as_flip_flop_node);
                let left = n.left().map(|l| self.expr(&l));
                let right = n.right().map(|r| self.expr(&r));
                ExprKind::FlipFlop(Box::new(FlipFlop {
                    left,
                    right,
                    exclude_end: n.is_exclude_end(),
                }))
            }

            // -- jumps ------------------------------------------------------
            pm::Node::BreakNode { .. } => {
                let arguments = get!(node, as_break_node).arguments();
                ExprKind::Break(self.jump_value(arguments.as_ref()))
            }
            pm::Node::NextNode { .. } => {
                let arguments = get!(node, as_next_node).arguments();
                ExprKind::Next(self.jump_value(arguments.as_ref()))
            }
            pm::Node::ReturnNode { .. } => {
                let arguments = get!(node, as_return_node).arguments();
                ExprKind::Return(self.jump_value(arguments.as_ref()))
            }

            // -- exceptions -------------------------------------------------
            pm::Node::BeginNode { .. } => {
                let n = get!(node, as_begin_node);
                let body = self.stmts(n.statements().as_ref());
                let rescues = self.rescues(n.rescue_clause());
                let else_body = self.else_body(n.else_clause());
                let ensure_body = n
                    .ensure_clause()
                    .map(|e| self.stmts(e.statements().as_ref()));
                ExprKind::Begin(Box::new(Begin {
                    body,
                    rescues,
                    else_body,
                    ensure_body,
                }))
            }
            pm::Node::RescueModifierNode { .. } => {
                let n = get!(node, as_rescue_modifier_node);
                let value = self.expr(&n.expression());
                let rescue_value = self.expr(&n.rescue_expression());
                ExprKind::RescueMod(Box::new(RescueMod {
                    value,
                    rescue_value,
                }))
            }

            // -- definitions ------------------------------------------------
            pm::Node::DefNode { .. } => {
                let n = get!(node, as_def_node);
                let receiver = n.receiver().map(|r| self.expr(&r));
                let params = n.parameters().map_or(Params::None, |p| {
                    Params::Explicit(Box::new(self.param_list(&p, Vec::new())))
                });
                let body = self.body(n.body());
                ExprKind::Def(Box::new(Def {
                    name: ident(&n.name()),
                    name_span: span_of(&n.name_loc()),
                    receiver,
                    params,
                    body,
                    locals: constants(&n.locals()),
                    endless: n.equal_loc().is_some(),
                }))
            }
            pm::Node::ClassNode { .. } => {
                let n = get!(node, as_class_node);
                let path_node = n.constant_path();
                let path = self.target(&path_node);
                let superclass = n.superclass().map(|s| self.expr(&s));
                let body = self.body(n.body());
                ExprKind::Class(Box::new(Class {
                    path,
                    superclass,
                    body,
                    locals: constants(&n.locals()),
                }))
            }
            pm::Node::ModuleNode { .. } => {
                let n = get!(node, as_module_node);
                let path_node = n.constant_path();
                let path = self.target(&path_node);
                let body = self.body(n.body());
                ExprKind::Module(Box::new(Module {
                    path,
                    body,
                    locals: constants(&n.locals()),
                }))
            }
            pm::Node::SingletonClassNode { .. } => {
                let n = get!(node, as_singleton_class_node);
                let expression = self.expr(&n.expression());
                let body = self.body(n.body());
                ExprKind::SingletonClass(Box::new(SingletonClass {
                    expression,
                    body,
                    locals: constants(&n.locals()),
                }))
            }
            pm::Node::AliasMethodNode { .. } => {
                let n = get!(node, as_alias_method_node);
                let new_name = self.expr(&n.new_name());
                let old_name = self.expr(&n.old_name());
                ExprKind::Alias(Box::new(Alias {
                    new_name,
                    old_name,
                    global: false,
                }))
            }
            pm::Node::AliasGlobalVariableNode { .. } => {
                let n = get!(node, as_alias_global_variable_node);
                let new_name = self.expr(&n.new_name());
                let old_name = self.expr(&n.old_name());
                ExprKind::Alias(Box::new(Alias {
                    new_name,
                    old_name,
                    global: true,
                }))
            }
            pm::Node::UndefNode { .. } => {
                let names = get!(node, as_undef_node).names();
                ExprKind::Undef(self.exprs(&names))
            }

            // -- structure --------------------------------------------------
            pm::Node::ParenthesesNode { .. } => {
                let body = get!(node, as_parentheses_node).body();
                ExprKind::Parens(self.body(body))
            }
            pm::Node::PreExecutionNode { .. } => {
                let statements = get!(node, as_pre_execution_node).statements();
                ExprKind::Exec(Box::new(Exec {
                    kind: ExecKind::Pre,
                    body: self.stmts(statements.as_ref()),
                }))
            }
            pm::Node::PostExecutionNode { .. } => {
                let statements = get!(node, as_post_execution_node).statements();
                ExprKind::Exec(Box::new(Exec {
                    kind: ExecKind::Post,
                    body: self.stmts(statements.as_ref()),
                }))
            }
            pm::Node::ShareableConstantNode { .. } => {
                let n = get!(node, as_shareable_constant_node);
                let mode = if n.is_experimental_everything() {
                    ShareableMode::ExperimentalEverything
                } else if n.is_experimental_copy() {
                    ShareableMode::ExperimentalCopy
                } else {
                    ShareableMode::Literal
                };
                let write = self.expr(&n.write());
                ExprKind::ShareableConstant(Box::new(ShareableConstant { mode, write }))
            }

            // -- pattern matching -------------------------------------------
            pm::Node::MatchPredicateNode { .. } => {
                let n = get!(node, as_match_predicate_node);
                let value = self.expr(&n.value());
                let pattern = self.expr(&n.pattern());
                ExprKind::MatchPattern(Box::new(MatchPattern {
                    value,
                    pattern,
                    raises: false,
                }))
            }
            pm::Node::MatchRequiredNode { .. } => {
                let n = get!(node, as_match_required_node);
                let value = self.expr(&n.value());
                let pattern = self.expr(&n.pattern());
                ExprKind::MatchPattern(Box::new(MatchPattern {
                    value,
                    pattern,
                    raises: true,
                }))
            }
            pm::Node::MatchWriteNode { .. } => {
                let n = get!(node, as_match_write_node);
                let call = n.call().as_node();
                let call = self.expr(&call);
                let targets = n.targets().iter().map(|t| self.target(&t)).collect();
                ExprKind::MatchWrite(Box::new(MatchWrite { call, targets }))
            }
            pm::Node::ArrayPatternNode { .. } => {
                let n = get!(node, as_array_pattern_node);
                let constant = n.constant().map(|c| self.expr(&c));
                let requireds = n.requireds();
                let requireds = self.exprs(&requireds);
                let rest = n.rest().map(|r| self.expr(&r));
                let posts = n.posts();
                let posts = self.exprs(&posts);
                ExprKind::ArrayPattern(Box::new(ArrayPattern {
                    constant,
                    requireds,
                    rest,
                    posts,
                }))
            }
            pm::Node::FindPatternNode { .. } => {
                let n = get!(node, as_find_pattern_node);
                let constant = n.constant().map(|c| self.expr(&c));
                let left = n.left().as_node();
                let left = self.expr(&left);
                let requireds = n.requireds();
                let requireds = self.exprs(&requireds);
                let right = self.expr(&n.right());
                ExprKind::FindPattern(Box::new(FindPattern {
                    constant,
                    left,
                    requireds,
                    right,
                }))
            }
            pm::Node::HashPatternNode { .. } => {
                let n = get!(node, as_hash_pattern_node);
                let constant = n.constant().map(|c| self.expr(&c));
                let elements = n.elements();
                let elements = self.hash_entries(&elements);
                let rest = n.rest().map(|r| match r {
                    // `**nil` — this pattern matches no other keys at all.
                    pm::Node::NoKeywordsParameterNode { .. } => PatternRest::Forbidden,
                    other => {
                        let value = other
                            .as_assoc_splat_node()
                            .and_then(|s| s.value())
                            .map(|v| self.expr(&v));
                        PatternRest::Splat(value)
                    }
                });
                ExprKind::HashPattern(Box::new(HashPattern {
                    constant,
                    elements,
                    rest,
                }))
            }
            pm::Node::AlternationPatternNode { .. } => {
                let n = get!(node, as_alternation_pattern_node);
                let left = self.expr(&n.left());
                let right = self.expr(&n.right());
                ExprKind::AltPattern(Box::new(AltPattern { left, right }))
            }
            pm::Node::CapturePatternNode { .. } => {
                let n = get!(node, as_capture_pattern_node);
                let value = self.expr(&n.value());
                let target_node = n.target().as_node();
                let target = self.target(&target_node);
                ExprKind::CapturePattern(Box::new(CapturePattern { value, target }))
            }
            pm::Node::PinnedExpressionNode { .. } => {
                ExprKind::Pin(self.boxed(&get!(node, as_pinned_expression_node).expression()))
            }
            pm::Node::PinnedVariableNode { .. } => {
                ExprKind::Pin(self.boxed(&get!(node, as_pinned_variable_node).variable()))
            }

            // -- nodes a parent owns ----------------------------------------
            //
            // Each of these is read by the node above it — a `WhenNode` by its
            // `case`, an `ArgumentsNode` by its call. Prism never puts one in
            // expression position, so arriving here means this file has a bug.
            // Report it and keep going; a corpus sweep should name every bad
            // file, not stop at the first.
            pm::Node::ArgumentsNode { .. } => self.internal(span, "ArgumentsNode"),
            pm::Node::AssocNode { .. } => self.internal(span, "AssocNode"),
            pm::Node::AssocSplatNode { .. } => self.internal(span, "AssocSplatNode"),
            pm::Node::BlockArgumentNode { .. } => self.internal(span, "BlockArgumentNode"),
            pm::Node::BlockLocalVariableNode { .. } => {
                self.internal(span, "BlockLocalVariableNode")
            }
            pm::Node::BlockNode { .. } => self.internal(span, "BlockNode"),
            pm::Node::BlockParameterNode { .. } => self.internal(span, "BlockParameterNode"),
            pm::Node::BlockParametersNode { .. } => self.internal(span, "BlockParametersNode"),
            pm::Node::ElseNode { .. } => self.internal(span, "ElseNode"),
            pm::Node::EmbeddedStatementsNode { .. } => {
                self.internal(span, "EmbeddedStatementsNode")
            }
            pm::Node::EmbeddedVariableNode { .. } => self.internal(span, "EmbeddedVariableNode"),
            pm::Node::EnsureNode { .. } => self.internal(span, "EnsureNode"),
            pm::Node::ForwardingParameterNode { .. } => {
                self.internal(span, "ForwardingParameterNode")
            }
            // `in [0, 1, ]` — the trailing comma means "and anything else", which
            // is a splat that binds nothing.
            pm::Node::ImplicitRestNode { .. } => ExprKind::Splat(None),
            pm::Node::InNode { .. } => self.internal(span, "InNode"),
            pm::Node::ItParametersNode { .. } => self.internal(span, "ItParametersNode"),
            pm::Node::KeywordRestParameterNode { .. } => {
                self.internal(span, "KeywordRestParameterNode")
            }
            pm::Node::NoKeywordsParameterNode { .. } => {
                self.internal(span, "NoKeywordsParameterNode")
            }
            pm::Node::NumberedParametersNode { .. } => {
                self.internal(span, "NumberedParametersNode")
            }
            pm::Node::OptionalKeywordParameterNode { .. } => {
                self.internal(span, "OptionalKeywordParameterNode")
            }
            pm::Node::OptionalParameterNode { .. } => self.internal(span, "OptionalParameterNode"),
            pm::Node::ParametersNode { .. } => self.internal(span, "ParametersNode"),
            pm::Node::RequiredKeywordParameterNode { .. } => {
                self.internal(span, "RequiredKeywordParameterNode")
            }
            pm::Node::RequiredParameterNode { .. } => self.internal(span, "RequiredParameterNode"),
            pm::Node::RescueNode { .. } => self.internal(span, "RescueNode"),
            pm::Node::RestParameterNode { .. } => self.internal(span, "RestParameterNode"),
            pm::Node::StatementsNode { .. } => self.internal(span, "StatementsNode"),
            pm::Node::WhenNode { .. } => self.internal(span, "WhenNode"),

            // A variable target in *expression* position is a pattern binding:
            // `in [x]` and `in {k: v}` hand over a `LocalVariableTargetNode`
            // where the tree wants an `Expr`. It reads as the name it binds.
            pm::Node::LocalVariableTargetNode { .. }
            | pm::Node::InstanceVariableTargetNode { .. }
            | pm::Node::ClassVariableTargetNode { .. }
            | pm::Node::GlobalVariableTargetNode { .. }
            | pm::Node::ConstantTargetNode { .. }
            | pm::Node::ConstantPathTargetNode { .. } => match self.target(node).kind {
                TargetKind::Var(v) => ExprKind::Var(v),
                TargetKind::ConstPath(p) => ExprKind::ConstPath(p),
                _ => self.internal(span, "variable target"),
            },

            // These three have no reading as an expression: Ruby has no pattern
            // that binds through a call, an index, or a nested multi-target.
            pm::Node::CallTargetNode { .. } => self.internal(span, "CallTargetNode"),
            pm::Node::IndexTargetNode { .. } => self.internal(span, "IndexTargetNode"),
            pm::Node::MultiTargetNode { .. } => self.internal(span, "MultiTargetNode"),

            pm::Node::ProgramNode { .. } => self.internal(span, "ProgramNode"),
        }
    }
}

// ---------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------

impl Lower {
    /// Record a lowering bug without producing a value.
    fn bug(&mut self, span: Span, message: String) {
        self.errors.push(Diagnostic {
            span,
            message,
            origin: Origin::Lowering,
        });
    }

    /// The left of an assignment, a `for` index, a `rescue => e` reference, or a
    /// slot in a multi-target. Prism has a separate `*TargetNode` for most of
    /// these; `class Foo` and `module A::B` hand over the *read* node instead,
    /// so both spellings land here.
    fn target(&mut self, node: &pm::Node<'_>) -> Target {
        let span = span_of(&node.location());
        let kind = match node {
            pm::Node::LocalVariableTargetNode { .. } => {
                let n = get!(node, as_local_variable_target_node);
                TargetKind::Var(VarRef::Local {
                    name: ident(&n.name()),
                    depth: n.depth(),
                })
            }
            pm::Node::LocalVariableReadNode { .. } => {
                let n = get!(node, as_local_variable_read_node);
                TargetKind::Var(VarRef::Local {
                    name: ident(&n.name()),
                    depth: n.depth(),
                })
            }
            pm::Node::InstanceVariableTargetNode { .. } => TargetKind::Var(VarRef::Instance(
                ident(&get!(node, as_instance_variable_target_node).name()),
            )),
            pm::Node::InstanceVariableReadNode { .. } => TargetKind::Var(VarRef::Instance(ident(
                &get!(node, as_instance_variable_read_node).name(),
            ))),
            pm::Node::ClassVariableTargetNode { .. } => TargetKind::Var(VarRef::Class(ident(
                &get!(node, as_class_variable_target_node).name(),
            ))),
            pm::Node::ClassVariableReadNode { .. } => TargetKind::Var(VarRef::Class(ident(
                &get!(node, as_class_variable_read_node).name(),
            ))),
            pm::Node::GlobalVariableTargetNode { .. } => TargetKind::Var(VarRef::Global(ident(
                &get!(node, as_global_variable_target_node).name(),
            ))),
            pm::Node::GlobalVariableReadNode { .. } => TargetKind::Var(VarRef::Global(ident(
                &get!(node, as_global_variable_read_node).name(),
            ))),
            pm::Node::ConstantTargetNode { .. } => TargetKind::Var(VarRef::Const(ident(
                &get!(node, as_constant_target_node).name(),
            ))),
            pm::Node::ConstantReadNode { .. } => TargetKind::Var(VarRef::Const(ident(
                &get!(node, as_constant_read_node).name(),
            ))),
            pm::Node::BackReferenceReadNode { .. } => TargetKind::Var(VarRef::BackRef(ident(
                &get!(node, as_back_reference_read_node).name(),
            ))),
            pm::Node::NumberedReferenceReadNode { .. } => TargetKind::Var(VarRef::NumberedRef(
                get!(node, as_numbered_reference_read_node).number(),
            )),

            pm::Node::ConstantPathTargetNode { .. } => {
                let n = get!(node, as_constant_path_target_node);
                let parent = n.parent().map(|p| self.expr(&p));
                TargetKind::ConstPath(Box::new(ConstPath {
                    parent,
                    name: n.name().as_ref().map(ident),
                }))
            }
            pm::Node::ConstantPathNode { .. } => {
                let n = get!(node, as_constant_path_node);
                let parent = n.parent().map(|p| self.expr(&p));
                TargetKind::ConstPath(Box::new(ConstPath {
                    parent,
                    name: n.name().as_ref().map(ident),
                }))
            }

            pm::Node::CallTargetNode { .. } => {
                let n = get!(node, as_call_target_node);
                let receiver = self.expr(&n.receiver());
                TargetKind::Call(Box::new(CallTarget {
                    receiver,
                    name: ident(&n.name()),
                    safe_nav: n.is_safe_navigation(),
                }))
            }
            // `for obj.attr in xs` hands over a plain call.
            pm::Node::CallNode { .. } => {
                let n = get!(node, as_call_node);
                let receiver = n.receiver().map(|r| self.expr(&r));
                TargetKind::Call(Box::new(CallTarget {
                    receiver: receiver.unwrap_or_else(|| Expr::new(span, ExprKind::SelfExpr)),
                    name: ident(&n.name()),
                    safe_nav: n.is_safe_navigation(),
                }))
            }
            pm::Node::IndexTargetNode { .. } => {
                let n = get!(node, as_index_target_node);
                let receiver = self.expr(&n.receiver());
                let args = self.args(n.arguments().as_ref());
                let block = n.block().map(|b| self.block_pass(&b));
                TargetKind::Index(Box::new(IndexTarget {
                    receiver,
                    args,
                    block,
                }))
            }
            pm::Node::MultiTargetNode { .. } => {
                let n = get!(node, as_multi_target_node);
                let lefts = n.lefts();
                let rest = n.rest();
                let rights = n.rights();
                TargetKind::Multi(Box::new(self.multi_target(&lefts, rest.as_ref(), &rights)))
            }
            pm::Node::SplatNode { .. } => {
                let expression = get!(node, as_splat_node).expression();
                TargetKind::Splat(expression.map(|e| Box::new(self.target(&e))))
            }
            // `a, = xs` — a rest with nothing to bind it to.
            pm::Node::ImplicitRestNode { .. } => TargetKind::Splat(None),

            // `{ |(a, b)| }`. A destructuring parameter is a multi-target whose
            // slots are parameters rather than the usual target nodes.
            pm::Node::RequiredParameterNode { .. } => TargetKind::Var(VarRef::Local {
                name: ident(&get!(node, as_required_parameter_node).name()),
                depth: 0,
            }),

            other => {
                self.bug(
                    span_of(&other.location()),
                    "unhandled node: node in target position".to_owned(),
                );
                // An empty name is not a legal Ruby identifier, so this cannot be
                // confused with a real local. The diagnostic above is the signal.
                TargetKind::Var(VarRef::Local {
                    name: "".into(),
                    depth: 0,
                })
            }
        };
        Target::new(span, kind)
    }

    fn multi_target(
        &mut self,
        lefts: &pm::NodeList<'_>,
        rest: Option<&pm::Node<'_>>,
        rights: &pm::NodeList<'_>,
    ) -> MultiTarget {
        MultiTarget {
            lefts: lefts.iter().map(|n| self.target(&n)).collect(),
            rest: rest.map(|n| self.target(n)),
            rights: rights.iter().map(|n| self.target(&n)).collect(),
        }
    }

    fn const_path_target(&mut self, n: &pm::ConstantPathNode<'_>) -> Target {
        let parent = n.parent().map(|p| self.expr(&p));
        Target::new(
            span_of(&n.as_node().location()),
            TargetKind::ConstPath(Box::new(ConstPath {
                parent,
                name: n.name().as_ref().map(ident),
            })),
        )
    }

    fn local_assign(
        &mut self,
        name: &pm::ConstantId<'_>,
        depth: u32,
        name_loc: &pm::Location<'_>,
        op: AssignOp,
        value: &pm::Node<'_>,
    ) -> ExprKind {
        ExprKind::Assign(Box::new(Assign {
            target: Target::new(
                span_of(name_loc),
                TargetKind::Var(VarRef::Local {
                    name: ident(name),
                    depth,
                }),
            ),
            op,
            value: self.expr(value),
        }))
    }
}

// ---------------------------------------------------------------------------
// Blocks and parameters
// ---------------------------------------------------------------------------

impl Lower {
    fn block_arg(&mut self, node: &pm::Node<'_>) -> BlockArg {
        match node {
            pm::Node::BlockNode { .. } => {
                BlockArg::Block(Box::new(self.block(&get!(node, as_block_node))))
            }
            pm::Node::BlockArgumentNode { .. } => {
                self.block_pass(&get!(node, as_block_argument_node))
            }
            other => {
                self.bug(
                    span_of(&other.location()),
                    "unhandled node: node in block position".to_owned(),
                );
                BlockArg::Pass(None)
            }
        }
    }

    fn block_pass(&mut self, n: &pm::BlockArgumentNode<'_>) -> BlockArg {
        BlockArg::Pass(n.expression().map(|e| self.boxed(&e)))
    }

    fn block(&mut self, n: &pm::BlockNode<'_>) -> Block {
        let params = self.params(n.parameters());
        let body = self.body(n.body());
        Block {
            params,
            body,
            locals: constants(&n.locals()),
        }
    }

    /// A block's or lambda's parameter list. `_1`/`it` are their own shapes here
    /// rather than booleans beside a list, because a block cannot have both.
    fn params(&mut self, node: Option<pm::Node<'_>>) -> Params {
        let Some(node) = node else {
            return Params::None;
        };
        match &node {
            pm::Node::NumberedParametersNode { .. } => {
                Params::Numbered(get!(&node, as_numbered_parameters_node).maximum())
            }
            pm::Node::ItParametersNode { .. } => Params::It,
            pm::Node::BlockParametersNode { .. } => {
                let bp = get!(&node, as_block_parameters_node);
                let locals = bp
                    .locals()
                    .iter()
                    .map(|l| BlockLocal {
                        span: span_of(&l.location()),
                        name: l
                            .as_block_local_variable_node()
                            .map_or_else(|| "".into(), |v| ident(&v.name())),
                    })
                    .collect();
                let list = match bp.parameters() {
                    Some(p) => self.param_list(&p, locals),
                    // `{ |; a| }` — no parameters, but block-locals to declare.
                    None => empty_param_list(span_of(&node.location()), locals),
                };
                Params::Explicit(Box::new(list))
            }
            other => {
                self.bug(
                    span_of(&other.location()),
                    "unhandled node: node in parameter position".to_owned(),
                );
                Params::None
            }
        }
    }

    fn param_list(&mut self, p: &pm::ParametersNode<'_>, locals: Vec<BlockLocal>) -> ParamList {
        let span = span_of(&p.as_node().location());

        let required = p.requireds().iter().map(|n| self.required(&n)).collect();
        let posts = p.posts().iter().map(|n| self.required(&n)).collect();

        let optional = p
            .optionals()
            .iter()
            .map(|n| {
                let span = span_of(&n.location());
                match n.as_optional_parameter_node() {
                    Some(o) => OptionalParam {
                        span,
                        name: ident(&o.name()),
                        default: self.expr(&o.value()),
                    },
                    None => {
                        self.bug(
                            span,
                            "unhandled node: node in optional parameter position".to_owned(),
                        );
                        OptionalParam {
                            span,
                            name: "".into(),
                            default: Expr::new(span, ExprKind::Missing),
                        }
                    }
                }
            })
            .collect();

        let rest = p.rest().map(|n| {
            let span = span_of(&n.location());
            // `|a,|` and `def f(*)` both land here with nothing to name, and
            // they do not mean the same thing — see `RestParam::implicit`.
            let implicit = n.as_implicit_rest_node().is_some();
            RestParam {
                span,
                name: n
                    .as_rest_parameter_node()
                    .and_then(|r| r.name())
                    .as_ref()
                    .map(ident),
                implicit,
            }
        });

        let keywords = p
            .keywords()
            .iter()
            .map(|n| {
                let span = span_of(&n.location());
                if let Some(k) = n.as_required_keyword_parameter_node() {
                    KeywordParam {
                        span,
                        name: ident(&k.name()),
                        default: None,
                    }
                } else if let Some(k) = n.as_optional_keyword_parameter_node() {
                    KeywordParam {
                        span,
                        name: ident(&k.name()),
                        default: Some(self.expr(&k.value())),
                    }
                } else {
                    self.bug(
                        span,
                        "unhandled node: node in keyword parameter position".to_owned(),
                    );
                    KeywordParam {
                        span,
                        name: "".into(),
                        default: None,
                    }
                }
            })
            .collect();

        let keyword_rest = p.keyword_rest().map(|n| {
            let span = span_of(&n.location());
            let kind = match &n {
                // `**nil`
                pm::Node::NoKeywordsParameterNode { .. } => KeywordRestKind::Forbidden,
                // `...`. Prism parks it in the keyword-rest slot; it stands for
                // the whole `*args, **kwargs, &block` bundle.
                pm::Node::ForwardingParameterNode { .. } => KeywordRestKind::Forwarding,
                other => KeywordRestKind::Named(
                    other
                        .as_keyword_rest_parameter_node()
                        .and_then(|k| k.name())
                        .as_ref()
                        .map(ident),
                ),
            };
            KeywordRestParam { span, kind }
        });

        let block = p.block().map(|b| BlockParam {
            span: span_of(&b.as_node().location()),
            name: b.name().as_ref().map(ident),
        });

        ParamList {
            span,
            required,
            optional,
            rest,
            posts,
            keywords,
            keyword_rest,
            block,
            locals,
        }
    }

    fn required(&mut self, node: &pm::Node<'_>) -> RequiredParam {
        let span = span_of(&node.location());
        let kind = match node {
            pm::Node::RequiredParameterNode { .. } => {
                RequiredParamKind::Named(ident(&get!(node, as_required_parameter_node).name()))
            }
            // `def f((a, b))` — a destructuring parameter.
            pm::Node::MultiTargetNode { .. } => {
                let n = get!(node, as_multi_target_node);
                let lefts = n.lefts();
                let rest = n.rest();
                let rights = n.rights();
                RequiredParamKind::Destructure(Box::new(self.multi_target(
                    &lefts,
                    rest.as_ref(),
                    &rights,
                )))
            }
            other => {
                self.bug(
                    span_of(&other.location()),
                    "unhandled node: node in parameter position".to_owned(),
                );
                RequiredParamKind::Named("".into())
            }
        };
        RequiredParam { span, kind }
    }
}

fn empty_param_list(span: Span, locals: Vec<BlockLocal>) -> ParamList {
    ParamList {
        span,
        required: Vec::new(),
        optional: Vec::new(),
        rest: None,
        posts: Vec::new(),
        keywords: Vec::new(),
        keyword_rest: None,
        block: None,
        locals,
    }
}

// ---------------------------------------------------------------------------
// Literal parts, hashes, clauses
// ---------------------------------------------------------------------------

impl Lower {
    /// The pieces of an interpolated string, symbol, backtick, or regexp.
    ///
    /// Adjacent literals and heredoc lines nest in Prism; splicing keeps the
    /// result a flat list of runs and holes, which is what the compiler wants.
    fn str_parts(&mut self, list: &pm::NodeList<'_>) -> Vec<StrPart> {
        let mut out = Vec::new();
        for part in list.iter() {
            match &part {
                pm::Node::StringNode { .. } => out.push(StrPart::Bytes(bytes(
                    get!(&part, as_string_node).unescaped(),
                ))),
                pm::Node::EmbeddedStatementsNode { .. } => {
                    let statements = get!(&part, as_embedded_statements_node).statements();
                    let body = self.stmts(statements.as_ref());
                    out.push(StrPart::Interp(body));
                }
                // `#@a` and `#$a`, which hold exactly one expression.
                pm::Node::EmbeddedVariableNode { .. } => {
                    let variable = get!(&part, as_embedded_variable_node).variable();
                    let e = self.expr(&variable);
                    out.push(StrPart::Interp(vec![e]));
                }
                pm::Node::InterpolatedStringNode { .. } => {
                    let inner = get!(&part, as_interpolated_string_node).parts();
                    let inner = self.str_parts(&inner);
                    out.extend(inner);
                }
                other => {
                    let e = self.expr(other);
                    out.push(StrPart::Interp(vec![e]));
                }
            }
        }
        out
    }

    fn hash_entries(&mut self, list: &pm::NodeList<'_>) -> Vec<HashEntry> {
        list.iter()
            .map(|n| {
                let span = span_of(&n.location());
                let kind = match &n {
                    pm::Node::AssocNode { .. } => {
                        let a = get!(&n, as_assoc_node);
                        let key = self.expr(&a.key());
                        let value = self.expr(&a.value());
                        HashEntryKind::Pair { key, value }
                    }
                    pm::Node::AssocSplatNode { .. } => {
                        let value = get!(&n, as_assoc_splat_node).value();
                        HashEntryKind::Splat(value.map(|v| self.expr(&v)))
                    }
                    other => {
                        self.bug(
                            span_of(&other.location()),
                            "unhandled node: node in hash position".to_owned(),
                        );
                        HashEntryKind::Splat(None)
                    }
                };
                HashEntry { span, kind }
            })
            .collect()
    }

    /// One `in` branch. Prism hangs a guard above the pattern as an `if`/`unless`
    /// whose body is the pattern; this lifts it back out into `InClause::guard`,
    /// which is where a reader expects it.
    fn in_clause(&mut self, node: &pm::Node<'_>) -> InClause {
        let span = span_of(&node.location());
        let Some(n) = node.as_in_node() else {
            self.bug(
                span,
                "unhandled node: node in `in` clause position".to_owned(),
            );
            return InClause {
                span,
                pattern: Expr::new(span, ExprKind::Missing),
                guard: None,
                body: Vec::new(),
            };
        };

        let raw = n.pattern();
        let (pattern, guard) = match &raw {
            pm::Node::IfNode { .. } => {
                let i = get!(&raw, as_if_node);
                let guard = Guard::If(self.expr(&i.predicate()));
                (
                    self.guarded_pattern(i.statements().as_ref(), span),
                    Some(guard),
                )
            }
            pm::Node::UnlessNode { .. } => {
                let u = get!(&raw, as_unless_node);
                let guard = Guard::Unless(self.expr(&u.predicate()));
                (
                    self.guarded_pattern(u.statements().as_ref(), span),
                    Some(guard),
                )
            }
            other => (self.expr(other), None),
        };

        InClause {
            span,
            pattern,
            guard,
            body: self.stmts(n.statements().as_ref()),
        }
    }

    fn guarded_pattern(&mut self, statements: Option<&pm::StatementsNode<'_>>, span: Span) -> Expr {
        let mut body = self.stmts(statements);
        if body.len() == 1 {
            body.pop().expect("checked length")
        } else {
            self.bug(
                span,
                "unhandled node: guarded pattern without exactly one pattern".to_owned(),
            );
            Expr::new(span, ExprKind::Missing)
        }
    }

    /// `rescue`s chain through `subsequent`; the tree wants them side by side.
    fn rescues(&mut self, first: Option<pm::RescueNode<'_>>) -> Vec<Rescue> {
        let mut out = Vec::new();
        let mut current = first;
        while let Some(r) = current {
            let exceptions = r.exceptions();
            let exceptions = self.exprs(&exceptions);
            let reference = r.reference().map(|t| self.target(&t));
            out.push(Rescue {
                span: span_of(&r.as_node().location()),
                exceptions,
                reference,
                body: self.stmts(r.statements().as_ref()),
            });
            current = r.subsequent();
        }
        out
    }

    /// `break`, `next`, and `return` each take at most one value. Ruby's
    /// `break 1, 2` means `break [1, 2]`, so the extra arguments become the array
    /// the programmer wrote without brackets.
    fn jump_value(&mut self, args: Option<&pm::ArgumentsNode<'_>>) -> Option<Box<Expr>> {
        let node = args?;
        let span = span_of(&node.as_node().location());
        let mut values = self.args(Some(node));
        match values.len() {
            0 => None,
            1 => Some(Box::new(values.pop().expect("checked length"))),
            _ => Some(Box::new(Expr::new(span, ExprKind::Array(values)))),
        }
    }
}

// ---------------------------------------------------------------------------
// Literal details
// ---------------------------------------------------------------------------

fn int_base(binary: bool, octal: bool, hexadecimal: bool) -> IntBase {
    if binary {
        IntBase::Binary
    } else if octal {
        IntBase::Octal
    } else if hexadecimal {
        IntBase::Hexadecimal
    } else {
        IntBase::Decimal
    }
}

/// Prism hands integers back as base-2^32 digits. Anything that fits stays an
/// `i64`; the rest becomes a digit string in the literal's own base, which is
/// what `IntValue::Big` promises.
fn int_value(int: &pm::Integer<'_>, base: IntBase) -> IntValue {
    let (negative, digits) = int.to_u32_digits();

    if digits.len() <= 2 {
        let mut value: i128 = 0;
        for (i, digit) in digits.iter().enumerate() {
            value |= i128::from(*digit) << (32 * i);
        }
        if negative {
            value = -value;
        }
        if let Ok(small) = i64::try_from(value) {
            return IntValue::Small(small);
        }
    }

    IntValue::Big(to_radix_string(negative, digits, base).into())
}

/// Base-2^32 digits to a digit string, by repeated division. Bignum literals are
/// rare enough that the naive algorithm is the right one.
fn to_radix_string(negative: bool, digits: &[u32], base: IntBase) -> String {
    let radix = match base {
        IntBase::Binary => 2u64,
        IntBase::Octal => 8,
        IntBase::Decimal => 10,
        IntBase::Hexadecimal => 16,
    };

    let mut work = digits.to_vec();
    let mut out = Vec::new();
    while work.iter().any(|d| *d != 0) {
        let mut remainder = 0u64;
        for digit in work.iter_mut().rev() {
            let current = (remainder << 32) | u64::from(*digit);
            *digit = u32::try_from(current / radix).unwrap_or(u32::MAX);
            remainder = current % radix;
        }
        let remainder = u32::try_from(remainder).unwrap_or(0);
        out.push(
            char::from_digit(remainder, u32::try_from(radix).unwrap_or(10))
                .expect("remainder is below the radix"),
        );
    }

    if out.is_empty() {
        out.push('0');
    }
    // The sign is not a prefix in the "0x"/"0b" sense; dropping it would lose
    // the value, so it stays.
    if negative {
        out.push('-');
    }
    out.iter().rev().collect()
}

fn forced(utf8: bool, binary: bool, us_ascii: bool) -> ForcedEncoding {
    if utf8 {
        ForcedEncoding::Utf8
    } else if binary {
        ForcedEncoding::Binary
    } else if us_ascii {
        ForcedEncoding::UsAscii
    } else {
        ForcedEncoding::None
    }
}

/// `Some(true)` under `# frozen_string_literal: true`, `Some(false)` under an
/// explicit `+"..."`, `None` when the file said nothing.
fn frozen(is_frozen: bool, is_mutable: bool) -> Option<bool> {
    match (is_frozen, is_mutable) {
        (true, _) => Some(true),
        (false, true) => Some(false),
        (false, false) => None,
    }
}

/// The four regexp-flavoured nodes carry an identical flag set on four unrelated
/// Rust types, so the reader is generated rather than written four times.
macro_rules! regexp_flags_fn {
    ($name:ident, $ty:ident) => {
        fn $name(n: &pm::$ty<'_>) -> RegexpFlags {
            RegexpFlags {
                ignore_case: n.is_ignore_case(),
                extended: n.is_extended(),
                multi_line: n.is_multi_line(),
                once: n.is_once(),
                encoding: if n.is_euc_jp() {
                    RegexpEncoding::EucJp
                } else if n.is_ascii_8bit() {
                    RegexpEncoding::Ascii8Bit
                } else if n.is_windows_31j() {
                    RegexpEncoding::Windows31J
                } else if n.is_utf_8() {
                    RegexpEncoding::Utf8
                } else {
                    RegexpEncoding::None
                },
                forced: forced(
                    n.is_forced_utf8_encoding(),
                    n.is_forced_binary_encoding(),
                    n.is_forced_us_ascii_encoding(),
                ),
            }
        }
    };
}

regexp_flags_fn!(regexp_flags_literal, RegularExpressionNode);
regexp_flags_fn!(regexp_flags_interpolated, InterpolatedRegularExpressionNode);
regexp_flags_fn!(regexp_flags_last_line, MatchLastLineNode);
regexp_flags_fn!(
    regexp_flags_interpolated_last_line,
    InterpolatedMatchLastLineNode
);
