//! A [`Plan`] in Cranelift IR.
//!
//! One Cranelift block per reachable instruction, plus one bail-out block per
//! instruction compiled code can stop at. The operand stack is a Cranelift
//! variable per depth, so the frontend turns it into SSA values; the locals
//! are loads and stores of the frame's own slots, so a bail-out has nothing
//! to write back but the operands. Every lowered instruction first checks the
//! fuel, then its guards, and only then charges one and does its work, so a
//! bail-out at an instruction has charged nothing for it.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, MemFlags, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use spinel_vm::Value;
use spinel_vm::bytecode::BinOp;
use spinel_vm::jit::Activation;

use crate::lower::{Op, Plan, Step, Test};

/// Fill `func`, whose signature [`signature`] made, with `plan`.
pub fn build(func: &mut ir::Function, context: &mut FunctionBuilderContext, plan: &Plan) {
    let mut b = FunctionBuilder::new(func, context);
    let flags = MemFlags::trusted();
    let word = types::I64;

    // The operand stack, one variable a depth, and the fuel left.
    let depth_vars: Vec<Variable> = (0..plan.max_depth).map(Variable::from_u32).collect();
    for &var in &depth_vars {
        b.declare_var(var, word);
    }
    let fuel_var = Variable::from_u32(plan.max_depth);
    b.declare_var(fuel_var, word);

    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    let activation = b.block_params(entry)[0];
    let field = |b: &mut FunctionBuilder, offset: usize| {
        let offset = i32::try_from(offset).expect("a small struct");
        b.ins().load(word, flags, activation, offset)
    };
    let locals = field(&mut b, std::mem::offset_of!(Activation, locals));
    let operands = field(&mut b, std::mem::offset_of!(Activation, operands));
    let receiver = field(&mut b, std::mem::offset_of!(Activation, receiver));
    let ops = field(&mut b, std::mem::offset_of!(Activation, ops));
    let fuel0 = field(&mut b, std::mem::offset_of!(Activation, fuel));
    b.def_var(fuel_var, fuel0);

    // A block for each instruction compiled code reaches, and a bail-out
    // for each it may stop at: every lowered one, for want of fuel, and
    // every one it does not lower.
    let blocks: Vec<Option<ir::Block>> = plan
        .steps
        .iter()
        .map(|step| step.map(|_| b.create_block()))
        .collect();
    let bails: Vec<Option<ir::Block>> = plan
        .steps
        .iter()
        .map(|step| step.map(|_| b.create_block()))
        .collect();
    b.ins()
        .jump(blocks[0].expect("the first instruction is reached"), &[]);

    let block_at = |pc: usize| blocks[pc].expect("a successor the plan reached");
    for (pc, step) in plan.steps.iter().enumerate() {
        let Some((step, depth)) = *step else { continue };
        let depth = depth as usize;
        let here = block_at(pc);
        let bail = bails[pc].expect("made beside the block");

        // The bail-out: the operands, what ran, and where to go on.
        b.switch_to_block(bail);
        for (at, &var) in depth_vars[..depth].iter().enumerate() {
            let value = b.use_var(var);
            let offset = i32::try_from(at * 8).expect("a small stack");
            b.ins().store(flags, value, operands, offset);
        }
        let left = b.use_var(fuel_var);
        let ran = b.ins().isub(fuel0, left);
        let ran_at = i32::try_from(std::mem::offset_of!(Activation, ran)).expect("small");
        b.ins().store(flags, ran, activation, ran_at);
        let depth_word = b.ins().iconst(word, i64::try_from(depth).expect("small"));
        let depth_at = i32::try_from(std::mem::offset_of!(Activation, depth)).expect("small");
        b.ins().store(flags, depth_word, activation, depth_at);
        let pc_word = b
            .ins()
            .iconst(word, i64::try_from(pc).expect("a small body"));
        b.ins().return_(&[pc_word]);

        b.switch_to_block(here);
        let Step::Lowered(op) = step else {
            b.ins().jump(bail, &[]);
            continue;
        };
        // Fuel first: out of it, the interpreter runs this instruction, with
        // its slow step before it.
        let fuel = b.use_var(fuel_var);
        let body = b.create_block();
        b.ins().brif(fuel, body, &[], bail, &[]);
        b.switch_to_block(body);
        let top =
            |b: &mut FunctionBuilder, from_top: usize| b.use_var(depth_vars[depth - from_top]);
        let charge = |b: &mut FunctionBuilder| {
            let less = b.ins().iadd_imm(fuel, -1);
            b.def_var(fuel_var, less);
        };
        match op {
            Op::Push(bits) => {
                charge(&mut b);
                let value = b.ins().iconst(word, bits as i64);
                b.def_var(depth_vars[depth], value);
                b.ins().jump(block_at(pc + 1), &[]);
            }
            Op::PushSelf => {
                charge(&mut b);
                b.def_var(depth_vars[depth], receiver);
                b.ins().jump(block_at(pc + 1), &[]);
            }
            Op::Pop => {
                charge(&mut b);
                b.ins().jump(block_at(pc + 1), &[]);
            }
            Op::Dup => {
                charge(&mut b);
                let value = top(&mut b, 1);
                b.def_var(depth_vars[depth], value);
                b.ins().jump(block_at(pc + 1), &[]);
            }
            Op::GetLocal(slot) => {
                charge(&mut b);
                let value = b.ins().load(word, flags, locals, i32::from(slot) * 8);
                b.def_var(depth_vars[depth], value);
                b.ins().jump(block_at(pc + 1), &[]);
            }
            Op::SetLocal(slot) => {
                charge(&mut b);
                let value = top(&mut b, 1);
                b.ins().store(flags, value, locals, i32::from(slot) * 8);
                b.ins().jump(block_at(pc + 1), &[]);
            }
            Op::Jump(target) => {
                charge(&mut b);
                b.ins().jump(block_at(target), &[]);
            }
            Op::Branch {
                test, when, target, ..
            } => {
                charge(&mut b);
                let value = top(&mut b, 1);
                let holds = match test {
                    // The two falsy words differ in one bit, as
                    // `Value::is_truthy` reads them.
                    Test::Truthy => {
                        let folded = b.ins().bor_imm(value, 0x10);
                        b.ins()
                            .icmp_imm(IntCC::NotEqual, folded, bits(Value::FALSE))
                    }
                    Test::Nil => b.ins().icmp_imm(IntCC::Equal, value, bits(Value::NIL)),
                    Test::Undef => b.ins().icmp_imm(IntCC::Equal, value, bits(Value::UNDEF)),
                };
                let (taken, fallen) = (block_at(target), block_at(pc + 1));
                if when {
                    b.ins().brif(holds, taken, &[], fallen, &[]);
                } else {
                    b.ins().brif(holds, fallen, &[], taken, &[]);
                }
            }
            Op::BinOp(operator) => {
                let (left, right) = (top(&mut b, 2), top(&mut b, 1));
                // The operator is still `Integer`'s own for this frame, and
                // both are fixnums: their low bits are both set.
                let mask = b.ins().ushr_imm(ops, i64::from(operator as u8));
                let ours = b.ins().band_imm(mask, 1);
                let both = b.ins().band(left, right);
                let tagged = b.ins().band(both, ours);
                let guarded = b.create_block();
                b.ins().brif(tagged, guarded, &[], bail, &[]);
                b.switch_to_block(guarded);
                let value = match operator {
                    // On the tagged words, as `Value::fixnum_add` and
                    // `fixnum_sub` compute them: `2a+1 ± 2b` is `2(a±b)+1`,
                    // and it overflows exactly when the answer is past the
                    // fixnum range, which is the interpreter's Bignum.
                    BinOp::Add | BinOp::Sub => {
                        let untagged = b.ins().iadd_imm(right, -1);
                        let (value, overflowed) = if operator == BinOp::Add {
                            b.ins().sadd_overflow(left, untagged)
                        } else {
                            b.ins().ssub_overflow(left, untagged)
                        };
                        let fits = b.create_block();
                        b.ins().brif(overflowed, bail, &[], fits, &[]);
                        b.switch_to_block(fits);
                        value
                    }
                    // Tagged words order as the numbers they hold.
                    _ => {
                        let condition = match operator {
                            BinOp::Lt => IntCC::SignedLessThan,
                            BinOp::Le => IntCC::SignedLessThanOrEqual,
                            BinOp::Gt => IntCC::SignedGreaterThan,
                            BinOp::Ge => IntCC::SignedGreaterThanOrEqual,
                            BinOp::Eq => IntCC::Equal,
                            BinOp::Neq => IntCC::NotEqual,
                            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                                unreachable!("`lower` lowers no other operator")
                            }
                        };
                        let holds = b.ins().icmp(condition, left, right);
                        let yes = b.ins().iconst(word, bits(Value::TRUE));
                        let no = b.ins().iconst(word, bits(Value::FALSE));
                        b.ins().select(holds, yes, no)
                    }
                };
                charge(&mut b);
                b.def_var(depth_vars[depth - 2], value);
                b.ins().jump(block_at(pc + 1), &[]);
            }
        }
    }
    b.seal_all_blocks();
    b.finalize();
}

/// `(activation) -> pc`, as [`spinel_vm::jit::Entry`] is.
pub fn signature(call_conv: cranelift_codegen::isa::CallConv) -> ir::Signature {
    let mut signature = ir::Signature::new(call_conv);
    signature.params.push(AbiParam::new(types::I64));
    signature.returns.push(AbiParam::new(types::I64));
    signature
}

/// A value's word, as Cranelift's immediates take it.
fn bits(value: Value) -> i64 {
    value.to_bits() as i64
}
