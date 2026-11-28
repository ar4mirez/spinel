//! What each instruction of a body becomes: the analysis half of lowering,
//! with no Cranelift in it.
//!
//! [`plan`] walks a body's [`Insn`]s from its first, as the interpreter would
//! run them, and gives every instruction it can reach a [`Step`]: an
//! operation compiled code performs, or a bail-out to the interpreter at that
//! instruction. It follows only the lowered steps, so what it reaches is the
//! part of the body compiled code can run before it bails. Along the way it
//! works out the operand-stack depth at each instruction, which is fixed per
//! instruction in well-formed bytecode; a body where it is not, or that would
//! overflow the stack the compiler sized, is not compiled at all.
//!
//! A superinstruction is read as its [`Insn::head`]. The rest of its run
//! follows it where the compiler wrote it, so what compiles is the unfused
//! program, and each of its instructions is charged to the budget as the
//! interpreter charges it.

use spinel_vm::bytecode::BinOp;
use spinel_vm::{Insn, Iseq, Value};

/// What compiled code does for one instruction, the operands it takes from
/// the stack being implied by the instruction's depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Push an immediate, by its word.
    Push(u64),
    /// Push the frame's `self`.
    PushSelf,
    Pop,
    Dup,
    /// Push this frame's local.
    GetLocal(u16),
    /// Pop into this frame's local.
    SetLocal(u16),
    /// Push a local of the scope `depth` out, through the frame's
    /// environment chain.
    GetOuter {
        slot: u16,
        depth: u16,
    },
    /// Pop into a local of the scope `depth` out.
    SetOuter {
        slot: u16,
        depth: u16,
    },
    /// Push `self`'s instance variable, which the site's cache, numbered
    /// from the frame's first, says where to find.
    GetIvar(u32),
    /// Pop an immediate into `self`'s instance variable where the site's
    /// cache says it already is.
    SetIvar(u32),
    /// Go on at this instruction.
    Jump(usize),
    /// Go on at `target` when `test` of the top of the stack is `when`, and
    /// at the next instruction otherwise; the value is popped unless `keeps`.
    Branch {
        test: Test,
        when: bool,
        keeps: bool,
        target: usize,
    },
    /// Two fixnums, or two flonums, under an operator the frame's mask says
    /// is `Integer`'s or `Float`'s own, whose answer is an immediate:
    /// anything else bails out at the instruction, its operands still on
    /// the stack.
    BinOp(BinOp),
    /// `-@` of a fixnum or a flonum, on the same terms.
    Neg,
    /// `!` of an immediate.
    Not,
}

/// What a [`Op::Branch`] asks of the value on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Test {
    /// Anything but `nil` and `false`.
    Truthy,
    /// `nil` itself.
    Nil,
    /// The "no argument was given" marker a default's guard looks for.
    Undef,
}

/// One reachable instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Compiled code performs it.
    Lowered(Op),
    /// Compiled code stops before it, and the interpreter runs it.
    Bail,
}

/// A body as compiled code sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Per instruction: `Some((step, depth))` for one compiled code can
    /// reach, with the operand-stack depth before it.
    pub steps: Vec<Option<(Step, u32)>>,
    /// The deepest the operand stack gets in compiled code.
    pub max_depth: u32,
    /// How many scopes out compiled code reaches for a local.
    pub env_depth: u16,
    /// Whether compiled code reads or writes an instance variable.
    pub ivars: bool,
}

impl Plan {
    /// How many instructions compiled code performs rather than bails at.
    #[must_use]
    pub fn lowered(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| matches!(step, Some((Step::Lowered(_), _))))
            .count()
    }
}

/// What instruction `insn`, at `pc` of `iseq`, becomes; `None` for a
/// bail-out.
///
/// Every instruction is named, with no catch-all arm, so that a new one is a
/// compile error here until someone decides whether it lowers, and
/// [`COVERAGE`] says what was decided for each: that is the "lowering covers
/// every opcode or explicitly bails out" of #120.
#[must_use]
pub fn lower(insn: Insn, pc: usize, iseq: &Iseq) -> Option<Op> {
    let len = iseq.insns.len();
    let target = |displacement: i32| {
        // Relative to the instruction after the jump, as the interpreter's
        // `jump` reads it.
        let to = i64::try_from(pc).ok()? + 1 + i64::from(displacement);
        usize::try_from(to).ok().filter(|&to| to < len)
    };
    let branch = |displacement, test, when, keeps| {
        Some(Op::Branch {
            test,
            when,
            keeps,
            target: target(displacement)?,
        })
    };
    match insn.head() {
        Insn::PushNil => Some(Op::Push(Value::NIL.to_bits())),
        Insn::PushTrue => Some(Op::Push(Value::TRUE.to_bits())),
        Insn::PushFalse => Some(Op::Push(Value::FALSE.to_bits())),
        Insn::PushSelf => Some(Op::PushSelf),
        // One past the fixnum range is a Bignum the interpreter allocates.
        Insn::PushInt(n) => Value::fixnum(n).map(|value| Op::Push(value.to_bits())),
        // The compiler writes one only for a double that fits a flonum, and
        // the interpreter allocates a `Float` for any other.
        Insn::PushFloat(bits) => {
            Value::flonum(f64::from_bits(bits)).map(|value| Op::Push(value.to_bits()))
        }
        // A symbol is an immediate of an id in the table every Ractor
        // shares, so this body's is fixed once resolved.
        Insn::PushSym(index) => iseq
            .symbol_ids()
            .get(index as usize)
            .map(|&id| Op::Push(Value::symbol(id).to_bits())),
        Insn::Pop => Some(Op::Pop),
        Insn::Dup => Some(Op::Dup),
        Insn::GetLocal(slot, 0) => Some(Op::GetLocal(slot)),
        Insn::SetLocal(slot, 0) => Some(Op::SetLocal(slot)),
        // A local of an enclosing scope lives in a heap environment, which
        // the interpreter finds for compiled code on entry.
        Insn::GetLocal(slot, depth) => Some(Op::GetOuter { slot, depth }),
        Insn::SetLocal(slot, depth) => Some(Op::SetOuter { slot, depth }),
        Insn::GetIvar(_, site) => Some(Op::GetIvar(site)),
        Insn::SetIvar(_, site) => Some(Op::SetIvar(site)),
        Insn::Jump(displacement) => target(displacement).map(Op::Jump),
        Insn::JumpUnless(d) => branch(d, Test::Truthy, false, false),
        Insn::JumpIf(d) => branch(d, Test::Truthy, true, false),
        Insn::JumpUnlessKeep(d) => branch(d, Test::Truthy, false, true),
        Insn::JumpIfKeep(d) => branch(d, Test::Truthy, true, true),
        Insn::JumpIfNilKeep(d) => branch(d, Test::Nil, true, true),
        // Jumps unless the value *is* the marker: the default is skipped.
        Insn::JumpUnlessUndef(d) => branch(d, Test::Undef, false, false),
        Insn::BinOp(op) => Some(Op::BinOp(op)),
        Insn::Neg => Some(Op::Neg),
        Insn::Not => Some(Op::Not),
        // Leaving a frame is the interpreter's, with its `ensure`s, traces
        // and `Class#new`'s receiver: compiled code stops at the
        // instruction, with the value on the stack.
        Insn::Leave | Insn::Return | Insn::Break | Insn::LeaveThroughEnsure => None,
        // Calls, and everything that may call, allocate, raise, read the
        // heap beyond `self` and the environments, or change the frames.
        // Phase 6's later slices take some of these on (#122, #123, #124).
        Insn::PushLit(_)
        | Insn::Intern
        | Insn::CaptureSplat
        | Insn::SplatArray
        | Insn::Refuse(_)
        | Insn::BlockToProc
        | Insn::NewArray(_)
        | Insn::CaseEq
        | Insn::Send(_)
        | Insn::Super(_)
        | Insn::Yield(_)
        | Insn::MakeProc(..)
        | Insn::DefineMethod(_)
        | Insn::DefineSingleton(_)
        | Insn::GetConst(..)
        | Insn::SetConst(..)
        | Insn::DefinedConst(..)
        | Insn::OpenClass(_)
        | Insn::DefinedIvar(_)
        | Insn::GetGlobal(_)
        | Insn::SetGlobal(_)
        | Insn::DefinedGlobal(_)
        | Insn::DefinedMethod(_)
        | Insn::DefinedSelfMethod(_)
        | Insn::DefinedYield
        | Insn::DefinedSuper
        | Insn::Raise
        | Insn::CheckMatch
        | Insn::CheckMatchAny
        | Insn::NewRegexp(_)
        | Insn::NewRegexpOnce(..)
        | Insn::OnceGet(..)
        | Insn::OnceSet(_)
        | Insn::GetCvar(_)
        | Insn::SetCvar(_)
        | Insn::DefinedCvar(_)
        | Insn::Alias(..)
        | Insn::Undef(_)
        | Insn::AliasFromStack
        | Insn::UndefFromStack
        | Insn::AliasGlobal(..)
        | Insn::AliasGlobalSpecial(..)
        | Insn::LastMatch(_)
        | Insn::SetLastMatch
        | Insn::Errinfo
        | Insn::SetErrinfo
        | Insn::DefinedMatch(_)
        | Insn::Goto(..)
        | Insn::GotoValue(..)
        | Insn::EnterEnsure
        | Insn::LeaveEnsure => None,
        // `head` has read these as their first instruction.
        Insn::GetLocalPushIntBinOp { .. }
        | Insn::GetLocalGetLocalBinOp { .. }
        | Insn::GetLocalGetIvarBinOp { .. }
        | Insn::PushSelfGetLocal { .. }
        | Insn::GetLocalBinOp { .. } => unreachable!("`Insn::head` is never a superinstruction"),
    }
}

/// How far an instruction is lowered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// Always an operation; its guards may still bail at run time.
    Lowered,
    /// Lowered for some operands, a bail-out for the rest.
    Partly,
    /// Always a bail-out.
    Bails,
    /// A superinstruction: read as its first instruction, the rest of its
    /// run following where the compiler wrote it.
    Fused,
}

/// Every [`Insn`], by name, with how far it is lowered and the rule: the
/// table #120 asks for. The test's name for an instruction is a `match`
/// with no catch-all, so a new one does not compile until it has a name, a
/// row here and a sample, and the test holds each row to what [`lower`]
/// does.
pub const COVERAGE: &[(&str, Coverage, &str)] = {
    use Coverage::{Bails, Fused, Lowered, Partly};
    const CALLS: &str = "may call, allocate, raise or change the frames";
    &[
        ("PushNil", Lowered, "an immediate"),
        ("PushTrue", Lowered, "an immediate"),
        ("PushFalse", Lowered, "an immediate"),
        ("PushSelf", Lowered, "the frame's `self`"),
        (
            "PushInt",
            Partly,
            "a fixnum; past its range, a Bignum allocates",
        ),
        ("PushFloat", Partly, "a flonum; any other double allocates"),
        ("PushLit", Bails, "materialises a literal, which allocates"),
        ("PushSym", Lowered, "an immediate of a shared symbol id"),
        ("Intern", Bails, "interns a String, which may raise"),
        ("Pop", Lowered, ""),
        ("Dup", Lowered, ""),
        ("CaptureSplat", Bails, "copies an Array"),
        ("SplatArray", Bails, CALLS),
        (
            "GetLocal",
            Lowered,
            "this frame's slot, or an enclosing scope's through the environment chain",
        ),
        ("SetLocal", Lowered, "as `GetLocal`"),
        ("Jump", Lowered, ""),
        ("JumpUnless", Lowered, ""),
        ("JumpIf", Lowered, ""),
        ("Refuse", Bails, "raises"),
        ("JumpUnlessKeep", Lowered, ""),
        ("JumpIfKeep", Lowered, ""),
        ("JumpIfNilKeep", Lowered, ""),
        ("JumpUnlessUndef", Lowered, ""),
        (
            "BinOp",
            Lowered,
            "two fixnums or two flonums under the operator mask, answered as an immediate; else bails",
        ),
        (
            "Neg",
            Lowered,
            "a fixnum or a flonum under the mask; else bails",
        ),
        (
            "Not",
            Lowered,
            "an immediate; a heap object bails to its `!`",
        ),
        ("BlockToProc", Bails, CALLS),
        ("NewArray", Bails, "allocates"),
        ("CaseEq", Bails, CALLS),
        ("Send", Bails, CALLS),
        ("Super", Bails, CALLS),
        ("Yield", Bails, CALLS),
        ("LeaveThroughEnsure", Bails, "leaves the frame"),
        ("MakeProc", Bails, "allocates"),
        ("DefineMethod", Bails, "changes the class tables"),
        ("DefineSingleton", Bails, "changes the class tables"),
        (
            "GetConst",
            Bails,
            "reads the constant tables, may autoload or raise",
        ),
        ("SetConst", Bails, "changes the constant tables"),
        ("DefinedConst", Bails, "allocates its answer"),
        ("OpenClass", Bails, "changes the frames"),
        (
            "GetIvar",
            Lowered,
            "`self`'s slot when the site's cache holds its shape; else bails to fill it",
        ),
        (
            "SetIvar",
            Lowered,
            "an immediate in place when the cache holds the shape and `self` may be written; else bails",
        ),
        ("DefinedIvar", Bails, "allocates its answer"),
        ("GetGlobal", Bails, "may call a hooked global"),
        ("SetGlobal", Bails, "may call a hooked global"),
        ("DefinedGlobal", Bails, "allocates its answer"),
        ("DefinedMethod", Bails, CALLS),
        ("DefinedSelfMethod", Bails, CALLS),
        ("DefinedYield", Bails, "allocates its answer"),
        ("DefinedSuper", Bails, "allocates its answer"),
        ("Leave", Bails, "leaves the frame"),
        ("Return", Bails, "leaves the frame"),
        ("Break", Bails, "leaves the frame"),
        ("Raise", Bails, "raises"),
        ("CheckMatch", Bails, CALLS),
        ("CheckMatchAny", Bails, CALLS),
        ("NewRegexp", Bails, "allocates"),
        ("NewRegexpOnce", Bails, "allocates"),
        ("OnceGet", Bails, "reads a per-heap cache of objects"),
        ("OnceSet", Bails, "writes a per-heap cache of objects"),
        ("GetCvar", Bails, "may raise"),
        ("SetCvar", Bails, "may raise"),
        ("DefinedCvar", Bails, "allocates its answer"),
        ("Alias", Bails, "changes the class tables"),
        ("Undef", Bails, "changes the class tables"),
        ("AliasFromStack", Bails, "changes the class tables"),
        ("UndefFromStack", Bails, "changes the class tables"),
        ("AliasGlobal", Bails, "changes the global table"),
        ("AliasGlobalSpecial", Bails, "changes the global table"),
        (
            "LastMatch",
            Bails,
            "reads frame state compiled code does not hold",
        ),
        (
            "SetLastMatch",
            Bails,
            "writes frame state compiled code does not hold",
        ),
        (
            "Errinfo",
            Bails,
            "reads frame state compiled code does not hold",
        ),
        (
            "SetErrinfo",
            Bails,
            "writes frame state compiled code does not hold",
        ),
        ("DefinedMatch", Bails, "allocates its answer"),
        ("Goto", Bails, "runs `ensure`s on the way"),
        ("GotoValue", Bails, "runs `ensure`s on the way"),
        ("EnterEnsure", Bails, "changes the frame's unwinding state"),
        ("LeaveEnsure", Bails, "changes the frame's unwinding state"),
        (
            "GetLocalPushIntBinOp",
            Fused,
            "its `GetLocal`, `PushInt` and `BinOp`",
        ),
        (
            "GetLocalGetLocalBinOp",
            Fused,
            "its two `GetLocal`s and `BinOp`",
        ),
        (
            "GetLocalGetIvarBinOp",
            Fused,
            "its `GetLocal`, `GetIvar` and `BinOp`",
        ),
        ("PushSelfGetLocal", Fused, "its `PushSelf` and `GetLocal`"),
        ("GetLocalBinOp", Fused, "its `GetLocal` and `BinOp`"),
    ]
};

/// The operands `op` takes, and the values it leaves in their place.
const fn effect(op: Op) -> (u32, u32) {
    match op {
        Op::Push(_) | Op::PushSelf | Op::GetLocal(_) | Op::GetOuter { .. } | Op::GetIvar(_) => {
            (0, 1)
        }
        Op::Pop | Op::SetLocal(_) | Op::SetOuter { .. } | Op::SetIvar(_) => (1, 0),
        Op::Dup => (1, 2),
        Op::Jump(_) => (0, 0),
        Op::Branch { keeps: true, .. } => (1, 1),
        Op::Branch { keeps: false, .. } => (1, 0),
        Op::Neg | Op::Not => (1, 1),
        Op::BinOp(_) => (2, 1),
    }
}

/// Where compiled code can go after `op` at `pc`.
fn successors(op: Op, pc: usize) -> impl Iterator<Item = usize> {
    let (first, second) = match op {
        Op::Jump(target) => (target, None),
        Op::Branch { target, .. } => (pc + 1, Some(target)),
        _ => (pc + 1, None),
    };
    std::iter::once(first).chain(second)
}

/// The plan for `iseq`, or `None` when compiled code would run none of it:
/// its first instruction bails, or its bytecode is not something the walk
/// can follow (see the module documentation).
#[must_use]
pub fn plan(iseq: &Iseq) -> Option<Plan> {
    let len = iseq.insns.len();
    let locals = iseq.locals.len();
    let room = u32::try_from(iseq.max_stack.max(1)).ok()?;
    let mut steps: Vec<Option<(Step, u32)>> = vec![None; len];
    let mut max_depth = 0;
    let mut env_depth = 0;
    let mut ivars = false;
    let mut work = vec![(0usize, 0u32)];
    while let Some((pc, depth)) = work.pop() {
        let insn = *iseq.insns.get(pc)?;
        match steps[pc] {
            Some((_, known)) if known == depth => continue,
            // Two ways in at two depths: not bytecode the compiler writes.
            Some(_) => return None,
            None => {}
        }
        let op = lower(insn, pc, iseq).filter(|op| match *op {
            // A slot past the frame's locals is the interpreter's to judge.
            Op::GetLocal(slot) | Op::SetLocal(slot) => usize::from(slot) < locals,
            _ => true,
        });
        let Some(op) = op else {
            steps[pc] = Some((Step::Bail, depth));
            continue;
        };
        let (takes, leaves) = effect(op);
        let after = depth.checked_sub(takes)? + leaves;
        if after > room {
            return None;
        }
        max_depth = max_depth.max(after).max(depth);
        match op {
            Op::GetOuter { depth, .. } | Op::SetOuter { depth, .. } => {
                env_depth = env_depth.max(depth);
            }
            Op::GetIvar(_) | Op::SetIvar(_) => ivars = true,
            _ => {}
        }
        steps[pc] = Some((Step::Lowered(op), depth));
        for next in successors(op, pc) {
            // Falling off the end of a body: not bytecode the compiler writes.
            if next >= len {
                return None;
            }
            work.push((next, after));
        }
    }
    if matches!(steps.first(), Some(Some((Step::Bail, _)))) {
        return None;
    }
    Some(Plan {
        steps,
        max_depth,
        env_depth,
        ivars,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iseq(insns: Vec<Insn>, locals: usize, max_stack: usize) -> Iseq {
        Iseq {
            name: "test".into(),
            insns,
            locals: (0..locals).map(|i| format!("l{i}").into()).collect(),
            max_stack,
            ..Iseq::default()
        }
    }

    #[test]
    fn a_body_whose_first_instruction_bails_is_not_compiled() {
        let body = iseq(vec![Insn::Send(0), Insn::Leave], 0, 1);
        assert_eq!(plan(&body), None);
    }

    #[test]
    fn the_walk_stops_at_each_bail_and_follows_both_arms_of_a_branch() {
        // def f(n) = n < 2 ? n : g(n)
        let body = iseq(
            vec![
                Insn::GetLocal(0, 0),   // 0
                Insn::PushInt(2),       // 1
                Insn::BinOp(BinOp::Lt), // 2
                Insn::JumpUnless(2),    // 3 -> 6
                Insn::GetLocal(0, 0),   // 4
                Insn::Leave,            // 5
                Insn::PushSelf,         // 6
                Insn::GetLocal(0, 0),   // 7
                Insn::Send(0),          // 8
                Insn::Leave,            // 9
            ],
            1,
            3,
        );
        let plan = plan(&body).expect("compiles");
        let depth = |pc: usize| plan.steps[pc].map(|(_, depth)| depth);
        assert_eq!(plan.steps[5], Some((Step::Bail, 1)));
        assert_eq!(plan.steps[8], Some((Step::Bail, 2)));
        assert_eq!(plan.steps[9], None, "past a bail-out, nothing is reached");
        assert_eq!(depth(3), Some(1));
        assert_eq!(depth(6), Some(0));
        assert_eq!(plan.lowered(), 7);
        assert_eq!(plan.max_depth, 2);
    }

    #[test]
    fn a_superinstruction_is_read_as_its_head() {
        let mut insns = vec![
            Insn::GetLocal(0, 0),
            Insn::PushInt(1),
            Insn::BinOp(BinOp::Add),
            Insn::Leave,
        ];
        spinel_vm::bytecode::fuse(&mut insns);
        assert!(matches!(insns[0], Insn::GetLocalPushIntBinOp { .. }));
        let plan = plan(&iseq(insns, 1, 2)).expect("compiles");
        assert_eq!(plan.steps[0], Some((Step::Lowered(Op::GetLocal(0)), 0)));
        assert_eq!(
            plan.steps[2],
            Some((Step::Lowered(Op::BinOp(BinOp::Add)), 2))
        );
    }

    #[test]
    fn what_is_not_bytecode_the_compiler_writes_is_not_compiled() {
        // A `Pop` with nothing to pop.
        assert_eq!(
            plan(&iseq(vec![Insn::PushNil, Insn::Pop, Insn::Pop], 0, 1)),
            None
        );
        // Deeper than the compiler said the stack gets.
        assert_eq!(
            plan(&iseq(vec![Insn::PushNil, Insn::PushNil, Insn::Leave], 0, 1)),
            None
        );
        // A jump out of the body.
        assert_eq!(plan(&iseq(vec![Insn::Jump(5)], 0, 1)), None);
        // Falling off the end.
        assert_eq!(plan(&iseq(vec![Insn::PushNil], 0, 1)), None);
        // A local the frame does not have bails, rather than reading past it.
        let plan = plan(&iseq(vec![Insn::PushNil, Insn::GetLocal(3, 0)], 1, 2)).unwrap();
        assert_eq!(plan.steps[1], Some((Step::Bail, 1)));
    }

    #[test]
    fn what_needs_the_heap_bails_and_what_reads_it_in_place_is_recorded() {
        let body = iseq(vec![Insn::Leave], 0, 1);
        assert_eq!(lower(Insn::PushInt(i64::MAX), 0, &body), None);
        assert_eq!(
            lower(Insn::PushFloat(f64::INFINITY.to_bits()), 0, &body),
            None
        );
        assert_eq!(
            lower(Insn::GetLocal(3, 2), 0, &body),
            Some(Op::GetOuter { slot: 3, depth: 2 })
        );
        let body = iseq(
            vec![
                Insn::GetLocal(0, 2),
                Insn::GetIvar(0, 1),
                Insn::BinOp(BinOp::Mul),
                Insn::SetLocal(1, 1),
                Insn::Leave,
            ],
            0,
            2,
        );
        let plan = plan(&body).expect("compiles");
        assert_eq!((plan.env_depth, plan.ivars), (2, true));
        assert_eq!(plan.lowered(), 4);
    }

    /// One instance of every instruction, and whether [`lower`] lowers it:
    /// two where it lowers some operands and not others.
    fn samples() -> Vec<(Insn, bool)> {
        use spinel_vm::bytecode::{ConstScope, MatchRef};
        let flonum = 1.5f64.to_bits();
        vec![
            (Insn::PushNil, true),
            (Insn::PushTrue, true),
            (Insn::PushFalse, true),
            (Insn::PushSelf, true),
            (Insn::PushInt(7), true),
            (Insn::PushInt(i64::MAX), false),
            (Insn::PushFloat(flonum), true),
            (Insn::PushFloat(f64::NAN.to_bits()), false),
            (Insn::PushLit(0), false),
            (Insn::PushSym(0), true),
            (Insn::Intern, false),
            (Insn::Pop, true),
            (Insn::Dup, true),
            (Insn::CaptureSplat, false),
            (Insn::SplatArray, false),
            (Insn::GetLocal(0, 0), true),
            (Insn::GetLocal(0, 1), true),
            (Insn::SetLocal(0, 0), true),
            (Insn::SetLocal(0, 3), true),
            (Insn::Jump(0), true),
            (Insn::JumpUnless(0), true),
            (Insn::JumpIf(0), true),
            (Insn::Refuse(0), false),
            (Insn::JumpUnlessKeep(0), true),
            (Insn::JumpIfKeep(0), true),
            (Insn::JumpIfNilKeep(0), true),
            (Insn::JumpUnlessUndef(0), true),
            (Insn::BinOp(BinOp::Mod), true),
            (Insn::Neg, true),
            (Insn::Not, true),
            (Insn::BlockToProc, false),
            (Insn::NewArray(2), false),
            (Insn::CaseEq, false),
            (Insn::Send(0), false),
            (Insn::Super(0), false),
            (Insn::Yield(0), false),
            (Insn::LeaveThroughEnsure, false),
            (Insn::MakeProc(0, false), false),
            (Insn::DefineMethod(0), false),
            (Insn::DefineSingleton(0), false),
            (Insn::GetConst(0, ConstScope::Top, 0), false),
            (Insn::SetConst(0, ConstScope::Top), false),
            (Insn::DefinedConst(0, ConstScope::Top), false),
            (Insn::OpenClass(0), false),
            (Insn::GetIvar(0, 0), true),
            (Insn::SetIvar(0, 0), true),
            (Insn::DefinedIvar(0), false),
            (Insn::GetGlobal(0), false),
            (Insn::SetGlobal(0), false),
            (Insn::DefinedGlobal(0), false),
            (Insn::DefinedMethod(0), false),
            (Insn::DefinedSelfMethod(0), false),
            (Insn::DefinedYield, false),
            (Insn::DefinedSuper, false),
            (Insn::Leave, false),
            (Insn::Return, false),
            (Insn::Break, false),
            (Insn::Raise, false),
            (Insn::CheckMatch, false),
            (Insn::CheckMatchAny, false),
            (Insn::NewRegexp(0), false),
            (Insn::NewRegexpOnce(0, 0), false),
            (Insn::OnceGet(0, 0), false),
            (Insn::OnceSet(0), false),
            (Insn::GetCvar(0), false),
            (Insn::SetCvar(0), false),
            (Insn::DefinedCvar(0), false),
            (Insn::Alias(0, 0), false),
            (Insn::Undef(0), false),
            (Insn::AliasFromStack, false),
            (Insn::UndefFromStack, false),
            (Insn::AliasGlobal(0, 0), false),
            (Insn::AliasGlobalSpecial(0, MatchRef::Whole), false),
            (Insn::LastMatch(MatchRef::Whole), false),
            (Insn::SetLastMatch, false),
            (Insn::Errinfo, false),
            (Insn::SetErrinfo, false),
            (Insn::DefinedMatch(MatchRef::Whole), false),
            (Insn::Goto(0, 0), false),
            (Insn::GotoValue(0, 0), false),
            (Insn::EnterEnsure, false),
            (Insn::LeaveEnsure, false),
            (
                Insn::GetLocalPushIntBinOp {
                    slot: 0,
                    int: 1,
                    op: BinOp::Add,
                },
                true,
            ),
            (
                Insn::GetLocalGetLocalBinOp {
                    left: 0,
                    right: 0,
                    op: BinOp::Lt,
                },
                true,
            ),
            (
                Insn::GetLocalGetIvarBinOp {
                    slot: 0,
                    site: 0,
                    op: BinOp::Add,
                },
                true,
            ),
            (Insn::PushSelfGetLocal { slot: 0 }, true),
            (
                Insn::GetLocalBinOp {
                    slot: 0,
                    op: BinOp::Mul,
                },
                true,
            ),
        ]
    }

    /// An instruction's `COVERAGE` name. No catch-all: a new `Insn` is a
    /// compile error here until it has a name, and so a row and a sample.
    fn name(insn: Insn) -> &'static str {
        match insn {
            Insn::PushNil => "PushNil",
            Insn::PushTrue => "PushTrue",
            Insn::PushFalse => "PushFalse",
            Insn::PushSelf => "PushSelf",
            Insn::PushInt(..) => "PushInt",
            Insn::PushFloat(..) => "PushFloat",
            Insn::PushLit(..) => "PushLit",
            Insn::PushSym(..) => "PushSym",
            Insn::Intern => "Intern",
            Insn::Pop => "Pop",
            Insn::Dup => "Dup",
            Insn::CaptureSplat => "CaptureSplat",
            Insn::SplatArray => "SplatArray",
            Insn::GetLocal(..) => "GetLocal",
            Insn::SetLocal(..) => "SetLocal",
            Insn::Jump(..) => "Jump",
            Insn::JumpUnless(..) => "JumpUnless",
            Insn::JumpIf(..) => "JumpIf",
            Insn::Refuse(..) => "Refuse",
            Insn::JumpUnlessKeep(..) => "JumpUnlessKeep",
            Insn::JumpIfKeep(..) => "JumpIfKeep",
            Insn::JumpIfNilKeep(..) => "JumpIfNilKeep",
            Insn::JumpUnlessUndef(..) => "JumpUnlessUndef",
            Insn::BinOp(..) => "BinOp",
            Insn::Neg => "Neg",
            Insn::Not => "Not",
            Insn::BlockToProc => "BlockToProc",
            Insn::NewArray(..) => "NewArray",
            Insn::CaseEq => "CaseEq",
            Insn::Send(..) => "Send",
            Insn::Super(..) => "Super",
            Insn::Yield(..) => "Yield",
            Insn::LeaveThroughEnsure => "LeaveThroughEnsure",
            Insn::MakeProc(..) => "MakeProc",
            Insn::DefineMethod(..) => "DefineMethod",
            Insn::DefineSingleton(..) => "DefineSingleton",
            Insn::GetConst(..) => "GetConst",
            Insn::SetConst(..) => "SetConst",
            Insn::DefinedConst(..) => "DefinedConst",
            Insn::OpenClass(..) => "OpenClass",
            Insn::GetIvar(..) => "GetIvar",
            Insn::SetIvar(..) => "SetIvar",
            Insn::DefinedIvar(..) => "DefinedIvar",
            Insn::GetGlobal(..) => "GetGlobal",
            Insn::SetGlobal(..) => "SetGlobal",
            Insn::DefinedGlobal(..) => "DefinedGlobal",
            Insn::DefinedMethod(..) => "DefinedMethod",
            Insn::DefinedSelfMethod(..) => "DefinedSelfMethod",
            Insn::DefinedYield => "DefinedYield",
            Insn::DefinedSuper => "DefinedSuper",
            Insn::Leave => "Leave",
            Insn::Return => "Return",
            Insn::Break => "Break",
            Insn::Raise => "Raise",
            Insn::CheckMatch => "CheckMatch",
            Insn::CheckMatchAny => "CheckMatchAny",
            Insn::NewRegexp(..) => "NewRegexp",
            Insn::NewRegexpOnce(..) => "NewRegexpOnce",
            Insn::OnceGet(..) => "OnceGet",
            Insn::OnceSet(..) => "OnceSet",
            Insn::GetCvar(..) => "GetCvar",
            Insn::SetCvar(..) => "SetCvar",
            Insn::DefinedCvar(..) => "DefinedCvar",
            Insn::Alias(..) => "Alias",
            Insn::Undef(..) => "Undef",
            Insn::AliasFromStack => "AliasFromStack",
            Insn::UndefFromStack => "UndefFromStack",
            Insn::AliasGlobal(..) => "AliasGlobal",
            Insn::AliasGlobalSpecial(..) => "AliasGlobalSpecial",
            Insn::LastMatch(..) => "LastMatch",
            Insn::SetLastMatch => "SetLastMatch",
            Insn::Errinfo => "Errinfo",
            Insn::SetErrinfo => "SetErrinfo",
            Insn::DefinedMatch(..) => "DefinedMatch",
            Insn::Goto(..) => "Goto",
            Insn::GotoValue(..) => "GotoValue",
            Insn::EnterEnsure => "EnterEnsure",
            Insn::LeaveEnsure => "LeaveEnsure",
            Insn::GetLocalPushIntBinOp { .. } => "GetLocalPushIntBinOp",
            Insn::GetLocalGetLocalBinOp { .. } => "GetLocalGetLocalBinOp",
            Insn::GetLocalGetIvarBinOp { .. } => "GetLocalGetIvarBinOp",
            Insn::PushSelfGetLocal { .. } => "PushSelfGetLocal",
            Insn::GetLocalBinOp { .. } => "GetLocalBinOp",
        }
    }

    #[test]
    fn every_instruction_is_lowered_or_bails_as_the_coverage_table_says() {
        let samples = samples();
        for (i, &(name, ..)) in COVERAGE.iter().enumerate() {
            assert!(
                COVERAGE[..i].iter().all(|&(seen, ..)| seen != name),
                "{name} has two rows in `COVERAGE`"
            );
        }
        for &(insn, _) in &samples {
            let name = self::name(insn);
            assert!(
                COVERAGE.iter().any(|&(row, ..)| row == name),
                "{name} has no row in `COVERAGE`"
            );
        }
        // A body each sample can be lowered in: a local, a symbol, and room
        // for a jump to land.
        let mut body = iseq(vec![Insn::Leave, Insn::Leave], 1, 2);
        body.symbols = vec!["s".into()];
        for &(name, coverage, why) in COVERAGE {
            let lowered: Vec<bool> = samples
                .iter()
                .filter(|&&(insn, _)| self::name(insn) == name)
                .map(|&(insn, expected)| {
                    assert_eq!(
                        lower(insn, 0, &body).is_some(),
                        expected,
                        "{insn:?}: `lower` disagrees with the sample"
                    );
                    expected
                })
                .collect();
            assert!(!lowered.is_empty(), "{name} has no sample");
            let holds = match coverage {
                Coverage::Lowered | Coverage::Fused => lowered.iter().all(|&l| l),
                Coverage::Bails => lowered.iter().all(|&l| !l),
                Coverage::Partly => lowered.contains(&true) && lowered.contains(&false),
            };
            assert!(holds, "{name} is not {coverage:?} ({why}): {lowered:?}");
            assert!(
                coverage == Coverage::Lowered || coverage == Coverage::Fused || !why.is_empty(),
                "{name} needs its reason"
            );
        }
    }

    /// The table as the pull request shows it: `cargo test -p spinel-jit
    /// coverage_table -- --ignored --nocapture`.
    #[test]
    #[ignore = "prints the table"]
    fn coverage_table() {
        println!("| Instruction | Lowering | Rule |\n|---|---|---|");
        for &(name, coverage, why) in COVERAGE {
            println!("| `{name}` | {coverage:?} | {why} |");
        }
    }
}
