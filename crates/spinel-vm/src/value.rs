//! `Value`: the 64-bit tagged word every Ruby object travels as.
//!
//! Fixnums, most floats, symbols, and `nil`/`true`/`false`/`undef` are *immediates*:
//! the word is the object, and nothing is allocated. Everything else is a pointer to
//! a heap object, which arrives with [`Heap`] in phase 1's next slice.
//!
//! | low bits | the rest of the word          | kind                         |
//! |----------|-------------------------------|------------------------------|
//! | `1`      | 63-bit signed integer         | fixnum                       |
//! | `10`     | double, rotated left by three | flonum                       |
//! | `0100`   | ordinal                       | `nil`/`false`/`true`/`undef` |
//! | `1100`   | symbol id                     | static symbol                |
//! | `000`    | 8-byte-aligned pointer        | heap object                  |
//!
//! The zero word is deliberately not a `Value`, so `Option<Value>` is still one word
//! and a zeroed slot is a detectable bug rather than a plausible object.
//!
//! Bitwise equality is Ruby's `equal?`: two `Value`s are equal exactly when they are
//! the same object. That holds for flonums too, because the encodable range excludes
//! NaN, the infinities, and `-0.0` — the three cases where bit equality and `==`
//! disagree.
//!
//! [`Heap`]: https://github.com/ar4mirez/spinel/issues/7

use std::fmt;
use std::num::NonZeroU64;
use std::ptr::{self, NonNull};

#[cfg(not(target_pointer_width = "64"))]
compile_error!("Value is a 64-bit tagged word; Spinel does not support 32-bit targets");

// The definition of done for #6, checked by the compiler rather than by a reviewer.
const _: () = {
    assert!(size_of::<Value>() == size_of::<*const ()>());
    assert!(size_of::<Option<Value>>() == size_of::<Value>());
};

/// A Ruby object: either an immediate or a pointer to a heap object.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Value(NonZeroU64);

/// The index of an interned symbol.
///
// ponytail: the table that maps these to names is shared, append-only state, so it
// lands with `src/shared/` in #8. Until then a `SymbolId` is a number that survives a
// round trip through a `Value`, which is all the encoding has to promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct SymbolId(pub u32);

/// A `HashMap` keyed by [`SymbolId`], hashed with [`SymbolHasher`] rather than
/// SipHash: method tables, method caches, constant and class-variable tables,
/// globals and shape transitions.
pub type SymbolMap<V> = std::collections::HashMap<SymbolId, V, BuildSymbolHasher>;

/// A `HashSet` of [`SymbolId`]s, hashed with [`SymbolHasher`].
pub type SymbolSet = std::collections::HashSet<SymbolId, BuildSymbolHasher>;

/// Builds a [`SymbolHasher`]. It carries no keys, so every map built with it
/// hashes the same id the same way.
pub type BuildSymbolHasher = std::hash::BuildHasherDefault<SymbolHasher>;

/// The hasher for [`SymbolId`] keys: one multiply and a rotate, the FxHash
/// step `rustc` hashes its own interned symbols with (#652).
///
/// SipHash is what `HashMap` uses by default because its keys are random per
/// process, which is what keeps someone who chooses the keys from choosing a
/// set that lands in one bucket. That is the defence for keys that come from
/// input, such as a Ruby `Hash`'s or the symbol table's own index of names,
/// and neither uses this hasher. A `SymbolId` is not such a key: it is the
/// index the symbol table hands out, in order, starting at zero (see
/// [`crate::shared::symbols`]). A program, or the input it reads, decides
/// which names get interned and in what order, never the number a name gets,
/// so the keys of a table here are a subset of a dense run of integers. The
/// multiply spreads that run over the top bits, which `HashMap` filters on,
/// and the rotate brings bits that depend on every bit of the id down to the
/// bottom ones it picks a bucket with. Choosing ids that share a bucket still
/// costs about one interned name per bucket of the table for each colliding
/// key, so a collision costs whoever causes it as much as it costs the table:
/// there is nothing to amplify, and no key to keep secret.
///
/// What it buys is the cost of a hash. Bootstrap does thousands of inserts
/// into these tables and every method call that misses its inline cache does
/// a lookup; natively SipHash's rounds are a small part of that, but Miri
/// interprets every one of them, which made them much of a heap's bootstrap
/// there (#652).
#[derive(Debug, Default, Clone, Copy)]
pub struct SymbolHasher(u64);

impl SymbolHasher {
    /// FxHash's multiplier: odd, so the multiply is a bijection on `u64`.
    const K: u64 = 0xf135_7aea_2e62_a9c5;

    fn add(&mut self, word: u64) {
        self.0 = self.0.wrapping_add(word).wrapping_mul(Self::K);
    }
}

impl std::hash::Hasher for SymbolHasher {
    fn write_u32(&mut self, id: u32) {
        self.add(u64::from(id));
    }

    // `SymbolId` hashes as its `u32`, so nothing else reaches this; it is here
    // so the hasher is correct, if slower, for any key.
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.add(u64::from(byte));
        }
    }

    fn finish(&self) -> u64 {
        self.0.rotate_left(26)
    }
}

/// A [`Value`] with its tag read, for exhaustive matching.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Unpacked {
    Fixnum(i64),
    Flonum(f64),
    Symbol(SymbolId),
    Nil,
    False,
    True,
    /// The interpreter's "no value here" marker. Never reaches Ruby code.
    Undef,
    Heap(NonNull<()>),
}

const FIXNUM_MASK: u64 = 0b1;
const FIXNUM_TAG: u64 = 0b1;

const FLONUM_MASK: u64 = 0b11;
const FLONUM_TAG: u64 = 0b10;

/// `nil`, `false`, `true`, `undef` and static symbols share the low three bits; bit 3
/// separates the constants from the symbols.
const SPECIAL_MASK: u64 = 0b1111;
const CONST_TAG: u64 = 0b0100;
const SYMBOL_TAG: u64 = 0b1100;
const SYMBOL_SHIFT: u32 = 8;

const HEAP_MASK: u64 = 0b111;
const HEAP_TAG: u64 = 0b000;

/// Doubles are encodable when their top three exponent bits are `011` or `100`, which
/// is a magnitude of roughly 1.7e-77 to 1.8e77 — every float a normal program holds.
const FLONUM_EXP_LO: u64 = 0b011;
const FLONUM_EXP_HI: u64 = 0b100;

/// `+0.0` has no exponent in range, so it gets the one spare pattern.
const FLONUM_ZERO: u64 = 0x8000_0000_0000_0002;

/// `2f64.powi(-255)`, the one in-range double whose rotation *is* [`FLONUM_ZERO`].
/// It goes to the heap so that the spare pattern stays spare.
const FLONUM_COLLIDES: u64 = 0x3000_0000_0000_0000;

impl Value {
    pub const NIL: Value = Value::from_bits(0x04);
    pub const FALSE: Value = Value::from_bits(0x14);
    pub const TRUE: Value = Value::from_bits(0x24);
    pub const UNDEF: Value = Value::from_bits(0x34);

    /// The widest integer an immediate holds. Past this, `Integer` is a heap bignum.
    pub const FIXNUM_MAX: i64 = i64::MAX >> 1;
    /// The narrowest integer an immediate holds. Past this, `Integer` is a heap bignum.
    pub const FIXNUM_MIN: i64 = i64::MIN >> 1;

    /// `n` as an immediate, or `None` when it needs a bignum.
    #[inline]
    pub const fn fixnum(n: i64) -> Option<Value> {
        if n < Value::FIXNUM_MIN || n > Value::FIXNUM_MAX {
            return None;
        }
        Some(Value::from_bits(((n << 1) as u64) | FIXNUM_TAG))
    }

    /// `d` as an immediate, or `None` when it needs a heap `Float`.
    #[inline]
    pub const fn flonum(d: f64) -> Option<Value> {
        let bits = d.to_bits();
        let exp = (bits >> 60) & 0b111;
        if bits != FLONUM_COLLIDES && (exp == FLONUM_EXP_LO || exp == FLONUM_EXP_HI) {
            // Rotating by three lifts the sign and the top two exponent bits into the
            // low bits, where the tag overwrites the two the decoder can reconstruct.
            Some(Value::from_bits(
                (bits.rotate_left(3) & !FLONUM_MASK) | FLONUM_TAG,
            ))
        } else if bits == 0 {
            Some(Value::from_bits(FLONUM_ZERO))
        } else {
            None
        }
    }

    /// An interned symbol as an immediate. Always succeeds.
    #[inline]
    pub const fn symbol(id: SymbolId) -> Value {
        Value::from_bits(((id.0 as u64) << SYMBOL_SHIFT) | SYMBOL_TAG)
    }

    /// A pointer to a heap object.
    ///
    /// # Panics
    ///
    /// If `ptr` is not 8-byte aligned; the low three bits are the tag. This is an
    /// assertion and not a `debug_assert`, because an unaligned pointer here is not a
    /// wrong answer, it is a `Value` that reads back as some other kind of object.
    #[inline]
    pub fn heap(ptr: NonNull<()>) -> Value {
        let addr = ptr.as_ptr().expose_provenance() as u64;
        assert!(
            addr & HEAP_MASK == HEAP_TAG,
            "heap object at {ptr:p} is not 8-byte aligned"
        );
        Value::from_bits(addr)
    }

    #[inline]
    pub const fn as_fixnum(self) -> Option<i64> {
        if self.bits() & FIXNUM_MASK == FIXNUM_TAG {
            Some((self.bits() as i64) >> 1)
        } else {
            None
        }
    }

    #[inline]
    pub const fn as_flonum(self) -> Option<f64> {
        let v = self.bits();
        if v & FLONUM_MASK != FLONUM_TAG {
            return None;
        }
        if v == FLONUM_ZERO {
            return Some(0.0);
        }
        // Bit 63 is the surviving third exponent bit, and the two the tag overwrote
        // follow from it: `011` when it is set, `100` when it is not.
        let restored = (2 - (v >> 63)) | (v & !FLONUM_MASK);
        Some(f64::from_bits(restored.rotate_right(3)))
    }

    #[inline]
    pub const fn as_symbol(self) -> Option<SymbolId> {
        if self.bits() & SPECIAL_MASK == SYMBOL_TAG {
            Some(SymbolId((self.bits() >> SYMBOL_SHIFT) as u32))
        } else {
            None
        }
    }

    #[inline]
    pub fn as_heap(self) -> Option<NonNull<()>> {
        if self.bits() & HEAP_MASK == HEAP_TAG {
            NonNull::new(ptr::with_exposed_provenance_mut(self.bits() as usize))
        } else {
            None
        }
    }

    /// True for everything except `nil` and `false`, which is Ruby's only truth rule.
    #[inline]
    pub const fn is_truthy(self) -> bool {
        // The two falsy constants differ in bit 4 alone, so one OR settles it.
        (self.bits() | 0x10) != Value::FALSE.bits()
    }

    #[inline]
    pub const fn is_nil(self) -> bool {
        self.bits() == Value::NIL.bits()
    }

    /// True when the word is the object, so no heap read can follow.
    #[inline]
    pub const fn is_immediate(self) -> bool {
        self.bits() & HEAP_MASK != HEAP_TAG
    }

    /// The tag, read once, for an exhaustive `match`.
    #[inline]
    pub fn unpack(self) -> Unpacked {
        let v = self.bits();
        if v & FIXNUM_MASK == FIXNUM_TAG {
            return Unpacked::Fixnum((v as i64) >> 1);
        }
        if v & FLONUM_MASK == FLONUM_TAG {
            // `as_flonum` re-tests the tag; the optimiser drops that, and duplicating
            // the rotation here would be one more place to get it wrong.
            return match self.as_flonum() {
                Some(d) => Unpacked::Flonum(d),
                None => unreachable!(),
            };
        }
        if v & SPECIAL_MASK == SYMBOL_TAG {
            return Unpacked::Symbol(SymbolId((v >> SYMBOL_SHIFT) as u32));
        }
        if v & SPECIAL_MASK == CONST_TAG {
            return match v {
                _ if v == Value::NIL.bits() => Unpacked::Nil,
                _ if v == Value::FALSE.bits() => Unpacked::False,
                _ if v == Value::TRUE.bits() => Unpacked::True,
                _ if v == Value::UNDEF.bits() => Unpacked::Undef,
                // Unreachable: every constructor is above, and `heap` rejects the
                // misaligned pointers that could otherwise land in this tag.
                _ => unreachable!(),
            };
        }
        match NonNull::new(ptr::with_exposed_provenance_mut(v as usize)) {
            Some(p) => Unpacked::Heap(p),
            None => unreachable!(),
        }
    }

    #[inline]
    const fn bits(self) -> u64 {
        self.0.get()
    }

    /// The raw tagged word, for the few callers outside this module that need a
    /// value's identity as a number rather than as a `Value`.
    ///
    /// `Object#object_id` is the one today. Not a hash and not an address: two
    /// `Value`s are the same object exactly when these are equal, which is the
    /// property `object_id` needs and the only one this promises.
    #[must_use]
    #[inline]
    pub const fn to_bits(self) -> u64 {
        self.bits()
    }

    /// `self + other` when both are fixnums and so is the sum, computed on
    /// the tagged words: `2a+1 + 2b+1 - 1` is `2(a+b)+1`, and it overflows an
    /// `i64` exactly when `a+b` is past the fixnum range. `None` otherwise.
    #[inline]
    pub const fn fixnum_add(self, other: Value) -> Option<Value> {
        let (a, b) = (self.bits() as i64, other.bits() as i64);
        if (a & b) as u64 & FIXNUM_MASK != FIXNUM_TAG {
            return None;
        }
        // `b` is odd, so `b - 1` cannot overflow, and the sum is odd.
        match a.checked_add(b - 1) {
            // The tag is already set; or-ing it in says so to the compiler,
            // which then knows the word is not zero.
            Some(sum) => Some(Value::from_bits(sum as u64 | FIXNUM_TAG)),
            None => None,
        }
    }

    /// `self - other`, as [`Value::fixnum_add`] is `+`: `2a+1 - 2b` is
    /// `2(a-b)+1`.
    #[inline]
    pub const fn fixnum_sub(self, other: Value) -> Option<Value> {
        let (a, b) = (self.bits() as i64, other.bits() as i64);
        if (a & b) as u64 & FIXNUM_MASK != FIXNUM_TAG {
            return None;
        }
        match a.checked_sub(b - 1) {
            Some(difference) => Some(Value::from_bits(difference as u64 | FIXNUM_TAG)),
            None => None,
        }
    }

    /// Two fixnums' tagged words, which order as the numbers do, since
    /// `2a+1 < 2b+1` exactly when `a < b`; `None` unless both are fixnums.
    #[inline]
    pub const fn fixnum_order(self, other: Value) -> Option<(i64, i64)> {
        let (a, b) = (self.bits() as i64, other.bits() as i64);
        if (a & b) as u64 & FIXNUM_MASK != FIXNUM_TAG {
            return None;
        }
        Some((a, b))
    }

    #[inline]
    const fn from_bits(bits: u64) -> Value {
        match NonZeroU64::new(bits) {
            Some(b) => Value(b),
            None => panic!("the zero word is not a Value"),
        }
    }
}

/// Reads like the Ruby object, because every future slice's failing assertion prints it.
impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.unpack() {
            Unpacked::Fixnum(n) => write!(f, "{n}"),
            Unpacked::Flonum(d) => write!(f, "{d:?}"),
            Unpacked::Symbol(SymbolId(id)) => write!(f, "Symbol({id})"),
            Unpacked::Nil => f.write_str("nil"),
            Unpacked::False => f.write_str("false"),
            Unpacked::True => f.write_str("true"),
            Unpacked::Undef => f.write_str("undef"),
            Unpacked::Heap(p) => write!(f, "heap({p:p})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind of `Value`, for the tests that must cover all of them.
    ///
    /// `anchor` is the caller's, and eight-byte aligned because it is a `u64`. It used
    /// to be a `Box::leak`, which made `cargo miri test` report a leak — and a leak
    /// check that starts red is one nobody reads. #7 added that check to CI.
    fn one_of_each(anchor: &u64) -> Vec<Value> {
        vec![
            Value::fixnum(0).unwrap(),
            Value::fixnum(-1).unwrap(),
            Value::fixnum(Value::FIXNUM_MAX).unwrap(),
            Value::fixnum(Value::FIXNUM_MIN).unwrap(),
            Value::flonum(1.5).unwrap(),
            Value::flonum(0.0).unwrap(),
            Value::symbol(SymbolId(0)),
            Value::symbol(SymbolId(u32::MAX)),
            Value::NIL,
            Value::FALSE,
            Value::TRUE,
            Value::UNDEF,
            Value::heap(NonNull::from(anchor).cast()),
        ]
    }

    #[test]
    fn value_is_pointer_sized_and_leaves_a_niche() {
        assert_eq!(size_of::<Value>(), size_of::<*const ()>());
        assert_eq!(size_of::<Option<Value>>(), size_of::<Value>());
    }

    #[test]
    fn fixnums_round_trip_across_the_range() {
        for n in [0, 1, -1, 2, -2, 42, -42, i32::MAX as i64, i32::MIN as i64] {
            assert_eq!(Value::fixnum(n).unwrap().as_fixnum(), Some(n), "{n}");
        }
    }

    #[test]
    fn tagged_fixnum_arithmetic_agrees_with_the_numbers() {
        let edges = [
            0,
            1,
            -1,
            7,
            -7,
            Value::FIXNUM_MAX,
            Value::FIXNUM_MAX - 1,
            Value::FIXNUM_MIN,
            Value::FIXNUM_MIN + 1,
        ];
        for a in edges {
            for b in edges {
                let (x, y) = (Value::fixnum(a).unwrap(), Value::fixnum(b).unwrap());
                let sum = a.checked_add(b).and_then(Value::fixnum);
                let difference = a.checked_sub(b).and_then(Value::fixnum);
                assert_eq!(x.fixnum_add(y), sum, "{a} + {b}");
                assert_eq!(x.fixnum_sub(y), difference, "{a} - {b}");
                let (p, q) = x.fixnum_order(y).unwrap();
                assert_eq!(p.cmp(&q), a.cmp(&b), "{a} <=> {b}");
            }
        }
        let one = Value::fixnum(1).unwrap();
        assert_eq!(one.fixnum_add(Value::NIL), None);
        assert_eq!(Value::NIL.fixnum_sub(one), None);
        assert_eq!(one.fixnum_order(Value::TRUE), None);
    }

    #[test]
    fn fixnum_boundaries_hold_and_overflow_becomes_a_bignum() {
        assert_eq!(Value::FIXNUM_MAX, (1i64 << 62) - 1);
        assert_eq!(Value::FIXNUM_MIN, -(1i64 << 62));

        for n in [
            Value::FIXNUM_MAX,
            Value::FIXNUM_MAX - 1,
            Value::FIXNUM_MIN,
            Value::FIXNUM_MIN + 1,
        ] {
            assert_eq!(Value::fixnum(n).unwrap().as_fixnum(), Some(n), "{n}");
        }
        // One past either end is where `Integer` promotes to a heap bignum.
        for n in [
            Value::FIXNUM_MAX + 1,
            Value::FIXNUM_MIN - 1,
            i64::MAX,
            i64::MIN,
        ] {
            assert_eq!(Value::fixnum(n), None, "{n} should need a bignum");
        }
    }

    /// Sweeps every exponent, both signs, and four mantissas: 16,384 doubles, which
    /// pins the encodable band's edges exactly rather than sampling near them.
    #[test]
    fn flonums_round_trip_inside_the_band_and_are_refused_outside_it() {
        for sign in [0u64, 1] {
            for exp in 0u64..2048 {
                for mantissa in [0u64, 1, 0x8_0000_0000_0000, 0xF_FFFF_FFFF_FFFF] {
                    let bits = (sign << 63) | (exp << 52) | mantissa;
                    let d = f64::from_bits(bits);
                    let in_band = matches!(exp >> 8, 3 | 4) && bits != FLONUM_COLLIDES;

                    match Value::flonum(d) {
                        Some(v) => {
                            assert!(in_band || bits == 0, "{bits:#018x} should not encode");
                            let back = v.as_flonum().expect("flonum reads back as a flonum");
                            assert_eq!(back.to_bits(), bits, "{bits:#018x} did not round trip");
                        }
                        None => assert!(!in_band && bits != 0, "{bits:#018x} should encode"),
                    }
                }
            }
        }
    }

    #[test]
    fn flonum_refuses_the_values_that_would_break_bit_equality() {
        // NaN, the infinities and -0.0 are the three cases where bit equality and
        // `==` disagree. All three sit outside the band, so `Value`'s derived `Eq`
        // stays exactly Ruby's `equal?`.
        for d in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -0.0,
            1e300,
            1e-300,
        ] {
            assert_eq!(Value::flonum(d), None, "{d:?} should need a heap Float");
        }
        assert_eq!(Value::flonum(f64::MAX), None);
        assert_eq!(Value::flonum(f64::MIN_POSITIVE), None);
    }

    #[test]
    fn positive_zero_uses_the_spare_pattern_and_its_collision_goes_to_the_heap() {
        let zero = Value::flonum(0.0).unwrap();
        assert_eq!(zero.as_flonum(), Some(0.0));
        assert_eq!(zero.as_flonum().unwrap().to_bits(), 0);

        // The one in-band double whose rotation is the spare pattern. Encoding it
        // would make `0.0` and `2**-255` the same object.
        let collides = f64::from_bits(FLONUM_COLLIDES);
        assert_eq!(Value::flonum(collides), None);
        assert_ne!(collides, 0.0);
    }

    #[test]
    fn symbols_round_trip() {
        for id in [0, 1, 255, 256, u32::MAX / 2, u32::MAX] {
            let v = Value::symbol(SymbolId(id));
            assert_eq!(v.as_symbol(), Some(SymbolId(id)), "{id}");
            assert_eq!(v.unpack(), Unpacked::Symbol(SymbolId(id)));
        }
    }

    #[test]
    fn the_special_constants_are_four_distinct_objects() {
        let all = [Value::NIL, Value::FALSE, Value::TRUE, Value::UNDEF];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                assert_eq!(i == j, a == b, "{a:?} vs {b:?}");
            }
        }
        assert_eq!(Value::NIL.unpack(), Unpacked::Nil);
        assert_eq!(Value::FALSE.unpack(), Unpacked::False);
        assert_eq!(Value::TRUE.unpack(), Unpacked::True);
        assert_eq!(Value::UNDEF.unpack(), Unpacked::Undef);
    }

    #[test]
    fn only_nil_and_false_are_falsy() {
        let anchor = 0u64;
        for v in one_of_each(&anchor) {
            let expected = v != Value::NIL && v != Value::FALSE;
            assert_eq!(v.is_truthy(), expected, "{v:?}");
        }
        // Zero and the empty-ish values are true in Ruby.
        assert!(Value::fixnum(0).unwrap().is_truthy());
        assert!(Value::flonum(0.0).unwrap().is_truthy());
        assert!(Value::NIL.is_nil());
        assert!(!Value::FALSE.is_nil());
    }

    #[test]
    fn every_value_has_exactly_one_tag() {
        let anchor = 0u64;
        for v in one_of_each(&anchor) {
            let claims = [
                v.as_fixnum().is_some(),
                v.as_flonum().is_some(),
                v.as_symbol().is_some(),
                matches!(
                    v.unpack(),
                    Unpacked::Nil | Unpacked::False | Unpacked::True | Unpacked::Undef
                ),
                v.as_heap().is_some(),
            ];
            assert_eq!(
                claims.iter().filter(|c| **c).count(),
                1,
                "{v:?} claims {claims:?}"
            );
            assert_eq!(v.is_immediate(), v.as_heap().is_none(), "{v:?}");
        }
    }

    #[test]
    fn heap_pointers_round_trip() {
        let boxed = Box::new(0u64);
        let ptr = NonNull::from(&*boxed).cast::<()>();
        let v = Value::heap(ptr);
        assert_eq!(v.as_heap(), Some(ptr));
        assert_eq!(v.unpack(), Unpacked::Heap(ptr));
        assert!(!v.is_immediate());
    }

    #[test]
    #[should_panic(expected = "not 8-byte aligned")]
    fn an_unaligned_heap_pointer_is_caught_rather_than_read_back_as_a_symbol() {
        // 0x4 has the tag of `nil`; silently accepting it would turn an object into
        // a special constant. In release builds too, hence `assert!`.
        Value::heap(NonNull::new(0x4 as *mut ()).unwrap());
    }

    #[test]
    fn debug_reads_like_the_ruby_object() {
        assert_eq!(format!("{:?}", Value::fixnum(-7).unwrap()), "-7");
        assert_eq!(format!("{:?}", Value::flonum(1.5).unwrap()), "1.5");
        assert_eq!(format!("{:?}", Value::NIL), "nil");
        assert_eq!(format!("{:?}", Value::TRUE), "true");
        assert_eq!(format!("{:?}", Value::symbol(SymbolId(3))), "Symbol(3)");
    }

    /// The property [`SymbolHasher`]'s comment leans on: a dense run of ids,
    /// which is what a table of them holds, spreads over both the bucket bits
    /// and the seven top bits `HashMap` filters a probe with.
    #[test]
    fn symbol_hasher_spreads_a_dense_run_of_ids() {
        use std::hash::BuildHasher as _;
        const IDS: u32 = 1024;
        let build = BuildSymbolHasher::default();
        let hashes: Vec<u64> = (0..IDS).map(|id| build.hash_one(SymbolId(id))).collect();

        let mut buckets = vec![0_u32; IDS as usize];
        let mut tags = [false; 128];
        for &hash in &hashes {
            buckets[(hash % u64::from(IDS)) as usize] += 1;
            tags[(hash >> 57) as usize] = true;
        }
        let fullest = buckets.iter().copied().max().unwrap();
        assert!(fullest <= 4, "{fullest} ids share one of {IDS} buckets");
        let used = tags.iter().filter(|&&used| used).count();
        assert!(used >= 64, "only {used} of 128 tags used");

        let mut map: SymbolMap<u32> = SymbolMap::default();
        for id in 0..IDS {
            map.insert(SymbolId(id), id);
        }
        assert!((0..IDS).all(|id| map[&SymbolId(id)] == id));
    }
}
