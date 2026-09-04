//! Ruby source in, [`spinel_ast`] out.
//!
//! The only crate permitted to import Prism. Nothing here leaks a Prism type
//! through its public API. See `docs/architecture.md`.
//!
//! # Shape
//!
//! [`parse`] always returns a [`Parsed`]: a tree, plus whatever the parser and
//! the lowering had to say about it. It never fails and never panics on bad
//! input, because Prism recovers from syntax errors and hands back a tree with
//! `MissingNode` holes, and callers want both halves — `spinel run` wants the
//! errors, an editor wants the tree anyway.
//!
//! # Coverage
//!
//! The lowering matches on Prism's `Node` enum exhaustively, so a Prism upgrade
//! that adds a node kind is a compile error here rather than a surprise at run
//! time. The few Prism nodes that only ever appear in a parent's field — an
//! `ArgumentsNode`, a `WhenNode` — are consumed by that parent; reaching one in
//! expression position means the lowering has a bug, and it is reported as an
//! error on the node rather than panicking. That is the "unhandled node" the
//! sweep in `spinel parse <dir>` looks for.

#![forbid(unsafe_code)]

mod lower;

use spinel_ast::{Program, Span};

/// Whose fault a [`Diagnostic`] is.
///
/// Worth separating because the two need opposite responses: a `Syntax` error
/// is reported to the user, a `Lowering` one is a bug in this crate and is what
/// `spinel parse <dir>` sweeps a corpus for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The source is not valid Ruby.
    Syntax,
    /// The source is fine and Spinel could not lower it.
    Lowering,
}

/// Something the parser or the lowering has to say, aimed at a source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Byte range in the source this is about.
    pub span: Span,
    /// One line, lowercase, no trailing period — Prism's own style.
    pub message: String,
    /// Whether the source or this crate is at fault.
    pub origin: Origin,
}

/// A parsed file: the tree, and everything said about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    /// The tree. Present even when `errors` is not empty; holes are
    /// [`spinel_ast::ExprKind::Missing`].
    pub program: Program,
    /// Syntax errors from Prism, then any lowering bug found on the way down.
    pub errors: Vec<Diagnostic>,
    /// Prism's warnings. Ruby's own `-w` warnings are the compiler's job, not
    /// the parser's; these are the ones the grammar can see.
    pub warnings: Vec<Diagnostic>,
}

impl Parsed {
    /// Whether the source parsed and lowered cleanly.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Errors this crate is responsible for, as opposed to the source being
    /// invalid Ruby. A corpus sweep fails on these and only these.
    pub fn lowering_bugs(&self) -> impl Iterator<Item = &Diagnostic> {
        self.errors.iter().filter(|d| d.origin == Origin::Lowering)
    }

    /// Errors that mean the source is not valid Ruby.
    pub fn syntax_errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.errors.iter().filter(|d| d.origin == Origin::Syntax)
    }
}

/// Parse Ruby source into a [`Program`].
///
/// `source` is bytes, not `&str`: Ruby files are not required to be UTF-8, and
/// a `String` literal in a binary-encoded file is still a valid Ruby String.
#[must_use]
pub fn parse(source: &[u8]) -> Parsed {
    let result = ruby_prism::parse(source);

    let to_diagnostic = |d: ruby_prism::Diagnostic<'_>| Diagnostic {
        span: lower::span_of(&d.location()),
        message: d.message().to_owned(),
        origin: Origin::Syntax,
    };
    let mut errors: Vec<Diagnostic> = result.errors().map(to_diagnostic).collect();
    let warnings: Vec<Diagnostic> = result.warnings().map(to_diagnostic).collect();

    let node = result.node();
    // Prism roots every parse at a ProgramNode, including a parse that failed
    // outright; there is no input for which this is None.
    let root = node
        .as_program_node()
        .expect("prism roots every parse at a ProgramNode");

    let (program, lowering_errors) = lower::program(&root);
    errors.extend(lowering_errors);

    Parsed {
        program,
        errors,
        warnings,
    }
}
