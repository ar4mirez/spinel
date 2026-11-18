//! The worked example of `spinel-ext`'s crate documentation and
//! docs/engine.md, and below it the methods `tests/ext.rs` needs to reach
//! the rest of the API. `tests/ext.rs` builds it as a `cdylib` and loads it,
//! and also compiles it into the test itself.

use spinel_ext::{Context, Error, Kind, Result, Value};

spinel_ext::extension!(init);

fn init(cx: &mut Context<'_>) -> Result<()> {
    let greeter = cx.define_module("Greeter")?;
    cx.define_singleton_method(greeter, "hello", 1, hello)?;
    cx.define_singleton_method(greeter, "add", 2, add)?;
    let counter = cx.define_class_under(greeter, "Counter", None)?;
    cx.define_method(counter, "zero?", 0, is_zero)?;
    tested(cx, greeter)
}

/// `Greeter.hello(name)`: `"Hello, <name>!"`.
fn hello<'cx>(cx: &mut Context<'cx>, _self: Value<'cx>, args: &[Value<'cx>]) -> Result<Value<'cx>> {
    let name = cx.to_string(args[0])?;
    if name.is_empty() {
        return Err(Error::new("ArgumentError", "name is empty"));
    }
    Ok(cx.string(&format!("Hello, {name}!")))
}

/// `Greeter.add(a, b)`: an `Integer`, or a `RangeError` past `i64`.
fn add<'cx>(cx: &mut Context<'cx>, _self: Value<'cx>, args: &[Value<'cx>]) -> Result<Value<'cx>> {
    let (a, b) = (cx.to_i64(args[0])?, cx.to_i64(args[1])?);
    match a.checked_add(b) {
        Some(sum) => Ok(cx.integer(sum)),
        None => Err(Error::new("RangeError", "sum overflows a 64-bit integer")),
    }
}

/// `Greeter::Counter#zero?`: always `true`, and `nil` would read as false.
fn is_zero<'cx>(
    cx: &mut Context<'cx>,
    _self: Value<'cx>,
    _args: &[Value<'cx>],
) -> Result<Value<'cx>> {
    Ok(cx.boolean(true))
}

// What follows is not the worked example: methods that reach the rest of
// the API, for the test.

fn tested<'cx>(cx: &mut Context<'cx>, greeter: Value<'cx>) -> Result<()> {
    cx.define_singleton_method(greeter, "count", -1, count)?;
    cx.define_singleton_method(greeter, "kind", 1, kind)?;
    cx.define_singleton_method(greeter, "truthy?", 1, truthy)?;
    cx.define_singleton_method(greeter, "raise_with", 2, raise_with)?;
    cx.define_singleton_method(greeter, "panic", 0, panic)?;
    cx.define_singleton_method(greeter, "nothing", 0, nothing)?;
    cx.define_singleton_method(greeter, "missing", 0, missing)?;
    let error = cx.const_get("StandardError")?;
    cx.define_class_under(greeter, "Error", Some(error))?;
    Ok(())
}

/// `Greeter.count(*args)`: how many arguments it was given.
fn count<'cx>(cx: &mut Context<'cx>, _self: Value<'cx>, args: &[Value<'cx>]) -> Result<Value<'cx>> {
    Ok(cx.integer(args.len() as i64))
}

/// `Greeter.kind(x)`: what `Context::kind` says `x` is, as a String.
fn kind<'cx>(cx: &mut Context<'cx>, _self: Value<'cx>, args: &[Value<'cx>]) -> Result<Value<'cx>> {
    let name = match cx.kind(args[0]) {
        Kind::NIL => "nil",
        Kind::TRUE => "true",
        Kind::FALSE => "false",
        Kind::INTEGER => "integer",
        Kind::FLOAT => "float",
        Kind::STRING => "string",
        Kind::SYMBOL => "symbol",
        Kind::ARRAY => "array",
        Kind::HASH => "hash",
        Kind::MODULE => "module",
        Kind::CLASS => "class",
        _ => "object",
    };
    Ok(cx.string(name))
}

/// `Greeter.truthy?(x)`.
fn truthy<'cx>(
    cx: &mut Context<'cx>,
    _self: Value<'cx>,
    args: &[Value<'cx>],
) -> Result<Value<'cx>> {
    let truthy = cx.is_truthy(args[0]);
    Ok(cx.boolean(truthy))
}

/// `Greeter.raise_with(class, message)`: raise an instance of `class`.
fn raise_with<'cx>(
    cx: &mut Context<'cx>,
    _self: Value<'cx>,
    args: &[Value<'cx>],
) -> Result<Value<'cx>> {
    let message = cx.to_string(args[1])?;
    Err(cx.raise(args[0], &message))
}

/// `Greeter.panic`: a Rust panic, which must reach Ruby as an exception.
fn panic<'cx>(
    _cx: &mut Context<'cx>,
    _self: Value<'cx>,
    _args: &[Value<'cx>],
) -> Result<Value<'cx>> {
    panic!("the greeter fell over")
}

/// `Greeter.nothing`: `nil`.
fn nothing<'cx>(
    cx: &mut Context<'cx>,
    _self: Value<'cx>,
    _args: &[Value<'cx>],
) -> Result<Value<'cx>> {
    Ok(cx.nil())
}

/// `Greeter.missing`: an `Error` naming a class that does not exist.
fn missing<'cx>(
    _cx: &mut Context<'cx>,
    _self: Value<'cx>,
    _args: &[Value<'cx>],
) -> Result<Value<'cx>> {
    Err(Error::new("Greeter::NoSuchError", "never raised"))
}
