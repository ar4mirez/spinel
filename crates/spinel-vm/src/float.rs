//! Floats that do not fit in a word.
//!
//! A flonum is an `f64` rotated into an immediate, and the rotation only has
//! room for two exponent bands: roughly 1e-77 to 1e77, and zero. Everything
//! else — the infinities, NaN, `-0.0`, `Float::MAX`, `1e100` — is a *boxed*
//! float: eight bytes in a heap cell whose class is `Float`. It is what
//! [`crate::bignum`] is to a fixnum, and for the same reason: the arithmetic
//! must not have a hole in the middle of the type.
//!
//! [`value`] picks the representation and [`read`] reads either, so nothing
//! outside this file has to know which one it holds. Two boxed floats are
//! different objects even when they are the same number, which is Ruby's
//! answer too: `1e300.equal?(1e300)` is false there.

use crate::class::Builtin;
use crate::heap::{HandleScope, Payload};
use crate::value::Value;

/// `f` as a Ruby `Float`: an immediate when it fits, a heap cell when not.
pub fn value(scope: &mut HandleScope<'_>, f: f64) -> Value {
    if let Some(immediate) = Value::flonum(f) {
        return immediate;
    }
    let class = scope.classes().object(Builtin::Float.id());
    let class = scope.root(class);
    let handle = scope.alloc(Some(class), Payload::Bytes, 8);
    scope.bytes_mut(handle).copy_from_slice(&f.to_le_bytes());
    // A number has nothing to mutate, boxed or not.
    scope.freeze(handle);
    scope.get(handle)
}

/// The `f64` behind a `Float`, immediate or boxed.
///
/// `None` for anything that is not a `Float`, so a caller can use it as the
/// "is this a float at all" test and get the number in the same step.
pub fn read(scope: &mut HandleScope<'_>, v: Value) -> Option<f64> {
    if let Some(f) = v.as_flonum() {
        return Some(f);
    }
    if v.is_immediate() {
        return None;
    }
    let handle = scope.root(v);
    if scope.payload(handle) != Payload::Bytes {
        return None;
    }
    // The payload check alone is not enough: a `String` buffer and a bignum
    // are bytes too. Only a cell whose class *is* `Float` holds a double.
    let class = scope.class_of(handle)?;
    if scope.classes().repr(class) != Some(Builtin::Float) {
        return None;
    }
    let bytes: [u8; 8] = scope.bytes(handle).try_into().ok()?;
    Some(f64::from_le_bytes(bytes))
}
