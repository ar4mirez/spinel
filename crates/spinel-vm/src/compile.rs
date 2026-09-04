//! `spinel_ast` → [`Iseq`].
//!
//! The scope is the part of Ruby that needs no calling convention: literals,
//! local variables, `if`/`unless`, `while`/`until`, `case`/`when`, `break` and
//! `next` inside a loop, the logical operators, and the specialised arithmetic
//! and comparison operators. Everything else is [`Unsupported`].
//!
//! # `Unsupported` is the whole safety property
//!
//! A node this slice does not implement returns an error naming the node and its
//! span. It never compiles to something approximate. `spec/harness` turns that
//! error into a `blocked` example, so there is no path from an unimplemented
//! construct to a *passing* spec — which is the reason
//! [#5](https://github.com/ar4mirez/spinel/issues/5) shipped a `blocked` column
//! rather than matchers it could not honour.
//!
//! # Locals
//!
//! Prism has already decided which bare identifiers are locals and which are
//! method calls, and the lowering keeps that as [`VarRef::Local`] versus a
//! [`Call`] with `variable_call`. So slot assignment is only a matter of giving
//! each name an index: the scope's declared list first, in the parser's order,
//! and any name a target introduces that the list somehow missed appended after.
//! A local read before its assignment is `nil`, which is Ruby's rule and falls
//! out of frames starting zeroed.
//!
//! [`Call`]: spinel_ast::Call

use spinel_ast::{
    Assign, AssignOp, Case, CaseBranches, Expr, ExprKind, If, IntValue, Logical, LogicalOp, Name,
    Program, Span, StrPart, Target, TargetKind, VarRef, While,
};

use crate::bytecode::{BinOp, Insn, Iseq, Literal};
use crate::value::Value;

/// A node this slice does not compile.
///
/// Carries what it was and where, because the harness prints it as the reason
/// an example is blocked and that reason is how the next slice is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    /// The construct, as a Ruby programmer would name it.
    pub node: &'static str,
    pub span: Span,
}

impl Unsupported {
    fn at(node: &'static str, span: Span) -> Unsupported {
        Unsupported { node, span }
    }
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is not compiled yet", self.node)
    }
}

impl std::error::Error for Unsupported {}

type Emit = Result<(), Unsupported>;

/// Compile a whole parsed file.
pub fn program(program: &Program) -> Result<Iseq, Unsupported> {
    body("<main>", &program.locals, &program.body)
}

/// Compile a statement list that shares one scope: a script, or — until
/// [#11](https://github.com/ar4mirez/spinel/issues/11) — a block body the
/// harness runs directly.
pub fn body(name: &str, locals: &[Name], statements: &[Expr]) -> Result<Iseq, Unsupported> {
    let mut compiler = Compiler::new(name, locals);
    compiler.statements(statements, true)?;
    Ok(compiler.finish())
}

/// Compile one expression against a known set of local slots.
///
/// The harness uses this to evaluate the two halves of `x.should == y` in a
/// frame both halves share, which is what keeps an example's locals alive from
/// one statement to the next without the VM knowing what a matcher is.
pub fn expression(name: &str, locals: &[Name], expr: &Expr) -> Result<Iseq, Unsupported> {
    let mut compiler = Compiler::new(name, locals);
    compiler.expr(expr)?;
    Ok(compiler.finish())
}

/// Every local name that appears as an assignment target anywhere in `body`.
///
/// The parser's own scope list is the authority and is used first; this is the
/// belt to its braces, and it is also what lets the harness pre-size a frame
/// that several separately compiled expressions share.
#[must_use]
pub fn declared_locals(statements: &[Expr]) -> Vec<Name> {
    let mut out: Vec<Name> = Vec::new();
    for statement in statements {
        collect_locals(statement, &mut out);
    }
    out
}

// ---------------------------------------------------------------------------
// The compiler
// ---------------------------------------------------------------------------

/// Where `break` and `next` go. Nested loops push and pop these.
struct Loop {
    /// The stack depth at the top of the loop.
    ///
    /// `break` and `next` always run at exactly this depth, and that is Ruby's
    /// doing rather than luck: `[1, (break 2)]` and `x = (next)` are both
    /// *"unexpected void value expression"* at parse time, so a jump can never
    /// leave a half-finished expression behind. Asserted rather than
    /// compensated for, because compensating would hide a lowering bug — a
    /// `when` body that forgot to drop its subject would silently leak a value
    /// per iteration instead of failing a test.
    base_depth: usize,
    /// Instruction index `next` jumps to: the predicate of a `while`, the body
    /// of a `begin ... end while`.
    next_target: usize,
    /// Jump instructions emitted by `break`, patched when the loop's end is
    /// known.
    breaks: Vec<usize>,
}

struct Compiler {
    name: Box<str>,
    insns: Vec<Insn>,
    literals: Vec<Literal>,
    symbols: Vec<Box<str>>,
    locals: Vec<Box<str>>,
    loops: Vec<Loop>,
    depth: usize,
    max_stack: usize,
}

impl Compiler {
    fn new(name: &str, locals: &[Name]) -> Compiler {
        Compiler {
            name: name.into(),
            insns: Vec::new(),
            literals: Vec::new(),
            symbols: Vec::new(),
            locals: locals.to_vec(),
            loops: Vec::new(),
            depth: 0,
            max_stack: 0,
        }
    }

    fn finish(mut self) -> Iseq {
        self.emit(Insn::Leave);
        Iseq {
            name: self.name,
            insns: self.insns,
            literals: self.literals,
            symbols: self.symbols,
            locals: self.locals,
            // `Leave` reads the top of the stack, so a frame always needs room
            // for one value even when the body pushed nothing.
            max_stack: self.max_stack.max(1),
        }
    }

    // -- emission ---------------------------------------------------------

    /// How much an instruction changes the stack depth.
    ///
    /// This is what makes `max_stack` exact rather than a guess. It is correct
    /// only because every lowering below leaves both sides of a branch at the
    /// same depth; a lowering that did not would be a bug this function cannot
    /// see, which is why the branch depths are asserted at each merge point.
    const fn effect(insn: Insn) -> isize {
        match insn {
            Insn::PushNil
            | Insn::PushTrue
            | Insn::PushFalse
            | Insn::PushSelf
            | Insn::PushInt(_)
            | Insn::PushLit(_)
            | Insn::PushSym(_)
            | Insn::Dup => 1,
            Insn::Pop
            | Insn::SetLocal(_)
            | Insn::JumpUnless(_)
            | Insn::JumpIf(_)
            | Insn::BinOp(_)
            | Insn::CaseEq
            | Insn::Leave => -1,
            Insn::GetLocal(_) => 1,
            Insn::Jump(_)
            | Insn::JumpUnlessKeep(_)
            | Insn::JumpIfKeep(_)
            | Insn::Neg
            | Insn::Not => 0,
            // Pops `n`, pushes the array.
            Insn::NewArray(n) => 1 - n as isize,
        }
    }

    fn emit(&mut self, insn: Insn) {
        let depth = self.depth as isize + Self::effect(insn);
        debug_assert!(depth >= 0, "{insn:?} underflows the stack in {}", self.name);
        self.depth = depth.max(0) as usize;
        self.max_stack = self.max_stack.max(self.depth);
        self.insns.push(insn);
    }

    fn here(&self) -> usize {
        self.insns.len()
    }

    /// Emit a jump whose target is not known yet; returns its index for
    /// [`Compiler::patch`].
    fn emit_jump(&mut self, jump: fn(i32) -> Insn) -> usize {
        let at = self.here();
        self.emit(jump(0));
        at
    }

    /// The one place a displacement is computed. Relative to the instruction
    /// *after* the jump, which is what makes an `Iseq` relocatable.
    fn patch(&mut self, at: usize, target: usize) {
        let displacement = target as isize - (at as isize + 1);
        let displacement = i32::try_from(displacement).expect("iseq larger than 2 GiB");
        self.insns[at] = match self.insns[at] {
            Insn::Jump(_) => Insn::Jump(displacement),
            Insn::JumpUnless(_) => Insn::JumpUnless(displacement),
            Insn::JumpIf(_) => Insn::JumpIf(displacement),
            Insn::JumpUnlessKeep(_) => Insn::JumpUnlessKeep(displacement),
            Insn::JumpIfKeep(_) => Insn::JumpIfKeep(displacement),
            other => unreachable!("{other:?} is not a jump"),
        };
    }

    fn patch_here(&mut self, at: usize) {
        let target = self.here();
        self.patch(at, target);
    }

    fn literal(&mut self, literal: Literal) -> u32 {
        self.literals.push(literal);
        (self.literals.len() - 1) as u32
    }

    fn symbol(&mut self, name: &str) -> u32 {
        if let Some(index) = self.symbols.iter().position(|s| &**s == name) {
            return index as u32;
        }
        self.symbols.push(name.into());
        (self.symbols.len() - 1) as u32
    }

    fn slot(&mut self, name: &str) -> u16 {
        if let Some(index) = self.locals.iter().position(|l| &**l == name) {
            return index as u16;
        }
        self.locals.push(name.into());
        (self.locals.len() - 1) as u16
    }

    // -- statements -------------------------------------------------------

    /// A statement list. Every expression pushes exactly one value, so the ones
    /// whose value is discarded are followed by `Pop`. An empty list is `nil`,
    /// which is what makes `if ()` falsy without a special case.
    fn statements(&mut self, statements: &[Expr], keep: bool) -> Emit {
        let Some((last, rest)) = statements.split_last() else {
            if keep {
                self.emit(Insn::PushNil);
            }
            return Ok(());
        };
        for statement in rest {
            self.expr(statement)?;
            self.emit(Insn::Pop);
        }
        self.expr(last)?;
        if !keep {
            self.emit(Insn::Pop);
        }
        Ok(())
    }

    // -- expressions ------------------------------------------------------

    /// Compile one expression. Always leaves exactly one value on the stack.
    fn expr(&mut self, expr: &Expr) -> Emit {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Nil => self.emit(Insn::PushNil),
            ExprKind::True => self.emit(Insn::PushTrue),
            ExprKind::False => self.emit(Insn::PushFalse),
            ExprKind::SelfExpr => self.emit(Insn::PushSelf),

            ExprKind::Int(int) => match &int.value {
                IntValue::Small(n) => match Value::fixnum(*n) {
                    Some(_) => self.emit(Insn::PushInt(*n)),
                    // Past 2^62 an Integer is a heap bignum, and there is no
                    // bignum. Refusing beats a wrapped answer.
                    None => return Err(Unsupported::at("an integer wider than a fixnum", span)),
                },
                IntValue::Big(_) => {
                    return Err(Unsupported::at("an integer wider than a fixnum", span));
                }
            },

            ExprKind::Float(f) => {
                let literal = if Value::flonum(*f).is_some() {
                    Literal::Float(*f)
                } else {
                    Literal::BoxedFloat(*f)
                };
                let index = self.literal(literal);
                self.emit(Insn::PushLit(index));
            }

            ExprKind::Str(string) => {
                let bytes = flat_bytes(&string.parts)
                    .ok_or_else(|| Unsupported::at("string interpolation", span))?;
                let index = self.literal(Literal::Str(bytes));
                self.emit(Insn::PushLit(index));
            }

            ExprKind::Sym(symbol) => {
                let bytes = flat_bytes(&symbol.parts)
                    .ok_or_else(|| Unsupported::at("symbol interpolation", span))?;
                let name = String::from_utf8(bytes.into_vec())
                    .map_err(|_| Unsupported::at("a symbol that is not UTF-8", span))?;
                let index = self.symbol(&name);
                self.emit(Insn::PushSym(index));
            }

            ExprKind::Array(elements) => {
                for element in elements {
                    if matches!(element.kind, ExprKind::Splat(_)) {
                        return Err(Unsupported::at("a splat in an array literal", element.span));
                    }
                    self.expr(element)?;
                }
                let count = u32::try_from(elements.len())
                    .map_err(|_| Unsupported::at("an array literal this large", span))?;
                self.emit(Insn::NewArray(count));
            }

            ExprKind::Var(var) => self.var(var, span)?,
            ExprKind::Assign(assign) => self.assign(assign, span)?,
            ExprKind::If(node) => self.if_expr(node)?,
            ExprKind::While(node) => self.while_expr(node)?,
            ExprKind::Case(node) => self.case_expr(node, span)?,
            ExprKind::Logical(node) => self.logical(node)?,
            ExprKind::Parens(statements) => self.statements(statements, true)?,
            ExprKind::Call(call) => self.call(call, span)?,

            // A bare `begin ... end` is a grouping, not an exception handler,
            // and `begin ... end while c` is how Ruby spells a do-while. Only
            // the handler forms need #12.
            ExprKind::Begin(node)
                if node.rescues.is_empty()
                    && node.else_body.is_none()
                    && node.ensure_body.is_none() =>
            {
                self.statements(&node.body, true)?;
            }

            ExprKind::Break(value) => self.jump_out(value.as_deref(), true, span)?,
            ExprKind::Next(value) => self.jump_out(value.as_deref(), false, span)?,

            other => return Err(Unsupported::at(node_name(other), span)),
        }
        Ok(())
    }

    fn var(&mut self, var: &VarRef, span: Span) -> Emit {
        match var {
            VarRef::Local { name, depth: 0 } => {
                let slot = self.slot(name);
                self.emit(Insn::GetLocal(slot));
                Ok(())
            }
            // A local from an enclosing scope needs the environment pointer that
            // arrives with blocks, in #11.
            VarRef::Local { .. } => Err(Unsupported::at("a captured local variable", span)),
            VarRef::Instance(_) => Err(Unsupported::at("an instance variable", span)),
            VarRef::Class(_) => Err(Unsupported::at("a class variable", span)),
            VarRef::Global(_) => Err(Unsupported::at("a global variable", span)),
            VarRef::Const(_) => Err(Unsupported::at("a constant", span)),
            VarRef::It => Err(Unsupported::at("the implicit block parameter `it`", span)),
            VarRef::BackRef(_) | VarRef::NumberedRef(_) => {
                Err(Unsupported::at("a regexp back-reference", span))
            }
        }
    }

    fn assign(&mut self, assign: &Assign, span: Span) -> Emit {
        let slot = self.local_target(&assign.target)?;
        match &assign.op {
            AssignOp::Assign => {
                self.expr(&assign.value)?;
                self.emit(Insn::Dup);
                self.emit(Insn::SetLocal(slot));
            }
            AssignOp::Binary(op) => {
                let op = BinOp::from_name(op)
                    .ok_or_else(|| Unsupported::at("this compound assignment operator", span))?;
                self.emit(Insn::GetLocal(slot));
                self.expr(&assign.value)?;
                self.emit(Insn::BinOp(op));
                self.emit(Insn::Dup);
                self.emit(Insn::SetLocal(slot));
            }
            // `a ||= v` reads `a`, and assigns only when it is falsy. The read
            // is safe before any write because a frame's locals start `nil`,
            // which is also Ruby's answer for a declared-but-unassigned local.
            AssignOp::Or | AssignOp::And => {
                let keep = if matches!(assign.op, AssignOp::Or) {
                    Insn::JumpIfKeep as fn(i32) -> Insn
                } else {
                    Insn::JumpUnlessKeep as fn(i32) -> Insn
                };
                self.emit(Insn::GetLocal(slot));
                let skip = self.emit_jump(keep);
                self.emit(Insn::Pop);
                self.expr(&assign.value)?;
                self.emit(Insn::Dup);
                self.emit(Insn::SetLocal(slot));
                self.patch_here(skip);
            }
        }
        Ok(())
    }

    /// The only assignment target this slice writes to.
    fn local_target(&mut self, target: &Target) -> Result<u16, Unsupported> {
        match &target.kind {
            TargetKind::Var(VarRef::Local { name, depth: 0 }) => Ok(self.slot(name)),
            TargetKind::Var(VarRef::Local { .. }) => Err(Unsupported::at(
                "assigning a captured local variable",
                target.span,
            )),
            TargetKind::Var(VarRef::Instance(_)) => Err(Unsupported::at(
                "assigning an instance variable",
                target.span,
            )),
            TargetKind::Var(VarRef::Class(_)) => {
                Err(Unsupported::at("assigning a class variable", target.span))
            }
            TargetKind::Var(VarRef::Global(_)) => {
                Err(Unsupported::at("assigning a global variable", target.span))
            }
            TargetKind::Var(_) | TargetKind::ConstPath(_) => {
                Err(Unsupported::at("assigning a constant", target.span))
            }
            TargetKind::Call(_) => Err(Unsupported::at("an attribute assignment", target.span)),
            TargetKind::Index(_) => Err(Unsupported::at("an index assignment", target.span)),
            TargetKind::Multi(_) | TargetKind::Splat(_) => {
                Err(Unsupported::at("a multiple assignment", target.span))
            }
        }
    }

    fn if_expr(&mut self, node: &If) -> Emit {
        self.expr(&node.predicate)?;
        // `unless` is the same shape with the arms swapped. The AST keeps the
        // flag rather than negating the predicate, so the formatter can reprint
        // the source; the compiler is where the two finally become one thing.
        let (then_body, else_body) = if node.unless {
            (
                node.else_body.as_deref().unwrap_or(&[]),
                Some(&node.then_body[..]),
            )
        } else {
            (&node.then_body[..], node.else_body.as_deref())
        };

        let to_else = self.emit_jump(Insn::JumpUnless);
        let before = self.depth;
        self.statements(then_body, true)?;
        let after_then = self.depth;
        let to_end = self.emit_jump(Insn::Jump);

        self.patch_here(to_else);
        // Both arms start from the same depth and must reach the same one.
        self.depth = before;
        match else_body {
            Some(body) => self.statements(body, true)?,
            None => self.emit(Insn::PushNil),
        }
        debug_assert_eq!(self.depth, after_then, "if arms disagree about stack depth");
        self.patch_here(to_end);
        Ok(())
    }

    fn while_expr(&mut self, node: &While) -> Emit {
        let test = if node.until {
            Insn::JumpIf as fn(i32) -> Insn
        } else {
            Insn::JumpUnless as fn(i32) -> Insn
        };

        // `begin ... end while c` runs the body once before testing, so `next`
        // goes to the *body* there and to the *predicate* in the ordinary form.
        let top = self.here();
        self.loops.push(Loop {
            base_depth: self.depth,
            next_target: top,
            breaks: Vec::new(),
        });

        let result = self.while_body(node, test, top);
        let loop_frame = self.loops.pop().expect("loop frame");
        result?;

        // A loop is `nil` when its condition ends it. `break v` jumps past this
        // push with `v` already on the stack, so both exits leave one value.
        self.emit(Insn::PushNil);
        for at in loop_frame.breaks {
            self.patch_here(at);
        }
        Ok(())
    }

    fn while_body(&mut self, node: &While, test: fn(i32) -> Insn, top: usize) -> Emit {
        if node.post {
            let body = self.here();
            self.statements(&node.body, false)?;
            // `next` in a post-condition loop re-tests rather than re-running
            // the body, so the frame's target moves to the predicate.
            let predicate = self.here();
            if let Some(frame) = self.loops.last_mut() {
                frame.next_target = predicate;
            }
            self.expr(&node.predicate)?;
            let back = self.emit_jump(if node.until {
                Insn::JumpUnless
            } else {
                Insn::JumpIf
            });
            self.patch(back, body);
        } else {
            self.expr(&node.predicate)?;
            let out = self.emit_jump(test);
            self.statements(&node.body, false)?;
            let back = self.emit_jump(Insn::Jump);
            self.patch(back, top);
            self.patch_here(out);
        }
        Ok(())
    }

    /// `break` and `next`, which are ordinary jumps as long as the loop is in
    /// this frame. Out of a *block* they are non-local exits, and those are
    /// [#12](https://github.com/ar4mirez/spinel/issues/12)'s.
    fn jump_out(&mut self, value: Option<&Expr>, is_break: bool, span: Span) -> Emit {
        // Out of a *block* rather than a loop, both are non-local exits through
        // the unwinding path, which is #12's.
        let Some(&Loop {
            base_depth,
            next_target,
            ..
        }) = self.loops.last()
        else {
            return Err(Unsupported::at(
                if is_break {
                    "`break` outside a loop"
                } else {
                    "`next` outside a loop"
                },
                span,
            ));
        };

        debug_assert_eq!(
            self.depth, base_depth,
            "a jump out of a loop would leave a value behind"
        );

        if is_break {
            match value {
                Some(value) => self.expr(value)?,
                None => self.emit(Insn::PushNil),
            }
            let at = self.emit_jump(Insn::Jump);
            self.loops.last_mut().expect("loop frame").breaks.push(at);
            // The jump leaves for the loop's end, so the value goes with it.
            self.depth -= 1;
        } else {
            // `next v` discards `v`; it is only evaluated for its effects.
            if let Some(value) = value {
                self.expr(value)?;
                self.emit(Insn::Pop);
            }
            let at = self.emit_jump(Insn::Jump);
            self.patch(at, next_target);
        }

        // Unreachable — both are jumps — but every expression must leave one
        // value for the statement list containing it, and keeping that true
        // keeps the depth arithmetic uniform for the code after the loop.
        self.emit(Insn::PushNil);
        Ok(())
    }

    fn case_expr(&mut self, node: &Case, span: Span) -> Emit {
        let CaseBranches::When(clauses) = &node.branches else {
            return Err(Unsupported::at("`case`/`in` pattern matching", span));
        };

        // With a subject, every test is `condition === subject` and the subject
        // sits under the tests until a branch claims it. Without one, `case`
        // is a chain of truthiness tests and there is nothing to clean up.
        let subject = node.predicate.is_some();
        if let Some(predicate) = &node.predicate {
            self.expr(predicate)?;
        }

        let tests_depth = self.depth;
        let mut bodies = Vec::with_capacity(clauses.len());
        for clause in clauses {
            let mut entries = Vec::with_capacity(clause.conditions.len());
            for condition in &clause.conditions {
                if matches!(condition.kind, ExprKind::Splat(_)) {
                    return Err(Unsupported::at("a splat in `when`", condition.span));
                }
                if subject {
                    self.emit(Insn::Dup);
                }
                self.expr(condition)?;
                if subject {
                    self.emit(Insn::CaseEq);
                }
                entries.push(self.emit_jump(Insn::JumpIf));
            }
            bodies.push((entries, &clause.body));
        }

        // Nothing matched.
        debug_assert_eq!(self.depth, tests_depth);
        if subject {
            self.emit(Insn::Pop);
        }
        match &node.else_body {
            Some(body) => self.statements(body, true)?,
            None => self.emit(Insn::PushNil),
        }
        let end_depth = self.depth;
        let mut to_end = vec![self.emit_jump(Insn::Jump)];

        for (entries, body) in bodies {
            let start = self.here();
            for at in entries {
                self.patch(at, start);
            }
            self.depth = tests_depth;
            if subject {
                self.emit(Insn::Pop);
            }
            self.statements(body, true)?;
            debug_assert_eq!(self.depth, end_depth, "`when` arms disagree about depth");
            to_end.push(self.emit_jump(Insn::Jump));
        }

        for at in to_end {
            self.patch_here(at);
        }
        self.depth = end_depth;
        Ok(())
    }

    fn logical(&mut self, node: &Logical) -> Emit {
        // `a && b` is `a` when `a` is falsy, so the left value stays on the
        // stack rather than being recomputed — the `Keep` jumps exist for this.
        let keep = match node.op {
            LogicalOp::And => Insn::JumpUnlessKeep as fn(i32) -> Insn,
            LogicalOp::Or => Insn::JumpIfKeep as fn(i32) -> Insn,
        };
        self.expr(&node.left)?;
        let short = self.emit_jump(keep);
        self.emit(Insn::Pop);
        self.expr(&node.right)?;
        self.patch_here(short);
        Ok(())
    }

    /// The only calls this slice compiles are the operators it specialises.
    ///
    /// Everything else needs [#11](https://github.com/ar4mirez/spinel/issues/11).
    fn call(&mut self, call: &spinel_ast::Call, span: Span) -> Emit {
        if call.block.is_some() {
            return Err(Unsupported::at("a method call with a block", span));
        }
        let Some(receiver) = &call.receiver else {
            return Err(Unsupported::at("a method call", span));
        };
        if call.flags.safe_nav {
            return Err(Unsupported::at("a safe-navigation call", span));
        }

        match (&*call.name, call.args.len()) {
            ("!", 0) => {
                self.expr(receiver)?;
                self.emit(Insn::Not);
            }
            ("-@", 0) => {
                self.expr(receiver)?;
                self.emit(Insn::Neg);
            }
            ("+@", 0) => self.expr(receiver)?,
            (name, 1) => {
                let op =
                    BinOp::from_name(name).ok_or_else(|| Unsupported::at("a method call", span))?;
                if matches!(call.args[0].kind, ExprKind::Splat(_) | ExprKind::Hash(_)) {
                    return Err(Unsupported::at("a method call", span));
                }
                self.expr(receiver)?;
                self.expr(&call.args[0])?;
                self.emit(Insn::BinOp(op));
            }
            _ => return Err(Unsupported::at("a method call", span)),
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The bytes of a literal with no interpolation, or `None` if it has any.
fn flat_bytes(parts: &[StrPart]) -> Option<Box<[u8]>> {
    let mut out = Vec::new();
    for part in parts {
        match part {
            StrPart::Bytes(bytes) => out.extend_from_slice(bytes),
            StrPart::Interp(_) => return None,
        }
    }
    Some(out.into_boxed_slice())
}

fn collect_locals(expr: &Expr, out: &mut Vec<Name>) {
    if let ExprKind::Assign(assign) = &expr.kind
        && let TargetKind::Var(VarRef::Local { name, depth: 0 }) = &assign.target.kind
        && !out.iter().any(|existing| existing == name)
    {
        out.push(name.clone());
    }
    for child in children(expr) {
        collect_locals(child, out);
    }
}

/// The sub-expressions of a node, for the local scan. Deliberately shallow:
/// only the forms this slice compiles can introduce a local it will read.
fn children(expr: &Expr) -> Vec<&Expr> {
    match &expr.kind {
        ExprKind::Assign(assign) => vec![&assign.value],
        ExprKind::If(node) => node
            .then_body
            .iter()
            .chain(node.else_body.iter().flatten())
            .chain(std::iter::once(&node.predicate))
            .collect(),
        ExprKind::While(node) => node
            .body
            .iter()
            .chain(std::iter::once(&node.predicate))
            .collect(),
        ExprKind::Case(node) => {
            let mut out: Vec<&Expr> = node.predicate.iter().collect();
            if let CaseBranches::When(clauses) = &node.branches {
                for clause in clauses {
                    out.extend(clause.conditions.iter());
                    out.extend(clause.body.iter());
                }
            }
            out.extend(node.else_body.iter().flatten());
            out
        }
        ExprKind::Logical(node) => vec![&node.left, &node.right],
        ExprKind::Parens(statements) => statements.iter().collect(),
        ExprKind::Begin(node) => node.body.iter().collect(),
        ExprKind::Array(elements) => elements.iter().collect(),
        ExprKind::Call(call) => call.receiver.iter().chain(call.args.iter()).collect(),
        ExprKind::Break(value) | ExprKind::Next(value) | ExprKind::Return(value) => {
            value.iter().map(|v| &**v).collect()
        }
        _ => Vec::new(),
    }
}

/// What to call a node in the "not compiled yet" message. Reads as Ruby, not as
/// an AST variant, because the reason lands in a spec report a human triages.
fn node_name(kind: &ExprKind) -> &'static str {
    match kind {
        ExprKind::Def(_) => "a method definition",
        ExprKind::Class(_) | ExprKind::Module(_) | ExprKind::SingletonClass(_) => {
            "a class or module body"
        }
        ExprKind::Begin(_) | ExprKind::RescueMod(_) => "`begin`/`rescue`",
        ExprKind::Yield(_) => "`yield`",
        ExprKind::Super(_) => "`super`",
        ExprKind::Lambda(_) => "a lambda",
        ExprKind::Return(_) => "`return`",
        ExprKind::Redo => "`redo`",
        ExprKind::Retry => "`retry`",
        ExprKind::For(_) => "a `for` loop",
        ExprKind::Hash(_) => "a hash literal",
        ExprKind::Range(_) => "a range literal",
        ExprKind::Regexp(_) | ExprKind::MatchLastLine(_) => "a regexp",
        ExprKind::Defined(_) => "`defined?`",
        ExprKind::ConstPath(_) => "a constant path",
        ExprKind::Splat(_) => "a splat",
        ExprKind::Rational(_) | ExprKind::Imaginary(_) => "a rational or complex literal",
        ExprKind::XStr(_) => "a backtick command",
        ExprKind::MatchPattern(_)
        | ExprKind::MatchWrite(_)
        | ExprKind::ArrayPattern(_)
        | ExprKind::FindPattern(_)
        | ExprKind::HashPattern(_)
        | ExprKind::AltPattern(_)
        | ExprKind::CapturePattern(_)
        | ExprKind::Pin(_) => "pattern matching",
        ExprKind::FlipFlop(_) => "a flip-flop",
        ExprKind::Alias(_) | ExprKind::Undef(_) => "`alias` or `undef`",
        ExprKind::Exec(_) => "`BEGIN`/`END`",
        ExprKind::ShareableConstant(_) => "a shareable-constant comment",
        ExprKind::SourceFile(_) | ExprKind::SourceLine | ExprKind::SourceEncoding => {
            "a source-position keyword"
        }
        ExprKind::ForwardingArgs => "argument forwarding",
        ExprKind::Implicit(_) => "an elided hash value",
        ExprKind::Missing => "a syntax error",
        _ => "this expression",
    }
}
