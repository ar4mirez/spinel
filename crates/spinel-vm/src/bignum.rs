//! `Integer` past the fixnum range.
//!
//! Ruby has one `Integer` with no visible boundary: `(2**70).class` and
//! `1.class` are the same class, and `((2**70) - (2**70)).class` is that class
//! too even though the result fits in a machine word again. Measured. So the
//! promotion has to be invisible in both directions — every operation that can
//! leave the fixnum range allocates, and every result that fits comes back as
//! an immediate. [`value`] is the one place that decision is made, and nothing
//! outside this module should build an `Integer` heap object without it.
//!
//! The magnitude lives in a `Payload::Bytes` cell, which the header has
//! documented as "a bignum's limbs" since #7, as two's-complement
//! little-endian bytes. `num-bigint` owns the arithmetic: `docs/engine.md`
//! settled that — "Bignum arithmetic is a primitive over a pure-Rust bigint
//! crate" — and hand-rolling division to save a dependency would be trading a
//! measured implementation for an unmeasured one.
//!
//! ponytail: every read deserialises and every write serialises, so a loop over
//! bignums pays two allocations per operation. The upgrade is to keep the limbs
//! in the cell in `num-bigint`'s own layout and view them in place; it is worth
//! writing when a benchmark has bignum arithmetic in it, and until then this
//! costs nothing a program that stays inside fixnums can see.

use num_bigint::BigInt;

use crate::class::Builtin;
use crate::heap::{HandleScope, Payload};
use crate::value::Value;

/// `n` as a Ruby `Integer`: an immediate when it fits, a heap cell when not.
///
/// The normalisation is not an optimisation. `2**70 - 2**70` has to be the same
/// value `0` is, or `==`, the method cache and every `case` on a small integer
/// would see two different things where Ruby has one.
pub fn value(scope: &mut HandleScope<'_>, n: &BigInt) -> Value {
    if let Some(small) = i64::try_from(n).ok().and_then(Value::fixnum) {
        return small;
    }
    let bytes = n.to_signed_bytes_le();
    let class = scope.classes().object(Builtin::Integer.id());
    let class = scope.root(class);
    let handle = scope.alloc(Some(class), Payload::Bytes, bytes.len() as u32);
    scope.bytes_mut(handle).copy_from_slice(&bytes);
    scope.get(handle)
}

/// The `BigInt` behind an `Integer`, immediate or heap.
///
/// `None` for anything that is not an `Integer`, so a caller can use it as the
/// "is this an integer at all" test and get the value in the same step.
pub fn read(scope: &mut HandleScope<'_>, v: Value) -> Option<BigInt> {
    if let Some(n) = v.as_fixnum() {
        return Some(BigInt::from(n));
    }
    if v.is_immediate() {
        return None;
    }
    let handle = scope.root(v);
    if scope.payload(handle) != Payload::Bytes {
        return None;
    }
    // The payload check alone is not enough: a `String` is bytes too. Only a
    // cell whose class *is* `Integer` holds limbs.
    let class = scope.class_of(handle)?;
    if scope.classes().repr(class) != Some(Builtin::Integer) {
        return None;
    }
    Some(BigInt::from_signed_bytes_le(scope.bytes(handle)))
}

/// Whether `v` is an `Integer` too wide for a fixnum.
///
/// Cheaper than [`read`] when the answer is all that is wanted, and it is the
/// test the fast paths use before deciding to deserialise.
pub fn is_big(scope: &mut HandleScope<'_>, v: Value) -> bool {
    if v.is_immediate() {
        return false;
    }
    let handle = scope.root(v);
    if scope.payload(handle) != Payload::Bytes {
        return false;
    }
    scope
        .class_of(handle)
        .and_then(|c| scope.classes().repr(c))
        .is_some_and(|r| r == Builtin::Integer)
}
