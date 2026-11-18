//! Public, versioned API for native extensions.
//!
//! Standalone on purpose: an extension depends on this crate and nothing else
//! of Spinel's, and this crate depends on nothing at all. There is no CRuby C
//! API and no `extconf.rb`.
//!
//! An extension is a `cdylib` crate that depends on `spinel-ext`, writes an
//! init function against [`Context`], and names it with [`extension!`]. The
//! macro exports two symbols: a [`abi::Header`] saying which [`ABI`] the
//! extension was built against, and `spinel_ext_init`, which a `spinel`
//! calls only once it has read the header and found the ABI one it speaks.
//! A mismatch is refused with a message naming the extension, both ABIs and
//! what to do, before any of the extension's code runs.
//!
//! Values are [`Value`]s: handles into the running heap's root stack, good
//! for the call they were made in, and never pointers, so the collector is
//! free to move what they name. A method that fails answers an [`Error`],
//! which becomes a Ruby exception when the method returns; a panic does too,
//! as a `RuntimeError`, as long as the extension is built with the default
//! `panic = "unwind"`.
//!
//! # A worked example
//!
//! `Cargo.toml`:
//!
//! ```toml
//! [package]
//! name = "greeter"
//! version = "0.1.0"
//! edition = "2024"
//!
//! [lib]
//! crate-type = ["cdylib"]
//!
//! [dependencies]
//! spinel-ext = "0.0.1"
//! ```
//!
//! `src/lib.rs`:
//!
//! ```no_run
//! use spinel_ext::{Context, Error, Result, Value};
//!
//! spinel_ext::extension!(init);
//!
//! fn init(cx: &mut Context<'_>) -> Result<()> {
//!     let greeter = cx.define_module("Greeter")?;
//!     cx.define_singleton_method(greeter, "hello", 1, hello)?;
//!     cx.define_singleton_method(greeter, "add", 2, add)?;
//!     let counter = cx.define_class_under(greeter, "Counter", None)?;
//!     cx.define_method(counter, "zero?", 0, is_zero)?;
//!     Ok(())
//! }
//!
//! /// `Greeter.hello(name)`: `"Hello, <name>!"`.
//! fn hello<'cx>(cx: &mut Context<'cx>, _self: Value<'cx>, args: &[Value<'cx>]) -> Result<Value<'cx>> {
//!     let name = cx.to_string(args[0])?;
//!     if name.is_empty() {
//!         return Err(Error::new("ArgumentError", "name is empty"));
//!     }
//!     Ok(cx.string(&format!("Hello, {name}!")))
//! }
//!
//! /// `Greeter.add(a, b)`: an `Integer`, or a `RangeError` past `i64`.
//! fn add<'cx>(cx: &mut Context<'cx>, _self: Value<'cx>, args: &[Value<'cx>]) -> Result<Value<'cx>> {
//!     let (a, b) = (cx.to_i64(args[0])?, cx.to_i64(args[1])?);
//!     match a.checked_add(b) {
//!         Some(sum) => Ok(cx.integer(sum)),
//!         None => Err(Error::new("RangeError", "sum overflows a 64-bit integer")),
//!     }
//! }
//!
//! /// `Greeter::Counter#zero?`: always `true`, and `nil` would read as false.
//! fn is_zero<'cx>(cx: &mut Context<'cx>, _self: Value<'cx>, _args: &[Value<'cx>]) -> Result<Value<'cx>> {
//!     Ok(cx.boolean(true))
//! }
//! ```
//!
//! `cargo build --release` writes the library (`libgreeter.so` on Linux,
//! `libgreeter.dylib` on macOS), which is the extension; installed on the
//! load path as `greeter.spinel`, it answers, in Ruby:
//!
//! ```ruby
//! require "greeter"
//! Greeter.hello("Ruby")   # => "Hello, Ruby!"
//! Greeter.add(2, 40)      # => 42
//! Greeter.hello("")       # ArgumentError: name is empty
//! Greeter.add(1)          # ArgumentError: wrong number of arguments (given 1, expected 2)
//! ```
//!
//! `require "greeter"` loads `greeter.spinel` from the load path (#104):
//! the host checks the header, then calls the init. What this crate and the
//! host's check settle is the contract that loader calls.

pub mod abi;

use std::ffi::c_void;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};

use abi::{Api, Bytes, NONE, RawContext, RawValue, Status};

pub use abi::Kind;

/// The extension ABI this `spinel-ext` speaks, and the one a `spinel` built
/// with it loads.
///
/// Embedded in every extension through [`extension!`]'s [`abi::Header`], and
/// recorded in every compiled app's `manifest.json` (docs/build.md) so a
/// payload's `.spinel` extensions can be checked against the binary that runs
/// them. A `spinel` loads an extension whose ABI is between [`OLDEST_ABI`] and
/// this, inclusive, and refuses any other.
///
/// 1 is the first API. It goes up when an entry of [`abi::Api`] changes, moves
/// or goes, or means something else; an entry added at the end does not move
/// it (see [`abi`]).
pub const ABI: u32 = 1;

/// The oldest extension ABI a `spinel` built with this crate still loads.
pub const OLDEST_ABI: u32 = 1;

/// The context of one call into an extension: its init, or one call of a
/// method it defined. Every value it makes lives as long as the call does.
pub struct Context<'cx> {
    raw: *mut RawContext,
    api: &'cx Api,
    /// Invariant in `'cx`, so values of two calls cannot be mixed.
    _call: PhantomData<fn(&'cx ()) -> &'cx ()>,
}

/// A Ruby value, rooted for the call it was handed to or made in.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Value<'cx> {
    raw: RawValue,
    _call: PhantomData<fn(&'cx ()) -> &'cx ()>,
}

impl<'cx> Value<'cx> {
    fn from_raw(raw: RawValue) -> Value<'cx> {
        Value {
            raw,
            _call: PhantomData,
        }
    }

    /// The handle this value is, for a call through [`abi::Api`] directly.
    #[must_use]
    pub fn raw(self) -> RawValue {
        self.raw
    }
}

/// Why a method or an init did not answer: a Ruby exception to raise when it
/// returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// An exception the host already has pending: a call through [`Context`]
    /// raised, or [`Context::raise`] made one. Pass it on with `?`.
    Pending,
    /// Raise an instance of the exception class `class` names, a
    /// `::`-separated constant path from `Object`, with `message`.
    Raise { class: String, message: String },
}

impl Error {
    /// Raise an instance of the exception class at `class` with `message`:
    /// `Error::new("ArgumentError", "name is empty")`.
    pub fn new(class: impl Into<String>, message: impl Into<String>) -> Error {
        Error::Raise {
            class: class.into(),
            message: message.into(),
        }
    }
}

/// The result of an extension's method or init.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A method body: the receiver, then the arguments, whose count the host has
/// already checked against the arity the method was defined with.
pub type Method = for<'cx> fn(&mut Context<'cx>, Value<'cx>, &[Value<'cx>]) -> Result<Value<'cx>>;

/// An extension's init: define its modules, classes and methods.
pub type Init = for<'cx> fn(&mut Context<'cx>) -> Result<()>;

impl<'cx> Context<'cx> {
    /// # Safety
    ///
    /// `raw` points at a live context whose `api` is a full table of this
    /// crate's [`ABI`], both valid for `'cx`.
    unsafe fn from_raw(raw: *mut RawContext) -> Context<'cx> {
        // SAFETY: the caller's contract.
        let api = unsafe { &*(*raw).api };
        Context {
            raw,
            api,
            _call: PhantomData,
        }
    }

    fn checked(&mut self, status: Status) -> Result<()> {
        if status == Status::OK {
            Ok(())
        } else {
            Err(Error::Pending)
        }
    }

    /// `nil`.
    pub fn nil(&mut self) -> Value<'cx> {
        // SAFETY: a live context, per `from_raw`.
        Value::from_raw(unsafe { (self.api.nil)(self.raw) })
    }

    /// `true` or `false`.
    pub fn boolean(&mut self, value: bool) -> Value<'cx> {
        // SAFETY: a live context, per `from_raw`.
        Value::from_raw(unsafe { (self.api.boolean)(self.raw, u8::from(value)) })
    }

    /// An `Integer`.
    pub fn integer(&mut self, value: i64) -> Value<'cx> {
        // SAFETY: a live context, per `from_raw`.
        Value::from_raw(unsafe { (self.api.integer)(self.raw, value) })
    }

    /// A UTF-8 `String` holding a copy of `text`.
    pub fn string(&mut self, text: &str) -> Value<'cx> {
        self.string_from_bytes(text.as_bytes())
    }

    /// A `String` holding a copy of `bytes`, marked UTF-8 whether or not
    /// they are valid in it.
    pub fn string_from_bytes(&mut self, bytes: &[u8]) -> Value<'cx> {
        // SAFETY: a live context, and `bytes` outlives the call.
        Value::from_raw(unsafe { (self.api.string)(self.raw, Bytes::of(bytes)) })
    }

    /// What kind of value `value` is.
    pub fn kind(&mut self, value: Value<'cx>) -> Kind {
        // SAFETY: a live context, per `from_raw`.
        unsafe { (self.api.kind_of)(self.raw, value.raw) }
    }

    /// Ruby truthiness: everything but `nil` and `false`.
    pub fn is_truthy(&mut self, value: Value<'cx>) -> bool {
        // SAFETY: a live context, per `from_raw`.
        unsafe { (self.api.truthy)(self.raw, value.raw) != 0 }
    }

    /// Whether `value` is `nil`.
    pub fn is_nil(&mut self, value: Value<'cx>) -> bool {
        self.kind(value) == Kind::NIL
    }

    /// An `Integer`'s value. `TypeError` for anything else, `RangeError` for
    /// one past `i64`.
    pub fn to_i64(&mut self, value: Value<'cx>) -> Result<i64> {
        let mut out = 0;
        // SAFETY: a live context, and `out` is a valid place for the answer.
        let status = unsafe { (self.api.integer_value)(self.raw, value.raw, &raw mut out) };
        self.checked(status).map(|()| out)
    }

    /// A `String`'s bytes. `TypeError` for anything else.
    pub fn to_bytes(&mut self, value: Value<'cx>) -> Result<Vec<u8>> {
        let mut len = 0;
        // SAFETY: a live context; a zero capacity writes no bytes.
        let status = unsafe {
            (self.api.string_bytes)(self.raw, value.raw, std::ptr::null_mut(), 0, &raw mut len)
        };
        self.checked(status)?;
        let mut buffer = vec![0u8; len];
        // SAFETY: `buffer` has room for `len` bytes, which is all the host
        // writes; it cannot have grown since, since nothing ran between.
        let status = unsafe {
            (self.api.string_bytes)(self.raw, value.raw, buffer.as_mut_ptr(), len, &raw mut len)
        };
        self.checked(status)?;
        buffer.truncate(len);
        Ok(buffer)
    }

    /// A `String`'s contents as UTF-8. `TypeError` for anything but a
    /// `String`, and `ArgumentError` for one whose bytes are not UTF-8.
    pub fn to_string(&mut self, value: Value<'cx>) -> Result<String> {
        String::from_utf8(self.to_bytes(value)?)
            .map_err(|_| Error::new("ArgumentError", "invalid byte sequence in UTF-8"))
    }

    /// The constant `path` names, `::`-separated, from `Object`:
    /// `cx.const_get("Encoding::CompatibilityError")`. `NameError` when one is
    /// not defined.
    pub fn const_get(&mut self, path: &str) -> Result<Value<'cx>> {
        let mut out = NONE;
        // SAFETY: a live context; `path` outlives the call.
        let status =
            unsafe { (self.api.const_get)(self.raw, Bytes::of(path.as_bytes()), &raw mut out) };
        self.checked(status).map(|()| Value::from_raw(out))
    }

    /// The top-level module `name`, created if it does not exist, as
    /// `module Name; end` does.
    pub fn define_module(&mut self, name: &str) -> Result<Value<'cx>> {
        self.module_in(NONE, name)
    }

    /// The module `name` in `under`, a module or class, created if it does
    /// not exist.
    pub fn define_module_under(&mut self, under: Value<'cx>, name: &str) -> Result<Value<'cx>> {
        self.module_in(under.raw, name)
    }

    fn module_in(&mut self, under: RawValue, name: &str) -> Result<Value<'cx>> {
        let mut out = NONE;
        // SAFETY: a live context; `name` outlives the call.
        let status = unsafe {
            (self.api.define_module)(self.raw, under, Bytes::of(name.as_bytes()), &raw mut out)
        };
        self.checked(status).map(|()| Value::from_raw(out))
    }

    /// The top-level class `name`, created under `superclass` (`Object` for
    /// `None`) if it does not exist, as `class Name < Superclass; end` does.
    pub fn define_class(
        &mut self,
        name: &str,
        superclass: Option<Value<'cx>>,
    ) -> Result<Value<'cx>> {
        self.class_in(NONE, name, superclass)
    }

    /// The class `name` in `under`, a module or class, as
    /// [`Context::define_class`] makes it.
    pub fn define_class_under(
        &mut self,
        under: Value<'cx>,
        name: &str,
        superclass: Option<Value<'cx>>,
    ) -> Result<Value<'cx>> {
        self.class_in(under.raw, name, superclass)
    }

    fn class_in(
        &mut self,
        under: RawValue,
        name: &str,
        superclass: Option<Value<'cx>>,
    ) -> Result<Value<'cx>> {
        let mut out = NONE;
        let superclass = superclass.map_or(NONE, |value| value.raw);
        // SAFETY: a live context; `name` outlives the call.
        let status = unsafe {
            (self.api.define_class)(
                self.raw,
                under,
                Bytes::of(name.as_bytes()),
                superclass,
                &raw mut out,
            )
        };
        self.checked(status).map(|()| Value::from_raw(out))
    }

    /// Define the instance method `name` on `module`, taking `arity`
    /// arguments, or any number for -1.
    pub fn define_method(
        &mut self,
        module: Value<'cx>,
        name: &str,
        arity: i32,
        method: Method,
    ) -> Result<()> {
        self.method_on(module, name, arity, false, method)
    }

    /// Define `name` on `module`'s singleton class: a class method, or a
    /// module function called on the module itself.
    pub fn define_singleton_method(
        &mut self,
        module: Value<'cx>,
        name: &str,
        arity: i32,
        method: Method,
    ) -> Result<()> {
        self.method_on(module, name, arity, true, method)
    }

    fn method_on(
        &mut self,
        module: Value<'cx>,
        name: &str,
        arity: i32,
        singleton: bool,
        method: Method,
    ) -> Result<()> {
        // SAFETY: a live context; `name` outlives the call, and `trampoline`
        // turns `data` back into the `Method` it was made from.
        let status = unsafe {
            (self.api.define_method)(
                self.raw,
                module.raw,
                Bytes::of(name.as_bytes()),
                arity,
                u8::from(singleton),
                trampoline,
                method as *const c_void,
            )
        };
        self.checked(status)
    }

    /// Leave an instance of `class`, an exception class, pending with
    /// `message`, and answer the [`Error`] to return:
    /// `return Err(cx.raise(class, "message"))`.
    pub fn raise(&mut self, class: Value<'cx>, message: &str) -> Error {
        // SAFETY: a live context; `message` outlives the call. Either way
        // something is pending: the exception, or the `TypeError` for a
        // class that is not an exception class.
        unsafe { (self.api.raise)(self.raw, class.raw, Bytes::of(message.as_bytes())) };
        Error::Pending
    }

    /// Turn `result` into the status to answer the host with, leaving its
    /// error pending.
    fn finish<T>(&mut self, result: std::thread::Result<Result<T>>) -> Result<T, Status> {
        let error = match result {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(error)) => error,
            Err(panic) => {
                let what = panic
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "a panic with no message".to_owned());
                Error::new("RuntimeError", format!("native extension panicked: {what}"))
            }
        };
        // A class path that names nothing leaves its `NameError` pending
        // instead.
        if let Error::Raise { class, message } = error {
            if let Ok(class) = self.const_get(&class) {
                self.raise(class, &message);
            }
        }
        Err(Status::RAISED)
    }
}

/// The [`abi::MethodFn`] behind every method a [`Context`] defines: `data` is
/// the [`Method`].
unsafe extern "C" fn trampoline(
    cx: *mut RawContext,
    data: *const c_void,
    receiver: RawValue,
    argc: usize,
    argv: *const RawValue,
    out: *mut RawValue,
) -> Status {
    // SAFETY: `data` is the `Method` `method_on` registered, handed back
    // unchanged; a fn pointer and a data pointer are the same size on every
    // platform Spinel builds for.
    let method = unsafe { std::mem::transmute::<*const c_void, Method>(data) };
    // SAFETY: the host calls a method with a live context of the ABI the
    // init checked.
    let mut cx = unsafe { Context::from_raw(cx) };
    let args: &[Value<'_>] = if argc == 0 {
        &[]
    } else {
        // SAFETY: the host passes `argc` handles at `argv`, live for the
        // call; `Value` is a transparent `RawValue`.
        unsafe { std::slice::from_raw_parts(argv.cast::<Value<'_>>(), argc) }
    };
    let receiver = Value::from_raw(receiver);
    let result = catch_unwind(AssertUnwindSafe(|| method(&mut cx, receiver, args)));
    match cx.finish(result) {
        Ok(value) => {
            // SAFETY: the host passes a valid place for the answer.
            unsafe { *out = value.raw };
            Status::OK
        }
        Err(status) => status,
    }
}

/// What [`extension!`]'s `spinel_ext_init` runs: check the host's table, then
/// run `init`. Not API.
///
/// # Safety
///
/// `cx` points at a live context whose `api` field points at a table with at
/// least its first two fields, `abi` and `size`, which every ABI keeps.
#[doc(hidden)]
pub unsafe fn __init(cx: *mut RawContext, init: Init) -> Status {
    // SAFETY: the caller's contract covers `abi` and `size`, and nothing
    // past them is read until they say the table is this crate's.
    let (abi, size) = unsafe { ((*(*cx).api).abi, (*(*cx).api).size) };
    if abi != ABI || (size as usize) < size_of::<Api>() {
        return Status::ABI_MISMATCH;
    }
    // SAFETY: checked just above.
    let mut cx = unsafe { Context::from_raw(cx) };
    let result = catch_unwind(AssertUnwindSafe(|| init(&mut cx)));
    match cx.finish(result) {
        Ok(()) => Status::OK,
        Err(status) => status,
    }
}

/// Export an extension's [`abi::Header`] and its `spinel_ext_init`, which
/// runs `init` — a function `fn(&mut Context<'_>) -> Result<()>` — once a
/// `spinel` has checked the header. Once per extension crate, at its root.
///
/// The header names the crate and its version from Cargo's, so that a
/// refusal can say which extension to rebuild.
#[macro_export]
macro_rules! extension {
    ($init:path) => {
        #[unsafe(no_mangle)]
        #[allow(non_upper_case_globals)]
        pub static spinel_ext_header: $crate::abi::Header =
            $crate::abi::Header::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

        /// # Safety
        ///
        /// Called by a `spinel` with a live context, after it has checked
        /// `spinel_ext_header`.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn spinel_ext_init(
            cx: *mut $crate::abi::RawContext,
        ) -> $crate::abi::Status {
            // SAFETY: the caller's contract.
            unsafe { $crate::__init(cx, $init) }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_records_this_abi_and_names_its_crate() {
        let header = abi::Header::new("greeter", "0.1.0");
        assert_eq!(header.magic, abi::MAGIC);
        assert_eq!(header.abi, ABI);
        assert_eq!(header.size as usize, size_of::<abi::Header>());
        assert_eq!(header.name(), "greeter");
        assert_eq!(header.version(), "0.1.0");
    }

    #[test]
    fn a_long_name_is_cut_rather_than_overflowing() {
        let long = "x".repeat(100);
        let header = abi::Header::new(&long, "1");
        assert_eq!(header.name(), "x".repeat(abi::NAME_LEN));
    }

    #[test]
    fn the_frozen_header_keeps_its_layout() {
        // Every ABI reads these offsets; moving one is not a version bump,
        // it is a `spinel` that cannot tell an old extension from garbage.
        assert_eq!(std::mem::offset_of!(abi::Header, magic), 0);
        assert_eq!(std::mem::offset_of!(abi::Header, abi), 8);
        assert_eq!(std::mem::offset_of!(abi::Header, size), 12);
        assert_eq!(std::mem::offset_of!(abi::Header, name), 16);
        assert_eq!(size_of::<abi::Header>(), 16 + 2 * abi::NAME_LEN);
        assert_eq!(std::mem::offset_of!(abi::Api, abi), 0);
        assert_eq!(std::mem::offset_of!(abi::Api, size), 4);
    }

    fn never_called(_cx: &mut Context<'_>) -> Result<()> {
        unreachable!("an init whose host table is wrong never runs")
    }

    #[test]
    fn an_init_refuses_a_host_table_of_another_abi_or_too_short() {
        // Only `abi` and `size` are read before the check, so two words
        // stand in for a table.
        for (abi, size) in [
            (ABI + 1, size_of::<Api>() as u32),
            (0, size_of::<Api>() as u32),
            (ABI, 8),
        ] {
            let table = [abi, size];
            let mut cx = RawContext {
                api: table.as_ptr().cast(),
                host: std::ptr::null_mut(),
            };
            // SAFETY: `__init` reads the two words and stops.
            let status = unsafe { __init(&raw mut cx, never_called) };
            assert_eq!(status, Status::ABI_MISMATCH, "abi {abi}, size {size}");
        }
    }

    #[test]
    fn the_oldest_abi_is_no_newer_than_this_one() {
        const { assert!(OLDEST_ABI <= ABI) };
    }
}
