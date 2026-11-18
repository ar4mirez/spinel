//! The boundary itself: what crosses between a `spinel` and an extension.
//!
//! Everything here is `#[repr(C)]` and every function is `extern "C"`,
//! because Rust's own ABI is not stable across compiler versions and an
//! extension is built by whatever `rustc` its author has. An extension never
//! needs this module: [`crate::Context`] and [`crate::extension!`] are the
//! API. It is public so that the host side, in `spinel-vm`, implements the
//! same declarations an extension is compiled against, and so that an
//! extension written in another language can follow it.
//!
//! # What may change, and how
//!
//! [`Header`] is frozen: its layout is the same in every ABI version, past
//! and future, because it is what a `spinel` reads to decide whether the rest
//! of the library speaks its ABI at all. Everything else is versioned by
//! [`ABI`](crate::ABI). Within one ABI version, [`Api`] only ever grows at the
//! end, and its `size` field says how much of it the running `spinel` filled
//! in; changing or removing an entry, or the meaning of one, is a new ABI.
//! [`Api`]'s first two fields, `abi` and `size`, are frozen like the header,
//! so an extension can tell a table of another ABI before it reads further.
//! A `spinel` that still loads an older ABI hands each extension a table of
//! the ABI its header names.
//!
//! # Values
//!
//! A [`RawValue`] is an index into the running heap's root stack, never a
//! pointer: the collector is free to move objects, and an index an
//! extension kept past its call names a slot that is out of range or holds
//! something else, which the host refuses or answers, but never reads
//! through. A handle is good for the call it was handed to or made in.

use std::ffi::c_void;

/// The first eight bytes of every [`Header`].
pub const MAGIC: [u8; 8] = *b"SPINELX\0";

/// The exported symbol holding an extension's [`Header`].
pub const HEADER_SYMBOL: &str = "spinel_ext_header";

/// The exported symbol of an extension's [`InitFn`].
pub const INIT_SYMBOL: &str = "spinel_ext_init";

/// How many bytes of an extension's crate name and version [`Header`] keeps.
pub const NAME_LEN: usize = 64;

/// What an extension says about itself before any of its code runs.
///
/// Frozen across ABI versions (see the module documentation): a `spinel`
/// reads `magic` and `abi` from a library built against any `spinel-ext`,
/// older or newer, and only then decides whether to call its
/// [`INIT_SYMBOL`]. `name` and `version` are the extension crate's own,
/// NUL-padded, so a refusal can say which gem to rebuild.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// [`MAGIC`].
    pub magic: [u8; 8],
    /// The ABI the extension was built against: `spinel_ext::ABI` then.
    pub abi: u32,
    /// `size_of::<Header>()` as the extension was built.
    pub size: u32,
    /// The extension crate's name, NUL-padded and cut at [`NAME_LEN`] bytes.
    pub name: [u8; NAME_LEN],
    /// The extension crate's version, NUL-padded and cut at [`NAME_LEN`] bytes.
    pub version: [u8; NAME_LEN],
}

impl Header {
    /// The header for an extension crate named `name` at `version`, built
    /// against this `spinel-ext`. What [`crate::extension!`] exports.
    #[must_use]
    pub const fn new(name: &str, version: &str) -> Header {
        Header {
            magic: MAGIC,
            abi: crate::ABI,
            size: size_of::<Header>() as u32,
            name: padded(name),
            version: padded(version),
        }
    }

    /// The crate name, up to its first NUL; lossy, for a message.
    #[must_use]
    pub fn name(&self) -> String {
        unpadded(&self.name)
    }

    /// The crate version, up to its first NUL; lossy, for a message.
    #[must_use]
    pub fn version(&self) -> String {
        unpadded(&self.version)
    }
}

const fn padded(text: &str) -> [u8; NAME_LEN] {
    let bytes = text.as_bytes();
    let mut out = [0; NAME_LEN];
    let mut i = 0;
    while i < bytes.len() && i < NAME_LEN {
        out[i] = bytes[i];
        i += 1;
    }
    out
}

fn unpadded(field: &[u8; NAME_LEN]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// A rooted value: an index into the running heap's root stack.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RawValue(pub usize);

/// What a call across the boundary answered.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status(pub u32);

impl Status {
    /// It worked, and any out-parameter was written.
    pub const OK: Status = Status(0);
    /// A Ruby exception is pending in the context. The caller returns
    /// [`Status::RAISED`] in turn, and the host raises it when the method
    /// returns.
    pub const RAISED: Status = Status(1);
    /// The other side speaks an ABI this side cannot. Answered by an
    /// extension's init when the host's [`Api`] is of another ABI, which a
    /// host that checks the [`Header`] first never lets happen, or is shorter
    /// than the one the extension was built against: the same ABI, from a
    /// `spinel` older than its `spinel-ext`.
    pub const ABI_MISMATCH: Status = Status(2);
}

/// A borrowed run of bytes, for names and string contents.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Bytes {
    pub ptr: *const u8,
    pub len: usize,
}

impl Bytes {
    /// `text`'s bytes. Valid while `text` is.
    #[must_use]
    pub fn of(text: &[u8]) -> Bytes {
        Bytes {
            ptr: text.as_ptr(),
            len: text.len(),
        }
    }
}

/// The context of one call across the boundary: an init, or one call of a
/// method an extension defined.
///
/// `api` is the host's function table; `host` is the host's own state for
/// the call, which an extension passes back and never reads.
#[repr(C)]
#[derive(Debug)]
pub struct RawContext {
    pub api: *const Api,
    pub host: *mut c_void,
}

/// A method body: `self`, then `argc` arguments at `argv`. Writes the
/// method's answer to `out` and answers [`Status::OK`], or answers
/// [`Status::RAISED`] with an exception pending. `data` is what the
/// extension registered the method with, handed back unchanged.
pub type MethodFn = unsafe extern "C" fn(
    cx: *mut RawContext,
    data: *const c_void,
    receiver: RawValue,
    argc: usize,
    argv: *const RawValue,
    out: *mut RawValue,
) -> Status;

/// An extension's entry point, exported as [`INIT_SYMBOL`]: define the
/// extension's modules, classes and methods.
pub type InitFn = unsafe extern "C" fn(cx: *mut RawContext) -> Status;

/// What kind of value a [`RawValue`] holds, as [`Api::kind_of`] answers.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kind(pub u32);

impl Kind {
    pub const NIL: Kind = Kind(0);
    pub const TRUE: Kind = Kind(1);
    pub const FALSE: Kind = Kind(2);
    pub const INTEGER: Kind = Kind(3);
    pub const FLOAT: Kind = Kind(4);
    pub const STRING: Kind = Kind(5);
    pub const SYMBOL: Kind = Kind(6);
    pub const ARRAY: Kind = Kind(7);
    pub const HASH: Kind = Kind(8);
    pub const MODULE: Kind = Kind(9);
    pub const CLASS: Kind = Kind(10);
    /// Anything else: an object of some other class.
    pub const OBJECT: Kind = Kind(11);
}

/// The host's function table, ABI version [`crate::ABI`].
///
/// Every function takes the call's [`RawContext`] first. One that answers a
/// [`Status`] writes its out-parameter only on [`Status::OK`], and on
/// [`Status::RAISED`] has left a Ruby exception pending, which the extension
/// passes on by answering [`Status::RAISED`] itself. Names are UTF-8.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Api {
    /// The ABI this table is: the host's `spinel_ext::ABI`.
    pub abi: u32,
    /// `size_of::<Api>()` as the host built it. A later minor addition grows
    /// the table at the end, so an extension built against a shorter table
    /// reads only the prefix it knows.
    pub size: u32,

    /// `nil`.
    pub nil: unsafe extern "C" fn(cx: *mut RawContext) -> RawValue,
    /// `true` for nonzero, `false` for zero.
    pub boolean: unsafe extern "C" fn(cx: *mut RawContext, value: u8) -> RawValue,
    /// An `Integer`, a bignum when it is past the fixnum range.
    pub integer: unsafe extern "C" fn(cx: *mut RawContext, value: i64) -> RawValue,
    /// A `String` holding a copy of `bytes`, in UTF-8 whether or not they
    /// are valid in it, as a Ruby literal with a `\x` escape can be.
    pub string: unsafe extern "C" fn(cx: *mut RawContext, bytes: Bytes) -> RawValue,

    /// What kind of value `value` is. A handle that names nothing reads as
    /// [`Kind::NIL`].
    pub kind_of: unsafe extern "C" fn(cx: *mut RawContext, value: RawValue) -> Kind,
    /// Ruby truthiness: everything but `nil` and `false`.
    pub truthy: unsafe extern "C" fn(cx: *mut RawContext, value: RawValue) -> u8,
    /// An `Integer` that fits in an `i64`. Raises `TypeError` for anything
    /// else that is not an `Integer`, and `RangeError` for one too big.
    pub integer_value:
        unsafe extern "C" fn(cx: *mut RawContext, value: RawValue, out: *mut i64) -> Status,
    /// A `String`'s bytes: writes its length to `len`, and the first
    /// `min(len, capacity)` bytes to `buffer`. Raises `TypeError` for
    /// anything but a `String`.
    pub string_bytes: unsafe extern "C" fn(
        cx: *mut RawContext,
        value: RawValue,
        buffer: *mut u8,
        capacity: usize,
        len: *mut usize,
    ) -> Status,

    /// The constant `path` names, `::`-separated, from `Object`. Raises
    /// `NameError` when one is not defined.
    pub const_get:
        unsafe extern "C" fn(cx: *mut RawContext, path: Bytes, out: *mut RawValue) -> Status,
    /// Find or create the module `name` in `under`, a module or class, or at
    /// the top level for [`RawValue`] `usize::MAX`. Raises `TypeError` when
    /// the name is taken by something that is not a module.
    pub define_module: unsafe extern "C" fn(
        cx: *mut RawContext,
        under: RawValue,
        name: Bytes,
        out: *mut RawValue,
    ) -> Status,
    /// Find or create the class `name` in `under`, as for `define_module`,
    /// with `superclass`, or `Object` for `usize::MAX`. Raises `TypeError`
    /// for a superclass mismatch, as `class` does.
    pub define_class: unsafe extern "C" fn(
        cx: *mut RawContext,
        under: RawValue,
        name: Bytes,
        superclass: RawValue,
        out: *mut RawValue,
    ) -> Status,
    /// Define `name` on `module` — on its singleton class when `singleton`
    /// is nonzero — calling `func` with `data`. `arity` is how many arguments
    /// it takes, or -1 for any number; the host checks it before calling.
    pub define_method: unsafe extern "C" fn(
        cx: *mut RawContext,
        module: RawValue,
        name: Bytes,
        arity: i32,
        singleton: u8,
        func: MethodFn,
        data: *const c_void,
    ) -> Status,

    /// Leave an instance of `class`, an exception class, pending with
    /// `message`. Answers [`Status::RAISED`], or raises `TypeError` in its
    /// place when `class` is not an exception class.
    pub raise: unsafe extern "C" fn(cx: *mut RawContext, class: RawValue, message: Bytes) -> Status,
}

/// The [`RawValue`] that means "none" where an argument is optional.
pub const NONE: RawValue = RawValue(usize::MAX);
