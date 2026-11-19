//! `openssl` phase 2 (#107): keys, certificates, stores and TLS, on the
//! system's OpenSSL 3.
//!
//! What Ruby cannot say for `ext/openssl`'s `ossl_pkey*.c`, `ossl_x509*.c`,
//! `ossl_ssl.c` and `ossl_ssl_session.c`: the OpenSSL objects and the calls
//! on them, made in the C's order so a failure reads as it does on CRuby.
//! Everything above that is `lib/openssl/spinel/*.rb`, over one primitive,
//! `__openssl_tls__(op, *args)`, which `interp.rs` turns into [`call`]: an
//! operation's name, its [`Arg`]s, and an [`Out`] or a [`Failure`].
//!
//! Each object lives in its heap's registry ([`Contexts`]) under an id that
//! is never handed out twice, as phase 1's digest contexts do, and holds one
//! reference of OpenSSL's own counting. Sharing is OpenSSL's: a context given
//! a certificate takes a reference of its own, so ids can be freed in any
//! order. Names and extensions have no id: Ruby keeps their DER, and each
//! call decodes it, as `X509_NAME` and `X509_EXTENSION` are values.
//!
//! A TLS connection never blocks here. The socket is nonblocking and every
//! call answers OpenSSL's `WANT_READ` or `WANT_WRITE` as `:wait_readable` or
//! `:wait_writable`, and Ruby waits on the IO, so other threads run.
//!
//! The one place OpenSSL asks the program a question mid-handshake that
//! Ruby must answer is certificate verification: CRuby's verify callback
//! runs `OpenSSL::SSL.verify_certificate_identity` and the context's
//! `verify_callback`. Ruby cannot run inside an OpenSSL callback here, so
//! the callback suspends the handshake with `SSL_set_retry_verify` (OpenSSL
//! 3.0), answers `:verify` with the question, and Ruby's answer is replayed
//! when OpenSSL verifies the chain again on the next call ([`Replay`]).
//! `X509::Store#verify` does the same by verifying again from the start.

#![allow(clippy::too_many_lines)]

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::ptr::{self, NonNull};

use openssl_sys as sys;

use super::ffi;
use super::{Class, Contexts, Failure, drain_errors, failure, plain, text};

/// An argument, as `interp.rs` reads it from a Ruby value.
#[derive(Debug, Clone)]
pub enum Arg {
    Nil,
    Bool(bool),
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Arg>),
}

/// An answer, as `interp.rs` makes a Ruby value of it.
#[derive(Debug, Clone)]
pub enum Out {
    Nil,
    Bool(bool),
    Int(i64),
    /// A BINARY String.
    Bytes(Vec<u8>),
    /// A UTF-8 String.
    Text(Vec<u8>),
    Sym(&'static str),
    List(Vec<Out>),
}

/// An OpenSSL object a heap owns one reference to.
#[derive(Debug)]
pub enum Object {
    Pkey(NonNull<sys::EVP_PKEY>),
    Cert(NonNull<sys::X509>),
    Store(NonNull<sys::X509_STORE>),
    StoreCtx(Box<StoreCtx>),
    Ctx(NonNull<sys::SSL_CTX>),
    Ssl(Box<SslState>),
    Session(NonNull<sys::SSL_SESSION>),
}

impl Drop for Object {
    fn drop(&mut self) {
        // SAFETY: each is the one reference this object holds, freed once.
        unsafe {
            match self {
                Object::Pkey(p) => ffi::EVP_PKEY_free(p.as_ptr()),
                Object::Cert(p) => ffi::X509_free(p.as_ptr()),
                Object::Store(p) => ffi::X509_STORE_free(p.as_ptr()),
                Object::Ctx(p) => ffi::SSL_CTX_free(p.as_ptr()),
                Object::Session(p) => ffi::SSL_SESSION_free(p.as_ptr()),
                Object::StoreCtx(_) | Object::Ssl(_) => {}
            }
        }
    }
}

/// A verification's question for Ruby: what OpenSSL's verify callback was
/// given at the point Ruby must decide.
#[derive(Debug, Clone)]
pub struct Question {
    preverify_ok: bool,
    error: c_int,
    depth: c_int,
    current: Option<Vec<u8>>,
    chain: Vec<Vec<u8>>,
}

/// Ruby's answers to one verification's questions, in the order OpenSSL
/// asks them. OpenSSL verifies a chain the same way each time it is given
/// it, so the `n`th call of the callback that needs Ruby is the `n`th
/// question every time; a verification that reaches a question with no
/// answer yet stops there and hands it over.
#[derive(Debug, Default)]
pub struct Replay {
    answers: Vec<(bool, c_int)>,
    next: usize,
    question: Option<Question>,
}

impl Replay {
    /// The callback's answer for this call, if Ruby gave one; otherwise the
    /// question is kept for Ruby and `None` says to stop.
    ///
    /// # Safety
    ///
    /// `ctx` is the live context the callback was given.
    unsafe fn consult(&mut self, ok: c_int, ctx: *mut sys::X509_STORE_CTX) -> Option<c_int> {
        let index = self.next;
        self.next += 1;
        if let Some(&(answer, error)) = self.answers.get(index) {
            // SAFETY: the caller's contract.
            unsafe { ffi::X509_STORE_CTX_set_error(ctx, error) };
            return Some(c_int::from(answer));
        }
        if self.question.is_none() {
            // SAFETY: the caller's contract; certificates the context
            // answers are its own, copied here as DER.
            let question = unsafe {
                let current = ffi::X509_STORE_CTX_get_current_cert(ctx);
                Question {
                    preverify_ok: ok == 1,
                    error: ffi::X509_STORE_CTX_get_error(ctx),
                    depth: ffi::X509_STORE_CTX_get_error_depth(ctx),
                    current: (!current.is_null()).then(|| cert_der_of(current)).flatten(),
                    chain: stack_ders(ffi::X509_STORE_CTX_get0_chain(ctx)),
                }
            };
            self.question = Some(question);
        }
        None
    }

    fn take_question(&mut self) -> Option<Out> {
        let q = self.question.take()?;
        Some(Out::List(vec![
            Out::Bool(q.preverify_ok),
            Out::Int(i64::from(q.error)),
            Out::Int(i64::from(q.depth)),
            q.current.map_or(Out::Nil, Out::Bytes),
            Out::List(q.chain.into_iter().map(Out::Bytes).collect()),
        ]))
    }
}

/// An `X509_STORE_CTX` and what it was made from, so it can be made again
/// for each verification a replay needs.
#[derive(Debug)]
pub struct StoreCtx {
    ctx: NonNull<sys::X509_STORE_CTX>,
    store: NonNull<sys::X509_STORE>,
    cert: Option<NonNull<sys::X509>>,
    chain: Option<NonNull<ffi::OPENSSL_STACK>>,
    settings: Vec<(Setting, i64)>,
    replay: Replay,
    asks: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setting {
    Flags,
    Purpose,
    Trust,
    Time,
    Error,
}

impl Drop for StoreCtx {
    fn drop(&mut self) {
        // SAFETY: all of these are this context's own references.
        unsafe {
            ffi::X509_STORE_CTX_free(self.ctx.as_ptr());
            ffi::X509_STORE_free(self.store.as_ptr());
            if let Some(cert) = self.cert {
                ffi::X509_free(cert.as_ptr());
            }
            if let Some(chain) = self.chain {
                free_cert_stack(chain.as_ptr());
            }
        }
    }
}

/// An `SSL` and what Ruby has not been told yet: the question a suspended
/// verification asks, and the sessions and key log lines OpenSSL handed
/// over in callbacks.
#[derive(Debug)]
pub struct SslState {
    ssl: NonNull<sys::SSL>,
    asks: bool,
    hostname_check: bool,
    replay: Replay,
    sessions: Vec<NonNull<sys::SSL_SESSION>>,
    keylog: Vec<Vec<u8>>,
    /// A server's ClientHello question for Ruby (`servername_cb`,
    /// `alpn_select_cb`), and Ruby's answer once given.
    hello: Hello,
    /// The protocol `alpn_select_cb` chose, which OpenSSL's ALPN callback
    /// points into.
    alpn: Option<Vec<u8>>,
    /// A client's `client_cert_cb`: whether it was asked, and its answer.
    client_cert: ClientCert,
}

#[derive(Debug, Default)]
enum Hello {
    #[default]
    Unasked,
    Asked(Option<Vec<u8>>, Option<Vec<Vec<u8>>>),
    Answered(bool),
}

#[derive(Debug, Default)]
enum ClientCert {
    #[default]
    Unasked,
    Asked,
    Answered(Option<(NonNull<sys::X509>, NonNull<sys::EVP_PKEY>)>),
}

impl Drop for SslState {
    fn drop(&mut self) {
        // SAFETY: the `SSL` and the sessions are this state's own.
        unsafe {
            ffi::SSL_set_ex_data(self.ssl.as_ptr(), 0, ptr::null_mut());
            ffi::SSL_free(self.ssl.as_ptr());
            for session in self.sessions.drain(..) {
                ffi::SSL_SESSION_free(session.as_ptr());
            }
            if let ClientCert::Answered(Some((x, k))) = std::mem::take(&mut self.client_cert) {
                ffi::X509_free(x.as_ptr());
                ffi::EVP_PKEY_free(k.as_ptr());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

fn bad(what: &str) -> Failure {
    plain(Class::Type, &format!("__openssl_tls__: {what}"))
}

fn int(args: &[Arg], i: usize) -> Result<i64, Failure> {
    match args.get(i) {
        Some(Arg::Int(n)) => Ok(*n),
        _ => Err(bad("an argument that is not an Integer")),
    }
}

fn c_int_at(args: &[Arg], i: usize) -> Result<c_int, Failure> {
    c_int::try_from(int(args, i)?).map_err(|_| bad("an Integer past a C int"))
}

fn id(args: &[Arg], i: usize) -> Result<u64, Failure> {
    u64::try_from(int(args, i)?).map_err(|_| bad("an id that is negative"))
}

fn opt_id(args: &[Arg], i: usize) -> Result<Option<u64>, Failure> {
    match args.get(i) {
        None | Some(Arg::Nil) => Ok(None),
        _ => id(args, i).map(Some),
    }
}

fn bytes(args: &[Arg], i: usize) -> Result<&[u8], Failure> {
    match args.get(i) {
        Some(Arg::Bytes(b)) => Ok(b),
        _ => Err(bad("an argument that is not a String")),
    }
}

fn opt_bytes(args: &[Arg], i: usize) -> Result<Option<&[u8]>, Failure> {
    match args.get(i) {
        None | Some(Arg::Nil) => Ok(None),
        _ => bytes(args, i).map(Some),
    }
}

fn cstring(args: &[Arg], i: usize) -> Result<CString, Failure> {
    CString::new(bytes(args, i)?).map_err(|_| plain(Class::Argument, "string contains null byte"))
}

fn opt_cstring(args: &[Arg], i: usize) -> Result<Option<CString>, Failure> {
    match args.get(i) {
        None | Some(Arg::Nil) => Ok(None),
        _ => cstring(args, i).map(Some),
    }
}

fn truthy(args: &[Arg], i: usize) -> bool {
    !matches!(args.get(i), None | Some(Arg::Nil) | Some(Arg::Bool(false)))
}

fn list(args: &[Arg], i: usize) -> Result<&[Arg], Failure> {
    match args.get(i) {
        Some(Arg::List(l)) => Ok(l),
        None | Some(Arg::Nil) => Ok(&[]),
        _ => Err(bad("an argument that is not an Array")),
    }
}

// ---------------------------------------------------------------------------
// BIOs, DER and stacks
// ---------------------------------------------------------------------------

/// A read-only memory BIO over `data`, freed on drop.
struct ReadBio(*mut sys::BIO);

impl ReadBio {
    fn new(data: &[u8]) -> Result<ReadBio, Failure> {
        let len = c_int::try_from(data.len()).map_err(|_| bad("a String past a C int"))?;
        // SAFETY: `data` outlives the BIO, which only reads it.
        let bio = unsafe { ffi::BIO_new_mem_buf(data.as_ptr().cast(), len) };
        if bio.is_null() {
            return Err(failure(Class::Type, b"BIO_new_mem_buf"));
        }
        Ok(ReadBio(bio))
    }

    /// `OSSL_BIO_reset`: back to the start, the error queue cleared.
    fn reset(&self) {
        // SAFETY: a live BIO.
        unsafe {
            ffi::BIO_ctrl(self.0, ffi::BIO_CTRL_RESET, 0, ptr::null_mut());
            ffi::ERR_clear_error();
        }
    }

    fn eof(&self) -> bool {
        // SAFETY: a live BIO.
        unsafe { ffi::BIO_ctrl(self.0, ffi::BIO_CTRL_EOF, 0, ptr::null_mut()) != 0 }
    }

    fn tell(&self) -> c_long {
        // SAFETY: a live BIO.
        unsafe { ffi::BIO_ctrl(self.0, ffi::BIO_C_FILE_TELL, 0, ptr::null_mut()) }
    }
}

impl Drop for ReadBio {
    fn drop(&mut self) {
        // SAFETY: the BIO this owns.
        unsafe { ffi::BIO_free(self.0) };
    }
}

/// A writable memory BIO, whose bytes [`WriteBio::take`] answers.
struct WriteBio(*mut sys::BIO);

impl WriteBio {
    fn new() -> Result<WriteBio, Failure> {
        // SAFETY: no preconditions.
        let bio = unsafe { ffi::BIO_new(ffi::BIO_s_mem()) };
        if bio.is_null() {
            return Err(failure(Class::Type, b"BIO_new"));
        }
        Ok(WriteBio(bio))
    }

    fn take(&self) -> Vec<u8> {
        let mut data: *mut c_char = ptr::null_mut();
        // SAFETY: a live memory BIO; `BIO_get_mem_data` points into it.
        unsafe {
            let len = ffi::BIO_ctrl(self.0, ffi::BIO_CTRL_INFO, 0, (&raw mut data).cast());
            match usize::try_from(len) {
                Ok(len) if !data.is_null() => {
                    std::slice::from_raw_parts(data.cast::<u8>(), len).to_vec()
                }
                _ => Vec::new(),
            }
        }
    }
}

impl Drop for WriteBio {
    fn drop(&mut self) {
        // SAFETY: the BIO this owns.
        unsafe { ffi::BIO_free(self.0) };
    }
}

/// `OPENSSL_free`.
///
/// # Safety
///
/// `p` was allocated by OpenSSL, and is freed once.
unsafe fn openssl_free(p: *mut c_void) {
    // SAFETY: the caller's contract.
    unsafe { ffi::CRYPTO_free(p, c"spinel".as_ptr(), 0) };
}

/// The DER an `i2d_*` function writes for `x`.
///
/// # Safety
///
/// `i2d` is the encoder for `x`'s type, and `x` is live.
unsafe fn der_of<T>(
    x: *const T,
    i2d: unsafe extern "C" fn(*const T, *mut *mut u8) -> c_int,
) -> Option<Vec<u8>> {
    // SAFETY: the caller's contract; OpenSSL allocates `out` for us.
    unsafe {
        let mut out: *mut u8 = ptr::null_mut();
        let len = i2d(x, &mut out);
        let len = usize::try_from(len).ok()?;
        if out.is_null() {
            return None;
        }
        let der = std::slice::from_raw_parts(out, len).to_vec();
        openssl_free(out.cast());
        Some(der)
    }
}

/// A certificate's DER.
///
/// # Safety
///
/// `x` is live.
unsafe fn cert_der_of(x: *const sys::X509) -> Option<Vec<u8>> {
    // SAFETY: the caller's contract.
    unsafe { der_of(x, ffi::i2d_X509) }
}

/// The DER of each certificate in a `STACK_OF(X509)`, which may be null.
///
/// # Safety
///
/// `stack` is null or a live stack of certificates.
unsafe fn stack_ders(stack: *mut ffi::OPENSSL_STACK) -> Vec<Vec<u8>> {
    if stack.is_null() {
        return Vec::new();
    }
    // SAFETY: the caller's contract.
    unsafe {
        let n = ffi::OPENSSL_sk_num(stack);
        (0..n)
            .filter_map(|i| cert_der_of(ffi::OPENSSL_sk_value(stack, i).cast()))
            .collect()
    }
}

/// Free a `STACK_OF(X509)` this code made, and the reference to each
/// certificate it holds.
///
/// # Safety
///
/// `stack` is a live stack whose certificates each hold one reference.
unsafe fn free_cert_stack(stack: *mut ffi::OPENSSL_STACK) {
    // SAFETY: the caller's contract.
    unsafe {
        for i in 0..ffi::OPENSSL_sk_num(stack) {
            ffi::X509_free(ffi::OPENSSL_sk_value(stack, i).cast());
        }
        ffi::OPENSSL_sk_free(stack);
    }
}

/// A big number's hex, `-` first when negative, as `BN_bn2hex` writes it.
///
/// # Safety
///
/// `bn` is live.
unsafe fn bn_hex(bn: *const sys::BIGNUM) -> Vec<u8> {
    // SAFETY: the caller's contract; the string is OpenSSL's to free.
    unsafe {
        let hex = ffi::BN_bn2hex(bn);
        if hex.is_null() {
            return b"0".to_vec();
        }
        let out = CStr::from_ptr(hex).to_bytes().to_vec();
        openssl_free(hex.cast());
        out
    }
}

/// A big number from hex, `-` first when negative.
fn bn_from_hex(hex: &[u8]) -> Result<*mut sys::BIGNUM, Failure> {
    let hex = CString::new(hex).map_err(|_| bad("a number with a NUL in it"))?;
    let mut bn = ptr::null_mut();
    // SAFETY: `hex` is NUL-terminated; OpenSSL allocates `bn`.
    if unsafe { ffi::BN_hex2bn(&mut bn, hex.as_ptr()) } == 0 || bn.is_null() {
        return Err(failure(Class::Bn, b"BN_hex2bn"));
    }
    Ok(bn)
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

impl Contexts {
    fn add(&mut self, object: Object) -> u64 {
        let id = self.next;
        self.next += 1;
        self.objects.insert(id, object);
        id
    }

    fn object(&self, id: u64) -> Option<&Object> {
        self.objects.get(&id)
    }

    fn pkey(&self, id: u64) -> Result<*mut sys::EVP_PKEY, Failure> {
        match self.object(id) {
            Some(Object::Pkey(p)) => Ok(p.as_ptr()),
            _ => Err(plain(Class::PKey, "no key")),
        }
    }

    fn cert(&self, id: u64) -> Result<*mut sys::X509, Failure> {
        match self.object(id) {
            Some(Object::Cert(p)) => Ok(p.as_ptr()),
            _ => Err(plain(Class::Cert, "no certificate")),
        }
    }

    fn store(&self, id: u64) -> Result<*mut sys::X509_STORE, Failure> {
        match self.object(id) {
            Some(Object::Store(p)) => Ok(p.as_ptr()),
            _ => Err(plain(Class::Store, "no store")),
        }
    }

    fn store_ctx(&mut self, id: u64) -> Result<&mut StoreCtx, Failure> {
        match self.objects.get_mut(&id) {
            Some(Object::StoreCtx(c)) => Ok(c),
            _ => Err(plain(Class::Store, "no store context")),
        }
    }

    fn ctx(&self, id: u64) -> Result<*mut sys::SSL_CTX, Failure> {
        match self.object(id) {
            Some(Object::Ctx(p)) => Ok(p.as_ptr()),
            _ => Err(plain(Class::Ssl, "no context")),
        }
    }

    fn ssl(&mut self, id: u64) -> Result<&mut SslState, Failure> {
        match self.objects.get_mut(&id) {
            Some(Object::Ssl(s)) => Ok(s),
            _ => Err(plain(Class::Ssl, "no SSL")),
        }
    }

    fn session(&self, id: u64) -> Result<*mut sys::SSL_SESSION, Failure> {
        match self.object(id) {
            Some(Object::Session(p)) => Ok(p.as_ptr()),
            _ => Err(plain(Class::Session, "no session")),
        }
    }

    /// A new id for a key this code holds one reference to.
    fn add_pkey(&mut self, pkey: *mut sys::EVP_PKEY) -> Out {
        match NonNull::new(pkey) {
            // SAFETY: a live key.
            Some(p) => {
                let base = unsafe { ffi::EVP_PKEY_get_base_id(pkey) };
                let id = self.add(Object::Pkey(p));
                Out::List(vec![id_out(id), Out::Int(i64::from(base))])
            }
            None => Out::Nil,
        }
    }

    /// A new id for a copy of `x509`, as `ossl_x509_new` copies one.
    ///
    /// # Safety
    ///
    /// `x509` is null or live.
    unsafe fn add_cert_copy(&mut self, x509: *const sys::X509) -> Result<Out, Failure> {
        if x509.is_null() {
            return Ok(Out::Nil);
        }
        // SAFETY: the caller's contract.
        let copy = unsafe { ffi::X509_dup(x509) };
        let copy = NonNull::new(copy).ok_or_else(|| failure(Class::Cert, b""))?;
        Ok(id_out(self.add(Object::Cert(copy))))
    }
}

fn id_out(id: u64) -> Out {
    Out::Int(i64::try_from(id).unwrap_or(i64::MAX))
}

/// One operation of `ext/openssl`'s C half: see each function for what it
/// takes and answers.
pub fn call(contexts: &mut Contexts, op: &[u8], args: &[Arg]) -> Result<Out, Failure> {
    sys::init();
    let out = match op {
        b"obj" => obj(args)?,
        b"bn_prime" => bn_prime(args)?,
        b"bn_generate_prime" => bn_generate_prime(args)?,
        b"defaults" => defaults(),
        b"verify_error_string" => {
            // SAFETY: OpenSSL answers a static string.
            let s = unsafe { ffi::X509_verify_cert_error_string(c_long::from(c_int_at(args, 0)?)) };
            // SAFETY: static or null.
            Out::Text(unsafe { text(s, b"(null)") })
        }

        b"pkey_read" => pkey_read(contexts, args, false)?,
        b"rsa_read" => pkey_read(contexts, args, true)?,
        b"pkey_generate" => pkey_generate(contexts, args)?,
        b"pkey_info" => pkey_info(contexts, args)?,
        b"pkey_bn" => pkey_bn(contexts, args)?,
        b"pkey_export" => pkey_export(contexts, args)?,
        b"pkey_sign" => pkey_sign(contexts, args)?,
        b"pkey_verify" => pkey_verify(contexts, args)?,
        b"pkey_compare" => pkey_compare(contexts, args)?,
        b"pkey_text" => pkey_text(contexts, args)?,

        b"cert_new" => {
            // SAFETY: no preconditions.
            let x = NonNull::new(unsafe { ffi::X509_new() })
                .ok_or_else(|| failure(Class::Cert, b"X509_new"))?;
            id_out(contexts.add(Object::Cert(x)))
        }
        b"cert_read" => cert_read(contexts, args)?,
        b"cert_load" => cert_load(contexts, args)?,
        b"cert_dup" => {
            let x = contexts.cert(id(args, 0)?)?;
            // SAFETY: a live certificate.
            unsafe { contexts.add_cert_copy(x)? }
        }
        b"cert_get" => cert_get(contexts, args)?,
        b"cert_set" => cert_set(contexts, args)?,
        b"cert_sign" => cert_sign(contexts, args)?,
        b"cert_verify" => cert_verify(contexts, args)?,
        b"cert_check_private_key" => {
            let (x, k) = (contexts.cert(id(args, 0)?)?, contexts.pkey(id(args, 1)?)?);
            // SAFETY: live objects.
            let ok = unsafe { ffi::X509_check_private_key(x, k) } == 1;
            if !ok {
                drain_errors();
            }
            Out::Bool(ok)
        }
        b"cert_eq" => {
            let (a, b) = (contexts.cert(id(args, 0)?)?, contexts.cert(id(args, 1)?)?);
            // SAFETY: live certificates.
            Out::Bool(unsafe { ffi::X509_cmp(a, b) } == 0)
        }
        b"cert_extensions" => cert_extensions(contexts, args)?,
        b"cert_add_extension" => cert_add_extension(contexts, args)?,
        b"cert_clear_extensions" => {
            let x = contexts.cert(id(args, 0)?)?;
            // SAFETY: a live certificate; each deleted extension is ours.
            unsafe {
                loop {
                    let ext = ffi::X509_delete_ext(x, 0);
                    if ext.is_null() {
                        break;
                    }
                    ffi::X509_EXTENSION_free(ext);
                }
            }
            Out::Nil
        }

        b"name_new" => name_new()?,
        b"name_check" => {
            Name::decode(bytes(args, 0)?, Class::Name)?;
            Out::Nil
        }
        b"name_add" => name_add(args)?,
        b"name_to_s" => name_to_s(args)?,
        b"name_to_a" => name_to_a(args)?,
        b"name_cmp" => {
            let (a, b) = (
                Name::decode(bytes(args, 0)?, Class::Name)?,
                Name::decode(bytes(args, 1)?, Class::Name)?,
            );
            // SAFETY: two live names.
            Out::Int(i64::from(unsafe { ffi::X509_NAME_cmp(a.0, b.0) }))
        }
        b"name_hash" => {
            let name = Name::decode(bytes(args, 0)?, Class::Name)?;
            // SAFETY: a live name.
            let hash = unsafe {
                if truthy(args, 1) {
                    ffi::X509_NAME_hash_old(name.0)
                } else {
                    ffi::X509_NAME_hash_ex(name.0, ptr::null_mut(), ptr::null(), ptr::null_mut())
                }
            };
            Out::Int(i64::try_from(hash).unwrap_or(i64::MAX))
        }

        b"ext_new" => ext_new(args)?,
        b"ext_info" => ext_info(args)?,
        b"ext_create" => ext_create(contexts, args)?,

        b"store_new" => {
            // SAFETY: no preconditions.
            let s = NonNull::new(unsafe { ffi::X509_STORE_new() })
                .ok_or_else(|| failure(Class::Store, b"X509_STORE_new"))?;
            id_out(contexts.add(Object::Store(s)))
        }
        b"store_op" => store_op(contexts, args)?,
        b"stctx_new" => stctx_new(contexts, args)?,
        b"stctx_verify" => stctx_verify(contexts, args)?,
        b"stctx_get" => stctx_get(contexts, args)?,
        b"stctx_set" => stctx_set(contexts, args)?,

        b"ctx_new" => ctx_new(contexts)?,
        b"ctx_get" => ctx_get(contexts, args)?,
        b"ctx_set" => ctx_set(contexts, args)?,
        b"ctx_setup" => ctx_setup(contexts, args)?,
        b"ctx_add_certificate" => ctx_add_certificate(contexts, args)?,
        b"ctx_session" => ctx_session(contexts, args)?,

        b"ssl_new" => ssl_new(contexts, args)?,
        b"ssl_setup" => ssl_setup(contexts, args)?,
        b"ssl_hostname" => ssl_hostname(contexts, args)?,
        b"ssl_handshake" => ssl_handshake(contexts, args)?,
        b"ssl_answer" => {
            let (ok, error) = (truthy(args, 1), c_int_at(args, 2)?);
            contexts.ssl(id(args, 0)?)?.replay.answers.push((ok, error));
            Out::Nil
        }
        b"ssl_hello_answer" => ssl_hello_answer(contexts, args)?,
        b"ssl_client_cert" => ssl_client_cert(contexts, args)?,
        b"ssl_read" => ssl_read(contexts, args)?,
        b"ssl_write" => ssl_write(contexts, args)?,
        b"ssl_shutdown" => ssl_shutdown(contexts, args)?,
        b"ssl_get" => ssl_get(contexts, args)?,
        b"ssl_set_session" => {
            let session = contexts.session(id(args, 1)?)?;
            let ssl = contexts.ssl(id(args, 0)?)?.ssl.as_ptr();
            // SAFETY: live objects; the `SSL` takes a reference of its own.
            if unsafe { ffi::SSL_set_session(ssl, session) } != 1 {
                return Err(failure(Class::Ssl, b"SSL_set_session"));
            }
            Out::Nil
        }
        b"ssl_take" => ssl_take(contexts, args)?,
        b"ssl_export_keying_material" => ssl_export_keying_material(contexts, args)?,

        b"session_read" => session_read(contexts, args)?,
        b"session_get" => session_get(contexts, args)?,
        b"session_set" => session_set(contexts, args)?,
        _ => return Err(bad("an unknown operation")),
    };
    Ok(out)
}

// ---------------------------------------------------------------------------
// Objects, numbers and defaults
// ---------------------------------------------------------------------------

/// `obj(text, message or nil)`: `[nid, short name, long name, dotted
/// OID]` for what `OBJ_txt2obj` makes of a name or a dotted OID; the names
/// are nil for an OID OpenSSL has no name for. When it makes nothing, nil,
/// or with a message an `ASN1Error` of it.
fn obj(args: &[Arg]) -> Result<Out, Failure> {
    let name = cstring(args, 0)?;
    // SAFETY: `name` is NUL-terminated; the object is ours to free, and the
    // names OpenSSL answers for a nid are static.
    unsafe {
        let o = ffi::OBJ_txt2obj(name.as_ptr(), 0);
        if o.is_null() {
            if let Some(message) = opt_bytes(args, 1)? {
                return Err(failure(Class::Asn1, message));
            }
            ffi::ERR_clear_error();
            return Ok(Out::Nil);
        }
        let nid = ffi::OBJ_obj2nid(o);
        let mut buf = [0 as c_char; 128];
        let len = ffi::OBJ_obj2txt(buf.as_mut_ptr(), 128, o, 1);
        let dotted = if len > 0 {
            CStr::from_ptr(buf.as_ptr()).to_bytes().to_vec()
        } else {
            Vec::new()
        };
        ffi::ASN1_OBJECT_free(o);
        let named = |p: *const c_char| {
            if p.is_null() {
                Out::Nil
            } else {
                Out::Text(CStr::from_ptr(p).to_bytes().to_vec())
            }
        };
        let (sn, ln) = if nid == 0 {
            (Out::Nil, Out::Nil)
        } else {
            (named(ffi::OBJ_nid2sn(nid)), named(ffi::OBJ_nid2ln(nid)))
        };
        Ok(Out::List(vec![
            Out::Int(i64::from(nid)),
            sn,
            ln,
            Out::Text(dotted),
        ]))
    }
}

/// `bn_prime(hex)`: `BN_check_prime`, true or false.
fn bn_prime(args: &[Arg]) -> Result<Out, Failure> {
    let bn = bn_from_hex(bytes(args, 0)?)?;
    // SAFETY: a live number, freed once.
    unsafe {
        let r = ffi::BN_check_prime(bn, ptr::null_mut(), ptr::null_mut());
        ffi::BN_free(bn);
        if r < 0 {
            return Err(failure(Class::Bn, b""));
        }
        Ok(Out::Bool(r == 1))
    }
}

/// `bn_generate_prime(bits, safe)`: a prime's hex.
fn bn_generate_prime(args: &[Arg]) -> Result<Out, Failure> {
    let (bits, safe) = (c_int_at(args, 0)?, truthy(args, 1));
    // SAFETY: a new number, freed once.
    unsafe {
        let bn = ffi::BN_new();
        if bn.is_null()
            || ffi::BN_generate_prime_ex(
                bn,
                bits,
                c_int::from(safe),
                ptr::null(),
                ptr::null(),
                ptr::null_mut(),
            ) != 1
        {
            ffi::BN_free(bn);
            return Err(failure(Class::Bn, b""));
        }
        let hex = bn_hex(bn);
        ffi::BN_free(bn);
        Ok(Out::Text(hex))
    }
}

/// `defaults`: `X509::DEFAULT_CERT_FILE` and its kin, as the library says.
fn defaults() -> Out {
    // SAFETY: OpenSSL answers static strings.
    unsafe {
        Out::List(
            [
                ffi::X509_get_default_cert_file(),
                ffi::X509_get_default_cert_dir(),
                ffi::X509_get_default_cert_file_env(),
                ffi::X509_get_default_cert_dir_env(),
                ffi::X509_get_default_cert_area(),
                ffi::X509_get_default_private_dir(),
            ]
            .into_iter()
            .map(|p| Out::Text(text(p, b"")))
            .collect(),
        )
    }
}

// ---------------------------------------------------------------------------
// Keys (ossl_pkey.c, ossl_pkey_rsa.c)
// ---------------------------------------------------------------------------

/// `ossl_pkey_read_generic`: DER first, then PEM, a private key's formats
/// before a public key's.
fn read_generic(bio: &ReadBio, pass: Option<&[u8]>) -> *mut sys::EVP_PKEY {
    let mut pkey: *mut sys::EVP_PKEY = ptr::null_mut();
    // SAFETY: the decoder writes `pkey` and reads the BIO, both live for the
    // call; the passphrase is copied by OpenSSL.
    unsafe {
        let dctx = ffi::OSSL_DECODER_CTX_new_for_pkey(
            &mut pkey,
            c"DER".as_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            ptr::null_mut(),
            ptr::null(),
        );
        if dctx.is_null() {
            return ptr::null_mut();
        }
        // ponytail: with no passphrase CRuby asks for one on the terminal
        // (`PEM_def_callback`); here an encrypted key with none given does
        // not decode. Upgrade: a passphrase callback reading the console.
        if let Some(pass) = pass {
            ffi::OSSL_DECODER_CTX_set_passphrase(dctx, pass.as_ptr(), pass.len());
        }
        let done = 'decode: {
            if ffi::OSSL_DECODER_from_bio(dctx, bio.0) == 1 {
                break 'decode true;
            }
            bio.reset();
            if ffi::OSSL_DECODER_CTX_set_input_type(dctx, c"PEM".as_ptr()) != 1 {
                break 'decode false;
            }
            for selection in [ffi::EVP_PKEY_KEYPAIR, 0] {
                if ffi::OSSL_DECODER_CTX_set_selection(dctx, selection) != 1 {
                    break 'decode false;
                }
                let mut pos = 0;
                loop {
                    if ffi::OSSL_DECODER_from_bio(dctx, bio.0) == 1 {
                        break 'decode true;
                    }
                    if bio.eof() {
                        break;
                    }
                    let pos2 = bio.tell();
                    if pos2 < 0 || pos2 <= pos {
                        break;
                    }
                    ffi::ERR_clear_error();
                    pos = pos2;
                }
                bio.reset();
            }
            false
        };
        let _ = done;
        bio.reset();
        ffi::OSSL_DECODER_CTX_free(dctx);
    }
    pkey
}

/// `pkey_read(data, pass)`: `OpenSSL::PKey.read`, `[id, base id]`.
/// `rsa_read` is `PKey::RSA.new(data, pass)`, which tries the RSAPublicKey
/// forms first and refuses another kind of key.
fn pkey_read(contexts: &mut Contexts, args: &[Arg], rsa: bool) -> Result<Out, Failure> {
    let data = bytes(args, 0)?;
    let pass = opt_bytes(args, 1)?;
    let bio = ReadBio::new(data)?;
    let mut pkey = ptr::null_mut();
    if rsa {
        // SAFETY: a live BIO; an RSA key made here goes to a new EVP_PKEY.
        unsafe {
            let mut key = ffi::d2i_RSAPublicKey_bio(bio.0, ptr::null_mut());
            bio.reset();
            if key.is_null() {
                key = ffi::PEM_read_bio_RSAPublicKey(
                    bio.0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                );
                bio.reset();
            }
            if !key.is_null() {
                pkey = ffi::EVP_PKEY_new();
                if pkey.is_null() || ffi::EVP_PKEY_assign(pkey, ffi::EVP_PKEY_RSA, key.cast()) != 1
                {
                    ffi::RSA_free(key);
                    ffi::EVP_PKEY_free(pkey);
                    return Err(failure(Class::PKey, b"EVP_PKEY_assign_RSA"));
                }
            }
        }
    }
    if pkey.is_null() {
        pkey = read_generic(&bio, pass);
    }
    if pkey.is_null() {
        drain_errors();
        return Err(plain(
            Class::PKey,
            if rsa {
                "Neither PUB key nor PRIV key"
            } else {
                "Could not parse PKey"
            },
        ));
    }
    if rsa {
        // SAFETY: a live key.
        let base = unsafe { ffi::EVP_PKEY_get_base_id(pkey) };
        if base != ffi::EVP_PKEY_RSA {
            // SAFETY: the key this made, freed once; the name is static.
            let name = unsafe {
                ffi::EVP_PKEY_free(pkey);
                text(ffi::OBJ_nid2sn(base), b"")
            };
            let mut message = b"incorrect pkey type: ".to_vec();
            message.extend_from_slice(&name);
            return Err(Failure {
                class: Class::PKey,
                message,
                errors: Vec::new(),
            });
        }
    }
    Ok(contexts.add_pkey(pkey))
}

/// A String as `%+PRIsVALUE` writes it, for the plain ASCII an option is.
fn quoted(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    for &b in bytes {
        match b {
            b'"' | b'\\' => {
                out.push('\\');
                out.push(char::from(b));
            }
            0x20..=0x7e => out.push(char::from(b)),
            _ => out.push_str(&format!("\\x{b:02X}")),
        }
    }
    out.push('"');
    out
}

/// `pkey_generate(algorithm or key id, [[key, value]...], parameters?)`:
/// `OpenSSL::PKey.generate_key` and `generate_parameters`.
fn pkey_generate(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let params = truthy(args, 2);
    // SAFETY: the context made here is freed on every path; options are
    // NUL-terminated.
    unsafe {
        let ctx = match args.first() {
            Some(Arg::Int(_)) => {
                let base = contexts.pkey(id(args, 0)?)?;
                let ctx = ffi::EVP_PKEY_CTX_new(base, ptr::null_mut());
                if ctx.is_null() {
                    return Err(failure(Class::PKey, b"EVP_PKEY_CTX_new"));
                }
                ctx
            }
            _ => {
                let name = cstring(args, 0)?;
                let ctx =
                    ffi::EVP_PKEY_CTX_new_from_name(ptr::null_mut(), name.as_ptr(), ptr::null());
                if ctx.is_null() {
                    return Err(failure(Class::PKey, b"EVP_PKEY_CTX_new_from_name"));
                }
                ctx
            }
        };
        let init = if params {
            ffi::EVP_PKEY_paramgen_init(ctx)
        } else {
            ffi::EVP_PKEY_keygen_init(ctx)
        };
        if init <= 0 {
            ffi::EVP_PKEY_CTX_free(ctx);
            return Err(failure(
                Class::PKey,
                if params {
                    b"EVP_PKEY_paramgen_init"
                } else {
                    b"EVP_PKEY_keygen_init"
                },
            ));
        }
        for option in list(args, 1)? {
            let Arg::List(pair) = option else { continue };
            let (Ok(key), Ok(value)) = (cstring(pair, 0), cstring(pair, 1)) else {
                ffi::EVP_PKEY_CTX_free(ctx);
                return Err(bad("an option that is not two Strings"));
            };
            if ffi::EVP_PKEY_CTX_ctrl_str(ctx, key.as_ptr(), value.as_ptr()) <= 0 {
                ffi::EVP_PKEY_CTX_free(ctx);
                let message = format!(
                    "EVP_PKEY_CTX_ctrl_str(ctx, {}, {})",
                    quoted(key.as_bytes()),
                    quoted(value.as_bytes())
                );
                return Err(failure(Class::PKey, message.as_bytes()));
            }
        }
        let mut pkey = ptr::null_mut();
        let made = if params {
            ffi::EVP_PKEY_paramgen(ctx, &mut pkey)
        } else {
            ffi::EVP_PKEY_keygen(ctx, &mut pkey)
        };
        ffi::EVP_PKEY_CTX_free(ctx);
        if made <= 0 || pkey.is_null() {
            return Err(failure(
                Class::PKey,
                if params {
                    b"EVP_PKEY_paramgen"
                } else {
                    b"EVP_PKEY_keygen"
                },
            ));
        }
        Ok(contexts.add_pkey(pkey))
    }
}

/// `pkey_info(id)`: `[base id, oid, private?]`.
fn pkey_info(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let pkey = contexts.pkey(id(args, 0)?)?;
    // SAFETY: a live key; names OpenSSL answers are static or the key's.
    unsafe {
        let base = ffi::EVP_PKEY_get_base_id(pkey);
        let nid = ffi::EVP_PKEY_get_id(pkey);
        let oid = if nid == 0 {
            text(ffi::EVP_PKEY_get0_type_name(pkey), b"")
        } else {
            text(ffi::OBJ_nid2sn(nid), b"")
        };
        Ok(Out::List(vec![
            Out::Int(i64::from(base)),
            Out::Text(oid),
            Out::Bool(is_private(pkey)),
        ]))
    }
}

/// Whether a key has its private half: a private component OpenSSL can
/// hand out, by whichever name the key's type gives it.
///
/// # Safety
///
/// `pkey` is live.
unsafe fn is_private(pkey: *mut sys::EVP_PKEY) -> bool {
    for name in [c"d", c"priv"] {
        let mut bn = ptr::null_mut();
        // SAFETY: the caller's contract; the number made is freed here.
        unsafe {
            if ffi::EVP_PKEY_get_bn_param(pkey, name.as_ptr(), &mut bn) == 1 {
                ffi::BN_free(bn);
                return true;
            }
        }
    }
    // SAFETY: no preconditions.
    unsafe { ffi::ERR_clear_error() };
    false
}

/// `pkey_bn(id, name)`: a key's component's hex, or nil.
fn pkey_bn(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let pkey = contexts.pkey(id(args, 0)?)?;
    let name = cstring(args, 1)?;
    let mut bn = ptr::null_mut();
    // SAFETY: a live key; the number made is freed here.
    unsafe {
        if ffi::EVP_PKEY_get_bn_param(pkey, name.as_ptr(), &mut bn) != 1 {
            ffi::ERR_clear_error();
            return Ok(Out::Nil);
        }
        let hex = bn_hex(bn);
        ffi::BN_free(bn);
        Ok(Out::Text(hex))
    }
}

/// `pkey_export(id, form, der?)`: form 0 is the traditional private key,
/// 1 PKCS #8, 2 SubjectPublicKeyInfo.
fn pkey_export(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let pkey = contexts.pkey(id(args, 0)?)?;
    let form = int(args, 1)?;
    let der = truthy(args, 2);
    let bio = WriteBio::new()?;
    let n = ptr::null_mut();
    // SAFETY: a live key and BIO; no cipher, so no passphrase is asked for.
    let (ok, name): (bool, &[u8]) = unsafe {
        match (form, der) {
            (0, true) => (
                ffi::i2d_PrivateKey_bio(bio.0, pkey) == 1,
                b"i2d_PrivateKey_bio",
            ),
            (0, false) => (
                ffi::PEM_write_bio_PrivateKey_traditional(
                    bio.0,
                    pkey,
                    ptr::null(),
                    ptr::null(),
                    0,
                    n,
                    n,
                ) == 1,
                b"PEM_write_bio_PrivateKey_traditional",
            ),
            (1, true) => (
                ffi::i2d_PKCS8PrivateKey_bio(bio.0, pkey, ptr::null(), ptr::null(), 0, n, n) == 1,
                b"i2d_PKCS8PrivateKey_bio",
            ),
            (1, false) => (
                ffi::PEM_write_bio_PKCS8PrivateKey(bio.0, pkey, ptr::null(), ptr::null(), 0, n, n)
                    == 1,
                b"PEM_write_bio_PKCS8PrivateKey",
            ),
            (_, true) => (ffi::i2d_PUBKEY_bio(bio.0, pkey) == 1, b"i2d_PUBKEY_bio"),
            (_, false) => (
                ffi::PEM_write_bio_PUBKEY(bio.0, pkey) == 1,
                b"PEM_write_bio_PUBKEY",
            ),
        }
    };
    if !ok {
        return Err(failure(Class::PKey, name));
    }
    let out = bio.take();
    Ok(if der { Out::Bytes(out) } else { Out::Text(out) })
}

/// The digest a name names, or null for nil: `ossl_evp_md_fetch`, short.
fn md_of(args: &[Arg], i: usize) -> Result<*const sys::EVP_MD, Failure> {
    let Some(name) = opt_cstring(args, i)? else {
        return Ok(ptr::null());
    };
    // SAFETY: `name` is NUL-terminated.
    let md = unsafe { ffi::EVP_get_digestbyname(name.as_ptr()) };
    if md.is_null() {
        let mut message = b"unsupported digest algorithm: ".to_vec();
        message.extend_from_slice(name.as_bytes());
        return Err(failure(Class::Digest, &message));
    }
    Ok(md)
}

/// Apply `[[key, value]...]` to a key context, `EVP_PKEY_CTX_ctrl_str`.
///
/// # Safety
///
/// `pctx` is live.
unsafe fn apply_options(pctx: *mut sys::EVP_PKEY_CTX, options: &[Arg]) -> Result<(), Failure> {
    for option in options {
        let Arg::List(pair) = option else { continue };
        let (key, value) = (cstring(pair, 0)?, cstring(pair, 1)?);
        // SAFETY: the caller's contract; both are NUL-terminated.
        if unsafe { ffi::EVP_PKEY_CTX_ctrl_str(pctx, key.as_ptr(), value.as_ptr()) } <= 0 {
            let message = format!(
                "EVP_PKEY_CTX_ctrl_str(ctx, {}, {})",
                quoted(key.as_bytes()),
                quoted(value.as_bytes())
            );
            return Err(failure(Class::PKey, message.as_bytes()));
        }
    }
    Ok(())
}

/// `pkey_sign(id, digest or nil, data, options)`: `PKey#sign`.
fn pkey_sign(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let pkey = contexts.pkey(id(args, 0)?)?;
    let md = md_of(args, 1)?;
    let data = bytes(args, 2)?;
    // SAFETY: a live key; the context made here is freed on every path.
    unsafe {
        let ctx = ffi::EVP_MD_CTX_new();
        let mut pctx = ptr::null_mut();
        if ctx.is_null() || ffi::EVP_DigestSignInit(ctx, &mut pctx, md, ptr::null_mut(), pkey) != 1
        {
            ffi::EVP_MD_CTX_free(ctx);
            return Err(failure(Class::PKey, b"EVP_DigestSignInit"));
        }
        if let Err(e) = apply_options(pctx, list(args, 3)?) {
            ffi::EVP_MD_CTX_free(ctx);
            return Err(e);
        }
        let mut len = 0;
        if ffi::EVP_DigestSign(ctx, ptr::null_mut(), &mut len, data.as_ptr(), data.len()) != 1 {
            ffi::EVP_MD_CTX_free(ctx);
            return Err(failure(Class::PKey, b"EVP_DigestSign"));
        }
        let mut sig = vec![0u8; len];
        if ffi::EVP_DigestSign(ctx, sig.as_mut_ptr(), &mut len, data.as_ptr(), data.len()) != 1 {
            ffi::EVP_MD_CTX_free(ctx);
            return Err(failure(Class::PKey, b"EVP_DigestSign"));
        }
        ffi::EVP_MD_CTX_free(ctx);
        sig.truncate(len);
        Ok(Out::Bytes(sig))
    }
}

/// `pkey_verify(id, digest or nil, signature, data, options)`.
fn pkey_verify(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let pkey = contexts.pkey(id(args, 0)?)?;
    let md = md_of(args, 1)?;
    let (sig, data) = (bytes(args, 2)?, bytes(args, 3)?);
    // SAFETY: a live key; the context made here is freed on every path.
    unsafe {
        let ctx = ffi::EVP_MD_CTX_new();
        let mut pctx = ptr::null_mut();
        if ctx.is_null()
            || ffi::EVP_DigestVerifyInit(ctx, &mut pctx, md, ptr::null_mut(), pkey) != 1
        {
            ffi::EVP_MD_CTX_free(ctx);
            return Err(failure(Class::PKey, b"EVP_DigestVerifyInit"));
        }
        if let Err(e) = apply_options(pctx, list(args, 4)?) {
            ffi::EVP_MD_CTX_free(ctx);
            return Err(e);
        }
        let r = ffi::EVP_DigestVerify(ctx, sig.as_ptr(), sig.len(), data.as_ptr(), data.len());
        ffi::EVP_MD_CTX_free(ctx);
        match r {
            1 => Ok(Out::Bool(true)),
            0 => {
                ffi::ERR_clear_error();
                Ok(Out::Bool(false))
            }
            _ => Err(failure(Class::PKey, b"EVP_DigestVerify")),
        }
    }
}

/// `pkey_compare(a, b)`: `PKey#compare?`.
fn pkey_compare(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let (a, b) = (contexts.pkey(id(args, 0)?)?, contexts.pkey(id(args, 1)?)?);
    // SAFETY: live keys.
    unsafe {
        if ffi::EVP_PKEY_get_id(a) != ffi::EVP_PKEY_get_id(b) {
            return Err(plain(Class::Type, "cannot match different PKey types"));
        }
        match ffi::EVP_PKEY_eq(a, b) {
            0 => Ok(Out::Bool(false)),
            1 => Ok(Out::Bool(true)),
            _ => Err(failure(Class::PKey, b"EVP_PKEY_eq")),
        }
    }
}

/// `pkey_text(id, public?)`: `PKey#to_text`, `public_to_text`.
fn pkey_text(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let pkey = contexts.pkey(id(args, 0)?)?;
    let public = truthy(args, 1);
    let bio = WriteBio::new()?;
    // SAFETY: a live key and BIO.
    let ok = unsafe {
        if public {
            ffi::EVP_PKEY_print_public(bio.0, pkey, 0, ptr::null_mut())
        } else {
            ffi::EVP_PKEY_print_private(bio.0, pkey, 0, ptr::null_mut())
        }
    };
    if ok != 1 {
        return Err(failure(
            Class::PKey,
            if public {
                b"EVP_PKEY_print_public"
            } else {
                b"EVP_PKEY_print_private"
            },
        ));
    }
    Ok(Out::Text(bio.take()))
}

// ---------------------------------------------------------------------------
// Certificates (ossl_x509cert.c)
// ---------------------------------------------------------------------------

/// `cert_read(data)`: `Certificate.new(der or pem)`.
fn cert_read(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let bio = ReadBio::new(bytes(args, 0)?)?;
    // SAFETY: a live BIO.
    let x = unsafe {
        let mut x = ffi::d2i_X509_bio(bio.0, ptr::null_mut());
        if x.is_null() {
            bio.reset();
            x = ffi::PEM_read_bio_X509(bio.0, ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
        }
        x
    };
    let x = NonNull::new(x).ok_or_else(|| failure(Class::Cert, b"PEM_read_bio_X509"))?;
    Ok(id_out(contexts.add(Object::Cert(x))))
}

/// `cert_load(data)`: `Certificate.load`, every certificate in a PEM
/// file, or the one a DER string holds.
fn cert_load(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let data = bytes(args, 0)?;
    let bio = ReadBio::new(data)?;
    let pem = data.windows(11).any(|w| w == b"-----BEGIN ");
    let mut ids = Vec::new();
    // SAFETY: a live BIO; each certificate read is ours.
    unsafe {
        if pem {
            loop {
                let x = ffi::PEM_read_bio_X509(
                    bio.0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                );
                let Some(x) = NonNull::new(x) else { break };
                ids.push(id_out(contexts.add(Object::Cert(x))));
            }
            // The read past the last one queues "no start line".
            ffi::ERR_clear_error();
            if ids.is_empty() {
                return Err(failure(Class::Cert, b"PEM_read_bio_X509"));
            }
        } else {
            let x = ffi::d2i_X509_bio(bio.0, ptr::null_mut());
            let x = NonNull::new(x).ok_or_else(|| failure(Class::Cert, b"d2i_X509_bio"))?;
            ids.push(id_out(contexts.add(Object::Cert(x))));
        }
    }
    Ok(Out::List(ids))
}

/// The time an `ASN1_TIME` says, `[year, month, day, hour, min, sec]` UTC.
///
/// # Safety
///
/// `t` is null or live.
unsafe fn time_out(t: *const sys::ASN1_TIME) -> Result<Out, Failure> {
    let mut tm = ffi::Tm::default();
    // SAFETY: the caller's contract; a null time fails as CRuby's does.
    if t.is_null() || unsafe { ffi::ASN1_TIME_to_tm(t, &mut tm) } != 1 {
        return Err(failure(Class::Asn1, b"ASN1_TIME_to_tm"));
    }
    Ok(Out::List(
        [
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
        ]
        .into_iter()
        .map(|n| Out::Int(i64::from(n)))
        .collect(),
    ))
}

/// `ossl_x509_time_adjust(NULL, time)`: an `ASN1_TIME` for seconds since
/// the epoch, UTCTime before 2050 and GeneralizedTime after, as
/// `X509_time_adj_ex` chooses.
fn time_of(seconds: i64) -> Result<*mut sys::ASN1_TIME, Failure> {
    // `ossl_time_split`: C's division, which truncates.
    let mut sec = seconds % 86400;
    let days =
        c_int::try_from(seconds / 86400).map_err(|_| plain(Class::Range, "time out of range"))?;
    // SAFETY: `sec` outlives the call; the time made is the caller's.
    let t = unsafe { ffi::X509_time_adj_ex(ptr::null_mut(), days, 0, &mut sec) };
    if t.is_null() {
        return Err(failure(Class::Cert, b"X509_time_adj_ex"));
    }
    Ok(t)
}

/// `cert_get(id, field)`.
fn cert_get(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let x = contexts.cert(id(args, 0)?)?;
    // SAFETY: a live certificate; what it answers is its own, copied here.
    unsafe {
        match bytes(args, 1)? {
            b"version" => Ok(Out::Int(ffi::X509_get_version(x))),
            b"serial" => {
                let ai = ffi::X509_get_serialNumber(x);
                let bn = ffi::ASN1_INTEGER_to_BN(ai, ptr::null_mut());
                if bn.is_null() {
                    return Err(failure(Class::Cert, b""));
                }
                let hex = bn_hex(bn);
                ffi::BN_free(bn);
                Ok(Out::Text(hex))
            }
            b"signature_algorithm" => {
                let mut alg = ptr::null();
                ffi::X509_get0_signature(ptr::null_mut(), &mut alg, x);
                let mut obj = ptr::null();
                ffi::X509_ALGOR_get0(&mut obj, ptr::null_mut(), ptr::null_mut(), alg);
                let bio = WriteBio::new()?;
                if ffi::i2a_ASN1_OBJECT(bio.0, obj) <= 0 {
                    return Err(failure(Class::Cert, b""));
                }
                Ok(Out::Text(bio.take()))
            }
            b"subject" | b"issuer" => {
                let name = if bytes(args, 1)? == b"subject" {
                    ffi::X509_get_subject_name(x)
                } else {
                    ffi::X509_get_issuer_name(x)
                };
                if name.is_null() {
                    return Err(failure(Class::Cert, b""));
                }
                der_of(name, ffi::i2d_X509_NAME)
                    .map(Out::Bytes)
                    .ok_or_else(|| failure(Class::Name, b""))
            }
            b"not_before" => time_out(ffi::X509_get0_notBefore(x)),
            b"not_after" => time_out(ffi::X509_get0_notAfter(x)),
            b"public_key" => {
                let pkey = ffi::X509_get_pubkey(x);
                if pkey.is_null() {
                    return Err(failure(Class::Cert, b""));
                }
                Ok(contexts.add_pkey(pkey))
            }
            b"der" => cert_der_of(x)
                .map(Out::Bytes)
                .ok_or_else(|| failure(Class::Cert, b"")),
            b"pem" => {
                let bio = WriteBio::new()?;
                if ffi::PEM_write_bio_X509(bio.0, x) != 1 {
                    return Err(failure(Class::Cert, b"PEM_write_bio_X509"));
                }
                Ok(Out::Text(bio.take()))
            }
            b"text" => {
                let bio = WriteBio::new()?;
                if ffi::X509_print(bio.0, x) != 1 {
                    return Err(failure(Class::Cert, b"X509_print"));
                }
                Ok(Out::Text(bio.take()))
            }
            _ => Err(bad("an unknown certificate field")),
        }
    }
}

/// `cert_set(id, field, value)`.
fn cert_set(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let x = contexts.cert(id(args, 0)?)?;
    // SAFETY: a live certificate; everything made here is freed or handed
    // to OpenSSL, which copies it.
    unsafe {
        match bytes(args, 1)? {
            b"version" => {
                let v = int(args, 2)?;
                if v < 0 {
                    return Err(plain(Class::Cert, "version must be >= 0!"));
                }
                if ffi::X509_set_version(x, c_long::try_from(v).unwrap_or(c_long::MAX)) != 1 {
                    return Err(failure(Class::Cert, b""));
                }
            }
            b"serial" => {
                let bn = bn_from_hex(bytes(args, 2)?)?;
                let ai = ffi::BN_to_ASN1_INTEGER(bn, ptr::null_mut());
                ffi::BN_free(bn);
                if ai.is_null() {
                    return Err(failure(Class::Asn1, b"BN_to_ASN1_INTEGER"));
                }
                let ok = ffi::X509_set_serialNumber(x, ai);
                ffi::ASN1_INTEGER_free(ai);
                if ok != 1 {
                    return Err(failure(Class::Cert, b""));
                }
            }
            field @ (b"subject" | b"issuer") => {
                let name = Name::decode(bytes(args, 2)?, Class::Name)?;
                let subject = field == b"subject";
                let ok = if subject {
                    ffi::X509_set_subject_name(x, name.0)
                } else {
                    ffi::X509_set_issuer_name(x, name.0)
                };
                if ok != 1 {
                    return Err(failure(
                        Class::Cert,
                        if subject {
                            b"X509_set_subject_name"
                        } else {
                            b"X509_set_issuer_name"
                        },
                    ));
                }
            }
            field @ (b"not_before" | b"not_after") => {
                let t = time_of(int(args, 2)?)?;
                let before = field == b"not_before";
                let ok = if before {
                    ffi::X509_set1_notBefore(x, t)
                } else {
                    ffi::X509_set1_notAfter(x, t)
                };
                ffi::ASN1_TIME_free(t);
                if ok != 1 {
                    return Err(failure(
                        Class::Cert,
                        if before {
                            b"X509_set_notBefore"
                        } else {
                            b"X509_set_notAfter"
                        },
                    ));
                }
            }
            b"public_key" => {
                let pkey = contexts.pkey(id(args, 2)?)?;
                if ffi::X509_set_pubkey(x, pkey) != 1 {
                    return Err(failure(Class::Cert, b"X509_set_pubkey"));
                }
            }
            _ => return Err(bad("an unknown certificate field")),
        }
    }
    Ok(Out::Nil)
}

/// `cert_sign(id, key, digest or nil)`.
fn cert_sign(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let x = contexts.cert(id(args, 0)?)?;
    let pkey = contexts.pkey(id(args, 1)?)?;
    let md = md_of(args, 2)?;
    // SAFETY: live objects.
    if unsafe { ffi::X509_sign(x, pkey, md) } <= 0 {
        return Err(failure(Class::Cert, b"X509_sign"));
    }
    Ok(Out::Nil)
}

/// `cert_verify(id, key)`.
fn cert_verify(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let x = contexts.cert(id(args, 0)?)?;
    let pkey = contexts.pkey(id(args, 1)?)?;
    // SAFETY: live objects.
    match unsafe { ffi::X509_verify(x, pkey) } {
        1 => Ok(Out::Bool(true)),
        0 => {
            drain_errors();
            Ok(Out::Bool(false))
        }
        _ => Err(failure(Class::Cert, b"")),
    }
}

/// `cert_extensions(id)`: each extension's DER.
fn cert_extensions(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let x = contexts.cert(id(args, 0)?)?;
    // SAFETY: a live certificate; its extensions are its own.
    unsafe {
        let n = ffi::X509_get_ext_count(x);
        let mut out = Vec::new();
        for i in 0..n {
            let ext = ffi::X509_get_ext(x, i);
            if let Some(der) = der_of(ext, ffi::i2d_X509_EXTENSION) {
                out.push(Out::Bytes(der));
            }
        }
        Ok(Out::List(out))
    }
}

/// `cert_add_extension(id, der)`.
fn cert_add_extension(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let x = contexts.cert(id(args, 0)?)?;
    let ext = Ext::decode(bytes(args, 1)?)?;
    // SAFETY: live objects; the certificate copies the extension.
    if unsafe { ffi::X509_add_ext(x, ext.0, -1) } != 1 {
        return Err(failure(Class::Cert, b"X509_add_ext"));
    }
    Ok(Out::Nil)
}

// ---------------------------------------------------------------------------
// Names (ossl_x509name.c)
// ---------------------------------------------------------------------------

/// An `X509_NAME` decoded from DER, freed on drop.
struct Name(*mut sys::X509_NAME);

impl Name {
    fn decode(der: &[u8], class: Class) -> Result<Name, Failure> {
        let mut p = der.as_ptr();
        let len = c_long::try_from(der.len()).map_err(|_| bad("a name past a C long"))?;
        // SAFETY: `p` reads `len` bytes of `der`.
        let name = unsafe { ffi::d2i_X509_NAME(ptr::null_mut(), &mut p, len) };
        if name.is_null() {
            return Err(failure(class, b""));
        }
        Ok(Name(name))
    }

    fn der(&self) -> Result<Vec<u8>, Failure> {
        // SAFETY: a live name.
        unsafe { der_of(self.0, ffi::i2d_X509_NAME) }.ok_or_else(|| failure(Class::Name, b""))
    }
}

impl Drop for Name {
    fn drop(&mut self) {
        // SAFETY: the name this owns.
        unsafe { ffi::X509_NAME_free(self.0) };
    }
}

fn name_new() -> Result<Out, Failure> {
    // SAFETY: no preconditions; the name is freed by `Name`.
    let name = Name(unsafe { ffi::X509_NAME_new() });
    if name.0.is_null() {
        return Err(failure(Class::Name, b""));
    }
    Ok(Out::Bytes(name.der()?))
}

/// `name_add(der, oid, value, type, loc, set)`: the DER with the entry
/// added, `X509_NAME_add_entry_by_txt`.
fn name_add(args: &[Arg]) -> Result<Out, Failure> {
    let name = Name::decode(bytes(args, 0)?, Class::Name)?;
    let oid = cstring(args, 1)?;
    let value = bytes(args, 2)?;
    let len = c_int::try_from(value.len()).map_err(|_| bad("a value past a C int"))?;
    let (kind, loc, set) = (c_int_at(args, 3)?, c_int_at(args, 4)?, c_int_at(args, 5)?);
    // SAFETY: a live name, a NUL-terminated field and `len` bytes of value.
    if unsafe {
        ffi::X509_NAME_add_entry_by_txt(name.0, oid.as_ptr(), kind, value.as_ptr(), len, loc, set)
    } != 1
    {
        return Err(failure(Class::Name, b"X509_NAME_add_entry_by_txt"));
    }
    Ok(Out::Bytes(name.der()?))
}

/// `name_to_s(der, flags or nil)`: `X509_NAME_oneline`, or
/// `X509_NAME_print_ex` with the flags.
fn name_to_s(args: &[Arg]) -> Result<Out, Failure> {
    let name = Name::decode(bytes(args, 0)?, Class::Name)?;
    match args.get(1) {
        Some(Arg::Int(flags)) => {
            let bio = WriteBio::new()?;
            let flags = c_ulong::try_from(*flags).map_err(|_| bad("negative flags"))?;
            // SAFETY: a live name and BIO.
            if unsafe { ffi::X509_NAME_print_ex(bio.0, name.0, 0, flags) } < 0 {
                return Err(failure(Class::Name, b"X509_NAME_print_ex"));
            }
            Ok(Out::Bytes(bio.take()))
        }
        _ => {
            // SAFETY: a live name; the string OpenSSL allocates is freed.
            unsafe {
                let s = ffi::X509_NAME_oneline(name.0, ptr::null_mut(), 0);
                if s.is_null() {
                    return Err(failure(Class::Name, b"X509_NAME_oneline"));
                }
                let out = CStr::from_ptr(s).to_bytes().to_vec();
                openssl_free(s.cast());
                Ok(Out::Bytes(out))
            }
        }
    }
}

/// `name_to_a(der)`: `[[short name or OID, value, type]...]`, as
/// `ossl_x509name_to_a` names each entry.
fn name_to_a(args: &[Arg]) -> Result<Out, Failure> {
    let name = Name::decode(bytes(args, 0)?, Class::Name)?;
    let mut out = Vec::new();
    // SAFETY: a live name; entries and names are its own or static.
    unsafe {
        for i in 0..ffi::X509_NAME_entry_count(name.0) {
            let entry = ffi::X509_NAME_get_entry(name.0, i);
            let obj = ffi::X509_NAME_ENTRY_get_object(entry);
            let mut long = [0 as c_char; 512];
            if ffi::i2t_ASN1_OBJECT(long.as_mut_ptr(), 512, obj) <= 0 {
                return Err(failure(Class::Name, b"i2t_ASN1_OBJECT"));
            }
            let nid = ffi::OBJ_ln2nid(long.as_ptr());
            let vname = if nid == 0 {
                CStr::from_ptr(long.as_ptr()).to_bytes().to_vec()
            } else {
                text(ffi::OBJ_nid2sn(nid), b"")
            };
            let data = ffi::X509_NAME_ENTRY_get_data(entry);
            let value = asn1_string(data);
            out.push(Out::List(vec![
                Out::Text(vname),
                Out::Bytes(value),
                Out::Int(i64::from(ffi::ASN1_STRING_type(data))),
            ]));
        }
    }
    Ok(Out::List(out))
}

/// An `ASN1_STRING`'s bytes.
///
/// # Safety
///
/// `s` is live.
unsafe fn asn1_string(s: *const sys::ASN1_STRING) -> Vec<u8> {
    // SAFETY: the caller's contract.
    unsafe {
        let p = ffi::ASN1_STRING_get0_data(s);
        let len = usize::try_from(ffi::ASN1_STRING_length(s)).unwrap_or(0);
        if p.is_null() {
            return Vec::new();
        }
        std::slice::from_raw_parts(p, len).to_vec()
    }
}

// ---------------------------------------------------------------------------
// Extensions (ossl_x509ext.c)
// ---------------------------------------------------------------------------

/// An `X509_EXTENSION` decoded from DER, freed on drop.
struct Ext(*mut sys::X509_EXTENSION);

impl Ext {
    fn decode(der: &[u8]) -> Result<Ext, Failure> {
        let mut p = der.as_ptr();
        let len = c_long::try_from(der.len()).map_err(|_| bad("an extension past a C long"))?;
        // SAFETY: `p` reads `len` bytes of `der`.
        let ext = unsafe { ffi::d2i_X509_EXTENSION(ptr::null_mut(), &mut p, len) };
        if ext.is_null() {
            return Err(failure(Class::Ext, b""));
        }
        Ok(Ext(ext))
    }

    fn der(&self) -> Result<Vec<u8>, Failure> {
        // SAFETY: a live extension.
        unsafe { der_of(self.0, ffi::i2d_X509_EXTENSION) }.ok_or_else(|| failure(Class::Ext, b""))
    }
}

impl Drop for Ext {
    fn drop(&mut self) {
        // SAFETY: the extension this owns.
        unsafe { ffi::X509_EXTENSION_free(self.0) };
    }
}

/// `ext_new(oid, value der, critical)`: `Extension.new(oid, value, crit)`.
fn ext_new(args: &[Arg]) -> Result<Out, Failure> {
    let oid = cstring(args, 0)?;
    let value = bytes(args, 1)?;
    let critical = truthy(args, 2);
    let len = c_int::try_from(value.len()).map_err(|_| bad("a value past a C int"))?;
    // SAFETY: everything made here is freed here or by `Ext`.
    unsafe {
        let obj = ffi::OBJ_txt2obj(oid.as_ptr(), 0);
        if obj.is_null() {
            return Err(failure(Class::Ext, b"OBJ_txt2obj"));
        }
        let data = ffi::ASN1_OCTET_STRING_new();
        if data.is_null() || ffi::ASN1_OCTET_STRING_set(data, value.as_ptr(), len) != 1 {
            ffi::ASN1_OBJECT_free(obj);
            ffi::ASN1_OCTET_STRING_free(data);
            return Err(failure(Class::Ext, b"ASN1_OCTET_STRING_set"));
        }
        let ext =
            ffi::X509_EXTENSION_create_by_OBJ(ptr::null_mut(), obj, c_int::from(critical), data);
        ffi::ASN1_OBJECT_free(obj);
        ffi::ASN1_OCTET_STRING_free(data);
        if ext.is_null() {
            return Err(failure(Class::Ext, b""));
        }
        Ok(Out::Bytes(Ext(ext).der()?))
    }
}

/// `ext_info(der)`: `[oid, value, critical?, value der]`, the first two as
/// `Extension#oid` and `#value` say them.
fn ext_info(args: &[Arg]) -> Result<Out, Failure> {
    let ext = Ext::decode(bytes(args, 0)?)?;
    // SAFETY: a live extension; what it answers is its own, copied here.
    unsafe {
        let obj = ffi::X509_EXTENSION_get_object(ext.0);
        let nid = ffi::OBJ_obj2nid(obj);
        let oid = if nid == 0 {
            let bio = WriteBio::new()?;
            ffi::i2a_ASN1_OBJECT(bio.0, obj);
            bio.take()
        } else {
            text(ffi::OBJ_nid2sn(nid), b"")
        };
        let bio = WriteBio::new()?;
        if ffi::X509V3_EXT_print(bio.0, ext.0, 0, 0) != 1 {
            ffi::ERR_clear_error();
            ffi::ASN1_STRING_print(bio.0, ffi::X509_EXTENSION_get_data(ext.0).cast());
        }
        let value = bio.take();
        let critical = ffi::X509_EXTENSION_get_critical(ext.0) > 0;
        let data = asn1_string(ffi::X509_EXTENSION_get_data(ext.0).cast());
        Ok(Out::List(vec![
            Out::Text(oid),
            Out::Text(value),
            Out::Bool(critical),
            Out::Bytes(data),
        ]))
    }
}

/// `ext_create(oid, value, issuer cert, subject cert)`:
/// `ExtensionFactory#create_ext`, `X509V3_EXT_nconf_nid`.
fn ext_create(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let oid = cstring(args, 0)?;
    let value = cstring(args, 1)?;
    let issuer = opt_id(args, 2)?.map(|i| contexts.cert(i)).transpose()?;
    let subject = opt_id(args, 3)?.map(|i| contexts.cert(i)).transpose()?;
    let mut ctx = ffi::X509V3_CTX { room: [0; 16] };
    // SAFETY: `ctx` is room for OpenSSL's struct, which it fills in; the
    // certificates are live and only read.
    unsafe {
        ffi::X509V3_set_ctx(
            &mut ctx,
            issuer.unwrap_or(ptr::null_mut()),
            subject.unwrap_or(ptr::null_mut()),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        );
        ffi::X509V3_set_nconf(&mut ctx, ptr::null_mut());
        let mut nid = ffi::OBJ_ln2nid(oid.as_ptr());
        if nid == 0 {
            nid = ffi::OBJ_sn2nid(oid.as_ptr());
        }
        let ext = ffi::X509V3_EXT_nconf_nid(ptr::null_mut(), &mut ctx, nid, value.as_ptr());
        if ext.is_null() {
            let mut message = oid.as_bytes().to_vec();
            message.extend_from_slice(b" = ");
            message.extend_from_slice(value.as_bytes());
            return Err(failure(Class::Ext, &message));
        }
        Ok(Out::Bytes(Ext(ext).der()?))
    }
}

// ---------------------------------------------------------------------------
// Stores and their contexts (ossl_x509store.c)
// ---------------------------------------------------------------------------

/// `store_op(id, op, arg)`.
fn store_op(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let store = contexts.store(id(args, 0)?)?;
    // SAFETY: a live store; certificates and lookups it takes are its own
    // references.
    unsafe {
        match bytes(args, 1)? {
            b"add_cert" => {
                let x = contexts.cert(id(args, 2)?)?;
                if ffi::X509_STORE_add_cert(store, x) != 1 {
                    return Err(failure(Class::Store, b"X509_STORE_add_cert"));
                }
            }
            b"add_file" => {
                let path = cstring(args, 2)?;
                let lookup = ffi::X509_STORE_add_lookup(store, ffi::X509_LOOKUP_file());
                if lookup.is_null() {
                    return Err(failure(Class::Store, b""));
                }
                if ffi::X509_LOOKUP_ctrl(
                    lookup,
                    ffi::X509_L_FILE_LOAD,
                    path.as_ptr(),
                    ffi::X509_FILETYPE_PEM,
                    ptr::null_mut(),
                ) != 1
                {
                    return Err(failure(Class::Store, b"X509_LOOKUP_load_file"));
                }
            }
            b"add_path" => {
                let path = cstring(args, 2)?;
                let lookup = ffi::X509_STORE_add_lookup(store, ffi::X509_LOOKUP_hash_dir());
                if lookup.is_null() {
                    return Err(failure(Class::Store, b""));
                }
                if ffi::X509_LOOKUP_ctrl(
                    lookup,
                    ffi::X509_L_ADD_DIR,
                    path.as_ptr(),
                    ffi::X509_FILETYPE_PEM,
                    ptr::null_mut(),
                ) != 1
                {
                    return Err(failure(Class::Store, b"X509_LOOKUP_add_dir"));
                }
            }
            b"set_default_paths" => {
                if ffi::X509_STORE_set_default_paths(store) != 1 {
                    return Err(failure(Class::Store, b"X509_STORE_set_default_paths"));
                }
            }
            b"flags" => {
                let flags = c_ulong::try_from(int(args, 2)?).map_err(|_| bad("negative flags"))?;
                ffi::X509_STORE_set_flags(store, flags);
            }
            b"purpose" => {
                if ffi::X509_STORE_set_purpose(store, c_int_at(args, 2)?) != 1 {
                    return Err(failure(Class::Store, b"X509_STORE_set_purpose"));
                }
            }
            b"trust" => {
                if ffi::X509_STORE_set_trust(store, c_int_at(args, 2)?) != 1 {
                    return Err(failure(Class::Store, b"X509_STORE_set_trust"));
                }
            }
            b"time" => {
                ffi::X509_VERIFY_PARAM_set_time(ffi::X509_STORE_get0_param(store), int(args, 2)?);
            }
            _ => return Err(bad("an unknown store operation")),
        }
    }
    Ok(Out::Nil)
}

/// A `STACK_OF(X509)` of the certificates `ids` name, each a reference of
/// the stack's own.
fn cert_stack(contexts: &Contexts, ids: &[Arg]) -> Result<*mut ffi::OPENSSL_STACK, Failure> {
    // SAFETY: a new stack; each certificate pushed gets a reference.
    unsafe {
        let stack = ffi::OPENSSL_sk_new_null();
        if stack.is_null() {
            return Err(failure(Class::Cert, b""));
        }
        for i in 0..ids.len() {
            let x = match contexts.cert(id(ids, i)?) {
                Ok(x) => x,
                Err(e) => {
                    free_cert_stack(stack);
                    return Err(e);
                }
            };
            ffi::X509_up_ref(x);
            ffi::OPENSSL_sk_push(stack, x.cast());
        }
        Ok(stack)
    }
}

/// `stctx_new(store, cert or nil, chain or nil)`:
/// `StoreContext.new`.
fn stctx_new(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let store = contexts.store(id(args, 0)?)?;
    let cert = opt_id(args, 1)?.map(|i| contexts.cert(i)).transpose()?;
    let chain = match args.get(2) {
        Some(Arg::List(ids)) => Some(cert_stack(contexts, ids)?),
        _ => None,
    };
    // SAFETY: references taken here are the context's own, released in its
    // `Drop`.
    let ctx = unsafe {
        let ctx = ffi::X509_STORE_CTX_new();
        let Some(ctx) = NonNull::new(ctx) else {
            if let Some(chain) = chain {
                free_cert_stack(chain);
            }
            return Err(failure(Class::Store, b"X509_STORE_CTX_new"));
        };
        ffi::X509_STORE_up_ref(store);
        if let Some(x) = cert {
            ffi::X509_up_ref(x);
        }
        StoreCtx {
            ctx,
            store: NonNull::new_unchecked(store),
            cert: cert.and_then(NonNull::new),
            chain: chain.and_then(NonNull::new),
            settings: Vec::new(),
            replay: Replay::default(),
            asks: false,
        }
    };
    ctx.init()?;
    Ok(id_out(contexts.add(Object::StoreCtx(Box::new(ctx)))))
}

impl StoreCtx {
    /// `X509_STORE_CTX_init`, again, with the settings made since.
    fn init(&self) -> Result<(), Failure> {
        let ctx = self.ctx.as_ptr();
        // SAFETY: a live context and the references it holds.
        unsafe {
            ffi::X509_STORE_CTX_cleanup(ctx);
            if ffi::X509_STORE_CTX_init(
                ctx,
                self.store.as_ptr(),
                self.cert.map_or(ptr::null_mut(), NonNull::as_ptr),
                self.chain.map_or(ptr::null_mut(), NonNull::as_ptr),
            ) != 1
            {
                return Err(failure(Class::Store, b"X509_STORE_CTX_init"));
            }
            ffi::X509_STORE_CTX_set_ex_data(ctx, 0, ptr::null_mut());
            for &(setting, value) in &self.settings {
                apply_setting(ctx, setting, value)?;
            }
        }
        Ok(())
    }
}

/// # Safety
///
/// `ctx` is a live, initialised context.
unsafe fn apply_setting(
    ctx: *mut sys::X509_STORE_CTX,
    setting: Setting,
    value: i64,
) -> Result<(), Failure> {
    // SAFETY: the caller's contract.
    unsafe {
        match setting {
            Setting::Flags => {
                ffi::X509_STORE_CTX_set_flags(ctx, c_ulong::try_from(value).unwrap_or(0))
            }
            Setting::Purpose => {
                if ffi::X509_STORE_CTX_set_purpose(ctx, c_int::try_from(value).unwrap_or(0)) != 1 {
                    return Err(failure(Class::Store, b"X509_STORE_CTX_set_purpose"));
                }
            }
            Setting::Trust => {
                if ffi::X509_STORE_CTX_set_trust(ctx, c_int::try_from(value).unwrap_or(0)) != 1 {
                    return Err(failure(Class::Store, b"X509_STORE_CTX_set_trust"));
                }
            }
            Setting::Time => ffi::X509_STORE_CTX_set_time(ctx, 0, value),
            Setting::Error => {
                ffi::X509_STORE_CTX_set_error(ctx, c_int::try_from(value).unwrap_or(0))
            }
        }
    }
    Ok(())
}

/// The verify callback of a store context: Ruby's answers, replayed, or a
/// stop at the first question it has not answered.
unsafe extern "C" fn store_verify_cb(ok: c_int, ctx: *mut sys::X509_STORE_CTX) -> c_int {
    // SAFETY: OpenSSL hands over the live context; its app data is the
    // `Replay` of the `StoreCtx` verifying, alive for the call.
    unsafe {
        let replay = ffi::X509_STORE_CTX_get_ex_data(ctx, 0).cast::<Replay>();
        if replay.is_null() {
            return ok;
        }
        (*replay).consult(ok, ctx).unwrap_or(0)
    }
}

/// `stctx_verify(id, asks?)`: `[result, question or nil]`, Ruby asked at
/// each step only when it has a callback. A question means
/// verification stopped for Ruby's answer: give it with `stctx_set(id,
/// "answer", ...)` and verify again.
fn stctx_verify(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let asks = truthy(args, 1);
    let sctx = contexts.store_ctx(id(args, 0)?)?;
    sctx.asks = asks;
    sctx.init()?;
    sctx.replay.next = 0;
    let ctx = sctx.ctx.as_ptr();
    // SAFETY: a live context; the replay it points at outlives the call.
    let r = unsafe {
        if sctx.asks {
            ffi::X509_STORE_CTX_set_ex_data(ctx, 0, (&raw mut sctx.replay).cast());
            ffi::X509_STORE_CTX_set_verify_cb(ctx, store_verify_cb);
        }
        let r = ffi::X509_verify_cert(ctx);
        ffi::X509_STORE_CTX_set_ex_data(ctx, 0, ptr::null_mut());
        r
    };
    if let Some(question) = sctx.replay.take_question() {
        drain_errors();
        return Ok(Out::List(vec![Out::Nil, question]));
    }
    sctx.replay.answers.clear();
    match r {
        1 => Ok(Out::List(vec![Out::Bool(true), Out::Nil])),
        0 => {
            drain_errors();
            Ok(Out::List(vec![Out::Bool(false), Out::Nil]))
        }
        _ => Err(failure(Class::Cert, b"X509_verify_cert")),
    }
}

/// `stctx_get(id, field)`.
fn stctx_get(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let field = bytes(args, 1)?.to_vec();
    let ctx = contexts.store_ctx(id(args, 0)?)?.ctx.as_ptr();
    // SAFETY: a live context; what it answers is copied.
    unsafe {
        match &field[..] {
            b"error" => Ok(Out::Int(i64::from(ffi::X509_STORE_CTX_get_error(ctx)))),
            b"error_depth" => Ok(Out::Int(i64::from(ffi::X509_STORE_CTX_get_error_depth(
                ctx,
            )))),
            b"chain" => {
                let chain = ffi::X509_STORE_CTX_get0_chain(ctx);
                if chain.is_null() {
                    return Ok(Out::Nil);
                }
                Ok(Out::List(
                    stack_ders(chain).into_iter().map(Out::Bytes).collect(),
                ))
            }
            b"current_cert" => {
                let x = ffi::X509_STORE_CTX_get_current_cert(ctx);
                Ok(if x.is_null() {
                    Out::Nil
                } else {
                    cert_der_of(x).map_or(Out::Nil, Out::Bytes)
                })
            }
            _ => Err(bad("an unknown store context field")),
        }
    }
}

/// `stctx_set(id, field, value...)`.
fn stctx_set(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let field = bytes(args, 1)?.to_vec();
    let sctx = contexts.store_ctx(id(args, 0)?)?;
    let setting = match &field[..] {
        b"answer" => {
            let (ok, error) = (truthy(args, 2), c_int_at(args, 3)?);
            sctx.replay.answers.push((ok, error));
            return Ok(Out::Nil);
        }
        b"flags" => Setting::Flags,
        b"purpose" => Setting::Purpose,
        b"trust" => Setting::Trust,
        b"time" => Setting::Time,
        b"error" => Setting::Error,
        _ => return Err(bad("an unknown store context field")),
    };
    let value = int(args, 2)?;
    // SAFETY: a live, initialised context.
    unsafe { apply_setting(sctx.ctx.as_ptr(), setting, value)? };
    if setting != Setting::Error {
        sctx.settings.push((setting, value));
    }
    Ok(Out::Nil)
}

// ---------------------------------------------------------------------------
// Contexts (ossl_ssl.c, SSLContext)
// ---------------------------------------------------------------------------

/// `ctx_new`: `ossl_sslctx_s_alloc`.
fn ctx_new(contexts: &mut Contexts) -> Result<Out, Failure> {
    // SAFETY: a new context, ours.
    unsafe {
        let ctx = ffi::SSL_CTX_new(ffi::TLS_method());
        let ctx = NonNull::new(ctx).ok_or_else(|| failure(Class::Ssl, b"SSL_CTX_new"))?;
        let mode = ffi::SSL_MODE_ENABLE_PARTIAL_WRITE
            | ffi::SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER
            | ffi::SSL_MODE_RELEASE_BUFFERS;
        ffi::SSL_CTX_ctrl(ctx.as_ptr(), ffi::SSL_CTRL_MODE, mode, ptr::null_mut());
        ffi::SSL_CTX_ctrl(ctx.as_ptr(), ffi::SSL_CTRL_SET_DH_AUTO, 1, ptr::null_mut());
        Ok(id_out(contexts.add(Object::Ctx(ctx))))
    }
}

/// `ctx_get(id, field)`.
fn ctx_get(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let ctx = contexts.ctx(id(args, 0)?)?;
    // SAFETY: a live context.
    unsafe {
        match bytes(args, 1)? {
            b"options" => Ok(Out::Int(
                i64::try_from(ffi::SSL_CTX_get_options(ctx)).unwrap_or(i64::MAX),
            )),
            b"security_level" => Ok(Out::Int(i64::from(ffi::SSL_CTX_get_security_level(ctx)))),
            b"session_cache_mode" => Ok(Out::Int(ffi::SSL_CTX_ctrl(
                ctx,
                ffi::SSL_CTRL_GET_SESS_CACHE_MODE,
                0,
                ptr::null_mut(),
            ))),
            b"session_cache_size" => Ok(Out::Int(ffi::SSL_CTX_ctrl(
                ctx,
                ffi::SSL_CTRL_GET_SESS_CACHE_SIZE,
                0,
                ptr::null_mut(),
            ))),
            b"timeout" => Ok(Out::Int(ffi::SSL_CTX_get_timeout(ctx))),
            b"ciphers" => {
                let stack = ffi::SSL_CTX_get_ciphers(ctx);
                if stack.is_null() {
                    return Ok(Out::Nil);
                }
                let out = (0..ffi::OPENSSL_sk_num(stack))
                    .map(|i| cipher_out(ffi::OPENSSL_sk_value(stack, i).cast()))
                    .collect();
                Ok(Out::List(out))
            }
            _ => Err(bad("an unknown context field")),
        }
    }
}

/// `[name, version, bits, algorithm bits]`, as `ossl_ssl_cipher_to_ary`.
///
/// # Safety
///
/// `cipher` is live.
unsafe fn cipher_out(cipher: *const sys::SSL_CIPHER) -> Out {
    let mut alg = 0;
    // SAFETY: the caller's contract; the names are static.
    unsafe {
        let bits = ffi::SSL_CIPHER_get_bits(cipher, &mut alg);
        Out::List(vec![
            Out::Text(text(ffi::SSL_CIPHER_get_name(cipher), b"")),
            Out::Text(text(ffi::SSL_CIPHER_get_version(cipher), b"")),
            Out::Int(i64::from(bits)),
            Out::Int(i64::from(alg)),
        ])
    }
}

/// `ctx_set(id, field, value)`.
fn ctx_set(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let ctx = contexts.ctx(id(args, 0)?)?;
    let ctrl_str = |cmd: c_int, name: &[u8]| -> Result<Out, Failure> {
        let value = cstring(args, 2)?;
        // SAFETY: a live context and a NUL-terminated list.
        if unsafe { ffi::SSL_CTX_ctrl(ctx, cmd, 0, value.as_ptr().cast_mut().cast()) } != 1 {
            return Err(failure(Class::Ssl, name));
        }
        Ok(Out::Nil)
    };
    // SAFETY: a live context.
    unsafe {
        match bytes(args, 1)? {
            b"options" => {
                let options = u64::try_from(int(args, 2)?).unwrap_or(0);
                ffi::SSL_CTX_clear_options(ctx, ffi::SSL_CTX_get_options(ctx));
                ffi::SSL_CTX_set_options(ctx, options);
            }
            field @ (b"min_version" | b"max_version") => {
                let min = field == b"min_version";
                let cmd = if min {
                    ffi::SSL_CTRL_SET_MIN_PROTO_VERSION
                } else {
                    ffi::SSL_CTRL_SET_MAX_PROTO_VERSION
                };
                if ffi::SSL_CTX_ctrl(ctx, cmd, c_long::from(c_int_at(args, 2)?), ptr::null_mut())
                    != 1
                {
                    return Err(failure(
                        Class::Ssl,
                        if min {
                            b"SSL_CTX_set_min_proto_version"
                        } else {
                            b"SSL_CTX_set_max_proto_version"
                        },
                    ));
                }
            }
            b"ciphers" => {
                let list = cstring(args, 2)?;
                if ffi::SSL_CTX_set_cipher_list(ctx, list.as_ptr()) != 1 {
                    return Err(failure(Class::Ssl, b"SSL_CTX_set_cipher_list"));
                }
            }
            b"ciphersuites" => {
                let list = cstring(args, 2)?;
                if ffi::SSL_CTX_set_ciphersuites(ctx, list.as_ptr()) != 1 {
                    return Err(failure(Class::Ssl, b"SSL_CTX_set_ciphersuites"));
                }
            }
            b"ecdh_curves" => {
                return ctrl_str(ffi::SSL_CTRL_SET_GROUPS_LIST, b"SSL_CTX_set1_curves_list");
            }
            b"groups" => {
                return ctrl_str(ffi::SSL_CTRL_SET_GROUPS_LIST, b"SSL_CTX_set1_groups_list");
            }
            b"sigalgs" => {
                return ctrl_str(ffi::SSL_CTRL_SET_SIGALGS_LIST, b"SSL_CTX_set1_sigalgs_list");
            }
            b"client_sigalgs" => {
                return ctrl_str(
                    ffi::SSL_CTRL_SET_CLIENT_SIGALGS_LIST,
                    b"SSL_CTX_set1_client_sigalgs_list",
                );
            }
            b"security_level" => ffi::SSL_CTX_set_security_level(ctx, c_int_at(args, 2)?),
            b"session_cache_mode" => {
                ffi::SSL_CTX_ctrl(
                    ctx,
                    ffi::SSL_CTRL_SET_SESS_CACHE_MODE,
                    c_long::try_from(int(args, 2)?).unwrap_or(0),
                    ptr::null_mut(),
                );
            }
            b"session_cache_size" => {
                ffi::SSL_CTX_ctrl(
                    ctx,
                    ffi::SSL_CTRL_SET_SESS_CACHE_SIZE,
                    c_long::try_from(int(args, 2)?).unwrap_or(0),
                    ptr::null_mut(),
                );
            }
            _ => return Err(bad("an unknown context field")),
        }
    }
    Ok(Out::Nil)
}

/// The verify callback every context gets at setup: `ossl_ssl_verify_callback`,
/// with Ruby's part asked for through a suspended handshake.
unsafe extern "C" fn ssl_verify_cb(ok: c_int, ctx: *mut sys::X509_STORE_CTX) -> c_int {
    // SAFETY: OpenSSL hands over a live context whose ex data names the
    // `SSL` verifying; that `SSL`'s app data is its `SslState`, alive while
    // the `SSL` is.
    unsafe {
        let ssl = ffi::X509_STORE_CTX_get_ex_data(ctx, ffi::SSL_get_ex_data_X509_STORE_CTX_idx())
            .cast::<sys::SSL>();
        if ssl.is_null() {
            return ok;
        }
        let state = ffi::SSL_get_ex_data(ssl, 0).cast::<SslState>();
        if state.is_null() {
            return ok;
        }
        let state = &mut *state;
        let depth = ffi::X509_STORE_CTX_get_error_depth(ctx);
        let hostname =
            ok == 1 && state.hostname_check && ffi::SSL_is_server(ssl) == 0 && depth == 0;
        if !state.asks && !hostname {
            return ok;
        }
        match state.replay.consult(ok, ctx) {
            Some(answer) => answer,
            // `SSL_set_retry_verify`, a macro: 1 once the handshake is set
            // to stop after this verification.
            None => c_int::from(
                ffi::SSL_ctrl(ssl, ffi::SSL_CTRL_SET_RETRY_VERIFY, 0, ptr::null_mut()) > 0,
            ),
        }
    }
}

/// The new-session callback: the session is kept for Ruby's
/// `session_new_cb`, which runs once the call that made it returns.
unsafe extern "C" fn new_session_cb(ssl: *mut sys::SSL, session: *mut sys::SSL_SESSION) -> c_int {
    // SAFETY: the `SSL`'s app data is its `SslState`; returning 1 keeps the
    // reference OpenSSL hands over.
    unsafe {
        let state = ffi::SSL_get_ex_data(ssl, 0).cast::<SslState>();
        match (state.is_null(), NonNull::new(session)) {
            (false, Some(session)) => {
                (*state).sessions.push(session);
                1
            }
            _ => 0,
        }
    }
}

/// The key log callback: each line kept for Ruby's `keylog_cb`.
unsafe extern "C" fn keylog_cb(ssl: *const sys::SSL, line: *const c_char) {
    // SAFETY: the `SSL`'s app data is its `SslState`; `line` is a string
    // OpenSSL holds for the call.
    unsafe {
        let state = ffi::SSL_get_ex_data(ssl, 0).cast::<SslState>();
        if !state.is_null() && !line.is_null() {
            (*state)
                .keylog
                .push(CStr::from_ptr(line).to_bytes().to_vec());
        }
    }
}

/// `ctx_setup(id, store, extra chain, cert, key, client CAs, ca_file,
/// ca_path, verify_mode, timeout, verify_depth, alpn wire, session id
/// context, session_new_cb?, keylog_cb?, servername_cb or alpn_select_cb?,
/// alpn_select_cb?, client_cert_cb?)`: `ossl_sslctx_setup`, in its order,
/// with its errors.
fn ctx_setup(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let ctx = contexts.ctx(id(args, 0)?)?;
    // SAFETY: a live context; everything it is given it takes references
    // to, or copies.
    unsafe {
        if let Some(store) = opt_id(args, 1)? {
            let store = contexts.store(store)?;
            ffi::SSL_CTX_set_cert_store(ctx, store);
            ffi::X509_STORE_up_ref(store);
        }
        let extra = list(args, 2)?;
        for i in 0..extra.len() {
            let x = contexts.cert(id(extra, i)?)?;
            ffi::X509_up_ref(x);
            if ffi::SSL_CTX_ctrl(ctx, ffi::SSL_CTRL_EXTRA_CHAIN_CERT, 0, x.cast()) != 1 {
                ffi::X509_free(x);
                return Err(failure(Class::Ssl, b"SSL_CTX_add_extra_chain_cert"));
            }
        }
        let cert = opt_id(args, 3)?.map(|i| contexts.cert(i)).transpose()?;
        let key = opt_id(args, 4)?.map(|i| contexts.pkey(i)).transpose()?;
        if let (Some(cert), Some(key)) = (cert, key) {
            if ffi::SSL_CTX_use_certificate(ctx, cert) != 1 {
                return Err(failure(Class::Ssl, b"SSL_CTX_use_certificate"));
            }
            if ffi::SSL_CTX_use_PrivateKey(ctx, key) != 1 {
                return Err(failure(Class::Ssl, b"SSL_CTX_use_PrivateKey"));
            }
            if ffi::SSL_CTX_check_private_key(ctx) != 1 {
                return Err(failure(Class::Ssl, b"SSL_CTX_check_private_key"));
            }
        }
        let client_cas = list(args, 5)?;
        for i in 0..client_cas.len() {
            let x = contexts.cert(id(client_cas, i)?)?;
            if ffi::SSL_CTX_add_client_CA(ctx, x) != 1 {
                return Err(failure(Class::Ssl, b"SSL_CTX_add_client_CA"));
            }
        }
        if let Some(file) = opt_cstring(args, 6)? {
            if ffi::SSL_CTX_load_verify_file(ctx, file.as_ptr()) != 1 {
                return Err(failure(Class::Ssl, b"SSL_CTX_load_verify_file"));
            }
        }
        if let Some(dir) = opt_cstring(args, 7)? {
            if ffi::SSL_CTX_load_verify_dir(ctx, dir.as_ptr()) != 1 {
                return Err(failure(Class::Ssl, b"SSL_CTX_load_verify_dir"));
            }
        }
        let mode = match args.get(8) {
            Some(Arg::Int(_)) => c_int_at(args, 8)?,
            _ => 0,
        };
        ffi::SSL_CTX_set_verify(ctx, mode, Some(ssl_verify_cb));
        if let Some(Arg::Int(t)) = args.get(9) {
            ffi::SSL_CTX_set_timeout(ctx, c_long::try_from(*t).unwrap_or(c_long::MAX));
        }
        if let Some(Arg::Int(_)) = args.get(10) {
            ffi::SSL_CTX_set_verify_depth(ctx, c_int_at(args, 10)?);
        }
        if let Some(wire) = opt_bytes(args, 11)? {
            let len = c_uint::try_from(wire.len()).map_err(|_| bad("ALPN past a C int"))?;
            if ffi::SSL_CTX_set_alpn_protos(ctx, wire.as_ptr(), len) != 0 {
                return Err(failure(Class::Ssl, b"SSL_CTX_set_alpn_protos"));
            }
        }
        if let Some(sid) = opt_bytes(args, 12)? {
            let len = c_uint::try_from(sid.len())
                .map_err(|_| bad("a session id context past a C int"))?;
            if ffi::SSL_CTX_set_session_id_context(ctx, sid.as_ptr(), len) != 1 {
                return Err(failure(Class::Ssl, b"SSL_CTX_set_session_id_context"));
            }
        }
        if truthy(args, 13) {
            ffi::SSL_CTX_sess_set_new_cb(ctx, Some(new_session_cb));
        }
        if truthy(args, 14) {
            ffi::SSL_CTX_set_keylog_callback(ctx, Some(keylog_cb));
        }
        if truthy(args, 15) {
            ffi::SSL_CTX_set_client_hello_cb(ctx, Some(client_hello_cb), ptr::null_mut());
        }
        if truthy(args, 16) {
            ffi::SSL_CTX_set_alpn_select_cb(ctx, Some(alpn_select_cb), ptr::null_mut());
        }
        if truthy(args, 17) {
            ffi::SSL_CTX_set_client_cert_cb(ctx, Some(client_cert_cb));
        }
    }
    Ok(Out::Nil)
}

/// The extension of type `kind` in the ClientHello being handled.
///
/// # Safety
///
/// `ssl` is live and inside its client hello callback.
unsafe fn hello_ext(ssl: *mut sys::SSL, kind: c_uint) -> Option<Vec<u8>> {
    let mut out = ptr::null();
    let mut len = 0;
    // SAFETY: the caller's contract; OpenSSL points into its ClientHello.
    unsafe {
        if ffi::SSL_client_hello_get0_ext(ssl, kind, &mut out, &mut len) != 1 || out.is_null() {
            return None;
        }
        Some(std::slice::from_raw_parts(out, len).to_vec())
    }
}

/// The host name in a `server_name` extension (RFC 6066): a list of
/// entries, each a type and a name; the `host_name` one.
fn server_name(ext: &[u8]) -> Option<Vec<u8>> {
    let list = ext.get(2..)?;
    let mut rest = list.get(..usize::from(u16::from_be_bytes([ext[0], ext[1]])))?;
    while rest.len() >= 3 {
        let (kind, len) = (rest[0], usize::from(u16::from_be_bytes([rest[1], rest[2]])));
        let name = rest.get(3..3 + len)?;
        if kind == 0 {
            return Some(name.to_vec());
        }
        rest = &rest[3 + len..];
    }
    None
}

/// The protocols in an ALPN extension (RFC 7301): each one a length octet
/// and its name, after the list's own two-octet length.
fn alpn_list(ext: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut rest = ext.get(2..)?;
    let mut out = Vec::new();
    while let Some((&len, tail)) = rest.split_first() {
        let name = tail.get(..usize::from(len))?;
        out.push(name.to_vec());
        rest = &tail[usize::from(len)..];
    }
    Some(out)
}

/// A server's client hello callback: the handshake suspended until Ruby's
/// `servername_cb` and `alpn_select_cb` have answered (CRuby runs both from
/// OpenSSL's callbacks for those extensions).
unsafe extern "C" fn client_hello_cb(
    ssl: *mut sys::SSL,
    alert: *mut c_int,
    _arg: *mut c_void,
) -> c_int {
    // SAFETY: the `SSL`'s app data is its `SslState`, alive while it is.
    unsafe {
        let state = ffi::SSL_get_ex_data(ssl, 0).cast::<SslState>();
        if state.is_null() {
            return ffi::SSL_CLIENT_HELLO_SUCCESS;
        }
        let state = &mut *state;
        match state.hello {
            Hello::Answered(true) => ffi::SSL_CLIENT_HELLO_SUCCESS,
            Hello::Answered(false) => {
                if !alert.is_null() {
                    *alert = ffi::SSL_AD_INTERNAL_ERROR;
                }
                ffi::SSL_CLIENT_HELLO_ERROR
            }
            Hello::Asked(..) | Hello::Unasked => {
                let name =
                    hello_ext(ssl, ffi::TLSEXT_TYPE_SERVER_NAME).and_then(|e| server_name(&e));
                let alpn = hello_ext(ssl, ffi::TLSEXT_TYPE_ALPN).and_then(|e| alpn_list(&e));
                state.hello = Hello::Asked(name, alpn);
                ffi::SSL_CLIENT_HELLO_RETRY
            }
        }
    }
}

/// OpenSSL's ALPN callback: the protocol Ruby's `alpn_select_cb` chose.
unsafe extern "C" fn alpn_select_cb(
    ssl: *mut sys::SSL,
    out: *mut *const u8,
    outlen: *mut u8,
    _input: *const u8,
    _inlen: c_uint,
    _arg: *mut c_void,
) -> c_int {
    // SAFETY: the `SSL`'s app data is its `SslState`; the protocol stays in
    // it for as long as the `SSL` lives.
    unsafe {
        let state = ffi::SSL_get_ex_data(ssl, 0).cast::<SslState>();
        if state.is_null() {
            return ffi::SSL_TLSEXT_ERR_NOACK;
        }
        match &(*state).alpn {
            Some(p) if !p.is_empty() && p.len() < 256 => {
                *out = p.as_ptr();
                *outlen = u8::try_from(p.len()).unwrap_or(0);
                ffi::SSL_TLSEXT_ERR_OK
            }
            Some(_) => ffi::SSL_TLSEXT_ERR_ALERT_FATAL,
            None => ffi::SSL_TLSEXT_ERR_NOACK,
        }
    }
}

/// A client's certificate callback: suspended until Ruby's
/// `client_cert_cb` answers, then that certificate and key, or none.
unsafe extern "C" fn client_cert_cb(
    ssl: *mut sys::SSL,
    x509: *mut *mut sys::X509,
    pkey: *mut *mut sys::EVP_PKEY,
) -> c_int {
    // SAFETY: the `SSL`'s app data is its `SslState`; OpenSSL takes the
    // references handed over.
    unsafe {
        let state = ffi::SSL_get_ex_data(ssl, 0).cast::<SslState>();
        if state.is_null() {
            return 0;
        }
        let state = &mut *state;
        match std::mem::take(&mut state.client_cert) {
            ClientCert::Answered(Some((x, k))) => {
                *x509 = x.as_ptr();
                *pkey = k.as_ptr();
                1
            }
            ClientCert::Answered(None) => 0,
            ClientCert::Unasked | ClientCert::Asked => {
                state.client_cert = ClientCert::Asked;
                -1
            }
        }
    }
}

/// `ssl_hello_answer(id, ok, context or nil, protocol or nil)`: Ruby's
/// answer to a ClientHello question; a context `servername_cb` chose is
/// the connection's from here on.
fn ssl_hello_answer(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let ok = truthy(args, 1);
    let ctx = opt_id(args, 2)?.map(|i| contexts.ctx(i)).transpose()?;
    let alpn = opt_bytes(args, 3)?.map(<[u8]>::to_vec);
    let state = contexts.ssl(id(args, 0)?)?;
    if let Some(ctx) = ctx {
        // SAFETY: live objects; the `SSL` takes a reference to the context.
        unsafe { ffi::SSL_set_SSL_CTX(state.ssl.as_ptr(), ctx) };
    }
    state.alpn = alpn;
    state.hello = Hello::Answered(ok);
    Ok(Out::Nil)
}

/// `ssl_client_cert(id, cert or nil, key or nil)`: Ruby's
/// `client_cert_cb` answer.
fn ssl_client_cert(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let pair = match (opt_id(args, 1)?, opt_id(args, 2)?) {
        (Some(x), Some(k)) => {
            let (x, k) = (contexts.cert(x)?, contexts.pkey(k)?);
            // SAFETY: live objects; the references taken go to OpenSSL.
            unsafe {
                ffi::X509_up_ref(x);
                ffi::EVP_PKEY_up_ref(k);
                Some((NonNull::new_unchecked(x), NonNull::new_unchecked(k)))
            }
        }
        _ => None,
    };
    contexts.ssl(id(args, 0)?)?.client_cert = ClientCert::Answered(pair);
    Ok(Out::Nil)
}

/// `ctx_add_certificate(id, cert, key, extra chain or nil)`.
fn ctx_add_certificate(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let ctx = contexts.ctx(id(args, 0)?)?;
    let x = contexts.cert(id(args, 1)?)?;
    let key = contexts.pkey(id(args, 2)?)?;
    // SAFETY: live objects; the context takes references of its own, and
    // the chain made here is the context's once set.
    unsafe {
        let public = ffi::X509_get_pubkey(x);
        if public.is_null() {
            ffi::ERR_clear_error();
            return Err(plain(
                Class::Argument,
                "certificate does not contain public key",
            ));
        }
        let eq = ffi::EVP_PKEY_eq(public, key);
        ffi::EVP_PKEY_free(public);
        if eq != 1 {
            ffi::ERR_clear_error();
            return Err(plain(Class::Argument, "public key mismatch"));
        }
        let chain = match args.get(3) {
            Some(Arg::List(ids)) => Some(cert_stack(contexts, ids)?),
            _ => None,
        };
        let release = |chain: Option<*mut ffi::OPENSSL_STACK>| {
            if let Some(chain) = chain {
                free_cert_stack(chain);
            }
        };
        if ffi::SSL_CTX_use_certificate(ctx, x) != 1 {
            release(chain);
            return Err(failure(Class::Ssl, b"SSL_CTX_use_certificate"));
        }
        if ffi::SSL_CTX_use_PrivateKey(ctx, key) != 1 {
            release(chain);
            return Err(failure(Class::Ssl, b"SSL_CTX_use_PrivateKey"));
        }
        if let Some(chain) = chain {
            if ffi::SSL_CTX_ctrl(ctx, ffi::SSL_CTRL_CHAIN, 0, chain.cast()) != 1 {
                free_cert_stack(chain);
                return Err(failure(Class::Ssl, b"SSL_CTX_set0_chain"));
            }
        }
    }
    Ok(Out::Nil)
}

/// `ctx_session(id, op, session or time)`: the session cache.
fn ctx_session(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let ctx = contexts.ctx(id(args, 0)?)?;
    // SAFETY: live objects.
    unsafe {
        match bytes(args, 1)? {
            b"add" => Ok(Out::Bool(
                ffi::SSL_CTX_add_session(ctx, contexts.session(id(args, 2)?)?) == 1,
            )),
            b"remove" => Ok(Out::Bool(
                ffi::SSL_CTX_remove_session(ctx, contexts.session(id(args, 2)?)?) == 1,
            )),
            b"flush" => {
                ffi::SSL_CTX_flush_sessions(ctx, c_long::try_from(int(args, 2)?).unwrap_or(0));
                Ok(Out::Nil)
            }
            _ => Err(bad("an unknown session cache operation")),
        }
    }
}

// ---------------------------------------------------------------------------
// Connections (ossl_ssl.c, SSLSocket)
// ---------------------------------------------------------------------------

/// `ssl_new(ctx, asks?, hostname check?)`: `SSL_new`, the state Ruby's
/// questions go through set as its app data.
fn ssl_new(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let ctx = contexts.ctx(id(args, 0)?)?;
    // SAFETY: a live context; the `SSL` is the state's, and the state is
    // boxed, so the address its app data holds is stable.
    unsafe {
        let ssl = NonNull::new(ffi::SSL_new(ctx)).ok_or_else(|| failure(Class::Ssl, b"SSL_new"))?;
        let mut state = Box::new(SslState {
            ssl,
            asks: truthy(args, 1),
            hostname_check: truthy(args, 2),
            replay: Replay::default(),
            sessions: Vec::new(),
            keylog: Vec::new(),
            hello: Hello::Unasked,
            alpn: None,
            client_cert: ClientCert::Unasked,
        });
        ffi::SSL_set_ex_data(ssl.as_ptr(), 0, (&raw mut *state).cast());
        Ok(id_out(contexts.add(Object::Ssl(state))))
    }
}

/// Whether a connection has its descriptor yet: CRuby's `ssl_started`.
fn started(ssl: *mut sys::SSL) -> bool {
    // SAFETY: a live `SSL`.
    !unsafe { ffi::SSL_get_rbio(ssl) }.is_null()
}

fn not_started() -> Failure {
    plain(Class::Ssl, "SSL session is not started yet")
}

/// `ssl_setup(id, fd)`: `ossl_ssl_setup`, once.
fn ssl_setup(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let fd = c_int_at(args, 1)?;
    let ssl = contexts.ssl(id(args, 0)?)?.ssl.as_ptr();
    if started(ssl) {
        return Ok(Out::Bool(false));
    }
    // SAFETY: a live `SSL`; the descriptor stays Ruby's IO's.
    if unsafe { ffi::SSL_set_fd(ssl, fd) } != 1 {
        return Err(failure(Class::Ssl, b"SSL_set_fd"));
    }
    Ok(Out::Bool(true))
}

/// `ssl_hostname(id, name or nil)`: `SSL_set_tlsext_host_name`.
fn ssl_hostname(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let name = opt_cstring(args, 1)?;
    let ssl = contexts.ssl(id(args, 0)?)?.ssl.as_ptr();
    let p = name
        .as_ref()
        .map_or(ptr::null_mut(), |n| n.as_ptr().cast_mut());
    // SAFETY: a live `SSL`; OpenSSL copies the name.
    if unsafe {
        ffi::SSL_ctrl(
            ssl,
            ffi::SSL_CTRL_SET_TLSEXT_HOSTNAME,
            ffi::TLSEXT_NAMETYPE_HOST_NAME,
            p.cast(),
        )
    } != 1
    {
        return Err(failure(Class::Ssl, b""));
    }
    Ok(Out::Nil)
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn clear_errno() {
    // SAFETY: the calling thread's own errno.
    unsafe {
        #[cfg(target_vendor = "apple")]
        {
            *libc::__error() = 0;
        }
        #[cfg(not(target_vendor = "apple"))]
        {
            *libc::__errno_location() = 0;
        }
    }
}

/// What an `SSL_*` call's failure was, beyond wanting to read or write: a
/// system call's errno, or the `[SSL_get_error, errno, state, message
/// tail, errors]` Ruby builds `ossl_start_ssl`'s message from.
fn ssl_failure(ssl: *mut sys::SSL, code: c_int, errno: i32, func: &'static [u8]) -> Out {
    if code == ffi::SSL_ERROR_SYSCALL && errno != 0 {
        drain_errors();
        return Out::List(vec![
            Out::Sym("syserr"),
            Out::Int(i64::from(errno)),
            Out::Text(func.to_vec()),
        ]);
    }
    // SAFETY: a live `SSL`; the strings OpenSSL answers are static.
    let (state, tail, errors) = unsafe {
        let state = text(ffi::SSL_state_string_long(ssl), b"");
        let err = ffi::ERR_peek_last_error();
        let lib = (err >> 23) & 0xFF;
        let reason = err & 0x7F_FFFF;
        if lib == ffi::ERR_LIB_SSL && reason == ffi::SSL_R_CERTIFICATE_VERIFY_FAILED {
            let message = text(sys::ERR_reason_error_string(err), b"(null)");
            let verify = text(
                ffi::X509_verify_cert_error_string(ffi::SSL_get_verify_result(ssl)),
                b"(null)",
            );
            drain_errors();
            let mut tail = b": ".to_vec();
            tail.extend_from_slice(&message);
            tail.extend_from_slice(b" (");
            tail.extend_from_slice(&verify);
            tail.push(b')');
            (state, tail, Vec::new())
        } else {
            let failed = failure(Class::Ssl, b"");
            let tail = if failed.message.is_empty() {
                Vec::new()
            } else {
                let mut tail = b": ".to_vec();
                tail.extend_from_slice(&failed.message);
                tail
            };
            (state, tail, failed.errors)
        }
    };
    Out::List(vec![
        Out::Sym("error"),
        Out::Int(i64::from(code)),
        Out::Int(i64::from(errno)),
        Out::Text(state),
        Out::Text(tail),
        Out::List(errors.into_iter().map(Out::Text).collect()),
    ])
}

/// `ssl_handshake(id, server?)`: one `SSL_connect` or `SSL_accept`. True
/// when done; `:wait_readable` or `:wait_writable`; `:verify` with
/// `[:verify, question]` when Ruby must answer before the next call; or a
/// failure for `ossl_start_ssl` to raise.
fn ssl_handshake(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let server = truthy(args, 1);
    let state = contexts.ssl(id(args, 0)?)?;
    let ssl = state.ssl.as_ptr();
    state.replay.next = 0;
    clear_errno();
    // SAFETY: a live `SSL` whose app data is `state`, which outlives the
    // call; the descriptor is nonblocking.
    let ret = unsafe {
        if server {
            ffi::SSL_accept(ssl)
        } else {
            ffi::SSL_connect(ssl)
        }
    };
    let saved = errno();
    if ret > 0 {
        state.replay.answers.clear();
        return Ok(Out::Bool(true));
    }
    // SAFETY: a live `SSL`.
    let code = unsafe { ffi::SSL_get_error(ssl, ret) };
    if code == ffi::SSL_ERROR_WANT_RETRY_VERIFY {
        if let Some(question) = state.replay.take_question() {
            return Ok(Out::List(vec![Out::Sym("verify"), question]));
        }
    }
    if code == ffi::SSL_ERROR_WANT_CLIENT_HELLO_CB {
        if let Hello::Asked(name, alpn) = &state.hello {
            let name = name.clone().map_or(Out::Nil, Out::Text);
            let alpn = alpn.clone().map_or(Out::Nil, |l| {
                Out::List(l.into_iter().map(Out::Bytes).collect())
            });
            return Ok(Out::List(vec![Out::Sym("hello"), name, alpn]));
        }
    }
    if code == ffi::SSL_ERROR_WANT_X509_LOOKUP {
        if let ClientCert::Asked = state.client_cert {
            return Ok(Out::List(vec![Out::Sym("client_cert")]));
        }
    }
    state.replay.answers.clear();
    Ok(match code {
        ffi::SSL_ERROR_WANT_READ => Out::Sym("wait_readable"),
        ffi::SSL_ERROR_WANT_WRITE => Out::Sym("wait_writable"),
        _ => ssl_failure(
            ssl,
            code,
            saved,
            if server {
                b"SSL_accept"
            } else {
                b"SSL_connect"
            },
        ),
    })
}

/// `ssl_read(id, length)`: `ossl_ssl_read_internal`'s one `SSL_read`. The
/// bytes; `:wait_readable` or `:wait_writable`; `:eof`; `[:syserr,
/// errno]`; or the `SSL_read` failure.
fn ssl_read(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let len = c_int_at(args, 1)?;
    let state = contexts.ssl(id(args, 0)?)?;
    let ssl = state.ssl.as_ptr();
    if !started(ssl) {
        return Err(not_started());
    }
    let mut buf = vec![0u8; usize::try_from(len).unwrap_or(0)];
    state.replay.next = 0;
    clear_errno();
    // SAFETY: a live `SSL` and `len` writable bytes.
    let n = unsafe { ffi::SSL_read(ssl, buf.as_mut_ptr().cast(), len) };
    let saved = errno();
    // SAFETY: a live `SSL`.
    let code = unsafe { ffi::SSL_get_error(ssl, n) };
    Ok(match code {
        ffi::SSL_ERROR_NONE => {
            buf.truncate(usize::try_from(n).unwrap_or(0));
            Out::Bytes(buf)
        }
        ffi::SSL_ERROR_ZERO_RETURN => Out::Sym("eof"),
        ffi::SSL_ERROR_WANT_WRITE => Out::Sym("wait_writable"),
        ffi::SSL_ERROR_WANT_READ => Out::Sym("wait_readable"),
        ffi::SSL_ERROR_SYSCALL if !queued() => {
            if saved != 0 {
                Out::List(vec![
                    Out::Sym("syserr"),
                    Out::Int(i64::from(saved)),
                    Out::Nil,
                ])
            } else {
                Out::Sym("eof")
            }
        }
        _ => return Err(failure(Class::Ssl, b"SSL_read")),
    })
}

/// Whether OpenSSL's error queue holds anything (`ERR_peek_error`).
fn queued() -> bool {
    // SAFETY: no preconditions.
    unsafe { ffi::ERR_peek_error() != 0 }
}

/// `ssl_write(id, data)`: one `SSL_write`. The count; `:wait_writable` or
/// `:wait_readable`; `[:syserr, errno]`; or the `SSL_write` failure.
fn ssl_write(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let data = bytes(args, 1)?.to_vec();
    let state = contexts.ssl(id(args, 0)?)?;
    let ssl = state.ssl.as_ptr();
    if !started(ssl) {
        return Err(not_started());
    }
    let num = c_int::try_from(data.len()).map_err(|_| plain(Class::Range, "string too long"))?;
    if num == 0 {
        return Ok(Out::Int(0));
    }
    state.replay.next = 0;
    clear_errno();
    // SAFETY: a live `SSL` and `num` readable bytes.
    let n = unsafe { ffi::SSL_write(ssl, data.as_ptr().cast(), num) };
    let saved = errno();
    // SAFETY: a live `SSL`.
    let code = unsafe { ffi::SSL_get_error(ssl, n) };
    Ok(match code {
        ffi::SSL_ERROR_NONE => Out::Int(i64::from(n)),
        ffi::SSL_ERROR_WANT_WRITE => Out::Sym("wait_writable"),
        ffi::SSL_ERROR_WANT_READ => Out::Sym("wait_readable"),
        ffi::SSL_ERROR_SYSCALL if saved != 0 => {
            drain_errors();
            Out::List(vec![
                Out::Sym("syserr"),
                Out::Int(i64::from(saved)),
                Out::Nil,
            ])
        }
        _ => return Err(failure(Class::Ssl, b"SSL_write")),
    })
}

/// `ssl_shutdown(id)`: `ossl_ssl_stop`: one `SSL_shutdown`, errors and all
/// dropped, as CRuby cannot wait here either.
fn ssl_shutdown(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let ssl = contexts.ssl(id(args, 0)?)?.ssl.as_ptr();
    if !started(ssl) {
        return Ok(Out::Nil);
    }
    // SAFETY: a live `SSL`.
    let r = unsafe { ffi::SSL_shutdown(ssl) };
    if r < 0 {
        drain_errors();
    }
    Ok(Out::Nil)
}

/// `ssl_get(id, field)`.
fn ssl_get(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let field = bytes(args, 1)?.to_vec();
    let ssl = contexts.ssl(id(args, 0)?)?.ssl.as_ptr();
    // SAFETY: a live `SSL`; what it answers is copied, or a reference taken.
    unsafe {
        match &field[..] {
            b"version" => Ok(Out::Text(text(ffi::SSL_get_version(ssl), b""))),
            b"state" => Ok(Out::Text(text(ffi::SSL_state_string(ssl), b""))),
            b"pending" => Ok(Out::Int(i64::from(ffi::SSL_pending(ssl)))),
            b"session_reused" => Ok(Out::Bool(ffi::SSL_session_reused(ssl) == 1)),
            b"verify_result" => Ok(Out::Int(ffi::SSL_get_verify_result(ssl))),
            b"started" => Ok(Out::Bool(started(ssl))),
            b"cipher" => {
                let c = ffi::SSL_get_current_cipher(ssl);
                Ok(if c.is_null() { Out::Nil } else { cipher_out(c) })
            }
            b"cert" => contexts.add_cert_copy(ffi::SSL_get_certificate(ssl)),
            b"peer_cert" => {
                let x = ffi::SSL_get1_peer_certificate(ssl);
                let out = contexts.add_cert_copy(x);
                if !x.is_null() {
                    ffi::X509_free(x);
                }
                out
            }
            b"peer_cert_chain" => {
                let chain = ffi::SSL_get_peer_cert_chain(ssl);
                if chain.is_null() {
                    return Ok(Out::Nil);
                }
                let mut out = Vec::new();
                for i in 0..ffi::OPENSSL_sk_num(chain) {
                    out.push(contexts.add_cert_copy(ffi::OPENSSL_sk_value(chain, i).cast())?);
                }
                Ok(Out::List(out))
            }
            b"alpn_protocol" => {
                let mut data = ptr::null();
                let mut len = 0;
                ffi::SSL_get0_alpn_selected(ssl, &mut data, &mut len);
                if data.is_null() || len == 0 {
                    return Ok(Out::Nil);
                }
                Ok(Out::Bytes(
                    std::slice::from_raw_parts(data, len as usize).to_vec(),
                ))
            }
            field @ (b"finished_message" | b"peer_finished_message") => {
                let mut buf = [0u8; 128];
                let n = if field == b"finished_message" {
                    ffi::SSL_get_finished(ssl, buf.as_mut_ptr().cast(), buf.len())
                } else {
                    ffi::SSL_get_peer_finished(ssl, buf.as_mut_ptr().cast(), buf.len())
                };
                if n == 0 {
                    return Ok(Out::Nil);
                }
                Ok(Out::Bytes(buf[..n.min(buf.len())].to_vec()))
            }
            b"tmp_key" => {
                let mut key: *mut sys::EVP_PKEY = ptr::null_mut();
                if ffi::SSL_ctrl(
                    ssl,
                    ffi::SSL_CTRL_GET_PEER_TMP_KEY,
                    0,
                    (&raw mut key).cast(),
                ) == 0
                    || key.is_null()
                {
                    return Ok(Out::Nil);
                }
                Ok(contexts.add_pkey(key))
            }
            b"session" => {
                let s = ffi::SSL_get1_session(ssl);
                match NonNull::new(s) {
                    Some(s) => Ok(id_out(contexts.add(Object::Session(s)))),
                    None => Ok(Out::Nil),
                }
            }
            _ => Err(bad("an unknown connection field")),
        }
    }
}

/// `ssl_take(id)`: what callbacks kept since the last call, `[question
/// or nil, [session id...], [key log line...]]`.
fn ssl_take(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let state = contexts.ssl(id(args, 0)?)?;
    let sessions = std::mem::take(&mut state.sessions);
    let lines: Vec<_> = state.keylog.drain(..).map(Out::Text).collect();
    let question = state.replay.take_question().unwrap_or(Out::Nil);
    let ids = sessions
        .into_iter()
        .map(|s| id_out(contexts.add(Object::Session(s))))
        .collect();
    Ok(Out::List(vec![question, Out::List(ids), Out::List(lines)]))
}

/// `ssl_export_keying_material(id, label, length, context or nil)`.
fn ssl_export_keying_material(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let label = bytes(args, 1)?.to_vec();
    let len = usize::try_from(int(args, 2)?)
        .map_err(|_| plain(Class::Argument, "negative string size (or size too big)"))?;
    let context = opt_bytes(args, 3)?.map(<[u8]>::to_vec);
    let ssl = contexts.ssl(id(args, 0)?)?.ssl.as_ptr();
    let mut out = vec![0u8; len];
    let (cp, cl) = context
        .as_ref()
        .map_or((ptr::null(), 0), |c| (c.as_ptr(), c.len()));
    // SAFETY: a live `SSL`, `len` writable bytes, and readable inputs.
    let ok = unsafe {
        ffi::SSL_export_keying_material(
            ssl,
            out.as_mut_ptr(),
            len,
            label.as_ptr().cast(),
            label.len(),
            cp,
            cl,
            c_int::from(context.is_some()),
        )
    };
    if ok != 1 {
        return Err(failure(Class::Ssl, b"SSL_export_keying_material"));
    }
    Ok(Out::Bytes(out))
}

// ---------------------------------------------------------------------------
// Sessions (ossl_ssl_session.c)
// ---------------------------------------------------------------------------

/// `session_read(data)`: `Session.new(der or pem)`, DER first.
fn session_read(contexts: &mut Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let data = bytes(args, 0)?;
    let bio = ReadBio::new(data)?;
    // SAFETY: a live BIO and `data`'s bytes.
    let s = unsafe {
        let mut p = data.as_ptr();
        let mut s = ffi::d2i_SSL_SESSION(
            ptr::null_mut(),
            &mut p,
            c_long::try_from(data.len()).unwrap_or(0),
        );
        if s.is_null() {
            bio.reset();
            s = ffi::PEM_read_bio_SSL_SESSION(
                bio.0,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
        }
        s
    };
    let s = NonNull::new(s).ok_or_else(|| failure(Class::Argument, b"unknown type"))?;
    Ok(id_out(contexts.add(Object::Session(s))))
}

/// `session_get(id, field)`.
fn session_get(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let s = contexts.session(id(args, 0)?)?;
    // SAFETY: a live session; what it answers is copied.
    unsafe {
        match bytes(args, 1)? {
            b"time" => Ok(Out::Int(ffi::SSL_SESSION_get_time(s))),
            b"timeout" => Ok(Out::Int(ffi::SSL_SESSION_get_timeout(s))),
            b"protocol" => Ok(Out::Int(i64::from(ffi::SSL_SESSION_get_protocol_version(
                s,
            )))),
            b"id" => {
                let mut len = 0;
                let p = ffi::SSL_SESSION_get_id(s, &mut len);
                Ok(Out::Bytes(if p.is_null() {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(p, len as usize).to_vec()
                }))
            }
            b"der" => der_of(s, ffi::i2d_SSL_SESSION)
                .map(Out::Bytes)
                .ok_or_else(|| failure(Class::Session, b"i2d_SSL_SESSION")),
            b"pem" => {
                let bio = WriteBio::new()?;
                if ffi::PEM_write_bio_SSL_SESSION(bio.0, s) != 1 {
                    return Err(failure(Class::Session, b"SSL_SESSION_print()"));
                }
                Ok(Out::Text(bio.take()))
            }
            b"text" => {
                let bio = WriteBio::new()?;
                if ffi::SSL_SESSION_print(bio.0, s) != 1 {
                    return Err(failure(Class::Session, b"SSL_SESSION_print()"));
                }
                Ok(Out::Text(bio.take()))
            }
            _ => Err(bad("an unknown session field")),
        }
    }
}

/// `session_set(id, field, value)`.
fn session_set(contexts: &Contexts, args: &[Arg]) -> Result<Out, Failure> {
    let s = contexts.session(id(args, 0)?)?;
    // SAFETY: a live session.
    unsafe {
        match bytes(args, 1)? {
            b"time" => {
                ffi::SSL_SESSION_set_time(s, c_long::try_from(int(args, 2)?).unwrap_or(0));
            }
            b"timeout" => {
                ffi::SSL_SESSION_set_timeout(s, c_long::try_from(int(args, 2)?).unwrap_or(0));
            }
            b"id" => {
                let sid = bytes(args, 2)?;
                if ffi::SSL_SESSION_set1_id(
                    s,
                    sid.as_ptr(),
                    c_uint::try_from(sid.len()).unwrap_or(0),
                ) != 1
                {
                    return Err(failure(Class::Session, b"SSL_SESSION_set1_id"));
                }
            }
            _ => return Err(bad("an unknown session field")),
        }
    }
    Ok(Out::Nil)
}
