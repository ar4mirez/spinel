//! `openssl` (#106), phase 1: digests, HMAC and random bytes, on the
//! system's OpenSSL 3.
//!
//! What Ruby cannot say for the `openssl` gem's C half: an `EVP_MD_CTX`, the
//! context OpenSSL hashes and MACs in, and the calls on it. Everything above
//! that — `OpenSSL::Digest`, `OpenSSL::HMAC`, `OpenSSL::Random` and the
//! errors they raise — is `lib/openssl/spinel.rb` over the `__openssl_*__`
//! primitives `interp.rs` makes of these.
//!
//! Each function here is the OpenSSL half of one function of ruby/openssl's
//! `ext/openssl` (`ossl_digest.c`, `ossl_hmac.c`, `ossl_rand.c`, `ossl.c`),
//! making the same calls in the same order, so a name resolves, a context
//! answers its name and lengths, and a failure reads as it does on CRuby:
//! the message `ossl_raise` builds, the last queued OpenSSL error appended,
//! and the queue drained into the exception's `errors`. No algorithm is
//! implemented here; OpenSSL computes every byte.
//!
//! A context lives in its heap's [`Contexts`], under an id that is never
//! handed out twice, so a finalizer that runs late can only miss. Ruby holds
//! the id and frees it when the object holding it is collected. Nothing here
//! is process-global but OpenSSL's own library state, which CRuby shares the
//! same way.
//!
//! OpenSSL's error queue is per OS thread; a primitive makes its calls and
//! drains the queue on the thread that holds the heap, before returning.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_void};
use std::ptr::{self, NonNull};

use openssl_sys as sys;

// What `openssl-sys` does not bind. All are in OpenSSL 3.0's libcrypto,
// which `openssl-sys` links and `build.rs` requires.
unsafe extern "C" {
    fn EVP_MD_get0_name(md: *const sys::EVP_MD) -> *const c_char;
    fn EVP_MD_CTX_get_pkey_ctx(ctx: *const sys::EVP_MD_CTX) -> *mut sys::EVP_PKEY_CTX;
    fn EVP_PKEY_CTX_get0_pkey(ctx: *mut sys::EVP_PKEY_CTX) -> *mut sys::EVP_PKEY;
    fn OBJ_NAME_do_all_sorted(
        kind: c_int,
        each: unsafe extern "C" fn(*const ObjName, *mut c_void),
        arg: *mut c_void,
    );
    fn RAND_seed(buf: *const c_void, num: c_int);
    fn RAND_load_file(name: *const c_char, max: c_long) -> c_int;
    fn RAND_write_file(name: *const c_char) -> c_int;
    #[cfg(ossl_ctx_size_ex)]
    fn EVP_MD_CTX_get_size_ex(ctx: *const sys::EVP_MD_CTX) -> c_int;
}

/// `EVP_MD_CTX_size(ctx)`, as the headers `openssl-sys` built against
/// define it, which is what CRuby calls: from OpenSSL 3.4 the context's own
/// output size, which is -1 for an extendable-output digest such as
/// SHAKE128 whose length was not set; before it, the algorithm's.
///
/// # Safety
///
/// `ctx` is a live context.
unsafe fn ctx_size(ctx: *const sys::EVP_MD_CTX) -> c_int {
    #[cfg(ossl_ctx_size_ex)]
    // SAFETY: the caller's contract.
    unsafe {
        EVP_MD_CTX_get_size_ex(ctx)
    }
    #[cfg(not(ossl_ctx_size_ex))]
    // SAFETY: the caller's contract.
    unsafe {
        sys::EVP_MD_CTX_get_size(ctx)
    }
}

/// `OBJ_NAME`, as `OBJ_NAME_do_all_sorted` hands each one over.
#[repr(C)]
struct ObjName {
    kind: c_int,
    alias: c_int,
    name: *const c_char,
    data: *const c_char,
}

/// `OBJ_NAME_TYPE_MD_METH`: the digests' names.
const OBJ_NAME_TYPE_MD_METH: c_int = 1;

/// `EVP_MAX_MD_SIZE`: the most an `EVP_DigestSignFinal` writes for a MAC.
const EVP_MAX_MD_SIZE: usize = 64;

/// The exception a failure raises. `lib/openssl/spinel.rb` maps each to its
/// class by [`Class::code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// `OpenSSL::Digest::DigestError`.
    Digest,
    /// `OpenSSL::HMACError`.
    Hmac,
    /// `OpenSSL::Random::RandomError`.
    Random,
    /// `ArgumentError`, for what CRuby's `rb_str_new` refuses before OpenSSL
    /// is asked: a negative length.
    Argument,
    /// `RangeError`, for a length past a C int (`RSTRING_LENINT`).
    Range,
    /// `RuntimeError`: a context that was never initialised.
    Runtime,
}

impl Class {
    /// The number the Ruby side reads the class from.
    #[must_use]
    pub fn code(self) -> i64 {
        match self {
            Class::Digest => 1,
            Class::Hmac => 2,
            Class::Random => 3,
            Class::Argument => 4,
            Class::Range => 5,
            Class::Runtime => 6,
        }
    }
}

/// A failed call, as `ossl_make_error` reports it: the message with the last
/// queued error's reason appended, and every queued error, oldest first, in
/// the form `OpenSSLError#errors` lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub class: Class,
    pub message: Vec<u8>,
    pub errors: Vec<Vec<u8>>,
}

/// A failure of `class` with `message`, OpenSSL's error queue drained into
/// it (`ossl_raise`).
fn failure(class: Class, message: &[u8]) -> Failure {
    let errors = drain_errors();
    let mut message = message.to_vec();
    if let Some((_, reason)) = errors.last() {
        if !message.is_empty() {
            message.extend_from_slice(b": ");
        }
        message.extend_from_slice(reason);
    }
    Failure {
        class,
        message,
        errors: errors.into_iter().map(|(entry, _)| entry).collect(),
    }
}

/// A failure CRuby raises without asking OpenSSL's queue.
fn plain(class: Class, message: &str) -> Failure {
    Failure {
        class,
        message: message.as_bytes().to_vec(),
        errors: Vec::new(),
    }
}

/// `GetDigest`'s complaint about an object with no context.
fn uninitialized() -> Failure {
    plain(Class::Runtime, "Digest CTX wasn't initialized!")
}

/// The text at `p`, or `fallback` for a null pointer.
///
/// # Safety
///
/// `p` is null or a NUL-terminated string that outlives the call.
unsafe fn text(p: *const c_char, fallback: &[u8]) -> Vec<u8> {
    if p.is_null() {
        fallback.to_vec()
    } else {
        // SAFETY: the caller's contract.
        unsafe { CStr::from_ptr(p) }.to_bytes().to_vec()
    }
}

/// Empty this thread's OpenSSL error queue, oldest first: each error as
/// `collect_errors_into` writes it (`error:%08lX:lib:func:reason (data)`)
/// and as `ossl_make_error` appends it to a message (`reason (data)`).
fn drain_errors() -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut drained = Vec::new();
    loop {
        let mut file = ptr::null();
        let mut line = 0;
        let mut func = ptr::null();
        let mut data = ptr::null();
        let mut flags = 0;
        // SAFETY: every out-pointer is a local, and the strings OpenSSL
        // answers are static or owned by the error entry until the next
        // queue call on this thread, which is after they are copied.
        let entry = unsafe {
            let code =
                sys::ERR_get_error_all(&mut file, &mut line, &mut func, &mut data, &mut flags);
            if code == 0 {
                break;
            }
            let lib = text(sys::ERR_lib_error_string(code), b"");
            let func = text(func, b"");
            let reason = sys::ERR_reason_error_string(code);
            let mut entry = format!("error:{code:08X}:").into_bytes();
            entry.extend_from_slice(&lib);
            entry.push(b':');
            entry.extend_from_slice(&func);
            entry.push(b':');
            entry.extend_from_slice(&text(reason, b""));
            // `ossl_make_error` says "(null)" for a reason OpenSSL has no
            // string for, and appends the data only when there is some;
            // `collect_errors_into` says "(null)" for missing data.
            let mut short = text(reason, b"(null)");
            if flags & sys::ERR_TXT_STRING != 0 {
                let detail = text(data, b"(null)");
                entry.extend_from_slice(b" (");
                entry.extend_from_slice(&detail);
                entry.push(b')');
                if !data.is_null() {
                    short.extend_from_slice(b" (");
                    short.extend_from_slice(&detail);
                    short.push(b')');
                }
            }
            (entry, short)
        };
        drained.push(entry);
    }
    drained
}

/// What a context is for, which decides which `EVP_*` calls it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Digest,
    Hmac,
}

/// An `EVP_MD_CTX` this heap owns.
#[derive(Debug)]
struct Context {
    ctx: NonNull<sys::EVP_MD_CTX>,
    kind: Kind,
}

impl Context {
    fn new(kind: Kind) -> Option<Context> {
        // SAFETY: no preconditions; null means out of memory.
        let ctx = NonNull::new(unsafe { sys::EVP_MD_CTX_new() })?;
        Some(Context { ctx, kind })
    }

    fn ptr(&self) -> *mut sys::EVP_MD_CTX {
        self.ctx.as_ptr()
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: the context is this one's alone, freed once, here.
        unsafe { sys::EVP_MD_CTX_free(self.ptr()) }
    }
}

/// Where a digest's algorithm comes from: a name, or a live context's
/// (`ossl_evp_md_fetch` given an `OpenSSL::Digest`).
#[derive(Debug)]
pub enum Algorithm {
    Name(CString),
    Of(u64),
}

/// An `EVP_MD` found by [`Contexts::fetch`]: freed on drop when it came from
/// `EVP_MD_fetch`, which hands out a reference; a context initialised with
/// one takes a reference of its own.
struct Md {
    md: *const sys::EVP_MD,
    fetched: bool,
}

impl Drop for Md {
    fn drop(&mut self) {
        if self.fetched {
            // SAFETY: the reference `EVP_MD_fetch` handed out, freed once.
            unsafe { sys::EVP_MD_free(self.md.cast_mut()) }
        }
    }
}

/// One heap's OpenSSL contexts, by id.
#[derive(Debug, Default)]
pub struct Contexts {
    live: HashMap<u64, Context>,
    next: u64,
}

impl Contexts {
    fn get(&self, id: u64, kind: Kind) -> Option<&Context> {
        self.live.get(&id).filter(|context| context.kind == kind)
    }

    fn insert(&mut self, context: Context) -> u64 {
        let id = self.next;
        self.next += 1;
        self.live.insert(id, context);
        id
    }

    /// An empty context of `kind`, for an object that was allocated but not
    /// initialised (`ossl_hmac_alloc`'s context, which every HMAC has).
    pub fn alloc(&mut self, kind: Kind) -> Result<u64, Failure> {
        sys::init();
        let class = match kind {
            Kind::Digest => Class::Digest,
            Kind::Hmac => Class::Hmac,
        };
        let context = Context::new(kind).ok_or_else(|| failure(class, b"EVP_MD_CTX_new"))?;
        Ok(self.insert(context))
    }

    /// Free context `id`; an id already freed is no context.
    pub fn free(&mut self, id: u64) {
        self.live.remove(&id);
    }

    /// `ossl_evp_md_fetch`: the algorithm a name or a digest names.
    fn fetch(&self, algorithm: &Algorithm) -> Result<Md, Failure> {
        sys::init();
        let name = match algorithm {
            Algorithm::Of(id) => {
                let ctx = self.live_ctx(*id, Kind::Digest)?;
                // SAFETY: a live context; null when it was never initialised.
                let md = unsafe { sys::EVP_MD_CTX_get0_md(ctx) };
                if md.is_null() {
                    return Err(uninitialized());
                }
                return Ok(Md { md, fetched: false });
            }
            Algorithm::Name(name) => name,
        };
        // SAFETY: `name` is NUL-terminated. A null `oid` is what
        // `EVP_get_digestbyobj` is given on CRuby too, and answers null.
        unsafe {
            let mut md = sys::EVP_get_digestbyname(name.as_ptr());
            if md.is_null() {
                let oid = sys::OBJ_txt2obj(name.as_ptr(), 0);
                md = sys::EVP_get_digestbynid(sys::OBJ_obj2nid(oid));
                sys::ASN1_OBJECT_free(oid);
            }
            if !md.is_null() {
                return Ok(Md { md, fetched: false });
            }
            drain_errors();
            let fetched = sys::EVP_MD_fetch(ptr::null_mut(), name.as_ptr(), ptr::null());
            if !fetched.is_null() {
                return Ok(Md {
                    md: fetched,
                    fetched: true,
                });
            }
        }
        let mut message = b"unsupported digest algorithm: ".to_vec();
        message.extend_from_slice(name.as_bytes());
        Err(failure(Class::Digest, &message))
    }

    /// `OpenSSL::Digest#initialize`: context `id` (a new one for nil) set to
    /// hash with `algorithm`. The id.
    pub fn digest_init(&mut self, id: Option<u64>, algorithm: &Algorithm) -> Result<u64, Failure> {
        let md = self.fetch(algorithm)?;
        let (id, made) = match id.filter(|id| self.get(*id, Kind::Digest).is_some()) {
            Some(id) => (id, false),
            None => (self.alloc(Kind::Digest)?, true),
        };
        let context = &self.live[&id];
        // SAFETY: a live context and an algorithm `md` holds alive.
        if unsafe { sys::EVP_DigestInit_ex(context.ptr(), md.md, ptr::null_mut()) } != 1 {
            let failed = failure(Class::Digest, b"Digest initialization failed");
            // A context made for this call is no object's yet: CRuby's stays
            // with the object that raised, which nothing then holds.
            if made {
                self.free(id);
            }
            return Err(failed);
        }
        Ok(id)
    }

    /// `OpenSSL::HMAC#initialize`: context `id` set to MAC with `key` and the
    /// digest `algorithm`.
    pub fn hmac_init(&mut self, id: u64, key: &[u8], algorithm: &Algorithm) -> Result<(), Failure> {
        let ctx = self.live_ctx(id, Kind::Hmac)?;
        if c_int::try_from(key.len()).is_err() {
            let message = format!("integer {} too big to convert to 'int'", key.len());
            return Err(plain(Class::Range, &message));
        }
        let md = self.fetch(algorithm)?;
        // SAFETY: `key` is `key.len()` readable bytes, copied by OpenSSL; the
        // key OpenSSL makes is referenced by the context once signing is
        // initialised, so this reference is dropped either way.
        unsafe {
            let pkey = sys::EVP_PKEY_new_raw_private_key(
                sys::EVP_PKEY_HMAC,
                ptr::null_mut(),
                key.as_ptr(),
                key.len(),
            );
            if pkey.is_null() {
                return Err(failure(Class::Hmac, b"EVP_PKEY_new_raw_private_key"));
            }
            let status =
                sys::EVP_DigestSignInit(ctx, ptr::null_mut(), md.md, ptr::null_mut(), pkey);
            sys::EVP_PKEY_free(pkey);
            if status != 1 {
                return Err(failure(Class::Hmac, b"EVP_DigestSignInit"));
            }
        }
        Ok(())
    }

    /// `initialize_copy`: a new context holding a copy of `id`'s state.
    pub fn copy(&mut self, id: u64, kind: Kind) -> Result<u64, Failure> {
        let (class, message): (Class, &[u8]) = match kind {
            Kind::Digest => (Class::Digest, b""),
            Kind::Hmac => (Class::Hmac, b"EVP_MD_CTX_copy"),
        };
        let source = self.live_ctx(id, kind)?;
        let copy = Context::new(kind).ok_or_else(|| failure(class, b"EVP_MD_CTX_new"))?;
        // SAFETY: two live contexts; the copy is new, so `EVP_MD_CTX_copy`'s
        // reset of it first is a no-op and `_ex` is the same call.
        if unsafe { sys::EVP_MD_CTX_copy_ex(copy.ptr(), source) } != 1 {
            return Err(failure(class, message));
        }
        Ok(self.insert(copy))
    }

    /// `#reset`: context `id` back to the state its initialisation left.
    pub fn reset(&mut self, id: u64, kind: Kind) -> Result<(), Failure> {
        let ctx = self.live_ctx(id, kind)?;
        // SAFETY: a live context. An HMAC context's key is held by its
        // signing context, which `EVP_DigestSignInit` reinitialises in
        // place, so the key it is given stays alive across the call. An
        // HMAC that was allocated and never initialised has no signing
        // context, and CRuby reads through the null one and crashes; here it
        // fails as the initialisation would.
        let ok = unsafe {
            let md = sys::EVP_MD_CTX_get0_md(ctx);
            match kind {
                Kind::Digest => {
                    !md.is_null() && sys::EVP_DigestInit_ex(ctx, md, ptr::null_mut()) == 1
                }
                Kind::Hmac => {
                    let pctx = EVP_MD_CTX_get_pkey_ctx(ctx);
                    !pctx.is_null() && {
                        let pkey = EVP_PKEY_CTX_get0_pkey(pctx);
                        sys::EVP_DigestSignInit(ctx, ptr::null_mut(), md, ptr::null_mut(), pkey)
                            == 1
                    }
                }
            }
        };
        if ok {
            return Ok(());
        }
        Err(match kind {
            Kind::Digest => failure(Class::Digest, b"Digest initialization failed."),
            Kind::Hmac => failure(Class::Hmac, b"EVP_DigestSignInit"),
        })
    }

    /// `#update`: `data` added to what context `id` hashes.
    pub fn update(&mut self, id: u64, kind: Kind, data: &[u8]) -> Result<(), Failure> {
        let ctx = self.live_ctx(id, kind)?;
        // SAFETY: a live context and `data.len()` readable bytes.
        let ok = unsafe {
            match kind {
                Kind::Digest => sys::EVP_DigestUpdate(ctx, data.as_ptr().cast(), data.len()),
                Kind::Hmac => sys::EVP_DigestSignUpdate(ctx, data.as_ptr().cast(), data.len()),
            }
        } == 1;
        if ok {
            return Ok(());
        }
        Err(match kind {
            Kind::Digest => failure(Class::Digest, b"EVP_DigestUpdate"),
            Kind::Hmac => failure(Class::Hmac, b"EVP_DigestSignUpdate"),
        })
    }

    /// A digest's `finish`: the digest, the context finalised as
    /// `EVP_DigestFinal_ex` leaves it. An HMAC's `digest`: the MAC so far,
    /// the context left to take more, as OpenSSL signs on a copy of it.
    pub fn finish(&mut self, id: u64, kind: Kind) -> Result<Vec<u8>, Failure> {
        let ctx = self.live_ctx(id, kind)?;
        match kind {
            Kind::Digest => {
                // SAFETY: a live context.
                let size = unsafe { ctx_size(ctx) };
                let Ok(size) = usize::try_from(size) else {
                    // `rb_str_new(NULL, EVP_MD_CTX_size(ctx))`, for an
                    // extendable-output digest, whose size is -1.
                    return Err(plain(
                        Class::Argument,
                        "negative string size (or size too big)",
                    ));
                };
                let mut out = vec![0; size];
                // SAFETY: `out` has the context's digest size.
                if unsafe { sys::EVP_DigestFinal_ex(ctx, out.as_mut_ptr(), ptr::null_mut()) } != 1 {
                    return Err(failure(Class::Digest, b"EVP_DigestFinal_ex"));
                }
                Ok(out)
            }
            Kind::Hmac => {
                let mut out = vec![0; EVP_MAX_MD_SIZE];
                let mut length = out.len();
                // SAFETY: `out` has room for any MAC; `length` says how much.
                if unsafe { sys::EVP_DigestSignFinal(ctx, out.as_mut_ptr(), &mut length) } != 1 {
                    return Err(failure(Class::Hmac, b"EVP_DigestSignFinal"));
                }
                out.truncate(length);
                Ok(out)
            }
        }
    }

    /// A digest's `[name, digest_length, block_length]`, as
    /// `EVP_MD_name`, `EVP_MD_CTX_size` and `EVP_MD_CTX_block_size` say.
    pub fn digest_info(&self, id: u64) -> Result<(Vec<u8>, i64, i64), Failure> {
        let ctx = self.live_ctx(id, Kind::Digest)?;
        // SAFETY: a live, initialised context, whose algorithm's name is
        // static or owned by the algorithm the context holds.
        unsafe {
            let md = sys::EVP_MD_CTX_get0_md(ctx);
            if md.is_null() {
                return Err(uninitialized());
            }
            let name = text(EVP_MD_get0_name(md), b"");
            let size = i64::from(ctx_size(ctx));
            let block = i64::from(sys::EVP_MD_get_block_size(md));
            Ok((name, size, block))
        }
    }

    /// Context `id`'s `EVP_MD_CTX`, if it is a live one of `kind`.
    fn live_ctx(&self, id: u64, kind: Kind) -> Result<*mut sys::EVP_MD_CTX, Failure> {
        self.get(id, kind)
            .map(Context::ptr)
            .ok_or_else(uninitialized)
    }
}

/// `OpenSSL::Digest.digests`: every digest name OpenSSL knows, sorted.
#[must_use]
pub fn digest_names() -> Vec<Vec<u8>> {
    unsafe extern "C" fn each(name: *const ObjName, arg: *mut c_void) {
        // SAFETY: `arg` is the `Vec` below, and `name` an entry OpenSSL
        // holds for the length of the walk.
        unsafe {
            let names = &mut *arg.cast::<Vec<Vec<u8>>>();
            names.push(text((*name).name, b""));
        }
    }
    sys::init();
    let mut names: Vec<Vec<u8>> = Vec::new();
    // SAFETY: the callback only pushes to `names`, which outlives the walk.
    unsafe {
        OBJ_NAME_do_all_sorted(
            OBJ_NAME_TYPE_MD_METH,
            each,
            (&raw mut names).cast::<c_void>(),
        );
    }
    names
}

/// `OpenSSL::Random.random_bytes(length)`, `length` already a C int.
pub fn random_bytes(length: i32) -> Result<Vec<u8>, Failure> {
    let Ok(size) = usize::try_from(length) else {
        return Err(plain(
            Class::Argument,
            "negative string size (or size too big)",
        ));
    };
    sys::init();
    let mut out = vec![0; size];
    // SAFETY: `out` is `length` writable bytes.
    match unsafe { sys::RAND_bytes(out.as_mut_ptr(), length) } {
        1 => Ok(out),
        -1 => Err(failure(Class::Random, b"RAND_bytes is not supported")),
        _ => Err(failure(Class::Random, b"RAND_bytes")),
    }
}

/// `OpenSSL::Random.seed(string)`.
pub fn random_seed(data: &[u8]) {
    sys::init();
    let length = c_int::try_from(data.len()).unwrap_or(c_int::MAX);
    // SAFETY: `length` of `data`'s bytes are readable.
    unsafe { RAND_seed(data.as_ptr().cast(), length) }
}

/// `OpenSSL::Random.random_add(string, entropy)`.
pub fn random_add(data: &[u8], entropy: f64) {
    sys::init();
    let length = c_int::try_from(data.len()).unwrap_or(c_int::MAX);
    // SAFETY: `length` of `data`'s bytes are readable.
    unsafe { sys::RAND_add(data.as_ptr().cast(), length, entropy) }
}

/// `OpenSSL::Random.status?`.
#[must_use]
pub fn random_status() -> bool {
    sys::init();
    // SAFETY: no preconditions.
    unsafe { sys::RAND_status() == 1 }
}

/// `OpenSSL::Random.load_random_file(path)`: true, or the failure.
pub fn random_load_file(path: &CStr) -> Result<(), Failure> {
    sys::init();
    // SAFETY: `path` is NUL-terminated; -1 reads the whole file.
    if unsafe { RAND_load_file(path.as_ptr(), -1) } == 0 {
        return Err(failure(Class::Random, b""));
    }
    Ok(())
}

/// `OpenSSL::Random.write_random_file(path)`: true, or the failure.
pub fn random_write_file(path: &CStr) -> Result<(), Failure> {
    sys::init();
    // SAFETY: `path` is NUL-terminated.
    if unsafe { RAND_write_file(path.as_ptr()) } == -1 {
        return Err(failure(Class::Random, b""));
    }
    Ok(())
}

/// `OpenSSL.fixed_length_secure_compare` on two equal-length strings:
/// `CRYPTO_memcmp`, whose time does not depend on where they differ.
#[must_use]
pub fn secure_equal(a: &[u8], b: &[u8]) -> bool {
    debug_assert_eq!(a.len(), b.len());
    if a.len() != b.len() {
        return false;
    }
    // SAFETY: both are `a.len()` readable bytes.
    unsafe { sys::CRYPTO_memcmp(a.as_ptr().cast(), b.as_ptr().cast(), a.len()) == 0 }
}

/// `OPENSSL_VERSION`, `OPENSSL_LIBRARY_VERSION` and
/// `OPENSSL_VERSION_NUMBER`.
///
/// CRuby's first and last come from the headers it was compiled against and
/// its second from the library it runs with. Here the number is the
/// headers' (`build.rs` passes it on from `openssl-sys`), and both strings
/// are the library's, `OpenSSL_version(OPENSSL_VERSION)`: the headers'
/// `OPENSSL_VERSION_TEXT` is a C macro no binding carries. They differ only
/// when the library was upgraded under the binary.
#[must_use]
pub fn versions() -> (Vec<u8>, i64) {
    sys::init();
    // SAFETY: OpenSSL answers a static string.
    let text = unsafe { text(sys::OpenSSL_version(sys::OPENSSL_VERSION), b"") };
    let number = i64::from_str_radix(env!("SPINEL_OPENSSL_VERSION_NUMBER"), 16).unwrap_or(0);
    (text, number)
}
