//! `spinel_ast` → [`Iseq`].
//!
//! Literals, local variables, `if`/`unless`, `while`/`until`, `case`/`when`,
//! `break` and `next` inside a loop, the logical operators, the specialised
//! arithmetic and comparison operators, and — since
//! [#11](https://github.com/ar4mirez/spinel/issues/11) — `def`, calls, blocks,
//! `yield`, and `->`. Everything else is [`Unsupported`].
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
//! Prism also decides how many scopes up a name lives, which is the other half.
//! A block compiler carries the enclosing scopes' name lists so that a depth can
//! become a slot; a method body carries none, because it cannot see them.
//!
//! [`Call`]: spinel_ast::Call

use std::sync::Arc;

use spinel_ast::{
    Assign, AssignOp, BlockArg, Case, CaseBranches, Expr, ExprKind, If, IntValue, Logical,
    LogicalOp, Name, ParamList, Params, Program, Span, StrPart, Target, TargetKind, VarRef, While,
};

use crate::bytecode::{
    BinOp, BlockRef, CallSite, ClassDef, ConstScope, DefKind, Insn, Iseq, Keyword, Literal,
    Optional, ParamSpec,
};
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

/// The slot name for the implicit block parameter.
///
/// A real local called `it` shadows it, and Prism has already decided which is
/// which — a shadowed one arrives as [`VarRef::Local`], not [`VarRef::It`], so
/// sharing the name here cannot collide with a user's variable.
const IT: &str = "it";

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
    children: Vec<Arc<Iseq>>,
    call_sites: Vec<CallSite>,
    definitions: Vec<(u32, u32)>,
    class_defs: Vec<ClassDef>,
    params: ParamSpec,
    /// A method body starts a new scope; a block body continues the enclosing
    /// one. `GetLocal` with a depth may not cross a barrier.
    scope_barrier: bool,
    /// A lambda body: shares a block's scope rules and a method's `return`.
    is_lambda_body: bool,
    /// The local-name lists of the enclosing scopes, innermost first, so a
    /// `VarRef::Local { depth }` can be turned into a slot number. Prism has
    /// already done the hard half by deciding the depth; this is the lookup.
    outer: Vec<Vec<Box<str>>>,
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
            children: Vec::new(),
            call_sites: Vec::new(),
            definitions: Vec::new(),
            class_defs: Vec::new(),
            params: ParamSpec::default(),
            scope_barrier: true,
            is_lambda_body: false,
            outer: Vec::new(),
        }
    }

    /// A compiler for a nested scope: a method body, a block, or a lambda.
    ///
    /// `barrier` is the difference between the two kinds. A method body cannot
    /// see the locals it is written inside; a block can, and carries the
    /// enclosing scopes' name lists so a depth can become a slot.
    fn nested(name: &str, locals: &[Name], parent: &Compiler, barrier: bool) -> Compiler {
        let mut compiler = Compiler::new(name, locals);
        compiler.scope_barrier = barrier;
        if !barrier {
            compiler.outer.push(parent.locals.clone());
            compiler.outer.extend(parent.outer.iter().cloned());
        }
        compiler
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
            params: self.params,
            children: self.children,
            call_sites: self.call_sites,
            definitions: self.definitions,
            class_defs: self.class_defs,
            scope_barrier: self.scope_barrier,
        }
    }

    // -- emission ---------------------------------------------------------

    /// How much an instruction changes the stack depth.
    ///
    /// This is what makes `max_stack` exact rather than a guess. It is correct
    /// only because every lowering below leaves both sides of a branch at the
    /// same depth; a lowering that did not would be a bug this function cannot
    /// see, which is why the branch depths are asserted at each merge point.
    fn effect(&self, insn: Insn) -> isize {
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
            | Insn::SetLocal(_, _)
            | Insn::JumpUnless(_)
            | Insn::JumpIf(_)
            | Insn::JumpUnlessUndef(_)
            | Insn::BinOp(_)
            | Insn::CaseEq
            | Insn::Leave => -1,
            Insn::GetLocal(_, _) | Insn::MakeProc(_, _) => 1,
            // `Return` pops its value at run time, but control does not fall
            // through to whatever follows. Counting it as neutral keeps the
            // linear depth model sane across the unreachable code after it,
            // and leaves `max_stack` one too large rather than one too small —
            // which is the safe direction for a frame's capacity.
            Insn::Return => 0,
            Insn::Jump(_)
            | Insn::JumpUnlessKeep(_)
            | Insn::JumpIfKeep(_)
            | Insn::Neg
            | Insn::Not
            // Pops the receiver, pushes the name it defined.
            | Insn::DefineSingleton(_)
            // Pops the module, pushes the constant.
            | Insn::GetConst(_, ConstScope::Qualified)
            | Insn::DefinedConst(_, ConstScope::Qualified)
            // Pops the receiver, pushes the answer.
            | Insn::DefinedMethod(_) => 0,
            // Defines on the lexical scope, so it takes no receiver; pushes the
            // name it defined.
            Insn::DefineMethod(_)
            | Insn::GetConst(_, ConstScope::Lexical | ConstScope::Top)
            | Insn::DefinedConst(_, ConstScope::Lexical | ConstScope::Top)
            | Insn::DefinedSelfMethod(_)
            | Insn::DefinedYield => 1,
            // Leaves the assigned value, having consumed the module under it.
            Insn::SetConst(_, ConstScope::Qualified) => -1,
            Insn::SetConst(_, ConstScope::Lexical | ConstScope::Top) => 0,
            // Pops a cbase and a superclass if the definition names them, and
            // pushes what the body evaluated to.
            Insn::OpenClass(index) => {
                let def = &self.class_defs[index as usize];
                let popped = isize::from(def.scoped)
                    + isize::from(def.superclass)
                    + isize::from(def.kind == DefKind::Singleton);
                1 - popped
            }
            // Pops `n`, pushes the array.
            Insn::NewArray(n) => 1 - n as isize,
            // A send pops the receiver, the arguments, and a passed block, and
            // pushes one result. The count lives in the call site rather than
            // the instruction, so this is the one case that has to look it up.
            Insn::Send(site) => 1 - self.site_operands(site, true),
            // `yield` is the same shape without a receiver: the block comes
            // from the frame.
            Insn::Yield(site) => 1 - self.site_operands(site, false),
        }
    }

    /// How many stack values a call site consumes below its result.
    fn site_operands(&self, site: u32, receiver: bool) -> isize {
        let site = &self.call_sites[site as usize];
        isize::from(receiver)
            + site.argc as isize
            + site.keywords.len() as isize
            + isize::from(site.block == BlockRef::Pass)
    }

    fn emit(&mut self, insn: Insn) {
        let depth = self.depth as isize + self.effect(insn);
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
            Insn::JumpUnlessUndef(_) => Insn::JumpUnlessUndef(displacement),
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

    /// The slot the parameter at position `at` occupies.
    ///
    /// Normally `slot`: Prism lists a scope's parameters first and in binder
    /// order, so the n-th parameter is already at index n. The exception is a
    /// repeated name. Ruby allows one when it starts with an underscore —
    /// `def m(_, _)` has arity 2, binds both, and `_` reads the first — and
    /// Prism's scope list holds a single `_` for the pair. Reusing that slot
    /// would bind two arguments to one local and leave `ParamSpec` claiming
    /// more slots than the frame has.
    ///
    /// So a repeat takes a fresh slot under a name no source can mention, which
    /// keeps the binder's positional arithmetic true and leaves `_` resolving to
    /// the first, as Ruby does.
    fn param_slot(&mut self, name: &str, at: usize) -> u16 {
        let slot = self.slot(name);
        if slot as usize == at {
            return slot;
        }
        self.slot(&format!("{name} ({at})"))
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

            ExprKind::ConstPath(path) => {
                let (symbol, how) = self.const_path(path, span)?;
                self.emit(Insn::GetConst(symbol, how));
            }
            ExprKind::Class(class) => self.class_expr(class, span)?,
            ExprKind::Module(module) => self.module_expr(module, span)?,
            ExprKind::SingletonClass(singleton) => self.singleton_expr(singleton, span)?,
            ExprKind::Defined(inner) => self.defined(inner, span)?,

            ExprKind::Def(def) => self.def_expr(def, span)?,
            ExprKind::Yield(node) => self.yield_expr(node, span)?,
            ExprKind::Lambda(block) => self.lambda(block, span)?,
            ExprKind::Return(value) => self.return_expr(value.as_deref(), span)?,

            other => return Err(Unsupported::at(node_name(other), span)),
        }
        Ok(())
    }

    fn var(&mut self, var: &VarRef, span: Span) -> Emit {
        match var {
            VarRef::Local { name, depth } => {
                let slot = self.outer_slot(name, *depth, span)?;
                self.emit(Insn::GetLocal(slot, *depth as u16));
                Ok(())
            }
            VarRef::Instance(_) => Err(Unsupported::at("an instance variable", span)),
            VarRef::Class(_) => Err(Unsupported::at("a class variable", span)),
            VarRef::Global(_) => Err(Unsupported::at("a global variable", span)),
            VarRef::Const(name) => {
                let symbol = self.symbol(name);
                self.emit(Insn::GetConst(symbol, ConstScope::Lexical));
                Ok(())
            }
            VarRef::It => {
                let slot = self.slot(IT);
                self.emit(Insn::GetLocal(slot, 0));
                Ok(())
            }
            VarRef::BackRef(_) | VarRef::NumberedRef(_) => {
                Err(Unsupported::at("a regexp back-reference", span))
            }
        }
    }

    fn assign(&mut self, assign: &Assign, span: Span) -> Emit {
        if let Some((name, how)) = self.const_target(&assign.target)? {
            // `X = v`. A compound form (`X ||= v`, `X += v`) would have to read
            // the constant first, and for `A::X` that means evaluating `A`
            // twice or spilling it — neither is free, and nothing in the corpus
            // asks. Refused rather than double-evaluated.
            if assign.op != AssignOp::Assign {
                return Err(Unsupported::at("a compound constant assignment", span));
            }
            self.expr(&assign.value)?;
            self.emit(Insn::SetConst(name, how));
            return Ok(());
        }
        let (slot, depth) = self.local_target(&assign.target)?;
        match &assign.op {
            AssignOp::Assign => {
                self.expr(&assign.value)?;
                self.emit(Insn::Dup);
                self.emit(Insn::SetLocal(slot, depth));
            }
            AssignOp::Binary(op) => {
                let op = BinOp::from_name(op)
                    .ok_or_else(|| Unsupported::at("this compound assignment operator", span))?;
                self.emit(Insn::GetLocal(slot, depth));
                self.expr(&assign.value)?;
                self.emit(Insn::BinOp(op));
                self.emit(Insn::Dup);
                self.emit(Insn::SetLocal(slot, depth));
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
                self.emit(Insn::GetLocal(slot, depth));
                let skip = self.emit_jump(keep);
                self.emit(Insn::Pop);
                self.expr(&assign.value)?;
                self.emit(Insn::Dup);
                self.emit(Insn::SetLocal(slot, depth));
                self.patch_here(skip);
            }
        }
        Ok(())
    }

    /// The only assignment target this slice writes to: a local, at whatever
    /// depth it was declared. A block assigning an enclosing scope's local
    /// writes through the captured environment rather than shadowing it.
    fn local_target(&mut self, target: &Target) -> Result<(u16, u16), Unsupported> {
        match &target.kind {
            TargetKind::Var(VarRef::Local { name, depth }) => {
                let slot = self.outer_slot(name, *depth, target.span)?;
                Ok((slot, *depth as u16))
            }
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
        let Some(&Loop {
            base_depth,
            next_target,
            ..
        }) = self.loops.last()
        else {
            // Outside a loop, the two part company. `next` in a block ends that
            // block's *call* with a value, which is a local exit and exactly
            // what leaving a frame already does. `break` ends the method that
            // yielded to the block, which is a non-local exit through the
            // unwinding path and #12's.
            if !is_break && !self.scope_barrier {
                match value {
                    Some(value) => self.expr(value)?,
                    None => self.emit(Insn::PushNil),
                }
                self.emit(Insn::Return);
                return Ok(());
            }
            return Err(Unsupported::at(
                if is_break {
                    "`break` out of a block"
                } else {
                    "`next` outside a loop or block"
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

    /// The slot a `VarRef::Local` names, `depth` scopes up.
    ///
    /// Depth 0 may create the slot: a target can be the first mention of a
    /// name. A depth above 0 may not — the enclosing scope's list is fixed by
    /// the time this block is compiled, and a name missing from it would mean
    /// Prism and this compiler disagreed about what a local is, which is a bug
    /// rather than a construct to lower.
    fn outer_slot(&mut self, name: &str, depth: u32, span: Span) -> Result<u16, Unsupported> {
        if depth == 0 {
            return Ok(self.slot(name));
        }
        // A method body cannot see the locals of the scope it was written in,
        // and the compiler never builds one that tries. A block nested in a
        // method that is nested in a block can, which is why this is a walk.
        let scope = self
            .outer
            .get(depth as usize - 1)
            .ok_or_else(|| Unsupported::at("a local variable from an enclosing scope", span))?;
        scope
            .iter()
            .position(|l| &**l == name)
            .map(|index| index as u16)
            .ok_or_else(|| Unsupported::at("a local variable from an enclosing scope", span))
    }

    // -- definitions ------------------------------------------------------

    /// `def name(params) body end`.
    ///
    /// The body is compiled as a child `Iseq` with a scope barrier, so it
    /// cannot see the locals around the `def` — which is Ruby, and is the one
    /// place a nested scope differs from a block.
    fn def_expr(&mut self, def: &spinel_ast::Def, span: Span) -> Emit {
        let child = self.child_iseq(&def.name, &def.params, &def.locals, &def.body, true, span)?;
        let name = self.symbol(&def.name);
        let index = self.push_child(child);
        let definition = self.definitions.len() as u32;
        self.definitions.push((name, index));
        match &def.receiver {
            // `def self.foo`, `def obj.foo`: the singleton of whatever the
            // receiver evaluates to.
            Some(receiver) => {
                self.expr(receiver)?;
                self.emit(Insn::DefineSingleton(definition));
            }
            // A plain `def` defines on the *lexical scope*, not on
            // `class_of(self)`. See `Insn::DefineMethod`.
            None => self.emit(Insn::DefineMethod(definition)),
        }
        Ok(())
    }

    // -- defined? --------------------------------------------------------

    /// Compile `defined?(expr)`, leaving its answer — a `String` or `nil` — on
    /// the stack.
    ///
    /// Ruby's answers are a table, not a rule, and the table was measured
    /// against `ruby 4.0.6` rather than reasoned about. Two entries are worth
    /// naming because they read wrong:
    ///
    /// - `nil`, `true`, and `false` answer `"nil"`, `"true"`, `"false"` — not
    ///   `"expression"`.
    /// - `!x` and `1 + 1` answer `"method"`, because `!` and `+` are methods.
    ///
    /// `defined?` recurses into exactly two kinds of position — a call's
    /// receiver and arguments, and a collection literal's elements — so
    /// `defined?([1, NoSuch])` is `nil` while `defined?(if NoSuch then 1 end)`
    /// is `"expression"`. Nothing else is recursed into, which is CRuby's
    /// `defined_expr0` and is not what the shape of the syntax suggests.
    ///
    /// Arguments are *checked* but never *evaluated*; only the receiver chain
    /// runs. `defined?(D.any(D.side))` is `"method"` and calls neither.
    fn defined(&mut self, expr: &Expr, span: Span) -> Emit {
        match &expr.kind {
            ExprKind::Nil => self.push_word("nil"),
            ExprKind::True => self.push_word("true"),
            ExprKind::False => self.push_word("false"),
            ExprKind::SelfExpr => self.push_word("self"),
            ExprKind::Assign(_) => self.push_word("assignment"),
            ExprKind::Var(VarRef::Local { .. } | VarRef::It) => self.push_word("local-variable"),

            ExprKind::Var(VarRef::Const(name)) => {
                let symbol = self.symbol(name);
                self.emit(Insn::DefinedConst(symbol, ConstScope::Lexical));
                Ok(())
            }
            ExprKind::ConstPath(path) => {
                let Some(name) = &path.name else {
                    return Err(Unsupported::at("a syntax error", span));
                };
                let symbol = self.symbol(name);
                match &path.parent {
                    // `A::B` is `nil` when `A` itself is not defined, so the
                    // parent is a guard before it is a receiver.
                    Some(parent) => self.guarded(std::slice::from_ref(parent), |me| {
                        me.expr(parent)?;
                        me.emit(Insn::DefinedConst(symbol, ConstScope::Qualified));
                        Ok(())
                    }),
                    None => {
                        self.emit(Insn::DefinedConst(symbol, ConstScope::Top));
                        Ok(())
                    }
                }
            }

            ExprKind::Yield(_) => {
                self.emit(Insn::DefinedYield);
                Ok(())
            }

            ExprKind::Call(call) => self.defined_call(call, span),

            // Every element must be defined for the literal to be.
            ExprKind::Array(elements) => {
                self.guarded(elements, |me| me.push_word("expression"))
            }

            // A kind whose answer this VM cannot know. Ruby answers `nil` for an
            // undefined instance variable, so answering `nil` here would pass
            // `defined?(@nope).should be_nil` while the VM has no instance
            // variables at all — a wrong answer wearing a passing spec.
            ExprKind::Var(VarRef::Instance(_)) => {
                Err(Unsupported::at("`defined?` on an instance variable", span))
            }
            ExprKind::Var(VarRef::Class(_)) => {
                Err(Unsupported::at("`defined?` on a class variable", span))
            }
            ExprKind::Var(VarRef::Global(_)) => {
                Err(Unsupported::at("`defined?` on a global variable", span))
            }
            ExprKind::Var(VarRef::BackRef(_) | VarRef::NumberedRef(_)) => {
                Err(Unsupported::at("`defined?` on a back-reference", span))
            }
            ExprKind::Super(_) => Err(Unsupported::at("`defined?` on `super`", span)),

            // Everything else is `"expression"` without being evaluated, so it
            // is answerable even for a node this compiler could not run.
            _ => self.push_word("expression"),
        }
    }

    /// `defined?` of a call: the receiver and every argument must be defined,
    /// then the method must exist.
    fn defined_call(&mut self, call: &spinel_ast::Call, span: Span) -> Emit {
        if call.block.is_some() {
            // A block is not an operand, and Ruby still answers for the call.
            // Refused rather than guessed: nothing in the corpus needs it and a
            // guess here is a wrong answer that passes.
            return Err(Unsupported::at("`defined?` on a call with a block", span));
        }
        let name = self.symbol(&call.name);
        // Receiver before arguments, which is Ruby's evaluation order and the
        // order the checks can have side effects in.
        let mut guards: Vec<Expr> = call.receiver.iter().cloned().collect();
        guards.extend(call.args.iter().cloned());
        let receiver = call.receiver.clone();
        self.guarded(&guards, move |me| {
            match &receiver {
                // The receiver runs; the arguments never do.
                Some(receiver) => {
                    me.expr(receiver)?;
                    me.emit(Insn::DefinedMethod(name));
                }
                None => me.emit(Insn::DefinedSelfMethod(name)),
            }
            Ok(())
        })
    }

    /// Emit `answer` guarded by a `defined?` check on each of `guards`.
    ///
    /// The first guard that answers `nil` is the answer for the whole
    /// expression, which is a chain of `JumpUnless` — the same shape `&&` has.
    fn guarded(
        &mut self,
        guards: &[Expr],
        answer: impl FnOnce(&mut Self) -> Emit,
    ) -> Emit {
        let mut undefined = Vec::new();
        for guard in guards {
            self.defined(guard, guard.span)?;
            undefined.push(self.emit_jump(Insn::JumpUnless));
        }
        answer(self)?;
        if undefined.is_empty() {
            return Ok(());
        }
        let done = self.emit_jump(Insn::Jump);
        // Both arms leave exactly one value, so the merge point is at the same
        // depth either way — which `emit` asserts.
        self.depth -= 1;
        for jump in undefined {
            self.patch_here(jump);
        }
        self.emit(Insn::PushNil);
        self.patch_here(done);
        Ok(())
    }

    /// Push one of `defined?`'s answer strings.
    fn push_word(&mut self, word: &str) -> Emit {
        let index = self.literal(Literal::Str(word.as_bytes().into()));
        self.emit(Insn::PushLit(index));
        Ok(())
    }

    // -- constants, classes, and modules ---------------------------------

    /// The symbol and lookup rule an `A::B` or `::B` names.
    fn const_path(&mut self, path: &spinel_ast::ConstPath, span: Span) -> Result<(u32, ConstScope), Unsupported> {
        let Some(name) = &path.name else {
            // `A::` with nothing after it — a syntax error the parser kept so
            // the rest of the tree survives.
            return Err(Unsupported::at("a syntax error", span));
        };
        let how = match &path.parent {
            Some(parent) => {
                self.expr(parent)?;
                ConstScope::Qualified
            }
            None => ConstScope::Top,
        };
        Ok((self.symbol(name), how))
    }

    /// Push the module a `class A::B` or `class ::B` is defined in.
    ///
    /// `::B` is `Object::B`, and `Object` is reachable as a top-level constant,
    /// so the general instruction covers it and no opcode is needed for "push
    /// `Object`".
    fn const_target(&mut self, target: &Target) -> Result<Option<(u32, ConstScope)>, Unsupported> {
        match &target.kind {
            TargetKind::Var(VarRef::Const(name)) => {
                Ok(Some((self.symbol(name), ConstScope::Lexical)))
            }
            TargetKind::ConstPath(path) => {
                Ok(Some(self.const_path(path, target.span)?))
            }
            _ => Ok(None),
        }
    }

    /// The definee half of a `class`/`module` path: the symbol, and whether a
    /// `cbase` was pushed for it.
    fn class_path(&mut self, path: &Target) -> Result<(u32, bool), Unsupported> {
        match self.const_target(path)? {
            Some((symbol, ConstScope::Lexical)) => Ok((symbol, false)),
            Some((symbol, ConstScope::Qualified)) => Ok((symbol, true)),
            Some((symbol, ConstScope::Top)) => {
                let object = self.symbol("Object");
                self.emit(Insn::GetConst(object, ConstScope::Top));
                Ok((symbol, true))
            }
            None => Err(Unsupported::at("a dynamic class name", path.span)),
        }
    }

    fn class_expr(&mut self, class: &spinel_ast::Class, span: Span) -> Emit {
        let (name, scoped) = self.class_path(&class.path)?;
        if let Some(superclass) = &class.superclass {
            self.expr(superclass)?;
        }
        self.open_class(
            name,
            DefKind::Class,
            scoped,
            class.superclass.is_some(),
            &class.body,
            &class.locals,
            span,
        )
    }

    fn module_expr(&mut self, module: &spinel_ast::Module, span: Span) -> Emit {
        let (name, scoped) = self.class_path(&module.path)?;
        self.open_class(
            name,
            DefKind::Module,
            scoped,
            false,
            &module.body,
            &module.locals,
            span,
        )
    }

    fn singleton_expr(&mut self, singleton: &spinel_ast::SingletonClass, span: Span) -> Emit {
        self.expr(&singleton.expression)?;
        self.open_class(
            0,
            DefKind::Singleton,
            false,
            false,
            &singleton.body,
            &singleton.locals,
            span,
        )
    }

    /// Compile a body and emit the instruction that opens it.
    ///
    /// The body is a barrier scope: `x = 1; class C; x; end` does not see `x`,
    /// which is Ruby — a class body starts a fresh set of locals the way a
    /// method body does.
    #[allow(clippy::too_many_arguments)]
    fn open_class(
        &mut self,
        name: u32,
        kind: DefKind,
        scoped: bool,
        superclass: bool,
        body: &[Expr],
        locals: &[Name],
        span: Span,
    ) -> Emit {
        let label = match kind {
            DefKind::Module => "<module>",
            DefKind::Singleton => "<singleton class>",
            DefKind::Class => "<class>",
        };
        let child = self.child_iseq(label, &Params::None, locals, body, true, span)?;
        let body = self.push_child(child);
        let index = self.class_defs.len() as u32;
        self.class_defs.push(ClassDef {
            name,
            body,
            kind,
            scoped,
            superclass,
        });
        self.emit(Insn::OpenClass(index));
        Ok(())
    }

    /// Compile a body that has its own parameters: a method, a block, a lambda.
    fn child_iseq(
        &mut self,
        name: &str,
        params: &Params,
        locals: &[Name],
        body: &[Expr],
        barrier: bool,
        span: Span,
    ) -> Result<Iseq, Unsupported> {
        self.child_iseq_as(name, params, locals, body, barrier, false, span)
    }

    #[allow(clippy::too_many_arguments)]
    fn child_iseq_as(
        &mut self,
        name: &str,
        params: &Params,
        locals: &[Name],
        body: &[Expr],
        barrier: bool,
        lambda: bool,
        span: Span,
    ) -> Result<Iseq, Unsupported> {
        let mut child = Compiler::nested(name, locals, self, barrier);
        child.is_lambda_body = lambda;
        child.params = child.lower_params(params, span)?;
        // Optional defaults are emitted first, each at a known instruction, so
        // the binder can enter the body at the first default it has to compute
        // and fall through the rest. The body proper starts after them.
        child.emit_defaults(params, span)?;
        child.statements(body, true)?;
        Ok(child.finish())
    }

    fn push_child(&mut self, child: Iseq) -> u32 {
        self.children.push(Arc::new(child));
        (self.children.len() - 1) as u32
    }

    /// `spinel_ast::Params` → [`ParamSpec`], allocating slots in binder order.
    fn lower_params(&mut self, params: &Params, span: Span) -> Result<ParamSpec, Unsupported> {
        let list = match params {
            Params::None => return Ok(ParamSpec::default()),
            Params::Explicit(list) => list,
            // `_1`/`_2` and `it` are sugar for a plain parameter list. Prism
            // lowers a use of `_1` to an ordinary local and `it` to
            // [`VarRef::It`], so naming the slots here in order is the whole
            // implementation: `_1` is slot 0 because it is bound first.
            Params::Numbered(highest) => {
                let mut spec = ParamSpec::default();
                for index in 1..=u16::from(*highest) {
                    self.slot(&format!("_{index}"));
                    spec.required += 1;
                }
                return Ok(spec);
            }
            Params::It => {
                self.slot(IT);
                return Ok(ParamSpec {
                    required: 1,
                    ..ParamSpec::default()
                });
            }
        };
        self.spec_from_list(list, span)
    }

    fn spec_from_list(&mut self, list: &ParamList, span: Span) -> Result<ParamSpec, Unsupported> {
        use spinel_ast::{KeywordRestKind, RequiredParamKind};

        let mut spec = ParamSpec::default();
        // How many parameters have claimed a slot. The binder addresses them by
        // position, so the n-th parameter owns slot n; `param_slot` holds that
        // true even when Prism's scope list is shorter than the parameter list.
        let mut at = 0usize;
        for required in &list.required {
            match &required.kind {
                RequiredParamKind::Named(name) => {
                    self.param_slot(name, at);
                    at += 1;
                    spec.required += 1;
                }
                // `{ |(a, b)| }` assigns through a multiple-assignment target,
                // which the binder would have to run rather than fill.
                RequiredParamKind::Destructure(_) => {
                    return Err(Unsupported::at("a destructuring block parameter", span));
                }
            }
        }
        for optional in &list.optional {
            let slot = self.param_slot(&optional.name, at);
            at += 1;
            spec.optional.push(Optional { slot });
        }
        if let Some(rest) = &list.rest {
            let name = rest.name.as_deref().unwrap_or("*");
            spec.rest = Some(self.param_slot(name, at));
            at += 1;
        }
        for post in &list.posts {
            match &post.kind {
                RequiredParamKind::Named(name) => {
                    self.param_slot(name, at);
                    at += 1;
                    spec.post += 1;
                }
                RequiredParamKind::Destructure(_) => {
                    return Err(Unsupported::at("a destructuring block parameter", span));
                }
            }
        }
        for keyword in &list.keywords {
            let slot = self.param_slot(&keyword.name, at);
            at += 1;
            let name = self.symbol(&keyword.name);
            spec.keywords.push(Keyword {
                name,
                slot,
                required: keyword.default.is_none(),
            });
        }
        if let Some(rest) = &list.keyword_rest {
            match rest.kind {
                // `**kw` collects into a Hash, and there is no Hash.
                KeywordRestKind::Named(_) => {
                    return Err(Unsupported::at("a keyword rest parameter", span));
                }
                KeywordRestKind::Forbidden => {}
                KeywordRestKind::Forwarding => {
                    return Err(Unsupported::at("argument forwarding", span));
                }
            }
        }
        if let Some(block) = &list.block {
            let name = block.name.as_deref().unwrap_or("&");
            spec.block = Some(self.param_slot(name, at));
        }
        // `{ |a; b| }`: block-locals are ordinary locals of the block's own
        // scope, already in Prism's list for it. Naming them here only fixes
        // their slots ahead of any later mention.
        for local in &list.locals {
            self.slot(&local.name);
        }

        // The binder derives the required and post slots by arithmetic —
        // required is `0..required`, post starts after required, optional, and
        // the splat — rather than storing them. That is only correct while
        // Prism lists a scope's parameters in exactly that order, which it
        // does, and which this asserts rather than assumes: a reordering
        // upstream would otherwise bind the right values to the wrong names,
        // and every test here would still pass on the shapes that happen to be
        // symmetrical.
        debug_assert!(
            self.locals.len() >= spec.slots()
                && spec
                    .optional
                    .iter()
                    .map(|o| o.slot as usize)
                    .chain(spec.rest.map(|slot| slot as usize))
                    .chain(spec.keywords.iter().map(|k| k.slot as usize))
                    .chain(spec.block.map(|slot| slot as usize))
                    .eq(binder_order(&spec)),
            "prism ordered {:?} against the binder's {:?}",
            self.locals,
            spec,
        );
        Ok(spec)
    }

    /// Emit each optional and keyword default, guarded.
    ///
    /// The binder marks a parameter it did not fill with the undef value, and
    /// each default here is `if this slot is undef, compute it`. Optionals and
    /// keywords take the same shape, and a default that calls a method works
    /// because it is ordinary code in the body rather than something the binder
    /// has to evaluate.
    fn emit_defaults(&mut self, params: &Params, _span: Span) -> Emit {
        let Params::Explicit(list) = params else {
            return Ok(());
        };
        let defaults: Vec<(Box<str>, &Expr)> =
            list.optional
                .iter()
                .map(|o| (o.name.to_string().into_boxed_str(), &o.default))
                .chain(list.keywords.iter().filter_map(|k| {
                    Some((k.name.to_string().into_boxed_str(), k.default.as_ref()?))
                }))
                .collect();
        for (name, default) in defaults {
            let slot = self.slot(&name);
            self.emit(Insn::GetLocal(slot, 0));
            let supplied = self.emit_jump(Insn::JumpUnlessUndef);
            self.expr(default)?;
            self.emit(Insn::SetLocal(slot, 0));
            self.patch_here(supplied);
        }
        Ok(())
    }

    // -- calls ------------------------------------------------------------

    /// A call: the specialised operators when they apply, a real send otherwise.
    fn call(&mut self, call: &spinel_ast::Call, span: Span) -> Emit {
        if call.flags.safe_nav {
            return Err(Unsupported::at("a safe-navigation call", span));
        }
        if self.try_operator(call)? {
            return Ok(());
        }

        // Receiver first: Ruby evaluates it before the arguments.
        match &call.receiver {
            Some(receiver) => self.expr(receiver)?,
            None => self.emit(Insn::PushSelf),
        }
        let site = self.arguments(&call.name, &call.args, call.block.as_ref(), span)?;
        let site = self.push_site(site, call.receiver.is_none());
        self.emit(Insn::Send(site));
        Ok(())
    }

    /// The operators #10 emits as instructions, when the shape is exactly one
    /// of them. A `+` with a block or a splat is a send like any other.
    fn try_operator(&mut self, call: &spinel_ast::Call) -> Result<bool, Unsupported> {
        let Some(receiver) = &call.receiver else {
            return Ok(false);
        };
        if call.block.is_some() {
            return Ok(false);
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
                let Some(op) = BinOp::from_name(name) else {
                    return Ok(false);
                };
                if matches!(call.args[0].kind, ExprKind::Splat(_) | ExprKind::Hash(_)) {
                    return Ok(false);
                }
                self.expr(receiver)?;
                self.expr(&call.args[0])?;
                self.emit(Insn::BinOp(op));
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Push the arguments and describe them.
    ///
    /// Positional values first, then keyword values, then a passed block —
    /// which is the order the interpreter pops them off in reverse.
    fn arguments(
        &mut self,
        name: &str,
        args: &[Expr],
        block: Option<&BlockArg>,
        span: Span,
    ) -> Result<CallSite, Unsupported> {
        let mut site = CallSite {
            name: self.symbol(name),
            argc: 0,
            splats: Vec::new(),
            keywords: Vec::new(),
            block: BlockRef::None,
            implicit_self: false,
        };

        for arg in args {
            match &arg.kind {
                ExprKind::Splat(Some(inner)) => {
                    self.expr(inner)?;
                    site.splats.push(site.argc);
                    site.argc += 1;
                }
                ExprKind::Splat(None) => {
                    return Err(Unsupported::at("argument forwarding", arg.span));
                }
                ExprKind::ForwardingArgs => {
                    return Err(Unsupported::at("argument forwarding", arg.span));
                }
                // A trailing brace-less hash is Ruby's keyword syntax.
                ExprKind::Hash(hash) if !hash.braces => {
                    for pair in &hash.entries {
                        let key = keyword_name(pair)
                            .ok_or_else(|| Unsupported::at("a hash literal", arg.span))?;
                        let symbol = self.symbol(key);
                        let value = pair_value(pair)
                            .ok_or_else(|| Unsupported::at("a hash literal", arg.span))?;
                        self.expr(value)?;
                        site.keywords.push(symbol);
                    }
                }
                _ => {
                    self.expr(arg)?;
                    site.argc += 1;
                }
            }
        }

        match block {
            None => {}
            Some(BlockArg::Block(block)) => {
                let child = self.child_iseq(
                    "block in <compiled>",
                    &block.params,
                    &block.locals,
                    &block.body,
                    false,
                    span,
                )?;
                site.block = BlockRef::Literal(self.push_child(child));
            }
            Some(BlockArg::Pass(Some(expr))) => {
                self.expr(expr)?;
                site.block = BlockRef::Pass;
            }
            Some(BlockArg::Pass(None)) => {
                return Err(Unsupported::at("an anonymous block parameter", span));
            }
        }
        Ok(site)
    }

    fn push_site(&mut self, mut site: CallSite, implicit_self: bool) -> u32 {
        site.implicit_self = implicit_self;
        self.call_sites.push(site);
        (self.call_sites.len() - 1) as u32
    }

    /// `yield`. The block comes from the frame, so there is no receiver.
    fn yield_expr(&mut self, node: &spinel_ast::Yield, span: Span) -> Emit {
        let site = self.arguments("yield", &node.args, None, span)?;
        let site = self.push_site(site, true);
        self.emit(Insn::Yield(site));
        Ok(())
    }

    /// `-> { }`, which is a lambda: strict arity and a local `return`.
    fn lambda(&mut self, block: &spinel_ast::Block, span: Span) -> Emit {
        let child = self.child_iseq_as(
            "lambda in <compiled>",
            &block.params,
            &block.locals,
            &block.body,
            false,
            true,
            span,
        )?;
        let index = self.push_child(child);
        self.emit(Insn::MakeProc(index, true));
        Ok(())
    }

    /// `return`, which leaves the frame it is written in.
    ///
    /// Inside a method or a lambda that is a local return. Inside a block it is
    /// a non-local exit through the enclosing method, which shares an unwinding
    /// path with exceptions and belongs to
    /// [#12](https://github.com/ar4mirez/spinel/issues/12). The compiler knows
    /// which it is — a block body has no scope barrier — so it refuses the one
    /// it cannot mean rather than emitting a local return for it.
    fn return_expr(&mut self, value: Option<&Expr>, span: Span) -> Emit {
        if !self.scope_barrier && !self.is_lambda_body {
            return Err(Unsupported::at("`return` from a block", span));
        }
        match value {
            Some(expr) => self.expr(expr)?,
            None => self.emit(Insn::PushNil),
        }
        self.emit(Insn::Return);
        Ok(())
    }
}

/// The `key:` of a keyword argument written as a brace-less hash pair.
fn keyword_name(pair: &spinel_ast::HashEntry) -> Option<&str> {
    match &pair.kind {
        spinel_ast::HashEntryKind::Pair { key, .. } => match &key.kind {
            ExprKind::Sym(symbol) => match symbol.parts.as_slice() {
                [StrPart::Bytes(bytes)] => std::str::from_utf8(bytes).ok(),
                _ => None,
            },
            _ => None,
        },
        spinel_ast::HashEntryKind::Splat(_) => None,
    }
}

fn pair_value(pair: &spinel_ast::HashEntry) -> Option<&Expr> {
    match &pair.kind {
        spinel_ast::HashEntryKind::Pair { value, .. } => Some(value),
        spinel_ast::HashEntryKind::Splat(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The slots the binder expects the non-required, non-post parameters to have,
/// in the order `spec_from_list` names them.
///
/// Not `cfg(debug_assertions)`: `debug_assert!` still type-checks its argument
/// in a release build, so a gated helper is a release-only compile error.
fn binder_order(spec: &ParamSpec) -> impl Iterator<Item = usize> + '_ {
    let required = spec.required as usize;
    let optionals = required..required + spec.optional.len();
    let rest = optionals.end..optionals.end + usize::from(spec.rest.is_some());
    // Post parameters sit between the splat and the keywords, and the binder
    // computes their base the same way.
    let keywords = rest.end + spec.post as usize;
    optionals
        .chain(rest)
        .chain(keywords..keywords + spec.keywords.len())
        .chain(
            (keywords + spec.keywords.len())
                ..(keywords + spec.keywords.len() + usize::from(spec.block.is_some())),
        )
}

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
