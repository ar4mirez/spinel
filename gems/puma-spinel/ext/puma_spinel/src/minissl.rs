//! MiniSSL: `mini_ssl.c`'s OpenSSL calls, on the system's OpenSSL 3, one
//! method each, made in the C's order with the C's arguments.
//!
//! An SSL context is a box around an `SSL_CTX`, which `ctx_*` set up a step
//! at a time as `sslctx_initialize` does, the Ruby side reading each of
//! `Puma::MiniSSL::Context`'s settings where the C calls `rb_funcall` for
//! it. An engine is a box around an `SSL` reading from and writing to two
//! memory BIOs, as `engine_alloc` makes it: Puma's Ruby moves the bytes
//! between those and the socket. Nothing here blocks or calls into Ruby;
//! the verify callback keeps the certificate it fails on, as the C's does.
//!
//! A failure answers the message the C would raise `Puma::MiniSSL::SSLError`
//! with, which the Ruby side raises.

#![allow(clippy::too_many_lines)]

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::ptr;

use openssl_sys as sys;
use spinel_ext::{Context, Error, Result, Value};

use crate::{Packer, boxed, handle, methods};

/// What an SSL context box starts with.
const CTX_TAG: u64 = 0x5350_494e_504d_5343;
/// What an engine box starts with.
const ENGINE_TAG: u64 = 0x5350_494e_504d_5345;

// OpenSSL's macros over `*_ctrl`, and functions openssl-sys leaves out.
const SSL_CTRL_SET_SESS_CACHE_SIZE: c_int = 42;
const SSL_CTRL_SET_SESS_CACHE_MODE: c_int = 44;
const SSL_CTRL_CHAIN: c_int = 88;
const SSL_CTRL_CHAIN_CERT: c_int = 89;
const SSL_CTRL_SET_DH_AUTO: c_int = 118;
const SSL_SESS_CACHE_OFF: c_long = 0;
const SSL_SESS_CACHE_SERVER: c_long = 2;
const SSL_MAX_SSL_SESSION_ID_LENGTH: usize = 32;
const BIO_C_SET_NBIO: c_int = 102;
const BIO_CTRL_PENDING: c_int = 10;
const BIO_FLAGS_SHOULD_RETRY: c_int = 0x08;
const SSL_WRITING: c_int = 2;
const ERR_LIB_PEM: c_int = 9;
const PEM_R_NO_START_LINE: c_int = 108;
const SSL_R_NO_CIPHER_MATCH: c_int = 185;
const X509_V_OK: c_long = 0;

unsafe extern "C" {
    fn SSL_in_init(s: *const sys::SSL) -> c_int;
    fn SSL_want(s: *const sys::SSL) -> c_int;
    fn BIO_test_flags(b: *const sys::BIO, flags: c_int) -> c_int;
    fn BIO_puts(b: *mut sys::BIO, buf: *const c_char) -> c_int;
    fn SSL_CTX_set_timeout(ctx: *mut sys::SSL_CTX, t: c_long) -> c_long;
    fn SSL_CTX_set1_param(ctx: *mut sys::SSL_CTX, vpm: *mut sys::X509_VERIFY_PARAM) -> c_int;
    fn SSL_CTX_set_default_passwd_cb_userdata(ctx: *mut sys::SSL_CTX, u: *mut c_void);
    fn ERR_error_string_n(e: c_ulong, buf: *mut c_char, len: usize);
    fn ERR_error_string(e: c_ulong, buf: *mut c_char) -> *mut c_char;
    fn BIO_free(b: *mut sys::BIO) -> c_int;
    fn SSL_CTX_set_default_passwd_cb(ctx: *mut sys::SSL_CTX, cb: sys::pem_password_cb);
}

/// An `SSL_CTX`.
struct CtxBox {
    tag: u64,
    ctx: *mut sys::SSL_CTX,
}

/// `ms_conn`.
struct EngineBox {
    tag: u64,
    ssl: *mut sys::SSL,
    /// The client's own context, null for a server.
    ctx: *mut sys::SSL_CTX,
}

/// `ms_cert_buf`: the DER of the certificate verification failed on.
struct CertBuf {
    buf: *mut u8,
    bytes: c_int,
}

pub fn init<'cx>(cx: &mut Context<'cx>, spinel: Value<'cx>) -> Result<()> {
    // SAFETY: OpenSSL's own initialization, as `Init_mini_ssl` calls it.
    sys::init();
    let table: &[(&str, i32, spinel_ext::Method)] = &[
        ("ssl_constants", 0, ssl_constants),
        ("ctx_new", 0, ctx_new),
        ("ctx_free", 1, ctx_free),
        (
            "ctx_use_certificate_chain_file",
            2,
            ctx_use_certificate_chain_file,
        ),
        ("ctx_use_private_key_file", 3, ctx_use_private_key_file),
        ("ctx_cert_pem", 2, ctx_cert_pem),
        ("ctx_key_pem", 3, ctx_key_pem),
        ("ctx_verification_flags", 2, ctx_verification_flags),
        ("ctx_load_verify_locations", 2, ctx_load_verify_locations),
        ("ctx_finish", 10, ctx_finish),
        ("engine_server", 1, engine_server),
        ("engine_client", 0, engine_client),
        ("engine_free", 1, engine_free),
        ("engine_inject", 2, engine_inject),
        ("engine_read", 1, engine_read),
        ("engine_write", 2, engine_write),
        ("engine_extract", 1, engine_extract),
        ("engine_shutdown", 1, engine_shutdown),
        ("engine_init?", 1, engine_init),
        ("engine_peercert", 1, engine_peercert),
        ("engine_ssl_vers_st", 1, engine_ssl_vers_st),
    ];
    for &(name, arity, method) in table {
        cx.define_singleton_method(spinel, name, arity, method)?;
    }
    Ok(())
}

fn ctx<'cx, 'a>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<&'a mut CtxBox> {
    boxed(cx, value, CTX_TAG, "SSL context")
}

fn engine<'cx, 'a>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<&'a mut EngineBox> {
    boxed(cx, value, ENGINE_TAG, "engine")
}

/// A Ruby String as the C reads `RSTRING_PTR`: up to its first NUL.
fn c_string(bytes: Vec<u8>) -> CString {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    CString::new(&bytes[..end]).expect("no NUL before the end")
}

/// `ERR_error_string(e, NULL)`.
fn err_string(e: c_ulong) -> String {
    let mut buf = [0 as c_char; 256];
    // SAFETY: OpenSSL writes at most 256 bytes, NUL included, into `buf`.
    unsafe {
        ERR_error_string(e, buf.as_mut_ptr());
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

/// `raise_file_error(caller, filename)`'s message.
fn file_error(caller: &str, filename: &CStr) -> String {
    // SAFETY: takes the oldest error off this thread's queue.
    let e = unsafe { sys::ERR_get_error() };
    format!(
        "{caller}: error in file '{}': {}",
        filename.to_string_lossy(),
        err_string(e)
    )
}

/// `raise_param_error(caller, param)`'s message.
fn param_error(caller: &str, param: &str) -> String {
    // SAFETY: takes the oldest error off this thread's queue.
    let e = unsafe { sys::ERR_get_error() };
    format!(
        "{caller}: error with parameter '{param}': {}",
        err_string(e)
    )
}

/// `password_callback`: the password in `userdata`, a C string.
extern "C" fn password_callback(
    buf: *mut c_char,
    size: c_int,
    _rwflag: c_int,
    userdata: *mut c_void,
) -> c_int {
    // SAFETY: `userdata` is the NUL-terminated password the caller set,
    // alive for the call it is set around; `buf` holds `size` bytes.
    unsafe {
        let password = CStr::from_ptr(userdata.cast::<c_char>()).to_bytes();
        let Ok(len) = c_int::try_from(password.len()) else {
            return 0;
        };
        if len > size {
            return 0;
        }
        ptr::copy_nonoverlapping(password.as_ptr().cast::<c_char>(), buf, password.len());
        len
    }
}

/// `engine_verify_callback`: on a failure, keeps the DER of the certificate
/// it failed on in the `SSL`'s app data for `engine_peercert`.
extern "C" fn verify_callback(preverify_ok: c_int, ctx: *mut sys::X509_STORE_CTX) -> c_int {
    if preverify_ok == 0 {
        // SAFETY: OpenSSL's store context for the handshake in progress; the
        // `SSL` behind it is the engine's, whose app data is null or a
        // `CertBuf` this callback put there, freed with the engine.
        unsafe {
            let err_cert = sys::X509_STORE_CTX_get_current_cert(ctx);
            if !err_cert.is_null() {
                let mut buf: *mut u8 = ptr::null_mut();
                let bytes = sys::i2d_X509(err_cert, &mut buf);
                if bytes > 0 {
                    let ssl = sys::X509_STORE_CTX_get_ex_data(
                        ctx,
                        sys::SSL_get_ex_data_X509_STORE_CTX_idx(),
                    )
                    .cast::<sys::SSL>();
                    // The C sets a new one each time and leaks the last.
                    let cert_buf = Box::into_raw(Box::new(CertBuf { buf, bytes }));
                    sys::SSL_set_ex_data(ssl, 0, cert_buf.cast::<c_void>());
                }
            }
        }
    }
    preverify_ok
}

/// `raise_error(ssl, result)`'s message, `errno` as it was after the call.
fn ssl_error(ssl: *mut sys::SSL, result: c_int, errno: c_int) -> String {
    // SAFETY: an engine's `SSL`.
    let (ssl_err, verify_err) = unsafe {
        (
            sys::SSL_get_error(ssl, result),
            sys::SSL_get_verify_result(ssl),
        )
    };
    let msg = if ssl_err == sys::SSL_ERROR_SYSCALL {
        // SAFETY: strerror answers a static or thread-local C string.
        let text = unsafe { CStr::from_ptr(libc::strerror(errno)) }.to_string_lossy();
        format!("System error: {text} - {errno}")
    } else if ssl_err == sys::SSL_ERROR_SSL {
        if verify_err != X509_V_OK {
            // SAFETY: OpenSSL answers a static string for any code.
            let text = unsafe { CStr::from_ptr(sys::X509_verify_cert_error_string(verify_err)) }
                .to_string_lossy();
            format!("OpenSSL certificate verification error: {text} - {verify_err}")
        } else {
            // SAFETY: as `ERR_error_string_n` writes at most 512 bytes.
            let (text, err) = unsafe {
                let err = sys::ERR_get_error() as c_int;
                let mut buf = [0 as c_char; 512];
                ERR_error_string_n(err as c_ulong, buf.as_mut_ptr(), buf.len());
                (
                    CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned(),
                    err,
                )
            };
            format!("OpenSSL error: {text} - {}", err & 4095)
        }
    } else {
        format!("Unknown OpenSSL error: {ssl_err}")
    };
    // SAFETY: clears this thread's error queue.
    unsafe { sys::ERR_clear_error() };
    msg
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// An optional Integer argument as the C's `int`, the Ruby side having
/// checked it with `NUM2INT`.
fn int<'cx>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<Option<c_int>> {
    if cx.is_nil(value) {
        return Ok(None);
    }
    let n = cx.to_i64(value)?;
    c_int::try_from(n).map(Some).map_err(|_| {
        Error::new(
            "RangeError",
            format!("integer {n} too big to convert to 'int'"),
        )
    })
}

fn answer<'cx>(cx: &mut Context<'cx>, failure: Option<String>) -> Value<'cx> {
    match failure {
        Some(message) => cx.string(&message),
        None => cx.nil(),
    }
}

methods! {
    /// What `Init_mini_ssl` defines as constants: the version OpenSSL's
    /// headers name, the version the library loaded is, and whether SSL 3,
    /// TLS 1 and TLS 1.1 were left out of the build, packed as records.
    fn ssl_constants(cx, _args) {
        // SAFETY: OpenSSL answers a static string.
        let library = unsafe { CStr::from_ptr(sys::OpenSSL_version(sys::OPENSSL_VERSION)) };
        let mut packer = Packer::default();
        packer.record(
            b'C',
            &[
                env!("PUMA_SPINEL_OPENSSL_VERSION_TEXT").as_bytes(),
                library.to_bytes(),
                env!("PUMA_SPINEL_OPENSSL_NO_SSL3").as_bytes(),
                env!("PUMA_SPINEL_OPENSSL_NO_TLS1").as_bytes(),
                env!("PUMA_SPINEL_OPENSSL_NO_TLS1_1").as_bytes(),
            ],
        );
        Ok(cx.string_from_bytes(packer.bytes()))
    }

    /// `sslctx_alloc`: a new `SSL_CTX` on `TLS_method()` with Puma's modes,
    /// as a handle; nil when `SSL_CTX_new` fails.
    fn ctx_new(cx, _args) {
        // SAFETY: a new context, owned by the box.
        let ctx = unsafe { sys::SSL_CTX_new(sys::TLS_method()) };
        if ctx.is_null() {
            return Ok(cx.nil());
        }
        let mode = sys::SSL_MODE_ENABLE_PARTIAL_WRITE
            | sys::SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER
            | sys::SSL_MODE_RELEASE_BUFFERS;
        // SAFETY: the new context.
        unsafe { sys::SSL_CTX_set_mode(ctx, mode) };
        Ok(handle(cx, Box::new(CtxBox { tag: CTX_TAG, ctx })))
    }

    /// `sslctx_free`.
    fn ctx_free(cx, args) {
        let boxed = ctx(cx, args[0])?;
        // SAFETY: the box's own reference, freed once.
        unsafe { sys::SSL_CTX_free(boxed.ctx) };
        boxed.tag = 0;
        // SAFETY: a box `ctx_new` made, freed once.
        drop(unsafe { Box::from_raw(ptr::from_mut(boxed)) });
        Ok(cx.nil())
    }

    /// `SSL_CTX_use_certificate_chain_file(ctx, cert)`: nil, or the
    /// message.
    fn ctx_use_certificate_chain_file(cx, args) {
        let ctx = ctx(cx, args[0])?.ctx;
        let cert = c_string(cx.to_bytes(args[1])?);
        // SAFETY: the context and a C string.
        let ok = unsafe { sys::SSL_CTX_use_certificate_chain_file(ctx, cert.as_ptr()) };
        let failure = (ok != 1).then(|| file_error("SSL_CTX_use_certificate_chain_file", &cert));
        Ok(answer(cx, failure))
    }

    /// `SSL_CTX_use_PrivateKey_file(ctx, key, SSL_FILETYPE_PEM)` with the
    /// password callback set around it when there is a password: nil, or
    /// the message.
    fn ctx_use_private_key_file(cx, args) {
        let ctx = ctx(cx, args[0])?.ctx;
        let key = c_string(cx.to_bytes(args[1])?);
        let password = if cx.is_nil(args[2]) { None } else { Some(c_string(cx.to_bytes(args[2])?)) };
        // SAFETY: the context; the password outlives the call it is set
        // around, and both are unset after.
        let result = unsafe {
            match &password {
                Some(password) => {
                    SSL_CTX_set_default_passwd_cb(ctx, Some(password_callback));
                    SSL_CTX_set_default_passwd_cb_userdata(ctx, password.as_ptr().cast_mut().cast());
                }
                None => {
                    SSL_CTX_set_default_passwd_cb(ctx, None);
                    SSL_CTX_set_default_passwd_cb_userdata(ctx, ptr::null_mut());
                }
            }
            let result = sys::SSL_CTX_use_PrivateKey_file(ctx, key.as_ptr(), sys::SSL_FILETYPE_PEM);
            SSL_CTX_set_default_passwd_cb(ctx, None);
            SSL_CTX_set_default_passwd_cb_userdata(ctx, ptr::null_mut());
            result
        };
        let failure = (result != 1).then(|| file_error("SSL_CTX_use_PrivateKey_file", &key));
        Ok(answer(cx, failure))
    }

    /// The `cert_pem` block: the first certificate as the context's, the
    /// rest as its chain. Nil, or the message.
    fn ctx_cert_pem(cx, args) {
        let ctx = ctx(cx, args[0])?.ctx;
        let pem = c_string(cx.to_bytes(args[1])?);
        // SAFETY: a memory BIO freed on every path; certificates the context
        // takes its own reference to, or owns once added to its chain.
        let failure = unsafe {
            let bio = sys::BIO_new(sys::BIO_s_mem());
            BIO_puts(bio, pem.as_ptr());
            let x509 = sys::PEM_read_bio_X509(bio, ptr::null_mut(), None, ptr::null_mut());
            'block: {
                if x509.is_null() {
                    sys::BIO_free_all(bio);
                    break 'block Some(param_error("PEM_read_bio_X509", "cert_pem"));
                }
                if sys::SSL_CTX_use_certificate(ctx, x509) != 1 {
                    sys::BIO_free_all(bio);
                    break 'block Some(param_error("SSL_CTX_use_certificate", "cert_pem"));
                }
                sys::X509_free(x509);
                if sys::SSL_CTX_ctrl(ctx, SSL_CTRL_CHAIN, 0, ptr::null_mut()) == 0 {
                    sys::BIO_free_all(bio);
                    break 'block Some(param_error("SSL_CTX_clear_chain_certs", "cert_pem"));
                }
                loop {
                    let ca = sys::PEM_read_bio_X509(bio, ptr::null_mut(), None, ptr::null_mut());
                    if ca.is_null() {
                        break;
                    }
                    if sys::SSL_CTX_ctrl(ctx, SSL_CTRL_CHAIN_CERT, 0, ca.cast()) == 0 {
                        sys::BIO_free_all(bio);
                        break 'block Some(param_error("SSL_CTX_add0_chain_cert", "cert_pem"));
                    }
                }
                let err = sys::ERR_peek_last_error();
                sys::BIO_free_all(bio);
                if sys::ERR_GET_LIB(err) == ERR_LIB_PEM && sys::ERR_GET_REASON(err) == PEM_R_NO_START_LINE {
                    sys::ERR_clear_error();
                    None
                } else {
                    Some(param_error("PEM_read_bio_X509", "cert_pem"))
                }
            }
        };
        Ok(answer(cx, failure))
    }

    /// The `key_pem` block: `PEM_read_bio_PrivateKey` with the password,
    /// if any, then `SSL_CTX_use_PrivateKey`. Nil, or the message.
    fn ctx_key_pem(cx, args) {
        let ctx = ctx(cx, args[0])?.ctx;
        let pem = c_string(cx.to_bytes(args[1])?);
        let password = if cx.is_nil(args[2]) { None } else { Some(c_string(cx.to_bytes(args[2])?)) };
        // SAFETY: a memory BIO and a key freed on every path; the password
        // outlives the read.
        let failure = unsafe {
            let bio = sys::BIO_new(sys::BIO_s_mem());
            BIO_puts(bio, pem.as_ptr());
            let (cb, userdata): (sys::pem_password_cb, *mut c_void) = match &password {
                Some(password) => (Some(password_callback), password.as_ptr().cast_mut().cast()),
                None => (None, ptr::null_mut()),
            };
            let pkey = sys::PEM_read_bio_PrivateKey(bio, ptr::null_mut(), cb, userdata);
            if sys::SSL_CTX_use_PrivateKey(ctx, pkey) != 1 {
                BIO_free(bio);
                Some(file_error("SSL_CTX_use_PrivateKey", &pem))
            } else {
                sys::EVP_PKEY_free(pkey);
                BIO_free(bio);
                None
            }
        };
        Ok(answer(cx, failure))
    }

    /// `X509_VERIFY_PARAM_set_flags` on the context's parameters.
    fn ctx_verification_flags(cx, args) {
        let ctx = ctx(cx, args[0])?.ctx;
        let flags = int(cx, args[1])?.unwrap_or(0);
        // SAFETY: the context's own parameters.
        unsafe {
            let param = sys::SSL_CTX_get0_param(ctx);
            sys::X509_VERIFY_PARAM_set_flags(param, flags as c_ulong);
            SSL_CTX_set1_param(ctx, param);
        }
        Ok(cx.nil())
    }

    /// `SSL_CTX_load_verify_locations(ctx, ca, NULL)`: nil, or the message.
    fn ctx_load_verify_locations(cx, args) {
        let ctx = ctx(cx, args[0])?.ctx;
        let ca = c_string(cx.to_bytes(args[1])?);
        // SAFETY: the context and a C string.
        let ok = unsafe { sys::SSL_CTX_load_verify_locations(ctx, ca.as_ptr(), ptr::null()) };
        let failure = (ok != 1).then(|| file_error("SSL_CTX_load_verify_locations", &ca));
        Ok(answer(cx, failure))
    }

    /// The rest of `sslctx_initialize`, from the options on:
    /// `ctx_finish(ctx, no_tlsv1, no_tlsv1_1, reuse, reuse_cache_size,
    /// reuse_timeout, ssl_cipher_filter, ssl_ciphersuites, verify_mode,
    /// session_id_bytes)`, the last the 32 random bytes the Ruby side drew.
    /// Nil, or the message.
    fn ctx_finish(cx, args) {
        let ctx = ctx(cx, args[0])?.ctx;
        let no_tlsv1 = cx.is_truthy(args[1]);
        let no_tlsv1_1 = cx.is_truthy(args[2]);
        let reuse = !cx.is_nil(args[3]);
        let cache_size = int(cx, args[4])?;
        let timeout = int(cx, args[5])?;
        let cipher_filter = if cx.is_nil(args[6]) { None } else { Some(c_string(cx.to_bytes(args[6])?)) };
        let ciphersuites = if cx.is_nil(args[7]) { None } else { Some(c_string(cx.to_bytes(args[7])?)) };
        let verify_mode = int(cx, args[8])?;
        let session_id = cx.to_bytes(args[9])?;
        if session_id.len() < SSL_MAX_SSL_SESSION_ID_LENGTH {
            return Err(Error::new("ArgumentError", "a session id context is 32 bytes"));
        }

        // SAFETY: the context, and C strings that outlive each call.
        let failure = unsafe {
            let ssl_options = sys::SSL_OP_CIPHER_SERVER_PREFERENCE | sys::SSL_OP_NO_COMPRESSION;
            let min = if no_tlsv1_1 {
                sys::TLS1_2_VERSION
            } else if no_tlsv1 {
                sys::TLS1_1_VERSION
            } else {
                sys::TLS1_VERSION
            };
            sys::SSL_CTX_set_min_proto_version(ctx, min);

            if reuse {
                sys::SSL_CTX_ctrl(ctx, SSL_CTRL_SET_SESS_CACHE_MODE, SSL_SESS_CACHE_SERVER, ptr::null_mut());
                if let Some(size) = cache_size {
                    sys::SSL_CTX_ctrl(ctx, SSL_CTRL_SET_SESS_CACHE_SIZE, c_long::from(size), ptr::null_mut());
                }
                if let Some(timeout) = timeout {
                    SSL_CTX_set_timeout(ctx, c_long::from(timeout));
                }
            } else {
                sys::SSL_CTX_ctrl(ctx, SSL_CTRL_SET_SESS_CACHE_MODE, SSL_SESS_CACHE_OFF, ptr::null_mut());
            }

            sys::SSL_CTX_set_options(ctx, ssl_options);

            'block: {
                if let Some(filter) = &cipher_filter {
                    sys::ERR_clear_error();
                    if sys::SSL_CTX_set_cipher_list(ctx, filter.as_ptr()) != 1 {
                        if sys::ERR_GET_REASON(sys::ERR_peek_last_error()) == SSL_R_NO_CIPHER_MATCH {
                            sys::ERR_clear_error();
                        } else {
                            break 'block Some(param_error("SSL_CTX_set_cipher_list", "ssl_cipher_filter"));
                        }
                    }
                } else {
                    sys::SSL_CTX_set_cipher_list(ctx, c"HIGH:!aNULL@STRENGTH".as_ptr());
                }
                if let Some(suites) = &ciphersuites {
                    sys::SSL_CTX_set_ciphersuites(ctx, suites.as_ptr());
                }

                if let Some(mode) = verify_mode {
                    sys::SSL_CTX_set_verify(ctx, mode, Some(verify_callback));
                }

                sys::SSL_CTX_set_session_id_context(
                    ctx,
                    session_id.as_ptr(),
                    SSL_MAX_SSL_SESSION_ID_LENGTH as c_uint,
                );
                sys::SSL_CTX_ctrl(ctx, SSL_CTRL_SET_DH_AUTO, 1, ptr::null_mut());
                None
            }
        };
        Ok(answer(cx, failure))
    }

    /// `engine_init_server(ctx)`: an engine accepting on the context.
    fn engine_server(cx, args) {
        let ctx = ctx(cx, args[0])?.ctx;
        // SAFETY: two memory BIOs the `SSL` owns once set; the `SSL` takes a
        // reference to the context.
        let ssl = unsafe {
            let (read, write) = bios();
            let ssl = sys::SSL_new(ctx);
            sys::SSL_set_ex_data(ssl, 0, ptr::null_mut());
            sys::SSL_set_bio(ssl, read, write);
            sys::SSL_set_accept_state(ssl);
            ssl
        };
        Ok(handle(cx, Box::new(EngineBox { tag: ENGINE_TAG, ssl, ctx: ptr::null_mut() })))
    }

    /// `engine_init_client`: an engine connecting on a DTLS context of its
    /// own, verifying nothing.
    fn engine_client(cx, _args) {
        // SAFETY: as `engine_server`, the context the engine's own.
        let (ssl, ctx) = unsafe {
            let (read, write) = bios();
            let ctx = sys::SSL_CTX_new(sys::DTLS_method());
            let ssl = sys::SSL_new(ctx);
            sys::SSL_set_ex_data(ssl, 0, ptr::null_mut());
            sys::SSL_set_verify(ssl, sys::SSL_VERIFY_NONE, None);
            sys::SSL_set_bio(ssl, read, write);
            sys::SSL_set_connect_state(ssl);
            (ssl, ctx)
        };
        Ok(handle(cx, Box::new(EngineBox { tag: ENGINE_TAG, ssl, ctx })))
    }

    /// `engine_free`.
    fn engine_free(cx, args) {
        let boxed = engine(cx, args[0])?;
        // SAFETY: the engine's `SSL`, its app data and its own context,
        // each freed once.
        unsafe {
            let cert_buf = sys::SSL_get_ex_data(boxed.ssl, 0).cast::<CertBuf>();
            if !cert_buf.is_null() {
                let cert_buf = Box::from_raw(cert_buf);
                sys::CRYPTO_free(cert_buf.buf.cast(), c"mini_ssl.rs".as_ptr(), 0);
            }
            sys::SSL_free(boxed.ssl);
            sys::SSL_CTX_free(boxed.ctx);
        }
        boxed.tag = 0;
        // SAFETY: a box `engine_*` made, freed once.
        drop(unsafe { Box::from_raw(ptr::from_mut(boxed)) });
        Ok(cx.nil())
    }

    /// `engine_inject(str)`: bytes written to the read BIO, or false.
    fn engine_inject(cx, args) {
        let ssl = engine(cx, args[0])?.ssl;
        let bytes = cx.to_bytes(args[1])?;
        let len = c_int::try_from(bytes.len()).unwrap_or(c_int::MAX);
        // SAFETY: the engine's read BIO and the bytes.
        let used = unsafe { sys::BIO_write(sys::SSL_get_rbio(ssl), bytes.as_ptr().cast(), len) };
        if used == 0 || used == -1 {
            return Ok(cx.boolean(false));
        }
        Ok(cx.integer(i64::from(used)))
    }

    /// `engine_read`: a record, `D` and the bytes read, `N` for none yet,
    /// `O` for the peer's close (the C's `EOFError`), `E` and the message.
    fn engine_read(cx, args) {
        let ssl = engine(cx, args[0])?.ssl;
        let mut buf = [0u8; 512];
        let mut packer = Packer::default();
        // SAFETY: the engine's `SSL` reading into `buf`.
        unsafe {
            sys::ERR_clear_error();
            let bytes = sys::SSL_read(ssl, buf.as_mut_ptr().cast(), buf.len() as c_int);
            let errno = errno();
            if bytes > 0 {
                packer.record(b'D', &[&buf[..bytes as usize]]);
            } else {
                let error = sys::SSL_get_error(ssl, bytes);
                if error == sys::SSL_ERROR_WANT_READ {
                    packer.record(b'N', &[]);
                } else if error == sys::SSL_ERROR_ZERO_RETURN {
                    packer.record(b'O', &[]);
                } else {
                    packer.record(b'E', &[ssl_error(ssl, bytes, errno).as_bytes()]);
                }
            }
        }
        Ok(cx.string_from_bytes(packer.bytes()))
    }

    /// `engine_write(str)`: the bytes written, nil when OpenSSL wants to
    /// write first, or the message as a String.
    fn engine_write(cx, args) {
        let ssl = engine(cx, args[0])?.ssl;
        let bytes = cx.to_bytes(args[1])?;
        let len = c_int::try_from(bytes.len()).unwrap_or(c_int::MAX);
        // SAFETY: the engine's `SSL` writing `bytes`.
        unsafe {
            sys::ERR_clear_error();
            let written = sys::SSL_write(ssl, bytes.as_ptr().cast(), len);
            let errno = errno();
            if written > 0 {
                return Ok(cx.integer(i64::from(written)));
            }
            if SSL_want(ssl) == SSL_WRITING {
                return Ok(cx.nil());
            }
            Ok(cx.string(&ssl_error(ssl, written, errno)))
        }
    }

    /// `engine_extract`: a record, `D` and up to 4096 bytes OpenSSL wrote,
    /// `N` for none, `E` and the message.
    fn engine_extract(cx, args) {
        let ssl = engine(cx, args[0])?.ssl;
        let mut buf = [0u8; 4096];
        let mut packer = Packer::default();
        // SAFETY: the engine's write BIO reading into `buf`.
        unsafe {
            let write = sys::SSL_get_wbio(ssl);
            let pending = sys::BIO_ctrl(write, BIO_CTRL_PENDING, 0, ptr::null_mut());
            if pending > 0 {
                let bytes = sys::BIO_read(write, buf.as_mut_ptr().cast(), buf.len() as c_int);
                let errno = errno();
                if bytes > 0 {
                    packer.record(b'D', &[&buf[..bytes as usize]]);
                } else if BIO_test_flags(write, BIO_FLAGS_SHOULD_RETRY) == 0 {
                    packer.record(b'E', &[ssl_error(ssl, bytes, errno).as_bytes()]);
                } else {
                    packer.record(b'N', &[]);
                }
            } else {
                packer.record(b'N', &[]);
            }
        }
        Ok(cx.string_from_bytes(packer.bytes()))
    }

    /// `engine_shutdown`: true or false as `SSL_shutdown` answers, true
    /// while the handshake is in progress, or the message as a String.
    fn engine_shutdown(cx, args) {
        let ssl = engine(cx, args[0])?.ssl;
        // SAFETY: the engine's `SSL`.
        unsafe {
            if SSL_in_init(ssl) != 0 {
                return Ok(cx.boolean(true));
            }
            sys::ERR_clear_error();
            let ok = sys::SSL_shutdown(ssl);
            let errno = errno();
            match ok {
                0 => Ok(cx.boolean(false)),
                1 => Ok(cx.boolean(true)),
                _ => Ok(cx.string(&ssl_error(ssl, ok, errno))),
            }
        }
    }

    /// `engine_init`: whether the handshake is in progress.
    fn engine_init(cx, args) {
        let ssl = engine(cx, args[0])?.ssl;
        // SAFETY: the engine's `SSL`.
        let init = unsafe { SSL_in_init(ssl) } != 0;
        Ok(cx.boolean(init))
    }

    /// `engine_peercert`: the peer's certificate as DER, the one
    /// verification failed on when there is none, or nil.
    fn engine_peercert(cx, args) {
        let ssl = engine(cx, args[0])?.ssl;
        // SAFETY: the engine's `SSL`; a certificate reference and the DER
        // OpenSSL allocates, both freed, or the app data's, kept.
        unsafe {
            let cert = sys::SSL_get1_peer_certificate(ssl);
            if cert.is_null() {
                let cert_buf = sys::SSL_get_ex_data(ssl, 0).cast::<CertBuf>();
                if cert_buf.is_null() {
                    return Ok(cx.nil());
                }
                let cert_buf = &*cert_buf;
                let der = std::slice::from_raw_parts(cert_buf.buf, cert_buf.bytes as usize);
                return Ok(cx.string_from_bytes(der));
            }
            let mut buf: *mut u8 = ptr::null_mut();
            let bytes = sys::i2d_X509(cert, &mut buf);
            sys::X509_free(cert);
            if bytes < 0 {
                return Ok(cx.nil());
            }
            let value = cx.string_from_bytes(std::slice::from_raw_parts(buf, bytes as usize));
            sys::CRYPTO_free(buf.cast(), c"mini_ssl.rs".as_ptr(), 0);
            Ok(value)
        }
    }

    /// `engine_ssl_vers_st`: the protocol's name and the handshake's state,
    /// packed.
    fn engine_ssl_vers_st(cx, args) {
        let ssl = engine(cx, args[0])?.ssl;
        // SAFETY: OpenSSL answers static strings.
        let (version, state) = unsafe {
            (
                CStr::from_ptr(sys::SSL_get_version(ssl)).to_bytes(),
                CStr::from_ptr(sys::SSL_state_string(ssl)).to_bytes(),
            )
        };
        let mut packer = Packer::default();
        packer.record(b'S', &[version, state]);
        Ok(cx.string_from_bytes(packer.bytes()))
    }
}

/// `engine_alloc`'s two nonblocking memory BIOs: read, then write.
unsafe fn bios() -> (*mut sys::BIO, *mut sys::BIO) {
    // SAFETY: new BIOs, owned by the `SSL` they are set on.
    unsafe {
        let read = sys::BIO_new(sys::BIO_s_mem());
        sys::BIO_ctrl(read, BIO_C_SET_NBIO, 1, ptr::null_mut());
        let write = sys::BIO_new(sys::BIO_s_mem());
        sys::BIO_ctrl(write, BIO_C_SET_NBIO, 1, ptr::null_mut());
        (read, write)
    }
}
