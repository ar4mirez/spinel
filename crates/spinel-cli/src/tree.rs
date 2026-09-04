//! A readable rendering of [`spinel_ast`], for `spinel parse`.
//!
//! # Why not `{:#?}`
//!
//! The derived `Debug` prints `1 + 2` as forty lines, because it shows every
//! wrapper — `Expr { span: Span { .. }, kind: Call(Call { .. }) }` — and a Ruby
//! tree is mostly wrappers. This crate's readers spend the next several phases
//! looking at this output, so it prints one line per node and puts the span in a
//! column where the eye can skip it:
//!
//! ```text
//! program                        0..9
//! └─ call +                      0..5
//!    ├─ recv: int 1              0..1
//!    └─ arg: int 2               4..5
//! ```
//!
//! `--format debug` still gives the full `{:#?}` for the times a field is
//! missing rather than merely unprinted.

use std::fmt::Write as _;

use spinel_ast::*;

/// One rendered node: its own label, and the nodes under it.
struct Node {
    label: String,
    children: Vec<Node>,
    span: Option<Span>,
}

fn leaf(label: impl Into<String>, span: Span) -> Node {
    Node {
        label: label.into(),
        children: Vec::new(),
        span: Some(span),
    }
}

fn group(label: impl Into<String>, children: Vec<Node>) -> Node {
    Node {
        label: label.into(),
        children,
        span: None,
    }
}

impl Node {
    /// Name the slot a child sits in, so `recv:` and `arg:` are not guesswork.
    fn named(mut self, field: &str) -> Self {
        self.label = format!("{field}: {}", self.label);
        self
    }

    fn with(mut self, children: impl IntoIterator<Item = Node>) -> Self {
        self.children.extend(children);
        self
    }
}

/// Render a whole program.
#[must_use]
pub fn render(program: &Program, color: bool) -> String {
    let mut root = leaf("program", program.span);
    if !program.locals.is_empty() {
        root.label = format!("program  locals: {}", program.locals.join(", "));
    }
    root.children = program.body.iter().map(expr).collect();

    // Two passes: collect the lines, then right-align the spans against the
    // longest one, so the tree reads as a shape and the offsets as a column.
    let mut lines = Vec::new();
    collect(&root, String::new(), true, true, &mut lines);
    let width = lines
        .iter()
        .map(|(text, _)| text.chars().count())
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    for (text, span) in lines {
        let _ = write!(out, "{text}");
        if let Some(span) = span {
            let pad = width - text.chars().count() + 2;
            let offsets = format!("{}..{}", span.start, span.end);
            let _ = if color {
                write!(out, "{:pad$}\x1b[2m{offsets}\x1b[0m", "")
            } else {
                write!(out, "{:pad$}{offsets}", "")
            };
        }
        out.push('\n');
    }
    out
}

fn collect(
    node: &Node,
    prefix: String,
    last: bool,
    root: bool,
    out: &mut Vec<(String, Option<Span>)>,
) {
    let line = if root {
        node.label.clone()
    } else {
        format!("{prefix}{} {}", if last { "└─" } else { "├─" }, node.label)
    };
    out.push((line, node.span));

    let child_prefix = if root {
        String::new()
    } else if last {
        format!("{prefix}   ")
    } else {
        format!("{prefix}│  ")
    };
    let count = node.children.len();
    for (i, child) in node.children.iter().enumerate() {
        collect(child, child_prefix.clone(), i + 1 == count, false, out);
    }
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

/// A slot holding statements. One statement inlines into the parent's line;
/// several get a heading, so `else:` never means "the first of three".
fn slot(field: &str, body: &[Expr]) -> Option<Node> {
    match body.len() {
        0 => None,
        1 => Some(expr(&body[0]).named(field)),
        _ => Some(group(format!("{field}:"), body.iter().map(expr).collect())),
    }
}

fn slots(field: &str, body: &[Expr]) -> Vec<Node> {
    slot(field, body).into_iter().collect()
}

/// Literal text, when it is short and printable enough to sit on the line.
fn inline_text(parts: &[StrPart]) -> Option<String> {
    match parts {
        [] => Some(String::new()),
        [StrPart::Bytes(b)] => {
            let text = std::str::from_utf8(b).ok()?;
            (text.len() <= 32 && !text.contains('\n')).then(|| text.to_owned())
        }
        _ => None,
    }
}

fn str_parts(parts: &[StrPart]) -> Vec<Node> {
    parts
        .iter()
        .map(|part| match part {
            StrPart::Bytes(b) => group(format!("bytes {:?}", String::from_utf8_lossy(b)), vec![]),
            StrPart::Interp(body) => group("interp:", body.iter().map(expr).collect()),
        })
        .collect()
}

/// Printed in the base it was written in. `0xff_ff` reads back as `0xffff`, not
/// as `0x65535`, which is what naively prefixing a decimal string produces.
fn int(lit: &IntLit) -> String {
    let digits = match (&lit.value, lit.base) {
        (IntValue::Small(v), IntBase::Decimal) => v.to_string(),
        (IntValue::Small(v), IntBase::Binary) => format!("{v:b}"),
        (IntValue::Small(v), IntBase::Octal) => format!("{v:o}"),
        (IntValue::Small(v), IntBase::Hexadecimal) => format!("{v:x}"),
        // `IntValue::Big` is already digits in the literal's own base.
        (IntValue::Big(s), _) => s.to_string(),
    };
    match lit.base {
        IntBase::Decimal => digits,
        IntBase::Binary => format!("0b{digits}"),
        IntBase::Octal => format!("0o{digits}"),
        IntBase::Hexadecimal => format!("0x{digits}"),
    }
}

fn var(v: &VarRef) -> String {
    match v {
        VarRef::Local { name, depth } if *depth == 0 => format!("local {name}"),
        VarRef::Local { name, depth } => format!("local {name} depth={depth}"),
        VarRef::Instance(n) => format!("ivar {n}"),
        VarRef::Class(n) => format!("cvar {n}"),
        VarRef::Global(n) => format!("gvar {n}"),
        VarRef::Const(n) => format!("const {n}"),
        VarRef::It => "it".to_owned(),
        VarRef::BackRef(n) => format!("backref {n}"),
        VarRef::NumberedRef(n) => format!("nthref ${n}"),
    }
}

fn assign_op(op: &AssignOp) -> String {
    match op {
        AssignOp::Assign => "=".to_owned(),
        AssignOp::And => "&&=".to_owned(),
        AssignOp::Or => "||=".to_owned(),
        AssignOp::Binary(name) => format!("{name}="),
    }
}

fn target(t: &Target) -> Node {
    match &t.kind {
        TargetKind::Var(v) => leaf(var(v), t.span),
        TargetKind::ConstPath(p) => const_path(p, t.span),
        TargetKind::Call(c) => leaf(
            format!("call {}{}", if c.safe_nav { "&." } else { "." }, c.name),
            t.span,
        )
        .with([expr(&c.receiver).named("recv")]),
        TargetKind::Index(i) => leaf("index []", t.span).with(
            std::iter::once(expr(&i.receiver).named("recv"))
                .chain(i.args.iter().map(|a| expr(a).named("arg")))
                .chain(i.block.iter().map(block_arg)),
        ),
        TargetKind::Multi(m) => leaf("multi", t.span).with(multi_target(m)),
        TargetKind::Splat(inner) => leaf("splat *", t.span).with(inner.iter().map(|i| target(i))),
    }
}

fn multi_target(m: &MultiTarget) -> Vec<Node> {
    m.lefts
        .iter()
        .map(target)
        .chain(m.rest.iter().map(|r| target(r).named("rest")))
        .chain(m.rights.iter().map(target))
        .collect()
}

fn const_path(p: &ConstPath, span: Span) -> Node {
    let name = p.name.as_deref().unwrap_or("<dynamic>");
    leaf(format!("constpath ::{name}"), span).with(p.parent.iter().map(|e| expr(e).named("of")))
}

fn block_arg(b: &BlockArg) -> Node {
    match b {
        BlockArg::Block(block) => group("block:", block_body(block)),
        BlockArg::Pass(e) => group(
            "block-pass: &",
            e.iter().map(|e| expr(e)).collect::<Vec<_>>(),
        ),
    }
}

fn block_body(block: &Block) -> Vec<Node> {
    params(&block.params)
        .into_iter()
        .chain(slots("body", &block.body))
        .collect()
}

fn params(p: &Params) -> Vec<Node> {
    match p {
        Params::None => Vec::new(),
        Params::Numbered(n) => vec![group(format!("params: numbered _1.._{n}"), vec![])],
        Params::It => vec![group("params: it", vec![])],
        Params::Explicit(list) => {
            let mut children = Vec::new();
            for r in &list.required {
                children.push(required_param(r));
            }
            for o in &list.optional {
                children.push(leaf(format!("opt {}", o.name), o.span).with([expr(&o.default)]));
            }
            if let Some(rest) = &list.rest {
                let name = rest.name.as_deref().unwrap_or("");
                children.push(leaf(format!("rest *{name}"), rest.span));
            }
            for r in &list.posts {
                children.push(required_param(r).named("post"));
            }
            for k in &list.keywords {
                let node = leaf(format!("key {}:", k.name), k.span);
                children.push(match &k.default {
                    Some(d) => node.with([expr(d)]),
                    None => node,
                });
            }
            if let Some(kr) = &list.keyword_rest {
                children.push(leaf(
                    match &kr.kind {
                        KeywordRestKind::Named(Some(n)) => format!("keyrest **{n}"),
                        KeywordRestKind::Named(None) => "keyrest **".to_owned(),
                        KeywordRestKind::Forbidden => "keyrest **nil".to_owned(),
                        KeywordRestKind::Forwarding => "forward ...".to_owned(),
                    },
                    kr.span,
                ));
            }
            if let Some(b) = &list.block {
                let name = b.name.as_deref().unwrap_or("");
                children.push(leaf(format!("blockparam &{name}"), b.span));
            }
            for l in &list.locals {
                children.push(leaf(format!("blocklocal {}", l.name), l.span));
            }
            vec![group("params:", children)]
        }
    }
}

fn required_param(r: &RequiredParam) -> Node {
    match &r.kind {
        RequiredParamKind::Named(n) => leaf(format!("req {n}"), r.span),
        RequiredParamKind::Destructure(m) => {
            leaf("req (destructure)", r.span).with(multi_target(m))
        }
    }
}

fn hash_entries(entries: &[HashEntry]) -> Vec<Node> {
    entries
        .iter()
        .map(|e| match &e.kind {
            HashEntryKind::Pair { key, value } => {
                leaf("pair", e.span).with([expr(key).named("key"), expr(value).named("value")])
            }
            HashEntryKind::Splat(v) => leaf("splat **", e.span).with(v.iter().map(expr)),
        })
        .collect()
}

#[allow(clippy::too_many_lines)] // One arm per node kind; splitting only hides the map.
fn expr(e: &Expr) -> Node {
    let span = e.span;
    let l = |label: &str| leaf(label.to_owned(), span);
    match &e.kind {
        // -- atoms ----------------------------------------------------------
        ExprKind::Nil => l("nil"),
        ExprKind::True => l("true"),
        ExprKind::False => l("false"),
        ExprKind::SelfExpr => l("self"),
        ExprKind::SourceLine => l("__LINE__"),
        ExprKind::SourceEncoding => l("__ENCODING__"),
        ExprKind::Missing => l("missing"),
        ExprKind::ForwardingArgs => l("forward ..."),
        ExprKind::Redo => l("redo"),
        ExprKind::Retry => l("retry"),
        ExprKind::SourceFile(path) => leaf(
            format!("__FILE__ {:?}", String::from_utf8_lossy(path)),
            span,
        ),

        // -- numbers --------------------------------------------------------
        ExprKind::Int(lit) => leaf(format!("int {}", int(lit)), span),
        ExprKind::Float(f) => leaf(format!("float {f}"), span),
        ExprKind::Rational(r) => leaf(
            format!(
                "rational {}/{}",
                int(&IntLit {
                    base: r.base,
                    value: r.numerator.clone()
                }),
                int(&IntLit {
                    base: r.base,
                    value: r.denominator.clone()
                })
            ),
            span,
        ),
        ExprKind::Imaginary(inner) => l("imaginary").with([expr(inner)]),

        // -- strings --------------------------------------------------------
        ExprKind::Str(s) => literal("str", s, span),
        ExprKind::XStr(s) => literal("xstr", s, span),
        ExprKind::Sym(s) => match inline_text(&s.parts) {
            Some(text) => leaf(format!("sym :{text}"), span),
            None => l("sym").with(str_parts(&s.parts)),
        },
        ExprKind::Regexp(r) => regexp("regexp", r, span),
        ExprKind::MatchLastLine(r) => regexp("match-last-line", r, span),

        // -- collections ----------------------------------------------------
        ExprKind::Array(items) => l("array").with(items.iter().map(expr)),
        ExprKind::Hash(h) => leaf(
            if h.braces { "hash" } else { "hash (bare)" }.to_owned(),
            span,
        )
        .with(hash_entries(&h.entries)),
        ExprKind::Range(r) => leaf(
            format!("range {}", if r.exclude_end { "..." } else { ".." }),
            span,
        )
        .with(
            r.left
                .iter()
                .map(|e| expr(e).named("from"))
                .chain(r.right.iter().map(|e| expr(e).named("to"))),
        ),
        ExprKind::Splat(inner) => l("splat *").with(inner.iter().map(|i| expr(i))),
        ExprKind::Implicit(inner) => l("implicit").with([expr(inner)]),

        // -- variables ------------------------------------------------------
        ExprKind::Var(v) => leaf(var(v), span),
        ExprKind::ConstPath(p) => const_path(p, span),
        ExprKind::Assign(a) => leaf(format!("assign {}", assign_op(&a.op)), span).with([
            target(&a.target).named("target"),
            expr(&a.value).named("value"),
        ]),

        // -- calls ----------------------------------------------------------
        ExprKind::Call(c) => {
            let dot = if c.flags.safe_nav { "&." } else { "." };
            let label = match &c.receiver {
                Some(_) => format!("call {dot}{}", c.name),
                None => format!("call {}", c.name),
            };
            leaf(label, span).with(
                c.receiver
                    .iter()
                    .map(|r| expr(r).named("recv"))
                    .chain(c.args.iter().map(|a| expr(a).named("arg")))
                    .chain(c.block.iter().map(block_arg)),
            )
        }
        ExprKind::Super(s) => leaf(
            match &s.args {
                None => "super (forwarding)".to_owned(),
                Some(_) => "super".to_owned(),
            },
            span,
        )
        .with(
            s.args
                .iter()
                .flatten()
                .map(|a| expr(a).named("arg"))
                .chain(s.block.iter().map(block_arg)),
        ),
        ExprKind::Yield(y) => l("yield").with(y.args.iter().map(|a| expr(a).named("arg"))),
        ExprKind::Lambda(b) => l("lambda ->").with(block_body(b)),

        // -- control flow ---------------------------------------------------
        ExprKind::If(i) => leaf(if i.unless { "unless" } else { "if" }.to_owned(), span).with(
            std::iter::once(expr(&i.predicate).named("cond"))
                .chain(slots("then", &i.then_body))
                .chain(i.else_body.iter().flat_map(|b| slots("else", b))),
        ),
        ExprKind::While(w) => leaf(
            format!(
                "{}{}",
                if w.until { "until" } else { "while" },
                if w.post { " (post)" } else { "" }
            ),
            span,
        )
        .with(std::iter::once(expr(&w.predicate).named("cond")).chain(slots("body", &w.body))),
        ExprKind::For(f) => l("for").with(
            [
                target(&f.index).named("index"),
                expr(&f.iterable).named("in"),
            ]
            .into_iter()
            .chain(slots("body", &f.body)),
        ),
        ExprKind::Case(c) => {
            let label = match &c.branches {
                CaseBranches::When(_) => "case",
                CaseBranches::In(_) => "case/in",
            };
            let branches: Vec<Node> = match &c.branches {
                CaseBranches::When(whens) => whens
                    .iter()
                    .map(|w| {
                        leaf("when", w.span)
                            .with(w.conditions.iter().map(expr).chain(slots("body", &w.body)))
                    })
                    .collect(),
                CaseBranches::In(ins) => ins
                    .iter()
                    .map(|i| {
                        leaf("in", i.span).with(
                            std::iter::once(expr(&i.pattern))
                                .chain(i.guard.iter().map(|g| match g {
                                    Guard::If(e) => expr(e).named("if"),
                                    Guard::Unless(e) => expr(e).named("unless"),
                                }))
                                .chain(slots("body", &i.body)),
                        )
                    })
                    .collect(),
            };
            leaf(label.to_owned(), span).with(
                c.predicate
                    .iter()
                    .map(|p| expr(p).named("subject"))
                    .chain(branches)
                    .chain(c.else_body.iter().flat_map(|b| slots("else", b))),
            )
        }
        ExprKind::Logical(lg) => leaf(
            match lg.op {
                LogicalOp::And => "and &&",
                LogicalOp::Or => "or ||",
            }
            .to_owned(),
            span,
        )
        .with([expr(&lg.left), expr(&lg.right)]),
        ExprKind::FlipFlop(f) => leaf(
            format!("flipflop {}", if f.exclude_end { "..." } else { ".." }),
            span,
        )
        .with(f.left.iter().chain(f.right.iter()).map(expr)),
        ExprKind::Defined(inner) => l("defined?").with([expr(inner)]),

        // -- jumps ----------------------------------------------------------
        ExprKind::Break(v) => l("break").with(v.iter().map(|e| expr(e))),
        ExprKind::Next(v) => l("next").with(v.iter().map(|e| expr(e))),
        ExprKind::Return(v) => l("return").with(v.iter().map(|e| expr(e))),

        // -- exceptions -----------------------------------------------------
        ExprKind::Begin(b) => l("begin").with(
            slots("body", &b.body)
                .into_iter()
                .chain(b.rescues.iter().map(|r| {
                    leaf("rescue", r.span).with(
                        r.exceptions
                            .iter()
                            .map(|e| expr(e).named("class"))
                            .chain(r.reference.iter().map(|t| target(t).named("=>")))
                            .chain(slots("body", &r.body)),
                    )
                }))
                .chain(b.else_body.iter().flat_map(|e| slots("else", e)))
                .chain(b.ensure_body.iter().flat_map(|e| slots("ensure", e))),
        ),
        ExprKind::RescueMod(r) => {
            l("rescue-modifier").with([expr(&r.value), expr(&r.rescue_value).named("rescue")])
        }

        // -- definitions ----------------------------------------------------
        ExprKind::Def(d) => leaf(
            format!(
                "def {}{}{}",
                if d.receiver.is_some() { "<recv>." } else { "" },
                d.name,
                if d.endless { " (endless)" } else { "" }
            ),
            span,
        )
        .with(
            d.receiver
                .iter()
                .map(|r| expr(r).named("recv"))
                .chain(params(&d.params))
                .chain(slots("body", &d.body)),
        ),
        ExprKind::Class(c) => l("class").with(
            std::iter::once(target(&c.path).named("name"))
                .chain(c.superclass.iter().map(|s| expr(s).named("superclass")))
                .chain(slots("body", &c.body)),
        ),
        ExprKind::Module(m) => l("module")
            .with(std::iter::once(target(&m.path).named("name")).chain(slots("body", &m.body))),
        ExprKind::SingletonClass(s) => l("class <<")
            .with(std::iter::once(expr(&s.expression).named("of")).chain(slots("body", &s.body))),
        ExprKind::Alias(a) => leaf(
            if a.global { "alias (global)" } else { "alias" }.to_owned(),
            span,
        )
        .with([
            expr(&a.new_name).named("new"),
            expr(&a.old_name).named("old"),
        ]),
        ExprKind::Undef(names) => l("undef").with(names.iter().map(expr)),

        // -- structure ------------------------------------------------------
        ExprKind::Parens(body) => l("parens").with(body.iter().map(expr)),
        ExprKind::Exec(x) => leaf(
            match x.kind {
                ExecKind::Pre => "BEGIN",
                ExecKind::Post => "END",
            }
            .to_owned(),
            span,
        )
        .with(x.body.iter().map(expr)),
        ExprKind::ShareableConstant(s) => leaf(
            format!(
                "shareable_constant_value: {}",
                match s.mode {
                    ShareableMode::Literal => "literal",
                    ShareableMode::ExperimentalEverything => "experimental_everything",
                    ShareableMode::ExperimentalCopy => "experimental_copy",
                }
            ),
            span,
        )
        .with([expr(&s.write)]),

        // -- pattern matching -----------------------------------------------
        ExprKind::MatchPattern(m) => leaf(
            if m.raises { "match => " } else { "match in" }.to_owned(),
            span,
        )
        .with([
            expr(&m.value).named("value"),
            expr(&m.pattern).named("pattern"),
        ]),
        ExprKind::MatchWrite(m) => l("match-write").with(
            std::iter::once(expr(&m.call))
                .chain(m.targets.iter().map(|t| target(t).named("target"))),
        ),
        ExprKind::ArrayPattern(p) => l("array-pattern").with(
            p.constant
                .iter()
                .map(|c| expr(c).named("const"))
                .chain(p.requireds.iter().map(expr))
                .chain(p.rest.iter().map(|r| expr(r).named("rest")))
                .chain(p.posts.iter().map(|e| expr(e).named("post"))),
        ),
        ExprKind::FindPattern(p) => l("find-pattern").with(
            p.constant
                .iter()
                .map(|c| expr(c).named("const"))
                .chain([expr(&p.left).named("pre")])
                .chain(p.requireds.iter().map(expr))
                .chain([expr(&p.right).named("post")]),
        ),
        ExprKind::HashPattern(p) => l("hash-pattern").with(
            p.constant
                .iter()
                .map(|c| expr(c).named("const"))
                .chain(hash_entries(&p.elements))
                .chain(p.rest.iter().map(|r| match r {
                    PatternRest::Splat(Some(e)) => expr(e).named("rest"),
                    PatternRest::Splat(None) => group("rest: **", vec![]),
                    PatternRest::Forbidden => group("rest: **nil", vec![]),
                })),
        ),
        ExprKind::AltPattern(p) => l("alt-pattern |").with([expr(&p.left), expr(&p.right)]),
        ExprKind::CapturePattern(p) => {
            l("capture-pattern =>").with([expr(&p.value), target(&p.target).named("as")])
        }
        ExprKind::Pin(inner) => l("pin ^").with([expr(inner)]),
    }
}

fn literal(kind: &str, s: &StrLit, span: Span) -> Node {
    let frozen = match s.frozen {
        Some(true) => " frozen",
        Some(false) => " mutable",
        None => "",
    };
    match inline_text(&s.parts) {
        Some(text) => leaf(format!("{kind} {text:?}{frozen}"), span),
        None => leaf(format!("{kind}{frozen}"), span).with(str_parts(&s.parts)),
    }
}

fn regexp(kind: &str, r: &RegexpLit, span: Span) -> Node {
    let mut flags = String::new();
    if r.flags.ignore_case {
        flags.push('i');
    }
    if r.flags.multi_line {
        flags.push('m');
    }
    if r.flags.extended {
        flags.push('x');
    }
    if r.flags.once {
        flags.push('o');
    }
    match inline_text(&r.parts) {
        Some(text) => leaf(format!("{kind} /{text}/{flags}"), span),
        None => leaf(format!("{kind} //{flags}"), span).with(str_parts(&r.parts)),
    }
}
