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
    /// Two fixnums under an operator the frame's mask says is `Integer`'s
    /// own, whose answer is a fixnum or a boolean: anything else bails out
    /// at the instruction, its operands still on the stack.
    BinOp(BinOp),
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

/// What instruction `insn`, at `pc`, becomes; `None` for a bail-out.
///
/// Every instruction is named, with no catch-all arm, so that a new one is a
/// compile error here until someone decides whether it lowers: that is the
/// "lowering covers every opcode or explicitly bails out" of #120.
#[must_use]
pub fn lower(insn: Insn, pc: usize, len: usize) -> Option<Op> {
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
        Insn::Pop => Some(Op::Pop),
        Insn::Dup => Some(Op::Dup),
        // A local of an enclosing scope lives in a heap environment.
        Insn::GetLocal(slot, 0) => Some(Op::GetLocal(slot)),
        Insn::SetLocal(slot, 0) => Some(Op::SetLocal(slot)),
        Insn::GetLocal(..) | Insn::SetLocal(..) => None,
        Insn::Jump(displacement) => target(displacement).map(Op::Jump),
        Insn::JumpUnless(d) => branch(d, Test::Truthy, false, false),
        Insn::JumpIf(d) => branch(d, Test::Truthy, true, false),
        Insn::JumpUnlessKeep(d) => branch(d, Test::Truthy, false, true),
        Insn::JumpIfKeep(d) => branch(d, Test::Truthy, true, true),
        Insn::JumpIfNilKeep(d) => branch(d, Test::Nil, true, true),
        // Jumps unless the value *is* the marker: the default is skipped.
        Insn::JumpUnlessUndef(d) => branch(d, Test::Undef, false, false),
        // The operators the light loops answer for two fixnums. `*`, `/`
        // and `%` are `run`'s there, and here the interpreter's.
        Insn::BinOp(
            op @ (BinOp::Add
            | BinOp::Sub
            | BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge
            | BinOp::Eq
            | BinOp::Neq),
        ) => Some(Op::BinOp(op)),
        Insn::BinOp(BinOp::Mul | BinOp::Div | BinOp::Mod) => None,
        // Leaving a frame is the interpreter's, with its `ensure`s, traces
        // and `Class#new`'s receiver: compiled code stops at the
        // instruction, with the value on the stack.
        Insn::Leave | Insn::Return | Insn::Break | Insn::LeaveThroughEnsure => None,
        // Calls, and everything that may call, allocate, raise, read the
        // heap or change the frames. Phase 6's later slices take some of
        // these on (#122, #123, #124).
        Insn::PushFloat(_)
        | Insn::PushLit(_)
        | Insn::PushSym(_)
        | Insn::Intern
        | Insn::CaptureSplat
        | Insn::SplatArray
        | Insn::Refuse(_)
        | Insn::Neg
        | Insn::Not
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
        | Insn::GetIvar(..)
        | Insn::SetIvar(..)
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

/// The operands `op` takes, and the values it leaves in their place.
const fn effect(op: Op) -> (u32, u32) {
    match op {
        Op::Push(_) | Op::PushSelf | Op::GetLocal(_) => (0, 1),
        Op::Pop | Op::SetLocal(_) => (1, 0),
        Op::Dup => (1, 2),
        Op::Jump(_) => (0, 0),
        Op::Branch { keeps: true, .. } => (1, 1),
        Op::Branch { keeps: false, .. } => (1, 0),
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
    let mut work = vec![(0usize, 0u32)];
    while let Some((pc, depth)) = work.pop() {
        let insn = *iseq.insns.get(pc)?;
        match steps[pc] {
            Some((_, known)) if known == depth => continue,
            // Two ways in at two depths: not bytecode the compiler writes.
            Some(_) => return None,
            None => {}
        }
        let op = lower(insn, pc, len).filter(|op| match *op {
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
    Some(Plan { steps, max_depth })
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
        let body = iseq(vec![Insn::PushSym(0), Insn::Leave], 0, 1);
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
    fn a_fixnum_out_of_range_and_an_outer_local_bail() {
        assert_eq!(lower(Insn::PushInt(i64::MAX), 0, 1), None);
        assert_eq!(lower(Insn::GetLocal(0, 1), 0, 1), None);
        assert_eq!(lower(Insn::BinOp(BinOp::Mul), 0, 1), None);
    }
}
