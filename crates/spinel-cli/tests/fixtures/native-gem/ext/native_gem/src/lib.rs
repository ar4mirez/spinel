//! The native half of the example gem (#104), which `require
//! "native_gem/native_gem"` loads: a module with methods, a class under it
//! with an instance method, an exception class, values converted both ways,
//! and each way an extension raises.

use spinel_ext::{Context, Error, Result, Value};

spinel_ext::extension!(init);

fn init(cx: &mut Context<'_>) -> Result<()> {
    let native_gem = cx.define_module("NativeGem")?;
    cx.define_singleton_method(native_gem, "reverse", 1, reverse)?;
    cx.define_singleton_method(native_gem, "sum", -1, sum)?;
    cx.define_singleton_method(native_gem, "palindrome?", 1, is_palindrome)?;
    cx.define_singleton_method(native_gem, "positive!", 1, positive)?;
    let standard_error = cx.const_get("StandardError")?;
    cx.define_class_under(native_gem, "Error", Some(standard_error))?;
    let words = cx.define_class_under(native_gem, "Words", None)?;
    cx.define_method(words, "count", 1, count)?;
    Ok(())
}

/// `NativeGem.reverse(text)`: `text`'s characters in reverse order.
fn reverse<'cx>(
    cx: &mut Context<'cx>,
    _self: Value<'cx>,
    args: &[Value<'cx>],
) -> Result<Value<'cx>> {
    let text = cx.to_string(args[0])?;
    Ok(cx.string(&text.chars().rev().collect::<String>()))
}

/// `NativeGem.sum(*integers)`: their sum, a `TypeError` for anything but an
/// Integer, and a `RangeError` past 64 bits.
fn sum<'cx>(cx: &mut Context<'cx>, _self: Value<'cx>, args: &[Value<'cx>]) -> Result<Value<'cx>> {
    let mut total: i64 = 0;
    for &arg in args {
        let n = cx.to_i64(arg)?;
        total = total
            .checked_add(n)
            .ok_or_else(|| Error::new("RangeError", "sum overflows a 64-bit integer"))?;
    }
    Ok(cx.integer(total))
}

/// `NativeGem.palindrome?(text)`: whether `text` reads the same backwards.
fn is_palindrome<'cx>(
    cx: &mut Context<'cx>,
    _self: Value<'cx>,
    args: &[Value<'cx>],
) -> Result<Value<'cx>> {
    let text = cx.to_string(args[0])?;
    let same = text.chars().eq(text.chars().rev());
    Ok(cx.boolean(same))
}

/// `NativeGem.positive!(n)`: `n` when it is above zero, else a
/// `NativeGem::Error`, the extension's own exception class.
fn positive<'cx>(
    cx: &mut Context<'cx>,
    _self: Value<'cx>,
    args: &[Value<'cx>],
) -> Result<Value<'cx>> {
    let n = cx.to_i64(args[0])?;
    if n > 0 {
        return Ok(args[0]);
    }
    let error = cx.const_get("NativeGem::Error")?;
    Err(cx.raise(error, &format!("{n} is not positive")))
}

/// `NativeGem::Words#count(text)`: how many whitespace-separated words
/// `text` has, or nil for none.
fn count<'cx>(
    cx: &mut Context<'cx>,
    _self: Value<'cx>,
    args: &[Value<'cx>],
) -> Result<Value<'cx>> {
    let text = cx.to_string(args[0])?;
    match text.split_whitespace().count() {
        0 => Ok(cx.nil()),
        n => Ok(cx.integer(i64::try_from(n).unwrap_or(i64::MAX))),
    }
}
