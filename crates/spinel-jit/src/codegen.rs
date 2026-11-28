//! A [`Plan`] in Cranelift IR.
//!
//! One Cranelift block per reachable instruction, plus one bail-out block per
//! instruction compiled code can stop at. The operand stack is a Cranelift
//! variable per depth, so the frontend turns it into SSA values; the locals,
//! the enclosing scopes' and `self`'s instance variables are loads and
//! stores of the slots themselves, so a bail-out has nothing to write back
//! but the operands. Every lowered instruction first checks the fuel, then
//! its guards, and only then charges one and does its work, so a bail-out at
//! an instruction has charged nothing for it.

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, MemFlags, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use spinel_vm::Value;
use spinel_vm::bytecode::BinOp;
use spinel_vm::jit::{
    Activation, IVAR_ENTRY_INDEX, IVAR_ENTRY_SHAPE, IVAR_ENTRY_SIZE, Ivars, Locals, NEG_OPERATOR,
};

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
    let float_ops = field(&mut b, std::mem::offset_of!(Activation, float_ops));
    let fuel0 = field(&mut b, std::mem::offset_of!(Activation, fuel));
    b.def_var(fuel_var, fuel0);
    let ivar = |b: &mut FunctionBuilder, offset: usize| {
        field(b, std::mem::offset_of!(Activation, ivars) + offset)
    };
    let (envs, ivars) = (
        (plan.env_depth > 0).then(|| field(&mut b, std::mem::offset_of!(Activation, envs))),
        plan.ivars.then(|| IvarFields {
            shape: ivar(&mut b, std::mem::offset_of!(Ivars, shape)),
            values: ivar(&mut b, std::mem::offset_of!(Ivars, values)),
            len: ivar(&mut b, std::mem::offset_of!(Ivars, len)),
            caches: ivar(&mut b, std::mem::offset_of!(Ivars, caches)),
            caches_len: ivar(&mut b, std::mem::offset_of!(Ivars, caches_len)),
            writable: ivar(&mut b, std::mem::offset_of!(Ivars, writable)),
        }),
    );

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
            Op::GetOuter { slot, depth: out } => {
                let envs = envs.expect("the plan reaches out");
                let at = outer(&mut b, envs, out, slot, bail);
                let value = b.ins().load(word, flags, at, 0);
                charge(&mut b);
                b.def_var(depth_vars[depth], value);
                b.ins().jump(block_at(pc + 1), &[]);
            }
            Op::SetOuter { slot, depth: out } => {
                let envs = envs.expect("the plan reaches out");
                let at = outer(&mut b, envs, out, slot, bail);
                charge(&mut b);
                let value = top(&mut b, 1);
                b.ins().store(flags, value, at, 0);
                b.ins().jump(block_at(pc + 1), &[]);
            }
            Op::GetIvar(site) => {
                let ivars = ivars.expect("the plan reads ivars");
                let at = ivar_slot(&mut b, &ivars, site, bail);
                let value = b.ins().load(word, flags, at, 0);
                charge(&mut b);
                b.def_var(depth_vars[depth], value);
                b.ins().jump(block_at(pc + 1), &[]);
            }
            Op::SetIvar(site) => {
                let ivars = ivars.expect("the plan writes ivars");
                let value = top(&mut b, 1);
                // An immediate, to an object that may be written, as
                // `IvarView::write` asks: a heap value is the interpreter's.
                let tag = b.ins().band_imm(value, 0b111);
                let immediate = b.ins().icmp_imm(IntCC::NotEqual, tag, 0);
                let writable = b.ins().icmp_imm(IntCC::NotEqual, ivars.writable, 0);
                let ok = b.ins().band(immediate, writable);
                guard(&mut b, ok, bail);
                let at = ivar_slot(&mut b, &ivars, site, bail);
                charge(&mut b);
                b.ins().store(flags, value, at, 0);
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
                    Test::Truthy => truthy(&mut b, value),
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
            Op::Not => {
                let value = top(&mut b, 1);
                // `!` of an immediate is its falsiness, as the interpreter
                // answers it without a lookup; a heap object may define one.
                let tag = b.ins().band_imm(value, 0b111);
                guard(&mut b, tag, bail);
                let holds = truthy(&mut b, value);
                let yes = b.ins().iconst(word, bits(Value::FALSE));
                let no = b.ins().iconst(word, bits(Value::TRUE));
                let answer = b.ins().select(holds, yes, no);
                charge(&mut b);
                b.def_var(depth_vars[depth - 1], answer);
                b.ins().jump(block_at(pc + 1), &[]);
            }
            Op::Neg => {
                let value = top(&mut b, 1);
                let done = b.create_block();
                b.append_block_param(done, word);
                let (fixnum, flonum) = (b.create_block(), b.create_block());
                let ours = mask_bit(&mut b, ops, NEG_OPERATOR);
                let tagged = b.ins().band(value, ours);
                b.ins().brif(tagged, fixnum, &[], flonum, &[]);

                // `-(2n+1) + 2` is `2(-n)+1`, and overflows exactly when
                // `-n` is past the fixnum range: `-FIXNUM_MIN`, a Bignum.
                b.switch_to_block(fixnum);
                let two = b.ins().iconst(word, 2);
                let (negated, overflowed) = b.ins().ssub_overflow(two, value);
                let fits = b.create_block();
                b.ins().brif(overflowed, bail, &[], fits, &[]);
                b.switch_to_block(fits);
                b.ins().jump(done, &[negated.into()]);

                b.switch_to_block(flonum);
                let ours = mask_bit(&mut b, float_ops, NEG_OPERATOR);
                let is = is_flonum(&mut b, value);
                let ok = b.ins().band(is, ours);
                guard(&mut b, ok, bail);
                let f = decode(&mut b, value);
                let negated = b.ins().fneg(f);
                let answer = encode(&mut b, negated, bail);
                b.ins().jump(done, &[answer.into()]);

                b.switch_to_block(done);
                let answer = b.block_params(done)[0];
                charge(&mut b);
                b.def_var(depth_vars[depth - 1], answer);
                b.ins().jump(block_at(pc + 1), &[]);
            }
            Op::BinOp(operator) => {
                let (left, right) = (top(&mut b, 2), top(&mut b, 1));
                let done = b.create_block();
                b.append_block_param(done, word);
                let (fixnums, flonums) = (b.create_block(), b.create_block());
                // The operator is still `Integer`'s own for this frame, and
                // both are fixnums: their low bits are both set.
                let ours = mask_bit(&mut b, ops, operator as u32);
                let both = b.ins().band(left, right);
                let tagged = b.ins().band(both, ours);
                b.ins().brif(tagged, fixnums, &[], flonums, &[]);

                b.switch_to_block(fixnums);
                let answer = fixnum_binop(&mut b, operator, left, right, bail);
                b.ins().jump(done, &[answer.into()]);

                // Two flonums under `Float`'s own operator, as
                // `flonum_binop` answers them: `%`, whose zero divisor
                // raises, is the interpreter's.
                b.switch_to_block(flonums);
                if operator == BinOp::Mod {
                    b.ins().jump(bail, &[]);
                } else {
                    let ours = mask_bit(&mut b, float_ops, operator as u32);
                    let (l, r) = (is_flonum(&mut b, left), is_flonum(&mut b, right));
                    let both = b.ins().band(l, r);
                    let ok = b.ins().band(both, ours);
                    guard(&mut b, ok, bail);
                    let (a, c) = (decode(&mut b, left), decode(&mut b, right));
                    let answer = flonum_binop(&mut b, operator, a, c, bail);
                    b.ins().jump(done, &[answer.into()]);
                }

                b.switch_to_block(done);
                let answer = b.block_params(done)[0];
                charge(&mut b);
                b.def_var(depth_vars[depth - 2], answer);
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

/// The fields of [`Activation::ivars`], loaded once on entry.
#[derive(Clone, Copy)]
struct IvarFields {
    shape: ir::Value,
    values: ir::Value,
    len: ir::Value,
    caches: ir::Value,
    caches_len: ir::Value,
    writable: ir::Value,
}

const FLAGS: MemFlags = MemFlags::trusted();
const WORD: ir::Type = types::I64;

/// Go on when `ok` is non-zero, and bail out otherwise.
fn guard(b: &mut FunctionBuilder, ok: ir::Value, bail: ir::Block) {
    let next = b.create_block();
    b.ins().brif(ok, next, &[], bail, &[]);
    b.switch_to_block(next);
}

/// Bit `bit` of `mask`, as 0 or 1.
fn mask_bit(b: &mut FunctionBuilder, mask: ir::Value, bit: u32) -> ir::Value {
    let shifted = b.ins().ushr_imm(mask, i64::from(bit));
    b.ins().band_imm(shifted, 1)
}

/// Whether `value` is truthy, as `Value::is_truthy` reads it.
fn truthy(b: &mut FunctionBuilder, value: ir::Value) -> ir::Value {
    let folded = b.ins().bor_imm(value, 0x10);
    b.ins()
        .icmp_imm(IntCC::NotEqual, folded, bits(Value::FALSE))
}

/// The address of local `slot` of the scope `depth` out, or a bail-out when
/// that scope has fewer.
fn outer(
    b: &mut FunctionBuilder,
    envs: ir::Value,
    depth: u16,
    slot: u16,
    bail: ir::Block,
) -> ir::Value {
    let entry = (usize::from(depth) - 1) * size_of::<Locals>();
    let offset = |field: usize| i32::try_from(entry + field).expect("a short chain");
    let values = b.ins().load(
        WORD,
        FLAGS,
        envs,
        offset(std::mem::offset_of!(Locals, values)),
    );
    let len = b
        .ins()
        .load(WORD, FLAGS, envs, offset(std::mem::offset_of!(Locals, len)));
    let inside = b
        .ins()
        .icmp_imm(IntCC::UnsignedGreaterThan, len, i64::from(slot));
    guard(b, inside, bail);
    b.ins().iadd_imm(values, i64::from(slot) * 8)
}

/// The address of `self`'s instance variable that site `site`'s cache
/// names, or a bail-out when the cache does not hold `self`'s shape, as
/// `IvarView::read` asks.
fn ivar_slot(b: &mut FunctionBuilder, ivars: &IvarFields, site: u32, bail: ir::Block) -> ir::Value {
    let inside = b.ins().icmp_imm(
        IntCC::UnsignedGreaterThan,
        ivars.caches_len,
        i64::from(site),
    );
    guard(b, inside, bail);
    let entry = i64::from(site) * i64::try_from(IVAR_ENTRY_SIZE).expect("small");
    let entry = b.ins().iadd_imm(ivars.caches, entry);
    let shape_at = i32::try_from(IVAR_ENTRY_SHAPE).expect("small");
    let index_at = i32::try_from(IVAR_ENTRY_INDEX).expect("small");
    let shape = b.ins().uload16(WORD, FLAGS, entry, shape_at);
    let index = b.ins().uload16(WORD, FLAGS, entry, index_at);
    let same = b.ins().icmp(IntCC::Equal, shape, ivars.shape);
    let below = b.ins().icmp(IntCC::UnsignedLessThan, index, ivars.len);
    let ok = b.ins().band(same, below);
    guard(b, ok, bail);
    let offset = b.ins().ishl_imm(index, 3);
    b.ins().iadd(ivars.values, offset)
}

/// `left op right` for two fixnums whose answer is a fixnum or a boolean, as
/// `fixnum_binop` and `integer_op` answer them: an answer past the fixnum
/// range, which is a Bignum, and a zero divisor, which raises, bail out.
fn fixnum_binop(
    b: &mut FunctionBuilder,
    operator: BinOp,
    left: ir::Value,
    right: ir::Value,
    bail: ir::Block,
) -> ir::Value {
    let overflow = |b: &mut FunctionBuilder, (value, overflowed): (ir::Value, ir::Value)| {
        let fits = b.create_block();
        b.ins().brif(overflowed, bail, &[], fits, &[]);
        b.switch_to_block(fits);
        value
    };
    let condition = match operator {
        // On the tagged words, as `Value::fixnum_add` and `fixnum_sub`
        // compute them: `2a+1 ± 2b` is `2(a±b)+1`, and it overflows exactly
        // when the answer is past the fixnum range.
        BinOp::Add | BinOp::Sub => {
            let untagged = b.ins().iadd_imm(right, -1);
            let sum = if operator == BinOp::Add {
                b.ins().sadd_overflow(left, untagged)
            } else {
                b.ins().ssub_overflow(left, untagged)
            };
            return overflow(b, sum);
        }
        // `a * 2b` is `2ab`, which overflows exactly when `ab` is past the
        // fixnum range; the tag goes back on after.
        BinOp::Mul => {
            let a = b.ins().sshr_imm(left, 1);
            let twice = b.ins().iadd_imm(right, -1);
            let product = b.ins().smul_overflow(a, twice);
            let product = overflow(b, product);
            return b.ins().bor_imm(product, 1);
        }
        // Ruby's `/` and `%` floor where the machine's truncate: `floor_div`
        // and `floor_mod`. The operands are within the fixnum range, so the
        // machine's division cannot trap but on a zero divisor, which is
        // `ZeroDivisionError` and the interpreter's.
        BinOp::Div | BinOp::Mod => {
            let a = b.ins().sshr_imm(left, 1);
            let d = b.ins().sshr_imm(right, 1);
            guard(b, d, bail);
            let quotient = b.ins().sdiv(a, d);
            let remainder = b.ins().srem(a, d);
            // The remainder is not zero and its sign is not the divisor's.
            let signs = b.ins().bxor(remainder, d);
            let differ = b.ins().icmp_imm(IntCC::SignedLessThan, signs, 0);
            let inexact = b.ins().icmp_imm(IntCC::NotEqual, remainder, 0);
            let adjust = b.ins().band(differ, inexact);
            let answer = if operator == BinOp::Div {
                let less = b.ins().iadd_imm(quotient, -1);
                b.ins().select(adjust, less, quotient)
            } else {
                let more = b.ins().iadd(remainder, d);
                b.ins().select(adjust, more, remainder)
            };
            // Only `FIXNUM_MIN / -1` leaves the range.
            let tagged = b.ins().ishl_imm(answer, 1);
            let back = b.ins().sshr_imm(tagged, 1);
            let fits = b.ins().icmp(IntCC::Equal, back, answer);
            guard(b, fits, bail);
            return b.ins().bor_imm(tagged, 1);
        }
        // Tagged words order as the numbers they hold.
        BinOp::Lt => IntCC::SignedLessThan,
        BinOp::Le => IntCC::SignedLessThanOrEqual,
        BinOp::Gt => IntCC::SignedGreaterThan,
        BinOp::Ge => IntCC::SignedGreaterThanOrEqual,
        BinOp::Eq => IntCC::Equal,
        BinOp::Neq => IntCC::NotEqual,
    };
    let holds = b.ins().icmp(condition, left, right);
    boolean(b, holds)
}

/// `a op b` for two doubles a pair of flonums held, whose answer is a
/// flonum or a boolean, as `flonum_binop` answers it: a double no flonum
/// holds, which allocates, bails out.
fn flonum_binop(
    b: &mut FunctionBuilder,
    operator: BinOp,
    a: ir::Value,
    c: ir::Value,
    bail: ir::Block,
) -> ir::Value {
    let condition = match operator {
        BinOp::Add => {
            let sum = b.ins().fadd(a, c);
            return encode(b, sum, bail);
        }
        BinOp::Sub => {
            let difference = b.ins().fsub(a, c);
            return encode(b, difference, bail);
        }
        BinOp::Mul => {
            let product = b.ins().fmul(a, c);
            return encode(b, product, bail);
        }
        BinOp::Div => {
            let quotient = b.ins().fdiv(a, c);
            return encode(b, quotient, bail);
        }
        BinOp::Mod => unreachable!("`%` of two Floats is the interpreter's"),
        BinOp::Lt => FloatCC::LessThan,
        BinOp::Le => FloatCC::LessThanOrEqual,
        BinOp::Gt => FloatCC::GreaterThan,
        BinOp::Ge => FloatCC::GreaterThanOrEqual,
        BinOp::Eq => FloatCC::Equal,
        BinOp::Neq => FloatCC::NotEqual,
    };
    let holds = b.ins().fcmp(condition, a, c);
    boolean(b, holds)
}

/// `true` or `false` for a Cranelift condition.
fn boolean(b: &mut FunctionBuilder, holds: ir::Value) -> ir::Value {
    let yes = b.ins().iconst(WORD, bits(Value::TRUE));
    let no = b.ins().iconst(WORD, bits(Value::FALSE));
    b.ins().select(holds, yes, no)
}

/// The words `Value::flonum` and `Value::as_flonum` agree on.
const FLONUM_ZERO: i64 = 0x8000_0000_0000_0002_u64 as i64;
const FLONUM_COLLIDES: i64 = 0x3000_0000_0000_0000;

/// Whether `value` is a flonum, by its tag, as 0 or 1.
fn is_flonum(b: &mut FunctionBuilder, value: ir::Value) -> ir::Value {
    let tag = b.ins().band_imm(value, 0b11);
    let is = b.ins().icmp_imm(IntCC::Equal, tag, 0b10);
    b.ins().uextend(WORD, is)
}

/// The double a flonum holds, as `Value::as_flonum` restores it.
fn decode(b: &mut FunctionBuilder, value: ir::Value) -> ir::Value {
    // Bit 63 is the surviving third exponent bit, and the two the tag
    // overwrote follow from it: `011` when it is set, `100` when it is not.
    let top = b.ins().ushr_imm(value, 63);
    let two = b.ins().iconst(WORD, 2);
    let low = b.ins().isub(two, top);
    let rest = b.ins().band_imm(value, !0b11);
    let restored = b.ins().bor(low, rest);
    let rotated = b.ins().rotr_imm(restored, 3);
    let zero = b.ins().icmp_imm(IntCC::Equal, value, FLONUM_ZERO);
    let nothing = b.ins().iconst(WORD, 0);
    let word = b.ins().select(zero, nothing, rotated);
    b.ins().bitcast(types::F64, MemFlags::new(), word)
}

/// `f` as a flonum, as `Value::flonum` encodes it, or a bail-out for a
/// double that needs a heap `Float`: NaN, an infinity, `-0.0`, the extremes.
fn encode(b: &mut FunctionBuilder, f: ir::Value, bail: ir::Block) -> ir::Value {
    let word = b.ins().bitcast(WORD, MemFlags::new(), f);
    let exponent = b.ins().ushr_imm(word, 60);
    let exponent = b.ins().band_imm(exponent, 0b111);
    // `011` or `100`: the exponent less three is zero or one.
    let shifted = b.ins().iadd_imm(exponent, -3);
    let banded = b.ins().icmp_imm(IntCC::UnsignedLessThanOrEqual, shifted, 1);
    let collides = b.ins().icmp_imm(IntCC::Equal, word, FLONUM_COLLIDES);
    let spare = b.ins().icmp_imm(IntCC::Equal, collides, 0);
    let fits = b.ins().band(banded, spare);
    let zero = b.ins().icmp_imm(IntCC::Equal, word, 0);
    let ok = b.ins().bor(fits, zero);
    guard(b, ok, bail);
    let rotated = b.ins().rotl_imm(word, 3);
    let cleared = b.ins().band_imm(rotated, !0b11);
    let tagged = b.ins().bor_imm(cleared, 0b10);
    let spare_zero = b.ins().iconst(WORD, FLONUM_ZERO);
    b.ins().select(zero, spare_zero, tagged)
}
