//! The host side of the native-extension ABI (#103): the function table a
//! `spinel` hands an extension, the check that decides whether an extension
//! speaks an ABI this `spinel` loads, and the calls into one.
//!
//! The declarations are `spinel_ext::abi`'s, so the host implements exactly
//! what an extension is compiled against. An extension is checked before any
//! of its code runs: its frozen [`Header`] says which ABI it was built
//! against, and one this `spinel` does not load is refused with a
//! [`LoadError`] naming the extension, both ABIs and what to do.
//!
//! Values cross as root-stack indexes ([`RawValue`]). Each call into an
//! extension opens a scope of its own, so every handle it is given or makes
//! lives as long as the call; an index handed back is checked against that
//! scope, never trusted, so a stale or made-up one is never a read through a
//! stale pointer: it is refused with an `ArgumentError`, or, if it falls in
//! this call's range, names a value of this call.
//!
//! `require` finding a `.spinel` file and calling [`Extension::open`] is
//! #104; until then a `.spinel` on the load path is refused as it was.

use std::ffi::{CStr, CString, c_void};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use spinel_ext::abi::{
    self, Api, Bytes, HEADER_SYMBOL, Header, INIT_SYMBOL, InitFn, Kind, MethodFn, NONE, RawContext,
    RawValue, Status,
};

use super::{
    Builtin, ClassId, DefKind, Definition, Error, HandleScope, Native, Pending, Stack, Unwind,
    Value, class_id_of, class_name_of, class_of, define_or_reopen, exception_message,
    exception_new, exception_of, hash_of_pairs, inspect, is_exception_class, singleton_of,
    string_bytes,
};
use crate::method::{ExtData, ExtMethod};

/// Why an extension was not loaded. Its `Display` is the message the
/// `LoadError` carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// The library could not be opened at all.
    Open { path: PathBuf, reason: String },
    /// The library is not a Spinel extension: no header, or not one of ours.
    NotAnExtension { path: PathBuf, reason: String },
    /// The extension was built against an ABI this `spinel` does not load.
    Abi {
        path: PathBuf,
        name: String,
        version: String,
        /// The ABI the extension was built against.
        built: u32,
    },
    /// The extension's own check of this `spinel`'s function table failed:
    /// it was built against the same ABI with entries this `spinel` does not
    /// have.
    TableTooShort {
        path: PathBuf,
        name: String,
        version: String,
    },
}

impl LoadError {
    fn label(path: &Path, name: &str, version: &str) -> String {
        let path = path.display();
        match (name.is_empty(), version.is_empty()) {
            (true, _) => format!("the extension {path}"),
            (false, true) => format!("the extension {name} ({path})"),
            (false, false) => format!("the extension {name} {version} ({path})"),
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let running = spinel_ext::ABI;
        let spinel = crate::ENGINE_VERSION;
        match self {
            LoadError::Open { path, reason } => {
                write!(f, "cannot load the extension {}: {reason}", path.display())
            }
            LoadError::NotAnExtension { path, reason } => write!(
                f,
                "{} is not a Spinel extension: {reason}; an extension is a cdylib crate built \
                 with spinel-ext that names its init with spinel_ext::extension!",
                path.display()
            ),
            LoadError::Abi {
                path,
                name,
                version,
                built,
            } if *built > running => write!(
                f,
                "{} was built against spinel-ext ABI {built}, newer than the ABI {running} \
                 this spinel {spinel} loads; upgrade spinel, or install a version of the gem \
                 built for ABI {running}",
                LoadError::label(path, name, version)
            ),
            LoadError::Abi {
                path,
                name,
                version,
                built,
            } => write!(
                f,
                "{} was built against spinel-ext ABI {built}, older than the ABI {} this \
                 spinel {spinel} loads; rebuild it against this spinel by \
                 reinstalling the gem that ships it",
                LoadError::label(path, name, version),
                if spinel_ext::OLDEST_ABI == running {
                    running.to_string()
                } else {
                    format!("{}..={running}", spinel_ext::OLDEST_ABI)
                }
            ),
            LoadError::TableTooShort {
                path,
                name,
                version,
            } => write!(
                f,
                "{} needs entries of spinel-ext ABI {running} that this spinel {spinel} does \
                 not provide; upgrade spinel",
                LoadError::label(path, name, version)
            ),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<LoadError> for Error {
    fn from(error: LoadError) -> Error {
        Error::raise("LoadError", error.to_string())
    }
}

/// Whether `header`, read from the extension at `path`, is one this `spinel`
/// loads: the right magic, and an ABI between `spinel_ext::OLDEST_ABI` and
/// `spinel_ext::ABI`.
pub fn check(path: &Path, header: &Header) -> Result<(), LoadError> {
    if header.magic != abi::MAGIC {
        return Err(LoadError::NotAnExtension {
            path: path.to_owned(),
            reason: format!("its {HEADER_SYMBOL} does not start with Spinel's magic bytes"),
        });
    }
    if !(spinel_ext::OLDEST_ABI..=spinel_ext::ABI).contains(&header.abi) {
        return Err(LoadError::Abi {
            path: path.to_owned(),
            name: header.name(),
            version: header.version(),
            built: header.abi,
        });
    }
    Ok(())
}

/// An extension this `spinel` has checked and will run: its header, and the
/// init behind it.
#[derive(Debug, Clone)]
pub struct Extension {
    path: PathBuf,
    header: Header,
    init: InitFn,
}

impl Extension {
    /// Check `header` and pair it with `init`, for an extension already in
    /// this process: one linked in, or one a test builds against
    /// `spinel-ext` directly.
    pub fn new(
        path: impl Into<PathBuf>,
        header: &Header,
        init: InitFn,
    ) -> Result<Extension, LoadError> {
        let path = path.into();
        check(&path, header)?;
        Ok(Extension {
            path,
            header: *header,
            init,
        })
    }

    /// Open the library at `path`, read its header, and check it, before
    /// any of the extension's own code is called.
    ///
    /// A library this refuses is closed again, so a rebuilt file at the same
    /// path is read afresh the next time. One it accepts is never closed:
    /// the methods it defines are in the heap's tables for as long as the
    /// process runs, as CRuby never unloads an extension either. Opening one
    /// runs whatever constructors its object format declares, which is the
    /// platform loader's doing and nothing `spinel-ext` emits.
    pub fn open(path: &Path) -> Result<Extension, LoadError> {
        let library = dlopen(path)?;
        let opened = Extension::from_library(path, library);
        if opened.is_err() {
            // SAFETY: `library` came from `dlopen` and nothing read from it
            // outlives this call: the header and every message are copies.
            unsafe { libc::dlclose(library) };
        }
        opened
    }

    /// [`Extension::open`]'s checks, on a library already open.
    fn from_library(path: &Path, library: *mut c_void) -> Result<Extension, LoadError> {
        let not_one = |reason: String| LoadError::NotAnExtension {
            path: path.to_owned(),
            reason,
        };
        let Some(symbol) = dlsym(library, HEADER_SYMBOL) else {
            return Err(not_one(format!("it exports no {HEADER_SYMBOL}")));
        };
        let header = read_header(symbol.cast()).map_err(not_one)?;
        check(path, &header)?;
        let Some(init) = dlsym(library, INIT_SYMBOL) else {
            return Err(not_one(format!("it exports no {INIT_SYMBOL}")));
        };
        // SAFETY: an extension whose header is ours exports its init under
        // this name with this signature; `spinel_ext::extension!` declares
        // both together.
        let init = unsafe { std::mem::transmute::<*mut c_void, InitFn>(init) };
        Ok(Extension {
            path: path.to_owned(),
            header,
            init,
        })
    }

    /// Where the extension came from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the extension says about itself.
    #[must_use]
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Run the extension's init in this heap: define its modules, classes
    /// and methods. An exception it raises is [`Error::Uncaught`], as one
    /// that leaves a program is; a table it finds too short, a `LoadError`.
    pub fn init(&self, scope: &mut HandleScope<'_>) -> Result<(), Error> {
        match self.init_in(scope)? {
            None => Ok(()),
            Some(exception) => Err(Error::Uncaught {
                class: class_name_of(scope, exception),
                message: exception_message(scope, exception),
            }),
        }
    }

    /// [`Extension::init`], answering the exception the init raised, rooted
    /// in `scope`, for a caller inside the interpreter to raise.
    pub(super) fn init_in(&self, scope: &mut HandleScope<'_>) -> Result<Option<Value>, Error> {
        let outcome = {
            let mut inner = scope.nested();
            let mut host = Host::new(&mut inner);
            let mut raw = host.context();
            // SAFETY: `raw` is a live context over `API` for the call, and
            // the header said the extension speaks this ABI.
            let status = unsafe { (self.init)(&raw mut raw) };
            if status == Status::ABI_MISMATCH {
                return Err(LoadError::TableTooShort {
                    path: self.path.clone(),
                    name: self.header.name(),
                    version: self.header.version(),
                }
                .into());
            }
            host.finish(status, None)
        };
        match outcome? {
            Answer::Value(_) => Ok(None),
            Answer::Raised(exception) => {
                let handle = scope.root(exception);
                Ok(Some(scope.get(handle)))
            }
        }
    }
}

/// The library at `path`, opened.
fn dlopen(path: &Path) -> Result<*mut c_void, LoadError> {
    let failed = |reason: String| LoadError::Open {
        path: path.to_owned(),
        reason,
    };
    let Some(text) = path.to_str() else {
        return Err(failed("the path is not UTF-8".to_owned()));
    };
    let Ok(text) = CString::new(text) else {
        return Err(failed("the path holds a NUL byte".to_owned()));
    };
    // SAFETY: a NUL-terminated path. RTLD_LOCAL keeps one extension's
    // symbols from answering another's lookups.
    let library = unsafe { libc::dlopen(text.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if library.is_null() {
        return Err(failed(dlerror()));
    }
    Ok(library)
}

/// The address of `name` in `library`, if it exports one.
fn dlsym(library: *mut c_void, name: &str) -> Option<*mut c_void> {
    let name = CString::new(name).expect("a symbol name holds no NUL");
    // SAFETY: an open library and a NUL-terminated name.
    let symbol = unsafe { libc::dlsym(library, name.as_ptr()) };
    (!symbol.is_null()).then_some(symbol)
}

/// The loader's last error, as text.
fn dlerror() -> String {
    // SAFETY: `dlerror` answers null or a NUL-terminated string that stays
    // valid until the next call on this thread, and it is copied at once.
    let text = unsafe { libc::dlerror() };
    if text.is_null() {
        return "the loader gave no reason".to_owned();
    }
    // SAFETY: as above.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// Read a [`Header`] at `at`, the address of an extension's
/// `spinel_ext_header`, a field at a time: the magic first, and nothing
/// past what the header's own `size` says it holds.
fn read_header(at: *const u8) -> Result<Header, String> {
    let field = |offset: usize, len: usize, out: &mut [u8]| {
        // SAFETY: `at` is an exported symbol; each read stays within the
        // part of the header the checks before it have shown is there.
        unsafe { std::ptr::copy_nonoverlapping(at.add(offset), out.as_mut_ptr(), len) };
    };
    let mut magic = [0u8; 8];
    field(0, 8, &mut magic);
    if magic != abi::MAGIC {
        return Err(format!(
            "its {HEADER_SYMBOL} does not start with Spinel's magic bytes"
        ));
    }
    let mut word = [0u8; 4];
    field(8, 4, &mut word);
    let built = u32::from_ne_bytes(word);
    field(12, 4, &mut word);
    let size = u32::from_ne_bytes(word) as usize;
    let mut header = Header {
        magic,
        abi: built,
        size: size as u32,
        name: [0; abi::NAME_LEN],
        version: [0; abi::NAME_LEN],
    };
    // The names are read only when the header says it has them, which every
    // header `spinel_ext` has written does.
    if size >= size_of::<Header>() {
        let (mut name, mut version) = ([0u8; abi::NAME_LEN], [0u8; abi::NAME_LEN]);
        field(16, abi::NAME_LEN, &mut name);
        field(16 + abi::NAME_LEN, abi::NAME_LEN, &mut version);
        header.name = name;
        header.version = version;
    }
    Ok(header)
}

/// Call the extension method [`Native::Ext`] `index` names.
pub(super) fn call(
    scope: &mut HandleScope<'_>,
    stack: &mut Stack,
    call: &Pending,
    index: u32,
) -> Result<Option<Unwind>, Error> {
    let method = scope
        .definitions()
        .ext(index)
        .expect("a Native::Ext names one of this heap's extension methods");
    let mut args = call.args.clone();
    // Keywords arrive as a trailing Hash, as CRuby hands them to a C method.
    if !call.keywords.is_empty() {
        let hash = hash_of_pairs(scope, &call.keywords)?;
        let hash = scope.root(hash);
        args.push(scope.get(hash));
    }
    if let Ok(arity) = usize::try_from(method.arity) {
        if args.len() != arity {
            return Err(Error::raise(
                "ArgumentError",
                format!(
                    "wrong number of arguments (given {}, expected {arity})",
                    args.len()
                ),
            ));
        }
    }
    let outcome = {
        let mut inner = scope.nested();
        let receiver = inner.root(call.receiver);
        let receiver = RawValue(inner.handle_index(receiver));
        let argv: Vec<RawValue> = args
            .iter()
            .map(|&arg| {
                let handle = inner.root(arg);
                RawValue(inner.handle_index(handle))
            })
            .collect();
        let mut host = Host::new(&mut inner);
        let mut raw = host.context();
        let mut out = NONE;
        // SAFETY: a live context over `API` for the call; `argv` holds
        // `argv.len()` handles and `out` is a place for the answer. The
        // function and its data are what the extension registered.
        let status = unsafe {
            (method.func)(
                &raw mut raw,
                method.data.0,
                receiver,
                argv.len(),
                argv.as_ptr(),
                &raw mut out,
            )
        };
        host.finish(status, Some(out))
    };
    // The answer was rooted only in the call's scope, which is gone; nothing
    // has allocated since, and the stack or the unwind roots it from here.
    match outcome? {
        Answer::Value(value) => {
            stack.push(value);
            Ok(None)
        }
        Answer::Raised(exception) => {
            let handle = scope.root(exception);
            Ok(Some(Unwind::Exception(scope.get(handle))))
        }
    }
}

/// What a call into an extension came back with.
enum Answer {
    Value(Value),
    Raised(Value),
}

/// The host's state for one call into an extension: the call's scope, and
/// what went wrong, if anything did.
struct Host<'s, 'h> {
    scope: &'s mut HandleScope<'h>,
    /// The exception pending, rooted in `scope`.
    pending: Option<Value>,
    /// Something the VM cannot run, which is not a Ruby exception and must
    /// leave as the [`Error`] it is.
    refusal: Option<Error>,
}

impl<'s, 'h> Host<'s, 'h> {
    fn new(scope: &'s mut HandleScope<'h>) -> Host<'s, 'h> {
        Host {
            scope,
            pending: None,
            refusal: None,
        }
    }

    fn context(&mut self) -> RawContext {
        RawContext {
            api: &raw const API,
            host: (self as *mut Host<'s, 'h>).cast(),
        }
    }

    /// Turn the status a call answered into its outcome.
    fn finish(self, status: Status, out: Option<RawValue>) -> Result<Answer, Error> {
        if let Some(refusal) = self.refusal {
            return Err(refusal);
        }
        if status == Status::RAISED {
            return match self.pending {
                Some(exception) => Ok(Answer::Raised(exception)),
                None => Err(Error::raise(
                    "RuntimeError",
                    "a native extension reported an exception it never raised",
                )),
            };
        }
        if status != Status::OK {
            return Err(Error::raise(
                "RuntimeError",
                format!("a native extension answered an unknown status {}", status.0),
            ));
        }
        match out {
            None => Ok(Answer::Value(Value::NIL)),
            Some(out) => match self.scope.rooted_at(out.0) {
                Some(value) => Ok(Answer::Value(value)),
                None => Err(Error::raise(
                    "RuntimeError",
                    "a native extension method answered a handle that names no value of its call",
                )),
            },
        }
    }

    /// Root `value` for the rest of the call, as the handle the extension
    /// gets.
    fn handle(&mut self, value: Value) -> RawValue {
        let handle = self.scope.root(value);
        RawValue(self.scope.handle_index(handle))
    }

    /// The value behind a handle the extension passed, if it is one of this
    /// call's.
    fn value(&self, raw: RawValue) -> Option<Value> {
        self.scope.rooted_at(raw.0)
    }

    /// [`Host::value`], raising `ArgumentError` for a handle that is not one
    /// of this call's.
    fn value_or_raise(&mut self, raw: RawValue) -> Result<Value, Status> {
        match self.value(raw) {
            Some(value) => Ok(value),
            None => Err(self.raise(Error::raise(
                "ArgumentError",
                "a handle that names no value of this call: one kept from another call, or made up",
            ))),
        }
    }

    /// Leave `error` pending: a Ruby exception, or a refusal that leaves as
    /// itself.
    fn raise(&mut self, error: Error) -> Status {
        match error {
            Error::Raise { class, message } => {
                let exception = exception_new(self.scope, class, &message);
                self.pend(exception)
            }
            refusal => {
                self.refusal = Some(refusal);
                Status::RAISED
            }
        }
    }

    fn pend(&mut self, exception: Value) -> Status {
        let handle = self.scope.root(exception);
        self.pending = Some(self.scope.get(handle));
        Status::RAISED
    }

    /// The module or class a handle names, or a pending `TypeError`.
    fn module(&mut self, raw: RawValue) -> Result<ClassId, Status> {
        let value = self.value_or_raise(raw)?;
        match class_id_of(self.scope, value) {
            Some(id) => Ok(id),
            None => {
                let shown = inspect(self.scope, value);
                Err(self.raise(Error::raise(
                    "TypeError",
                    format!("{shown} is not a class/module"),
                )))
            }
        }
    }

    /// A name the extension passed, as UTF-8, or a pending `ArgumentError`.
    fn name(&mut self, bytes: Bytes) -> Result<String, Status> {
        match std::str::from_utf8(bytes_of(bytes)) {
            Ok(name) if !name.is_empty() => Ok(name.to_owned()),
            _ => Err(self.raise(Error::raise(
                "ArgumentError",
                "a name that is empty or not UTF-8",
            ))),
        }
    }

    /// A constant name the extension passed, or a pending `NameError`.
    fn constant_name(&mut self, bytes: Bytes) -> Result<String, Status> {
        let name = self.name(bytes)?;
        if name.starts_with(|c: char| c.is_ascii_uppercase()) {
            Ok(name)
        } else {
            Err(self.raise(Error::raise(
                "NameError",
                format!("wrong constant name {name}"),
            )))
        }
    }
}

/// The bytes a [`Bytes`] borrows; empty for a null pointer.
fn bytes_of<'a>(bytes: Bytes) -> &'a [u8] {
    if bytes.ptr.is_null() || bytes.len == 0 {
        return &[];
    }
    // SAFETY: the ABI's contract: `len` bytes at `ptr`, valid for the call.
    unsafe { std::slice::from_raw_parts(bytes.ptr, bytes.len) }
}

/// The host behind a context the extension passed back, for one host
/// function's body; `on_panic` is what that function answers if the VM
/// panics underneath it, which must not unwind into the extension.
///
/// # Safety
///
/// `cx` is the context the host made for the call in progress.
unsafe fn with_host<T>(
    cx: *mut RawContext,
    on_panic: T,
    body: impl FnOnce(&mut Host<'_, '_>) -> T,
) -> T {
    // SAFETY: the caller's contract: `host` is the `Host` that made `cx`,
    // alive for the call, and nothing else holds it while this runs.
    let host = unsafe { &mut *(*cx).host.cast::<Host<'_, '_>>() };
    match catch_unwind(AssertUnwindSafe(|| body(&mut *host))) {
        Ok(answer) => answer,
        Err(_) => {
            host.refusal = Some(Error::raise(
                "RuntimeError",
                "spinel failed inside a call from a native extension",
            ));
            on_panic
        }
    }
}

/// [`with_host`] for a function that answers a [`Status`].
///
/// # Safety
///
/// As for [`with_host`].
unsafe fn status(
    cx: *mut RawContext,
    body: impl FnOnce(&mut Host<'_, '_>) -> Result<(), Status>,
) -> Status {
    // SAFETY: the caller's contract.
    unsafe {
        with_host(cx, Status::RAISED, |host| match body(host) {
            Ok(()) => Status::OK,
            Err(status) => status,
        })
    }
}

/// Write `value` to the out-parameter `out`.
///
/// # Safety
///
/// `out` is a valid place for a `T`, as every out-parameter of the ABI is.
unsafe fn write<T>(out: *mut T, value: T) {
    // SAFETY: the caller's contract.
    unsafe { out.write(value) };
}

/// The table every extension is handed: ABI `spinel_ext::ABI`. Immutable,
/// and holding no VM state, which lives in each call's [`Host`].
static API: Api = Api {
    abi: spinel_ext::ABI,
    size: size_of::<Api>() as u32,
    nil: api_nil,
    boolean: api_boolean,
    integer: api_integer,
    string: api_string,
    kind_of: api_kind_of,
    truthy: api_truthy,
    integer_value: api_integer_value,
    string_bytes: api_string_bytes,
    const_get: api_const_get,
    define_module: api_define_module,
    define_class: api_define_class,
    define_method: api_define_method,
    raise: api_raise,
};

unsafe extern "C" fn api_nil(cx: *mut RawContext) -> RawValue {
    // SAFETY: called by the extension with the context it was given.
    unsafe { with_host(cx, NONE, |host| host.handle(Value::NIL)) }
}

unsafe extern "C" fn api_boolean(cx: *mut RawContext, value: u8) -> RawValue {
    let value = if value == 0 {
        Value::FALSE
    } else {
        Value::TRUE
    };
    // SAFETY: called by the extension with the context it was given.
    unsafe { with_host(cx, NONE, |host| host.handle(value)) }
}

unsafe extern "C" fn api_integer(cx: *mut RawContext, value: i64) -> RawValue {
    // SAFETY: called by the extension with the context it was given.
    unsafe {
        with_host(cx, NONE, |host| {
            let integer = match Value::fixnum(value) {
                Some(fixnum) => fixnum,
                None => crate::bignum::value(host.scope, &num_bigint::BigInt::from(value)),
            };
            host.handle(integer)
        })
    }
}

unsafe extern "C" fn api_string(cx: *mut RawContext, bytes: Bytes) -> RawValue {
    // SAFETY: called by the extension with the context it was given.
    unsafe {
        with_host(cx, NONE, |host| {
            let class = super::class_handle(host.scope, Builtin::String);
            let string =
                super::string_alloc(host.scope, class, bytes_of(bytes), crate::strings::UTF_8);
            host.handle(string)
        })
    }
}

unsafe extern "C" fn api_kind_of(cx: *mut RawContext, value: RawValue) -> Kind {
    // SAFETY: called by the extension with the context it was given.
    unsafe {
        with_host(cx, Kind::NIL, |host| {
            let Some(value) = host.value(value) else {
                return Kind::NIL;
            };
            match value {
                Value::NIL => return Kind::NIL,
                Value::TRUE => return Kind::TRUE,
                Value::FALSE => return Kind::FALSE,
                _ => {}
            }
            if value.as_fixnum().is_some() {
                return Kind::INTEGER;
            }
            if value.as_flonum().is_some() {
                return Kind::FLOAT;
            }
            if value.as_symbol().is_some() {
                return Kind::SYMBOL;
            }
            if let Some(id) = class_id_of(host.scope, value) {
                return match host.scope.classes().kind(id) {
                    crate::class::Kind::Class => Kind::CLASS,
                    crate::class::Kind::Module => Kind::MODULE,
                };
            }
            let repr = class_of(host.scope, value).and_then(|id| host.scope.classes().repr(id));
            match repr {
                Some(Builtin::Integer) => Kind::INTEGER,
                Some(Builtin::Float) => Kind::FLOAT,
                Some(Builtin::String) => Kind::STRING,
                Some(Builtin::Symbol) => Kind::SYMBOL,
                Some(Builtin::Array) => Kind::ARRAY,
                Some(Builtin::Hash) => Kind::HASH,
                _ => Kind::OBJECT,
            }
        })
    }
}

unsafe extern "C" fn api_truthy(cx: *mut RawContext, value: RawValue) -> u8 {
    // SAFETY: called by the extension with the context it was given.
    unsafe {
        with_host(cx, 0, |host| {
            u8::from(host.value(value).is_some_and(Value::is_truthy))
        })
    }
}

/// Ruby's `TypeError` for converting `value` to `into` implicitly.
fn no_implicit_conversion(host: &mut Host<'_, '_>, value: Value, into: &str) -> Error {
    let what = match value {
        Value::NIL => "nil".to_owned(),
        Value::TRUE => "true".to_owned(),
        Value::FALSE => "false".to_owned(),
        _ => class_name_of(host.scope, value),
    };
    Error::raise(
        "TypeError",
        format!("no implicit conversion of {what} into {into}"),
    )
}

unsafe extern "C" fn api_integer_value(
    cx: *mut RawContext,
    value: RawValue,
    out: *mut i64,
) -> Status {
    // SAFETY: called by the extension with the context it was given.
    unsafe {
        status(cx, |host| {
            let value = host.value_or_raise(value)?;
            if let Some(n) = value.as_fixnum() {
                write(out, n);
                return Ok(());
            }
            let Some(big) = crate::bignum::read(host.scope, value) else {
                let error = no_implicit_conversion(host, value, "Integer");
                return Err(host.raise(error));
            };
            match i64::try_from(&big) {
                Ok(n) => {
                    write(out, n);
                    Ok(())
                }
                Err(_) => Err(host.raise(Error::raise(
                    "RangeError",
                    "bignum too big to convert into 'long'",
                ))),
            }
        })
    }
}

unsafe extern "C" fn api_string_bytes(
    cx: *mut RawContext,
    value: RawValue,
    buffer: *mut u8,
    capacity: usize,
    len: *mut usize,
) -> Status {
    // SAFETY: called by the extension with the context it was given.
    unsafe {
        status(cx, |host| {
            let value = host.value_or_raise(value)?;
            let is_string = class_of(host.scope, value)
                .and_then(|id| host.scope.classes().repr(id))
                == Some(Builtin::String);
            let bytes = is_string.then(|| string_bytes(host.scope, value)).flatten();
            let Some(bytes) = bytes else {
                let error = no_implicit_conversion(host, value, "String");
                return Err(host.raise(error));
            };
            let copied = bytes.len().min(capacity);
            if copied > 0 && !buffer.is_null() {
                // SAFETY: the ABI's contract: room for `capacity` bytes.
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, copied);
            }
            write(len, bytes.len());
            Ok(())
        })
    }
}

unsafe extern "C" fn api_const_get(cx: *mut RawContext, path: Bytes, out: *mut RawValue) -> Status {
    // SAFETY: called by the extension with the context it was given.
    unsafe {
        status(cx, |host| {
            let path = host.name(path)?;
            let mut owner = Builtin::Object.id();
            let mut value = host.scope.classes().object(owner);
            let segments: Vec<&str> = path.split("::").collect();
            for (at, segment) in segments.iter().enumerate() {
                if !segment.starts_with(|c: char| c.is_ascii_uppercase()) {
                    return Err(host.raise(Error::raise(
                        "NameError",
                        format!("wrong constant name {segment}"),
                    )));
                }
                let name = crate::shared::symbols::intern(segment);
                let Some(found) = host.scope.classes().const_get_here(owner, name) else {
                    let error = super::uninitialized(
                        host.scope,
                        owner,
                        name,
                        if at == 0 {
                            super::ConstScope::Top
                        } else {
                            super::ConstScope::Qualified
                        },
                    );
                    return Err(host.raise(error));
                };
                value = found;
                if at + 1 < segments.len() {
                    let Some(id) = class_id_of(host.scope, value) else {
                        let shown = inspect(host.scope, value);
                        return Err(host.raise(Error::raise(
                            "TypeError",
                            format!("{shown} is not a class/module"),
                        )));
                    };
                    owner = id;
                }
            }
            let handle = host.handle(value);
            write(out, handle);
            Ok(())
        })
    }
}

/// `define_module` and `define_class`, which differ in the kind and the
/// superclass.
fn define(
    host: &mut Host<'_, '_>,
    under: RawValue,
    name: Bytes,
    kind: DefKind,
    superclass: RawValue,
) -> Result<RawValue, Status> {
    let cbase = if under == NONE {
        Builtin::Object.id()
    } else {
        host.module(under)?
    };
    let name = host.constant_name(name)?;
    let superclass = if superclass == NONE {
        None
    } else {
        Some(host.value_or_raise(superclass)?)
    };
    let symbol = crate::shared::symbols::intern(&name);
    // ponytail: the hooks a new definition owes — `inherited` on a Ruby
    // superclass — are dropped, since an init has no frames to run them on.
    // The upgrade is to queue them on the `require` that loads the extension
    // (#104), which does.
    match define_or_reopen(host.scope, cbase, symbol, kind, superclass, None) {
        Ok((id, _hooks)) => {
            let object = host.scope.classes().object(id);
            Ok(host.handle(object))
        }
        Err(error) => Err(host.raise(error)),
    }
}

unsafe extern "C" fn api_define_module(
    cx: *mut RawContext,
    under: RawValue,
    name: Bytes,
    out: *mut RawValue,
) -> Status {
    // SAFETY: called by the extension with the context it was given.
    unsafe {
        status(cx, |host| {
            let module = define(host, under, name, DefKind::Module, NONE)?;
            write(out, module);
            Ok(())
        })
    }
}

unsafe extern "C" fn api_define_class(
    cx: *mut RawContext,
    under: RawValue,
    name: Bytes,
    superclass: RawValue,
    out: *mut RawValue,
) -> Status {
    // SAFETY: called by the extension with the context it was given.
    unsafe {
        status(cx, |host| {
            let class = define(host, under, name, DefKind::Class, superclass)?;
            write(out, class);
            Ok(())
        })
    }
}

unsafe extern "C" fn api_define_method(
    cx: *mut RawContext,
    module: RawValue,
    name: Bytes,
    arity: i32,
    singleton: u8,
    func: MethodFn,
    data: *const c_void,
) -> Status {
    // SAFETY: called by the extension with the context it was given.
    unsafe {
        status(cx, |host| {
            let mut id = host.module(module)?;
            let name = host.name(name)?;
            if arity < -1 {
                return Err(host.raise(Error::raise(
                    "ArgumentError",
                    format!("arity {arity} is neither a count nor -1"),
                )));
            }
            if singleton != 0 {
                let object = host.scope.classes().object(id);
                id = singleton_of(host.scope, object).map_err(|error| host.raise(error))?;
            }
            let index = host.scope.definitions_mut().add_ext(ExtMethod {
                func,
                data: ExtData(data),
                arity,
            });
            let body = host
                .scope
                .definitions_mut()
                .add(Definition::Native(Native::Ext(index)));
            let symbol = crate::shared::symbols::intern(&name);
            host.scope.classes_mut().define_method(id, symbol, body);
            Ok(())
        })
    }
}

unsafe extern "C" fn api_raise(cx: *mut RawContext, class: RawValue, message: Bytes) -> Status {
    // SAFETY: called by the extension with the context it was given.
    unsafe {
        with_host(cx, Status::RAISED, |host| {
            let class = match host.value_or_raise(class) {
                Ok(class) => class,
                Err(status) => return status,
            };
            let is_exception =
                class_id_of(host.scope, class).is_some_and(|id| is_exception_class(host.scope, id));
            if !is_exception {
                return host.raise(Error::raise("TypeError", "exception class/object expected"));
            }
            let message = String::from_utf8_lossy(bytes_of(message)).into_owned();
            let exception = exception_of(host.scope, class, &message);
            host.pend(exception)
        })
    }
}
