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
    parse_file(EVAL, source)
}

/// What `__FILE__` answers for source that came from no file. CRuby's own
/// answer for `eval`, measured: `eval("__FILE__")` is `"(eval at ...)"`.
const EVAL: &str = "(eval)";

/// Parse source that came from a file, so `__FILE__` can answer its path.
///
/// Prism is never told the path — `ruby-prism` exposes no parse options and
/// `SourceFileNode::filepath()` is always empty — so lowering carries it.
#[must_use]
pub fn parse_file(path: &str, source: &[u8]) -> Parsed {
    parse_at(path, source, 1, false)
}

/// Parse a string handed to `eval` (#38): `path` and `first_line` are its
/// `file` and `lineno` arguments.
///
/// Prism parses it as a file, because `ruby-prism` takes no options, so it
/// cannot be told the string runs inside a method (`in_method`): a `yield`
/// there is valid and answers the method's block, unless a `class` or
/// `module` body in the string encloses it. Those diagnostics are dropped.
/// The caller's locals are the compiler's business — see `compile::eval`.
#[must_use]
pub fn parse_eval(path: &str, source: &[u8], first_line: i64, in_method: bool) -> Parsed {
    parse_at(path, source, first_line, in_method)
}

/// The `yield`s outside any `class`, `module` or `class <<` body, by offset.
#[derive(Default)]
struct OpenYields {
    bodies: usize,
    starts: Vec<u32>,
}

impl<'pr> ruby_prism::Visit<'pr> for OpenYields {
    fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
        self.bodies += 1;
        ruby_prism::visit_class_node(self, node);
        self.bodies -= 1;
    }

    fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
        self.bodies += 1;
        ruby_prism::visit_module_node(self, node);
        self.bodies -= 1;
    }

    fn visit_singleton_class_node(&mut self, node: &ruby_prism::SingletonClassNode<'pr>) {
        self.bodies += 1;
        ruby_prism::visit_singleton_class_node(self, node);
        self.bodies -= 1;
    }

    fn visit_yield_node(&mut self, node: &ruby_prism::YieldNode<'pr>) {
        if self.bodies == 0 {
            self.starts.push(lower::span_of(&node.location()).start);
        }
        ruby_prism::visit_yield_node(self, node);
    }
}

fn parse_at(path: &str, source: &[u8], first_line: i64, yield_allowed: bool) -> Parsed {
    let result = ruby_prism::parse(source);

    let to_diagnostic = |d: ruby_prism::Diagnostic<'_>| Diagnostic {
        span: lower::span_of(&d.location()),
        message: d.message().to_owned(),
        origin: Origin::Syntax,
    };
    let mut errors: Vec<Diagnostic> = result.errors().map(to_diagnostic).collect();
    let mut warnings: Vec<Diagnostic> = result.warnings().map(to_diagnostic).collect();

    let node = result.node();
    // Prism roots every parse at a ProgramNode, including a parse that failed
    // outright; there is no input for which this is None.
    let root = node
        .as_program_node()
        .expect("prism roots every parse at a ProgramNode");

    // Prism is not told whether the source is a script or an `eval` string, and
    // treats the last statement as one whose value nothing reads. Ruby reads
    // it — it is what `eval` and `require` answer — and does not warn.
    if let Some(last) = root.statements().body().iter().last() {
        let start = lower::span_of(&last.location()).start;
        warnings
            .retain(|d| d.span.start < start || !d.message.starts_with("possibly useless use of "));
    }

    if yield_allowed && errors.iter().any(|d| d.message == "Invalid yield") {
        let mut open = OpenYields::default();
        ruby_prism::Visit::visit_program_node(&mut open, &root);
        errors.retain(|d| d.message != "Invalid yield" || !open.starts.contains(&d.span.start));
    }

    let (program, lowering_errors) = lower::program(
        &root,
        lower::SourceOrigin {
            path,
            line_starts: &line_starts(source),
            first_line,
        },
    );
    errors.extend(lowering_errors);
    let mut program = program;
    if let Some(map) = std::sync::Arc::get_mut(&mut program.source) {
        map.encoding = magic_encoding(source).map(String::into_boxed_str);
    }

    Parsed {
        program,
        errors,
        warnings,
    }
}

/// The file's source encoding from its magic comment, CRuby's rule: only the
/// first line, or the second after a `#!` line, and only a comment that is
/// the line's first token. Anywhere in it, `coding` followed by `:` or `=`
/// names the encoding — which is what makes the emacs (`-*- coding: x -*-`)
/// and vim (`fileencoding=x`) styles work as well as `# encoding: x`.
fn magic_encoding(source: &[u8]) -> Option<String> {
    let mut lines = source.split(|&b| b == b'\n');
    let first = lines.next()?;
    let line = if first.starts_with(b"#!") {
        lines.next()?
    } else {
        first
    };
    let text = line.trim_ascii_start();
    if !text.starts_with(b"#") {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    let mut at = 0;
    while let Some(found) = lower[at..].windows(6).position(|w| w == b"coding") {
        let mut i = at + found + 6;
        at = i;
        if !matches!(text.get(i), Some(b':' | b'=')) {
            continue;
        }
        i += 1;
        while text.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        let start = i;
        while text
            .get(i)
            .is_some_and(|&b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            i += 1;
        }
        if i > start {
            return Some(String::from_utf8_lossy(&text[start..i]).into_owned());
        }
    }
    None
}

/// The byte offset each line starts at, line 1 first.
///
/// One pass per file rather than a scan per `__LINE__`, which matters because
/// the corpus reaches for the keyword in backtrace assertions across many
/// files.
fn line_starts(source: &[u8]) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (index, &byte) in source.iter().enumerate() {
        if byte == b'\n' {
            starts.push(u32::try_from(index + 1).unwrap_or(u32::MAX));
        }
    }
    starts
}

/// Whether Ruby prints a parser warning only under `-w`.
///
/// Prism gives every warning a level, and the Rust binding does not expose
/// it, so this is `PM_WARNING_LEVEL_DEFAULT`'s list from Prism's
/// `diagnostic.c` by message: the ones `ruby -c` prints with no flags. Every
/// other warning is verbose.
#[must_use]
pub fn is_verbose_warning(message: &str) -> bool {
    const DEFAULT: &[&str] = &[
        "... at EOL, should be parenthesized?",
        "END in method; use at_exit",
        "integer literal in flip-flop",
        "shebang line ending with \\r may cause problems",
        "encountered \\r in middle of line, treated as a mere space",
    ];
    let default = DEFAULT.contains(&message)
        || (message.starts_with("key ")
            && message.contains(" is duplicated and overwritten on line "))
        || message.starts_with("found '= literal' in conditional")
        || message.starts_with("found `= literal' in conditional")
        || message.starts_with("invalid character syntax; use ")
        || message.ends_with(" is too big for a number variable, always nil")
        || message.starts_with("string literal in ")
        || message.starts_with("regex literal in ");
    !default
}
