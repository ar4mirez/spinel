//! The native half of the pg-spinel gem: libpq's C API as singleton methods
//! of `PG::Spinel`, one libpq call each, answering what libpq answers.
//! Everything the pg gem's C extension decides on top of those calls —
//! which exception a status raises and with what message, what a row is,
//! how a parameter is encoded, which encoding a string is in — is Ruby, in
//! `lib/pg_ext.rb`.
//!
//! A connection, a result and a cancel connection are boxes the Ruby side
//! holds by an Integer handle, the box's address: [`Conn`] around a
//! `PGconn*`, [`Res`] around a `PGresult*`, [`Cancon`] around a
//! `PGcancelConn*`. `*_new` (or a call that makes a result) makes one,
//! `*_free` (from the holder's finalizer) frees it.
//!
//! What crosses as more than one value is packed in a String: a list of
//! entries, each a little-endian 32-bit length, -1 for nil, and that many
//! bytes (`Packer`, `unpack`, and `PG::Spinel.unpack` on the Ruby side).
//!
//! What libpq added after PostgreSQL 10 is looked up in the loaded library
//! by name when called ([`optional`]), where the pg gem decides at compile
//! time which of its methods exist; `has` tells the Ruby side.
//!
//! ponytail: a handle is an address, so a forged or freed one passed back
//! is undefined behavior, which a tag in each box only catches when the
//! address still holds a box. Only `pg_ext.rb` passes them, from instance
//! variables a finalizer frees after. Upgrade: an object with native data
//! and a free function in `spinel-ext`, as CRuby's typed data.

mod ffi;

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;

use spinel_ext::{Context, Error, Result, Value};

spinel_ext::extension!(init);

/// What a [`Conn`] box starts with, so that another box's handle passed as
/// a connection's is refused.
const CONN_TAG: u64 = 0x5350_494e_5047_434e;
/// What a [`Res`] box starts with.
const RES_TAG: u64 = 0x5350_494e_5047_5253;
/// What a [`Cancon`] box starts with.
const CANCON_TAG: u64 = 0x5350_494e_5047_4343;

/// A connection: null before it connects and after `conn_finish`.
struct Conn {
    tag: u64,
    raw: *mut ffi::PGconn,
    /// The stream `PQtrace` writes to, closed by `conn_untrace`.
    trace: *mut ffi::CFile,
}

/// A result: null after `res_clear`.
struct Res {
    tag: u64,
    raw: *mut ffi::PGresult,
}

/// A cancel connection (libpq 17): null after `cancon_finish`.
struct Cancon {
    tag: u64,
    raw: *mut ffi::PGcancelConn,
}

/// One method of `PG::Spinel`: `$cx`, and the arguments as `$args`.
macro_rules! methods {
    ($($(#[$meta:meta])* fn $name:ident($cx:ident, $args:ident) $body:block)*) => {
        $(
            $(#[$meta])*
            fn $name<'cx>(
                $cx: &mut Context<'cx>,
                _self: Value<'cx>,
                $args: &[Value<'cx>],
            ) -> Result<Value<'cx>> $body
        )*
    };
}

/// A libpq function added after PostgreSQL 10, by name, from the libpq
/// this extension is linked with: null when that libpq has none. Looked up
/// each time, so that nothing is kept between calls.
fn optional(name: &CStr) -> *mut c_void {
    // SAFETY: `PQlibVersion` is a function of the loaded libpq; `dladdr`
    // fills `info` for it, and `dlopen` of that library's own path answers
    // the loaded library (closed again at once, which only drops the count
    // `dlopen` added, as this extension's link keeps it loaded).
    unsafe {
        let mut info: libc::Dl_info = std::mem::zeroed();
        let anchor = ffi::PQlibVersion as *const c_void;
        if libc::dladdr(anchor, &raw mut info) == 0 || info.dli_fname.is_null() {
            return ptr::null_mut();
        }
        let library = libc::dlopen(info.dli_fname, libc::RTLD_LAZY);
        if library.is_null() {
            return ptr::null_mut();
        }
        let symbol = libc::dlsym(library, name.as_ptr());
        libc::dlclose(library);
        symbol
    }
}

/// Call the optional libpq function `$name` as a `$ty`, or answer
/// `NotImplementedError` when the libpq has none (the Ruby side defines
/// the method only when `has` says it is there).
macro_rules! call_optional {
    ($name:literal as $ty:ty, $($arg:expr),*) => {{
        let symbol = optional($name);
        if symbol.is_null() {
            return Err(Error::new(
                "NotImplementedError",
                format!("{} is not in this libpq", $name.to_string_lossy()),
            ));
        }
        // SAFETY: libpq's function of that name has this signature
        // (`libpq-fe.h`).
        let function: $ty = unsafe { std::mem::transmute::<*mut c_void, $ty>(symbol) };
        // SAFETY: as each call site says of its arguments.
        unsafe { function($($arg),*) }
    }};
}

fn init(cx: &mut Context<'_>) -> Result<()> {
    let pg = cx.define_module("PG")?;
    let spinel = cx.define_module_under(pg, "Spinel")?;
    let table: &[(&str, i32, spinel_ext::Method)] = &[
        ("has", 1, has),
        ("lib_version", 0, lib_version),
        ("isthreadsafe", 0, isthreadsafe),
        ("init_openssl", 2, init_openssl),
        ("init_ssl", 1, init_ssl),
        ("encoding_to_char", 1, encoding_to_char),
        ("res_status_name", 1, res_status_name),
        ("conndefaults", 0, conndefaults),
        ("conninfo_parse", 1, conninfo_parse),
        ("ping", 1, ping),
        ("encrypt_password", 2, encrypt_password),
        ("escape_string", 1, escape_string),
        ("escape_bytea", 1, escape_bytea),
        ("unescape_bytea", 1, unescape_bytea),
        ("conn_new", 0, conn_new),
        ("conn_free", 1, conn_free),
        ("conn_open?", 1, conn_is_open),
        ("conn_connectdb", 2, conn_connectdb),
        ("conn_connect_start", 2, conn_connect_start),
        ("conn_finish", 1, conn_finish),
        ("conn_connect_poll", 1, conn_connect_poll),
        ("conn_reset", 1, conn_reset),
        ("conn_reset_start", 1, conn_reset_start),
        ("conn_reset_poll", 1, conn_reset_poll),
        ("conn_db", 1, conn_db),
        ("conn_user", 1, conn_user),
        ("conn_pass", 1, conn_pass),
        ("conn_host", 1, conn_host),
        ("conn_hostaddr", 1, conn_hostaddr),
        ("conn_port", 1, conn_port),
        ("conn_options", 1, conn_options),
        ("conn_conninfo", 1, conn_conninfo),
        ("conn_status", 1, conn_status),
        ("conn_transaction_status", 1, conn_transaction_status),
        ("conn_parameter_status", 2, conn_parameter_status),
        ("conn_protocol_version", 1, conn_protocol_version),
        ("conn_full_protocol_version", 1, conn_full_protocol_version),
        ("conn_server_version", 1, conn_server_version),
        ("conn_error_message", 1, conn_error_message),
        ("conn_socket", 1, conn_socket),
        ("conn_backend_pid", 1, conn_backend_pid),
        ("conn_backend_key", 1, conn_backend_key),
        ("conn_needs_password", 1, conn_needs_password),
        ("conn_used_password", 1, conn_used_password),
        ("conn_ssl_in_use", 1, conn_ssl_in_use),
        ("conn_ssl_attribute", 2, conn_ssl_attribute),
        ("conn_ssl_attribute_names", 1, conn_ssl_attribute_names),
        ("conn_client_encoding", 1, conn_client_encoding),
        ("conn_set_client_encoding", 2, conn_set_client_encoding),
        ("conn_set_error_verbosity", 2, conn_set_error_verbosity),
        (
            "conn_set_error_context_visibility",
            2,
            conn_set_error_context_visibility,
        ),
        ("conn_trace", 2, conn_trace),
        ("conn_untrace", 1, conn_untrace),
        ("conn_cancel", 1, conn_cancel),
        ("conn_exec", 2, conn_exec),
        ("conn_exec_params", 6, conn_exec_params),
        ("conn_prepare", 4, conn_prepare),
        ("conn_exec_prepared", 5, conn_exec_prepared),
        ("conn_describe_prepared", 2, conn_describe_prepared),
        ("conn_describe_portal", 2, conn_describe_portal),
        ("conn_close_prepared", 2, conn_close_prepared),
        ("conn_close_portal", 2, conn_close_portal),
        ("conn_make_empty_result", 2, conn_make_empty_result),
        ("conn_escape_string", 2, conn_escape_string),
        ("conn_escape_literal", 2, conn_escape_literal),
        ("conn_escape_identifier", 2, conn_escape_identifier),
        ("conn_escape_bytea", 2, conn_escape_bytea),
        ("conn_encrypt_password", 4, conn_encrypt_password),
        ("conn_set_single_row_mode", 1, conn_set_single_row_mode),
        ("conn_set_chunked_rows_mode", 2, conn_set_chunked_rows_mode),
        ("conn_send_query", 2, conn_send_query),
        ("conn_send_query_params", 6, conn_send_query_params),
        ("conn_send_prepare", 4, conn_send_prepare),
        ("conn_send_query_prepared", 5, conn_send_query_prepared),
        (
            "conn_send_describe_prepared",
            2,
            conn_send_describe_prepared,
        ),
        ("conn_send_describe_portal", 2, conn_send_describe_portal),
        ("conn_send_close_prepared", 2, conn_send_close_prepared),
        ("conn_send_close_portal", 2, conn_send_close_portal),
        ("conn_get_result", 1, conn_get_result),
        ("conn_consume_input", 1, conn_consume_input),
        ("conn_is_busy", 1, conn_is_busy),
        ("conn_setnonblocking", 2, conn_setnonblocking),
        ("conn_isnonblocking", 1, conn_isnonblocking),
        ("conn_flush", 1, conn_flush),
        ("conn_notifies", 1, conn_notifies),
        ("conn_put_copy_data", 2, conn_put_copy_data),
        ("conn_put_copy_end", 2, conn_put_copy_end),
        ("conn_get_copy_data", 2, conn_get_copy_data),
        ("conn_pipeline_status", 1, conn_pipeline_status),
        ("conn_enter_pipeline_mode", 1, conn_enter_pipeline_mode),
        ("conn_exit_pipeline_mode", 1, conn_exit_pipeline_mode),
        ("conn_pipeline_sync", 1, conn_pipeline_sync),
        ("conn_send_pipeline_sync", 1, conn_send_pipeline_sync),
        ("conn_send_flush_request", 1, conn_send_flush_request),
        ("conn_lo_creat", 2, conn_lo_creat),
        ("conn_lo_create", 2, conn_lo_create),
        ("conn_lo_import", 2, conn_lo_import),
        ("conn_lo_export", 3, conn_lo_export),
        ("conn_lo_open", 3, conn_lo_open),
        ("conn_lo_write", 3, conn_lo_write),
        ("conn_lo_read", 3, conn_lo_read),
        ("conn_lo_lseek", 4, conn_lo_lseek),
        ("conn_lo_tell", 2, conn_lo_tell),
        ("conn_lo_truncate", 3, conn_lo_truncate),
        ("conn_lo_close", 2, conn_lo_close),
        ("conn_lo_unlink", 2, conn_lo_unlink),
        ("res_free", 1, res_free),
        ("res_clear", 1, res_clear),
        ("res_cleared?", 1, res_is_cleared),
        ("res_status", 1, res_status),
        ("res_error_message", 1, res_error_message),
        ("res_verbose_error_message", 3, res_verbose_error_message),
        ("res_error_field", 2, res_error_field),
        ("res_ntuples", 1, res_ntuples),
        ("res_nfields", 1, res_nfields),
        ("res_binary_tuples", 1, res_binary_tuples),
        ("res_fname", 2, res_fname),
        ("res_fnumber", 2, res_fnumber),
        ("res_ftable", 2, res_ftable),
        ("res_ftablecol", 2, res_ftablecol),
        ("res_fformat", 2, res_fformat),
        ("res_ftype", 2, res_ftype),
        ("res_fmod", 2, res_fmod),
        ("res_fsize", 2, res_fsize),
        ("res_getvalue", 3, res_getvalue),
        ("res_getisnull", 3, res_getisnull),
        ("res_getlength", 3, res_getlength),
        ("res_nparams", 1, res_nparams),
        ("res_paramtype", 2, res_paramtype),
        ("res_cmd_status", 1, res_cmd_status),
        ("res_cmd_tuples", 1, res_cmd_tuples),
        ("res_oid_value", 1, res_oid_value),
        ("res_fnames", 1, res_fnames),
        ("res_formats", 1, res_formats),
        ("res_row", 2, res_row),
        ("res_rows", 1, res_rows),
        ("res_column", 2, res_column),
        ("cancon_new", 1, cancon_new),
        ("cancon_free", 1, cancon_free),
        ("cancon_open?", 1, cancon_is_open),
        ("cancon_blocking", 1, cancon_blocking),
        ("cancon_start", 1, cancon_start),
        ("cancon_poll", 1, cancon_poll),
        ("cancon_status", 1, cancon_status),
        ("cancon_socket", 1, cancon_socket),
        ("cancon_error_message", 1, cancon_error_message),
        ("cancon_reset", 1, cancon_reset),
        ("cancon_finish", 1, cancon_finish),
    ];
    for &(name, arity, method) in table {
        cx.define_singleton_method(spinel, name, arity, method)?;
    }
    Ok(())
}

/// The box at the address `value` holds, if its first word is `tag`.
fn boxed<'cx, 'a, T>(
    cx: &mut Context<'cx>,
    value: Value<'cx>,
    tag: u64,
    what: &str,
) -> Result<&'a mut T> {
    let address = cx.to_i64(value)?;
    let pointer = usize::try_from(address).unwrap_or(0) as *mut T;
    // SAFETY: the handle is the address of a live box (the module notes),
    // each of which starts with its `u64` tag.
    if pointer.is_null() || unsafe { *pointer.cast::<u64>() } != tag {
        return Err(Error::new("ArgumentError", format!("not a {what} handle")));
    }
    // SAFETY: as above, and the Ruby side uses a box from one call at a time.
    Ok(unsafe { &mut *pointer })
}

fn conn<'cx, 'a>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<&'a mut Conn> {
    boxed(cx, value, CONN_TAG, "connection")
}

fn res<'cx, 'a>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<&'a mut Res> {
    boxed(cx, value, RES_TAG, "result")
}

fn cancon<'cx, 'a>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<&'a mut Cancon> {
    boxed(cx, value, CANCON_TAG, "cancel connection")
}

/// The open `PGconn` of the connection handle `value`; the Ruby side asks
/// only once it has checked there is one.
fn pgconn<'cx>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<*mut ffi::PGconn> {
    let raw = conn(cx, value)?.raw;
    if raw.is_null() {
        Err(Error::new("PG::ConnectionBad", "connection is closed"))
    } else {
        Ok(raw)
    }
}

/// The `PGresult` of the result handle `value`, not cleared.
fn pgresult<'cx>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<*mut ffi::PGresult> {
    let raw = res(cx, value)?.raw;
    if raw.is_null() {
        Err(Error::new("PG::Error", "result has been cleared"))
    } else {
        Ok(raw)
    }
}

/// The `PGcancelConn` of the cancel connection handle `value`.
fn pgcancon<'cx>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<*mut ffi::PGcancelConn> {
    let raw = cancon(cx, value)?.raw;
    if raw.is_null() {
        Err(Error::new(
            "PG::ConnectionBad",
            "PG::CancelConnection is closed",
        ))
    } else {
        Ok(raw)
    }
}

/// A box's address, as the `Integer` handle the Ruby side keeps.
fn handle<'cx, T>(cx: &mut Context<'cx>, boxed: Box<T>) -> Value<'cx> {
    let address = Box::into_raw(boxed) as usize;
    cx.integer(i64::try_from(address).expect("a user-space address fits in i64"))
}

/// A result libpq answered, as a new result handle, or nil for none.
fn result_handle<'cx>(cx: &mut Context<'cx>, raw: *mut ffi::PGresult) -> Value<'cx> {
    if raw.is_null() {
        cx.nil()
    } else {
        handle(cx, Box::new(Res { tag: RES_TAG, raw }))
    }
}

/// `value`, a `String`, as a C string (the Ruby side has refused one with
/// a NUL in it, as `StringValueCStr` does).
fn c_string<'cx>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<CString> {
    CString::new(cx.to_bytes(value)?)
        .map_err(|_| Error::new("ArgumentError", "string contains null byte"))
}

/// `value` as a C string, or None for nil.
fn c_string_or_null<'cx>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<Option<CString>> {
    if cx.is_nil(value) {
        Ok(None)
    } else {
        c_string(cx, value).map(Some)
    }
}

fn as_ptr(text: &Option<CString>) -> *const c_char {
    text.as_ref().map_or(ptr::null(), |t| t.as_ptr())
}

/// `value` as a C `int`.
fn c_int_of<'cx>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<c_int> {
    let n = cx.to_i64(value)?;
    c_int::try_from(n).map_err(|_| {
        Error::new(
            "RangeError",
            format!("integer {n} too big to convert to 'int'"),
        )
    })
}

/// `value` as an `Oid`.
fn oid_of<'cx>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<ffi::Oid> {
    let n = cx.to_i64(value)?;
    ffi::Oid::try_from(n).map_err(|_| {
        Error::new(
            "RangeError",
            format!("integer {n} too big to convert to 'unsigned int'"),
        )
    })
}

fn int<'cx>(cx: &mut Context<'cx>, n: impl Into<i64>) -> Value<'cx> {
    cx.integer(n.into())
}

/// A NUL-terminated C string libpq answered, as a `String`, or nil for
/// none.
fn c_str_or_nil<'cx>(cx: &mut Context<'cx>, text: *const c_char) -> Value<'cx> {
    if text.is_null() {
        cx.nil()
    } else {
        // SAFETY: libpq answers NUL-terminated strings that live at least
        // until the next call on the same object, and this copies it at once.
        let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
        cx.string_from_bytes(bytes)
    }
}

/// A string libpq allocated, as a `String`, freed with `PQfreemem`; nil
/// for none.
fn owned_c_str<'cx>(cx: &mut Context<'cx>, text: *mut c_char) -> Value<'cx> {
    let value = c_str_or_nil(cx, text);
    if !text.is_null() {
        // SAFETY: libpq allocated it, and nothing uses it after this.
        unsafe { ffi::PQfreemem(text.cast()) };
    }
    value
}

/// Entries packed for the Ruby side: each a little-endian `i32` length, -1
/// for nil, and its bytes.
#[derive(Default)]
struct Packer(Vec<u8>);

impl Packer {
    fn bytes(&mut self, bytes: Option<&[u8]>) {
        match bytes {
            Some(bytes) => {
                let len = i32::try_from(bytes.len()).expect("a libpq value fits in an i32 length");
                self.0.extend_from_slice(&len.to_le_bytes());
                self.0.extend_from_slice(bytes);
            }
            None => self.0.extend_from_slice(&(-1i32).to_le_bytes()),
        }
    }

    fn c_str(&mut self, text: *const c_char) {
        if text.is_null() {
            self.bytes(None);
        } else {
            // SAFETY: a NUL-terminated string libpq answered, copied at once.
            self.bytes(Some(unsafe { CStr::from_ptr(text) }.to_bytes()));
        }
    }

    fn int(&mut self, n: i64) {
        self.bytes(Some(n.to_string().as_bytes()));
    }

    fn done<'cx>(self, cx: &mut Context<'cx>) -> Value<'cx> {
        cx.string_from_bytes(&self.0)
    }
}

/// What `PG::Spinel.pack` packed: Strings and nils, in order.
fn unpack(bytes: &[u8]) -> Result<Vec<Option<Vec<u8>>>> {
    let bad = || Error::new("ArgumentError", "malformed packed values");
    let mut out = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let (len, tail) = rest.split_first_chunk::<4>().ok_or_else(bad)?;
        let len = i32::from_le_bytes(*len);
        if len < 0 {
            out.push(None);
            rest = tail;
        } else {
            let len = usize::try_from(len).map_err(|_| bad())?;
            if tail.len() < len {
                return Err(bad());
            }
            out.push(Some(tail[..len].to_vec()));
            rest = &tail[len..];
        }
    }
    Ok(out)
}

/// Little-endian 32-bit words packed with `pack("l<*")` or `pack("L<*")`.
fn words(bytes: &[u8]) -> Result<Vec<[u8; 4]>> {
    if bytes.len() % 4 != 0 {
        return Err(Error::new("ArgumentError", "malformed packed words"));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|w| [w[0], w[1], w[2], w[3]])
        .collect())
}

/// A query's parameters as libpq takes them: values (NUL-terminated past
/// their length, as libpq reads text ones as C strings), lengths, formats
/// and, when given, types.
struct Params {
    values: Vec<Option<Vec<u8>>>,
    lengths: Vec<c_int>,
    formats: Vec<c_int>,
    types: Option<Vec<ffi::Oid>>,
    pointers: Vec<*const c_char>,
}

impl Params {
    fn new<'cx>(
        cx: &mut Context<'cx>,
        values: Value<'cx>,
        types: Option<Value<'cx>>,
        formats: Value<'cx>,
    ) -> Result<Params> {
        let mut values = unpack(&cx.to_bytes(values)?)?;
        let formats: Vec<c_int> = words(&cx.to_bytes(formats)?)?
            .into_iter()
            .map(c_int::from_le_bytes)
            .collect();
        let types = match types {
            Some(types) if !cx.is_nil(types) => Some(
                words(&cx.to_bytes(types)?)?
                    .into_iter()
                    .map(ffi::Oid::from_le_bytes)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        };
        if formats.len() != values.len() || types.as_ref().is_some_and(|t| t.len() != values.len())
        {
            return Err(Error::new("ArgumentError", "parameter counts differ"));
        }
        let mut lengths = Vec::with_capacity(values.len());
        for value in &mut values {
            match value {
                Some(bytes) => {
                    let len = c_int::try_from(bytes.len())
                        .map_err(|_| Error::new("RangeError", "parameter too long for a C int"))?;
                    lengths.push(len);
                    bytes.push(0);
                }
                None => lengths.push(0),
            }
        }
        let pointers = values
            .iter()
            .map(|v| v.as_ref().map_or(ptr::null(), |b| b.as_ptr().cast()))
            .collect();
        Ok(Params {
            values,
            lengths,
            formats,
            types,
            pointers,
        })
    }

    fn count(&self) -> Result<c_int> {
        c_int::try_from(self.values.len())
            .map_err(|_| Error::new("RangeError", "too many parameters"))
    }

    fn types(&self) -> *const ffi::Oid {
        self.types.as_ref().map_or(ptr::null(), |t| t.as_ptr())
    }
}

/// Parameter types packed with `pack("L<*")`, or nil for none.
fn param_types<'cx>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<Option<Vec<ffi::Oid>>> {
    if cx.is_nil(value) {
        return Ok(None);
    }
    Ok(Some(
        words(&cx.to_bytes(value)?)?
            .into_iter()
            .map(ffi::Oid::from_le_bytes)
            .collect(),
    ))
}

/// The options libpq answered, packed: per option its keyword, envvar,
/// compiled, val, label and dispchar, then dispsize. Frees them.
fn pack_options(options: *mut ffi::PQconninfoOption, packer: &mut Packer) {
    if options.is_null() {
        return;
    }
    let mut at = options;
    // SAFETY: an array libpq answered, ended by an option with a null
    // keyword, freed once after.
    unsafe {
        while !(*at).keyword.is_null() {
            let option = &*at;
            for text in [
                option.keyword,
                option.envvar,
                option.compiled,
                option.val,
                option.label,
                option.dispchar,
            ] {
                packer.c_str(text);
            }
            packer.int(i64::from(option.dispsize));
            at = at.add(1);
        }
        ffi::PQconninfoFree(options);
    }
}

/// Run `body` with the connection in blocking mode, as the pg gem's
/// `BLOCKING_BEGIN`/`BLOCKING_END` do around large-object calls.
fn blocking<T>(raw: *mut ffi::PGconn, body: impl FnOnce() -> T) -> T {
    // SAFETY: an open connection.
    let old = unsafe { ffi::PQisnonblocking(raw) };
    // SAFETY: as above.
    unsafe { ffi::PQsetnonblocking(raw, 0) };
    let out = body();
    // SAFETY: as above.
    unsafe { ffi::PQsetnonblocking(raw, old) };
    out
}

/// Bytes libpq allocated, `len` of them, as a `String`, freed with
/// `PQfreemem`.
fn owned_bytes<'cx>(cx: &mut Context<'cx>, data: *mut u8, len: usize) -> Value<'cx> {
    if data.is_null() {
        return cx.nil();
    }
    // SAFETY: libpq allocated `len` bytes at `data`, copied at once and then
    // freed.
    let value = cx.string_from_bytes(unsafe { std::slice::from_raw_parts(data, len) });
    // SAFETY: as above.
    unsafe { ffi::PQfreemem(data.cast()) };
    value
}

/// The value at `row`, `column` of `raw`, or None for SQL NULL.
fn value_at(raw: *const ffi::PGresult, row: c_int, column: c_int) -> Option<&'static [u8]> {
    // SAFETY: a result, and a row and column the Ruby side checked are in
    // it. The bytes live as long as the result, which outlives the copy the
    // caller makes at once.
    unsafe {
        if ffi::PQgetisnull(raw, row, column) != 0 {
            return None;
        }
        let data = ffi::PQgetvalue(raw, row, column).cast::<u8>();
        let len = usize::try_from(ffi::PQgetlength(raw, row, column)).unwrap_or(0);
        Some(std::slice::from_raw_parts(data, len))
    }
}

/// The libpq call that a `describe_*`/`close_*` method makes.
type NameCall = unsafe extern "C" fn(*mut ffi::PGconn, *const c_char) -> *mut ffi::PGresult;
type SendNameCall = unsafe extern "C" fn(*mut ffi::PGconn, *const c_char) -> c_int;
type ConnCall = unsafe extern "C" fn(*mut ffi::PGconn) -> c_int;
type CanconCall = unsafe extern "C" fn(*mut ffi::PGcancelConn) -> c_int;
type CanconText = unsafe extern "C" fn(*const ffi::PGcancelConn) -> *mut c_char;

methods! {
    /// `has(name)`: whether the libpq has the function `name`.
    fn has(cx, args) {
        let name = c_string(cx, args[0])?;
        Ok(cx.boolean(!optional(&name).is_null()))
    }

    /// `lib_version`: `PQlibVersion()`.
    fn lib_version(cx, _args) {
        // SAFETY: no arguments.
        Ok(int(cx, unsafe { ffi::PQlibVersion() }))
    }

    /// `isthreadsafe`: `PQisthreadsafe()`.
    fn isthreadsafe(cx, _args) {
        // SAFETY: no arguments.
        Ok(cx.boolean(unsafe { ffi::PQisthreadsafe() } != 0))
    }

    /// `init_openssl(ssl, crypto)`: `PQinitOpenSSL`.
    fn init_openssl(cx, args) {
        let (ssl, crypto) = (c_int_of(cx, args[0])?, c_int_of(cx, args[1])?);
        // SAFETY: two flags.
        unsafe { ffi::PQinitOpenSSL(ssl, crypto) };
        Ok(cx.nil())
    }

    /// `init_ssl(ssl)`: `PQinitSSL`.
    fn init_ssl(cx, args) {
        let ssl = c_int_of(cx, args[0])?;
        // SAFETY: a flag.
        unsafe { ffi::PQinitSSL(ssl) };
        Ok(cx.nil())
    }

    /// `encoding_to_char(id)`: `pg_encoding_to_char`, libpq's name for an
    /// encoding id.
    fn encoding_to_char(cx, args) {
        let id = c_int_of(cx, args[0])?;
        // SAFETY: any id; libpq answers a static string.
        Ok(c_str_or_nil(cx, unsafe { ffi::pg_encoding_to_char(id) }))
    }

    /// `res_status_name(status)`: `PQresStatus`.
    fn res_status_name(cx, args) {
        let status = c_int_of(cx, args[0])?;
        // SAFETY: any status; libpq answers a static string.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQresStatus(status) }))
    }

    /// `conndefaults`: `PQconndefaults()`'s options, packed, or nil.
    fn conndefaults(cx, _args) {
        // SAFETY: no arguments.
        let options = unsafe { ffi::PQconndefaults() };
        if options.is_null() {
            return Ok(cx.nil());
        }
        let mut packer = Packer::default();
        pack_options(options, &mut packer);
        Ok(packer.done(cx))
    }

    /// `conninfo_parse(conninfo)`: `PQconninfoParse`, packed: its error
    /// message (nil for none), then its options.
    fn conninfo_parse(cx, args) {
        let conninfo = c_string(cx, args[0])?;
        let mut errmsg: *mut c_char = ptr::null_mut();
        // SAFETY: a C string, and a place for the message.
        let options = unsafe { ffi::PQconninfoParse(conninfo.as_ptr(), &raw mut errmsg) };
        let mut packer = Packer::default();
        packer.c_str(errmsg);
        if !errmsg.is_null() {
            // SAFETY: libpq allocated it.
            unsafe { ffi::PQfreemem(errmsg.cast()) };
        }
        pack_options(options, &mut packer);
        Ok(packer.done(cx))
    }

    /// `ping(conninfo)`: `PQping`.
    fn ping(cx, args) {
        let conninfo = c_string(cx, args[0])?;
        // SAFETY: a C string.
        Ok(int(cx, unsafe { ffi::PQping(conninfo.as_ptr()) }))
    }

    /// `encrypt_password(password, user)`: `PQencryptPassword`.
    fn encrypt_password(cx, args) {
        let (password, user) = (c_string(cx, args[0])?, c_string(cx, args[1])?);
        // SAFETY: C strings.
        Ok(owned_c_str(cx, unsafe { ffi::PQencryptPassword(password.as_ptr(), user.as_ptr()) }))
    }

    /// `escape_string(bytes)`: `PQescapeString`.
    fn escape_string(cx, args) {
        let from = cx.to_bytes(args[0])?;
        let mut to = vec![0u8; from.len() * 2 + 1];
        // SAFETY: `to` has room for twice `from` and a NUL, as libpq needs.
        let len = unsafe { ffi::PQescapeString(to.as_mut_ptr().cast(), from.as_ptr().cast(), from.len()) };
        Ok(cx.string_from_bytes(&to[..len]))
    }

    /// `escape_bytea(bytes)`: `PQescapeBytea`, without its NUL.
    fn escape_bytea(cx, args) {
        let from = cx.to_bytes(args[0])?;
        let mut len = 0usize;
        // SAFETY: `from`'s bytes, and a place for the length.
        let to = unsafe { ffi::PQescapeBytea(from.as_ptr(), from.len(), &raw mut len) };
        Ok(owned_bytes(cx, to, len.saturating_sub(1)))
    }

    /// `unescape_bytea(text)`: `PQunescapeBytea`.
    fn unescape_bytea(cx, args) {
        let from = c_string(cx, args[0])?;
        let mut len = 0usize;
        // SAFETY: a C string, and a place for the length.
        let to = unsafe { ffi::PQunescapeBytea(from.as_ptr().cast(), &raw mut len) };
        Ok(owned_bytes(cx, to, len))
    }

    /// `conn_new`: an empty connection box's handle.
    fn conn_new(cx, _args) {
        Ok(handle(cx, Box::new(Conn { tag: CONN_TAG, raw: ptr::null_mut(), trace: ptr::null_mut() })))
    }

    /// `conn_free(conn)`: finish the connection and free its box. Its
    /// holder's finalizer calls it; nothing uses the handle after.
    fn conn_free(cx, args) {
        let conn = conn(cx, args[0])?;
        finish(conn);
        conn.tag = 0;
        // SAFETY: a box `conn_new` made, freed once.
        drop(unsafe { Box::from_raw(ptr::from_mut(conn)) });
        Ok(cx.nil())
    }

    /// `conn_open?(conn)`: whether it holds a `PGconn`.
    fn conn_is_open(cx, args) {
        let open = !conn(cx, args[0])?.raw.is_null();
        Ok(cx.boolean(open))
    }

    /// `conn_connectdb(conn, conninfo)`: `PQconnectdb`, kept in the box;
    /// whether libpq made a `PGconn`.
    fn conn_connectdb(cx, args) {
        let conninfo = c_string(cx, args[1])?;
        let conn = conn(cx, args[0])?;
        finish(conn);
        // SAFETY: a C string.
        conn.raw = unsafe { ffi::PQconnectdb(conninfo.as_ptr()) };
        let made = !conn.raw.is_null();
        Ok(cx.boolean(made))
    }

    /// `conn_connect_start(conn, conninfo)`: `PQconnectStart`, kept in the
    /// box after the one it held is finished; whether libpq made one.
    fn conn_connect_start(cx, args) {
        let conninfo = c_string(cx, args[1])?;
        let conn = conn(cx, args[0])?;
        finish(conn);
        // SAFETY: a C string.
        conn.raw = unsafe { ffi::PQconnectStart(conninfo.as_ptr()) };
        let made = !conn.raw.is_null();
        Ok(cx.boolean(made))
    }

    /// `conn_finish(conn)`: `PQfinish`.
    fn conn_finish(cx, args) {
        finish(conn(cx, args[0])?);
        Ok(cx.nil())
    }

    /// `conn_connect_poll(conn)`: `PQconnectPoll`.
    fn conn_connect_poll(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQconnectPoll(raw) }))
    }

    /// `conn_reset(conn)`: `PQreset`.
    fn conn_reset(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        unsafe { ffi::PQreset(raw) };
        Ok(cx.nil())
    }

    /// `conn_reset_start(conn)`: `PQresetStart`.
    fn conn_reset_start(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQresetStart(raw) }))
    }

    /// `conn_reset_poll(conn)`: `PQresetPoll`.
    fn conn_reset_poll(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQresetPoll(raw) }))
    }

    /// `conn_db(conn)`: `PQdb`.
    fn conn_db(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQdb(raw) }))
    }

    /// `conn_user(conn)`: `PQuser`.
    fn conn_user(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQuser(raw) }))
    }

    /// `conn_pass(conn)`: `PQpass`.
    fn conn_pass(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQpass(raw) }))
    }

    /// `conn_host(conn)`: `PQhost`.
    fn conn_host(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQhost(raw) }))
    }

    /// `conn_hostaddr(conn)`: `PQhostaddr` (libpq 12).
    fn conn_hostaddr(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let text = call_optional!(c"PQhostaddr" as unsafe extern "C" fn(*const ffi::PGconn) -> *mut c_char, raw);
        Ok(c_str_or_nil(cx, text))
    }

    /// `conn_port(conn)`: `PQport`, as libpq has it.
    fn conn_port(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQport(raw) }))
    }

    /// `conn_options(conn)`: `PQoptions`.
    fn conn_options(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQoptions(raw) }))
    }

    /// `conn_conninfo(conn)`: `PQconninfo`'s options, packed, or nil.
    fn conn_conninfo(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        let options = unsafe { ffi::PQconninfo(raw) };
        if options.is_null() {
            return Ok(cx.nil());
        }
        let mut packer = Packer::default();
        pack_options(options, &mut packer);
        Ok(packer.done(cx))
    }

    /// `conn_status(conn)`: `PQstatus`.
    fn conn_status(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQstatus(raw) }))
    }

    /// `conn_transaction_status(conn)`: `PQtransactionStatus`.
    fn conn_transaction_status(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQtransactionStatus(raw) }))
    }

    /// `conn_parameter_status(conn, name)`: `PQparameterStatus`.
    fn conn_parameter_status(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let name = c_string(cx, args[1])?;
        // SAFETY: an open connection and a C string.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQparameterStatus(raw, name.as_ptr()) }))
    }

    /// `conn_protocol_version(conn)`: `PQprotocolVersion`.
    fn conn_protocol_version(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQprotocolVersion(raw) }))
    }

    /// `conn_full_protocol_version(conn)`: `PQfullProtocolVersion` (libpq 18).
    fn conn_full_protocol_version(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let n = call_optional!(c"PQfullProtocolVersion" as unsafe extern "C" fn(*const ffi::PGconn) -> c_int, raw);
        Ok(int(cx, n))
    }

    /// `conn_server_version(conn)`: `PQserverVersion`.
    fn conn_server_version(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQserverVersion(raw) }))
    }

    /// `conn_error_message(conn)`: `PQerrorMessage`.
    fn conn_error_message(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQerrorMessage(raw) }))
    }

    /// `conn_socket(conn)`: `PQsocket`.
    fn conn_socket(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQsocket(raw) }))
    }

    /// `conn_backend_pid(conn)`: `PQbackendPID`.
    fn conn_backend_pid(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQbackendPID(raw) }))
    }

    /// `conn_backend_key(conn)`: the backend's secret key from libpq's
    /// `pg_cancel` struct, as the pg gem reads it from a libpq before 17
    /// (whose layout that is; the Ruby side defines `backend_key` only
    /// then). nil when `PQgetCancel` answers none, false when the struct
    /// does not hold the backend's pid where it should.
    fn conn_backend_key(cx, args) {
        /// libpq's `pg_cancel` before 17: the server's address, then the
        /// pid and key.
        #[repr(C)]
        struct PgCancel {
            raddr: libc::sockaddr_storage,
            salen: libc::socklen_t,
            be_pid: c_int,
            be_key: c_int,
        }
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        let cancel = unsafe { ffi::PQgetCancel(raw) };
        if cancel.is_null() {
            return Ok(cx.nil());
        }
        // SAFETY: a libpq before 17's `PGcancel` is a `pg_cancel`, freed once.
        let (pid, key) = unsafe {
            let fields = &*cancel.cast::<PgCancel>();
            let out = (fields.be_pid, fields.be_key);
            ffi::PQfreeCancel(cancel);
            out
        };
        // SAFETY: an open connection.
        if pid != unsafe { ffi::PQbackendPID(raw) } {
            return Ok(cx.boolean(false));
        }
        Ok(int(cx, key))
    }

    /// `conn_needs_password(conn)`: `PQconnectionNeedsPassword`.
    fn conn_needs_password(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(cx.boolean(unsafe { ffi::PQconnectionNeedsPassword(raw) } != 0))
    }

    /// `conn_used_password(conn)`: `PQconnectionUsedPassword`.
    fn conn_used_password(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(cx.boolean(unsafe { ffi::PQconnectionUsedPassword(raw) } != 0))
    }

    /// `conn_ssl_in_use(conn)`: `PQsslInUse`.
    fn conn_ssl_in_use(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(cx.boolean(unsafe { ffi::PQsslInUse(raw) } != 0))
    }

    /// `conn_ssl_attribute(conn, name)`: `PQsslAttribute`.
    fn conn_ssl_attribute(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let name = c_string(cx, args[1])?;
        // SAFETY: an open connection and a C string.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQsslAttribute(raw, name.as_ptr()) }))
    }

    /// `conn_ssl_attribute_names(conn)`: `PQsslAttributeNames`, packed.
    fn conn_ssl_attribute_names(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let mut packer = Packer::default();
        // SAFETY: an open connection; libpq answers a static array ended by
        // a null.
        unsafe {
            let mut at = ffi::PQsslAttributeNames(raw);
            while !at.is_null() && !(*at).is_null() {
                packer.c_str(*at);
                at = at.add(1);
            }
        }
        Ok(packer.done(cx))
    }

    /// `conn_client_encoding(conn)`: `PQclientEncoding`.
    fn conn_client_encoding(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQclientEncoding(raw) }))
    }

    /// `conn_set_client_encoding(conn, name)`: `PQsetClientEncoding`.
    fn conn_set_client_encoding(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let name = c_string(cx, args[1])?;
        // SAFETY: an open connection and a C string.
        Ok(int(cx, unsafe { ffi::PQsetClientEncoding(raw, name.as_ptr()) }))
    }

    /// `conn_set_error_verbosity(conn, verbosity)`: `PQsetErrorVerbosity`.
    fn conn_set_error_verbosity(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let verbosity = c_int_of(cx, args[1])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQsetErrorVerbosity(raw, verbosity) }))
    }

    /// `conn_set_error_context_visibility(conn, visibility)`:
    /// `PQsetErrorContextVisibility`.
    fn conn_set_error_context_visibility(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let visibility = c_int_of(cx, args[1])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQsetErrorContextVisibility(raw, visibility) }))
    }

    /// `conn_trace(conn, fd)`: `PQtrace` to a `FILE` over `fd`, which the
    /// caller duplicated for it; false when the stream would not open.
    fn conn_trace(cx, args) {
        let fd = c_int_of(cx, args[1])?;
        let conn = conn(cx, args[0])?;
        if conn.raw.is_null() {
            return Err(Error::new("PG::ConnectionBad", "connection is closed"));
        }
        // SAFETY: a descriptor the caller owns and gives to the stream.
        let stream = unsafe { libc::fdopen(fd, c"w".as_ptr()) };
        if stream.is_null() {
            return Ok(cx.boolean(false));
        }
        untrace(conn);
        conn.trace = stream.cast();
        // SAFETY: an open connection and an open stream, kept until untrace.
        unsafe { ffi::PQtrace(conn.raw, conn.trace) };
        Ok(cx.boolean(true))
    }

    /// `conn_untrace(conn)`: `PQuntrace`, and close the stream.
    fn conn_untrace(cx, args) {
        let conn = conn(cx, args[0])?;
        untrace(conn);
        Ok(cx.nil())
    }

    /// `conn_cancel(conn)`: `PQcancel`, through `PQgetCancel`: nil when the
    /// request went, the message when not, false when there is no cancel
    /// object.
    fn conn_cancel(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        let cancel = unsafe { ffi::PQgetCancel(raw) };
        if cancel.is_null() {
            return Ok(cx.boolean(false));
        }
        let mut errbuf = [0 as c_char; 256];
        // SAFETY: a cancel object, freed once, and a buffer of its size.
        let sent = unsafe {
            let sent = ffi::PQcancel(cancel, errbuf.as_mut_ptr(), 256);
            ffi::PQfreeCancel(cancel);
            sent
        };
        if sent == 1 {
            Ok(cx.nil())
        } else {
            Ok(c_str_or_nil(cx, errbuf.as_ptr()))
        }
    }

    /// `conn_exec(conn, sql)`: `PQexec`, a result handle or nil.
    fn conn_exec(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let sql = c_string(cx, args[1])?;
        // SAFETY: an open connection and a C string.
        let result = unsafe { ffi::PQexec(raw, sql.as_ptr()) };
        Ok(result_handle(cx, result))
    }

    /// `conn_exec_params(conn, sql, values, types, formats, result_format)`:
    /// `PQexecParams`.
    fn conn_exec_params(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let sql = c_string(cx, args[1])?;
        let params = Params::new(cx, args[2], Some(args[3]), args[4])?;
        let result_format = c_int_of(cx, args[5])?;
        // SAFETY: an open connection, a C string, and arrays of the count.
        let result = unsafe {
            ffi::PQexecParams(raw, sql.as_ptr(), params.count()?, params.types(), params.pointers.as_ptr(),
                params.lengths.as_ptr(), params.formats.as_ptr(), result_format)
        };
        Ok(result_handle(cx, result))
    }

    /// `conn_prepare(conn, name, sql, types)`: `PQprepare`.
    fn conn_prepare(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let (name, sql) = (c_string(cx, args[1])?, c_string(cx, args[2])?);
        let types = param_types(cx, args[3])?.unwrap_or_default();
        let count = c_int::try_from(types.len()).map_err(|_| Error::new("RangeError", "too many parameters"))?;
        // SAFETY: an open connection, C strings, and an array of the count.
        let result = unsafe {
            ffi::PQprepare(raw, name.as_ptr(), sql.as_ptr(), count, if count == 0 { ptr::null() } else { types.as_ptr() })
        };
        Ok(result_handle(cx, result))
    }

    /// `conn_exec_prepared(conn, name, values, formats, result_format)`:
    /// `PQexecPrepared`.
    fn conn_exec_prepared(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let name = c_string(cx, args[1])?;
        let params = Params::new(cx, args[2], None, args[3])?;
        let result_format = c_int_of(cx, args[4])?;
        // SAFETY: an open connection, a C string, and arrays of the count.
        let result = unsafe {
            ffi::PQexecPrepared(raw, name.as_ptr(), params.count()?, params.pointers.as_ptr(),
                params.lengths.as_ptr(), params.formats.as_ptr(), result_format)
        };
        Ok(result_handle(cx, result))
    }

    /// `conn_describe_prepared(conn, name)`: `PQdescribePrepared`.
    fn conn_describe_prepared(cx, args) {
        name_call(cx, args, ffi::PQdescribePrepared)
    }

    /// `conn_describe_portal(conn, name)`: `PQdescribePortal`.
    fn conn_describe_portal(cx, args) {
        name_call(cx, args, ffi::PQdescribePortal)
    }

    /// `conn_close_prepared(conn, name)`: `PQclosePrepared` (libpq 17).
    fn conn_close_prepared(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let name = c_string_or_null(cx, args[1])?;
        let result = call_optional!(c"PQclosePrepared" as NameCall, raw, as_ptr(&name));
        Ok(result_handle(cx, result))
    }

    /// `conn_close_portal(conn, name)`: `PQclosePortal` (libpq 17).
    fn conn_close_portal(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let name = c_string_or_null(cx, args[1])?;
        let result = call_optional!(c"PQclosePortal" as NameCall, raw, as_ptr(&name));
        Ok(result_handle(cx, result))
    }

    /// `conn_make_empty_result(conn, status)`: `PQmakeEmptyPGresult`.
    fn conn_make_empty_result(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let status = c_int_of(cx, args[1])?;
        // SAFETY: an open connection.
        let result = unsafe { ffi::PQmakeEmptyPGresult(raw, status) };
        Ok(result_handle(cx, result))
    }

    /// `conn_escape_string(conn, bytes)`: `PQescapeStringConn`, or nil when
    /// it reports an error (in `PQerrorMessage`).
    fn conn_escape_string(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let from = cx.to_bytes(args[1])?;
        let mut to = vec![0u8; from.len() * 2 + 1];
        let mut error: c_int = 0;
        // SAFETY: an open connection; `to` has room for twice `from` and a
        // NUL, as libpq needs.
        let len = unsafe {
            ffi::PQescapeStringConn(raw, to.as_mut_ptr().cast(), from.as_ptr().cast(), from.len(), &raw mut error)
        };
        if error != 0 {
            return Ok(cx.nil());
        }
        Ok(cx.string_from_bytes(&to[..len]))
    }

    /// `conn_escape_literal(conn, bytes)`: `PQescapeLiteral`, or nil.
    fn conn_escape_literal(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let from = cx.to_bytes(args[1])?;
        // SAFETY: an open connection and `from`'s bytes.
        let to = unsafe { ffi::PQescapeLiteral(raw, from.as_ptr().cast(), from.len()) };
        Ok(owned_c_str(cx, to))
    }

    /// `conn_escape_identifier(conn, bytes)`: `PQescapeIdentifier`, or nil.
    fn conn_escape_identifier(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let from = cx.to_bytes(args[1])?;
        // SAFETY: an open connection and `from`'s bytes.
        let to = unsafe { ffi::PQescapeIdentifier(raw, from.as_ptr().cast(), from.len()) };
        Ok(owned_c_str(cx, to))
    }

    /// `conn_escape_bytea(conn, bytes)`: `PQescapeByteaConn`, without its
    /// NUL.
    fn conn_escape_bytea(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let from = cx.to_bytes(args[1])?;
        let mut len = 0usize;
        // SAFETY: an open connection, `from`'s bytes, and a place for the
        // length.
        let to = unsafe { ffi::PQescapeByteaConn(raw, from.as_ptr(), from.len(), &raw mut len) };
        Ok(owned_bytes(cx, to, len.saturating_sub(1)))
    }

    /// `conn_encrypt_password(conn, password, user, algorithm)`:
    /// `PQencryptPasswordConn`, or nil.
    fn conn_encrypt_password(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let (password, user) = (c_string(cx, args[1])?, c_string(cx, args[2])?);
        let algorithm = c_string_or_null(cx, args[3])?;
        // SAFETY: an open connection and C strings.
        let text = unsafe { ffi::PQencryptPasswordConn(raw, password.as_ptr(), user.as_ptr(), as_ptr(&algorithm)) };
        Ok(owned_c_str(cx, text))
    }

    /// `conn_set_single_row_mode(conn)`: `PQsetSingleRowMode`.
    fn conn_set_single_row_mode(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQsetSingleRowMode(raw) }))
    }

    /// `conn_set_chunked_rows_mode(conn, size)`: `PQsetChunkedRowsMode`
    /// (libpq 17).
    fn conn_set_chunked_rows_mode(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let size = c_int_of(cx, args[1])?;
        let n = call_optional!(c"PQsetChunkedRowsMode" as unsafe extern "C" fn(*mut ffi::PGconn, c_int) -> c_int, raw, size);
        Ok(int(cx, n))
    }

    /// `conn_send_query(conn, sql)`: `PQsendQuery`.
    fn conn_send_query(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let sql = c_string(cx, args[1])?;
        // SAFETY: an open connection and a C string.
        Ok(int(cx, unsafe { ffi::PQsendQuery(raw, sql.as_ptr()) }))
    }

    /// `conn_send_query_params(conn, sql, values, types, formats,
    /// result_format)`: `PQsendQueryParams`.
    fn conn_send_query_params(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let sql = c_string(cx, args[1])?;
        let params = Params::new(cx, args[2], Some(args[3]), args[4])?;
        let result_format = c_int_of(cx, args[5])?;
        // SAFETY: an open connection, a C string, and arrays of the count.
        let sent = unsafe {
            ffi::PQsendQueryParams(raw, sql.as_ptr(), params.count()?, params.types(), params.pointers.as_ptr(),
                params.lengths.as_ptr(), params.formats.as_ptr(), result_format)
        };
        Ok(int(cx, sent))
    }

    /// `conn_send_prepare(conn, name, sql, types)`: `PQsendPrepare`.
    fn conn_send_prepare(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let (name, sql) = (c_string(cx, args[1])?, c_string(cx, args[2])?);
        let types = param_types(cx, args[3])?.unwrap_or_default();
        let count = c_int::try_from(types.len()).map_err(|_| Error::new("RangeError", "too many parameters"))?;
        // SAFETY: an open connection, C strings, and an array of the count.
        let sent = unsafe {
            ffi::PQsendPrepare(raw, name.as_ptr(), sql.as_ptr(), count, if count == 0 { ptr::null() } else { types.as_ptr() })
        };
        Ok(int(cx, sent))
    }

    /// `conn_send_query_prepared(conn, name, values, formats,
    /// result_format)`: `PQsendQueryPrepared`.
    fn conn_send_query_prepared(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let name = c_string(cx, args[1])?;
        let params = Params::new(cx, args[2], None, args[3])?;
        let result_format = c_int_of(cx, args[4])?;
        // SAFETY: an open connection, a C string, and arrays of the count.
        let sent = unsafe {
            ffi::PQsendQueryPrepared(raw, name.as_ptr(), params.count()?, params.pointers.as_ptr(),
                params.lengths.as_ptr(), params.formats.as_ptr(), result_format)
        };
        Ok(int(cx, sent))
    }

    /// `conn_send_describe_prepared(conn, name)`: `PQsendDescribePrepared`.
    fn conn_send_describe_prepared(cx, args) {
        send_name_call(cx, args, ffi::PQsendDescribePrepared)
    }

    /// `conn_send_describe_portal(conn, name)`: `PQsendDescribePortal`.
    fn conn_send_describe_portal(cx, args) {
        send_name_call(cx, args, ffi::PQsendDescribePortal)
    }

    /// `conn_send_close_prepared(conn, name)`: `PQsendClosePrepared`
    /// (libpq 17).
    fn conn_send_close_prepared(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let name = c_string_or_null(cx, args[1])?;
        let sent = call_optional!(c"PQsendClosePrepared" as SendNameCall, raw, as_ptr(&name));
        Ok(int(cx, sent))
    }

    /// `conn_send_close_portal(conn, name)`: `PQsendClosePortal` (libpq 17).
    fn conn_send_close_portal(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let name = c_string_or_null(cx, args[1])?;
        let sent = call_optional!(c"PQsendClosePortal" as SendNameCall, raw, as_ptr(&name));
        Ok(int(cx, sent))
    }

    /// `conn_get_result(conn)`: `PQgetResult`, a result handle or nil.
    fn conn_get_result(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        let result = unsafe { ffi::PQgetResult(raw) };
        Ok(result_handle(cx, result))
    }

    /// `conn_consume_input(conn)`: `PQconsumeInput`.
    fn conn_consume_input(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQconsumeInput(raw) }))
    }

    /// `conn_is_busy(conn)`: `PQisBusy`.
    fn conn_is_busy(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(cx.boolean(unsafe { ffi::PQisBusy(raw) } != 0))
    }

    /// `conn_setnonblocking(conn, arg)`: `PQsetnonblocking`.
    fn conn_setnonblocking(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let arg = c_int_of(cx, args[1])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQsetnonblocking(raw, arg) }))
    }

    /// `conn_isnonblocking(conn)`: `PQisnonblocking`.
    fn conn_isnonblocking(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(cx.boolean(unsafe { ffi::PQisnonblocking(raw) } != 0))
    }

    /// `conn_flush(conn)`: `PQflush`.
    fn conn_flush(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        Ok(int(cx, unsafe { ffi::PQflush(raw) }))
    }

    /// `conn_notifies(conn)`: `PQnotifies`, packed as its relname, be_pid
    /// and extra, or nil for none.
    fn conn_notifies(cx, args) {
        let raw = pgconn(cx, args[0])?;
        // SAFETY: an open connection.
        let notify = unsafe { ffi::PQnotifies(raw) };
        if notify.is_null() {
            return Ok(cx.nil());
        }
        let mut packer = Packer::default();
        // SAFETY: a notification libpq answered, freed once.
        unsafe {
            packer.c_str((*notify).relname);
            packer.int(i64::from((*notify).be_pid));
            packer.c_str((*notify).extra);
            ffi::PQfreemem(notify.cast());
        }
        Ok(packer.done(cx))
    }

    /// `conn_put_copy_data(conn, bytes)`: `PQputCopyData`.
    fn conn_put_copy_data(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let bytes = cx.to_bytes(args[1])?;
        let len = c_int::try_from(bytes.len()).map_err(|_| Error::new("RangeError", "buffer too long for a C int"))?;
        // SAFETY: an open connection and `len` bytes.
        Ok(int(cx, unsafe { ffi::PQputCopyData(raw, bytes.as_ptr().cast(), len) }))
    }

    /// `conn_put_copy_end(conn, message)`: `PQputCopyEnd`.
    fn conn_put_copy_end(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let message = c_string_or_null(cx, args[1])?;
        // SAFETY: an open connection and a C string or null.
        Ok(int(cx, unsafe { ffi::PQputCopyEnd(raw, as_ptr(&message)) }))
    }

    /// `conn_get_copy_data(conn, async)`: `PQgetCopyData`: the row, or the
    /// Integer it answered for none (-1 done, 0 would block, -2 error).
    fn conn_get_copy_data(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let is_async = c_int::from(cx.is_truthy(args[1]));
        let mut buffer: *mut c_char = ptr::null_mut();
        // SAFETY: an open connection and a place for the buffer.
        let n = unsafe { ffi::PQgetCopyData(raw, &raw mut buffer, is_async) };
        if n <= 0 {
            return Ok(int(cx, n));
        }
        Ok(owned_bytes(cx, buffer.cast(), usize::try_from(n).unwrap_or(0)))
    }

    /// `conn_pipeline_status(conn)`: `PQpipelineStatus` (libpq 14).
    fn conn_pipeline_status(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let n = call_optional!(c"PQpipelineStatus" as ConnCall, raw);
        Ok(int(cx, n))
    }

    /// `conn_enter_pipeline_mode(conn)`: `PQenterPipelineMode` (libpq 14).
    fn conn_enter_pipeline_mode(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let n = call_optional!(c"PQenterPipelineMode" as ConnCall, raw);
        Ok(int(cx, n))
    }

    /// `conn_exit_pipeline_mode(conn)`: `PQexitPipelineMode` (libpq 14).
    fn conn_exit_pipeline_mode(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let n = call_optional!(c"PQexitPipelineMode" as ConnCall, raw);
        Ok(int(cx, n))
    }

    /// `conn_pipeline_sync(conn)`: `PQpipelineSync` (libpq 14).
    fn conn_pipeline_sync(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let n = call_optional!(c"PQpipelineSync" as ConnCall, raw);
        Ok(int(cx, n))
    }

    /// `conn_send_pipeline_sync(conn)`: `PQsendPipelineSync` (libpq 17).
    fn conn_send_pipeline_sync(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let n = call_optional!(c"PQsendPipelineSync" as ConnCall, raw);
        Ok(int(cx, n))
    }

    /// `conn_send_flush_request(conn)`: `PQsendFlushRequest` (libpq 14).
    fn conn_send_flush_request(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let n = call_optional!(c"PQsendFlushRequest" as ConnCall, raw);
        Ok(int(cx, n))
    }

    /// `conn_lo_creat(conn, mode)`: `lo_creat`, in blocking mode.
    fn conn_lo_creat(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let mode = c_int_of(cx, args[1])?;
        // SAFETY: an open connection.
        let oid = blocking(raw, || unsafe { ffi::lo_creat(raw, mode) });
        Ok(int(cx, oid))
    }

    /// `conn_lo_create(conn, oid)`: `lo_create`, in blocking mode.
    fn conn_lo_create(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let oid = oid_of(cx, args[1])?;
        // SAFETY: an open connection.
        let oid = blocking(raw, || unsafe { ffi::lo_create(raw, oid) });
        Ok(int(cx, oid))
    }

    /// `conn_lo_import(conn, filename)`: `lo_import`, in blocking mode.
    fn conn_lo_import(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let filename = c_string(cx, args[1])?;
        // SAFETY: an open connection and a C string.
        let oid = blocking(raw, || unsafe { ffi::lo_import(raw, filename.as_ptr()) });
        Ok(int(cx, oid))
    }

    /// `conn_lo_export(conn, oid, filename)`: `lo_export`, in blocking mode.
    fn conn_lo_export(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let oid = oid_of(cx, args[1])?;
        let filename = c_string(cx, args[2])?;
        // SAFETY: an open connection and a C string.
        let n = blocking(raw, || unsafe { ffi::lo_export(raw, oid, filename.as_ptr()) });
        Ok(int(cx, n))
    }

    /// `conn_lo_open(conn, oid, mode)`: `lo_open`, in blocking mode.
    fn conn_lo_open(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let oid = oid_of(cx, args[1])?;
        let mode = c_int_of(cx, args[2])?;
        // SAFETY: an open connection.
        let fd = blocking(raw, || unsafe { ffi::lo_open(raw, oid, mode) });
        Ok(int(cx, fd))
    }

    /// `conn_lo_write(conn, fd, bytes)`: `lo_write`, in blocking mode.
    fn conn_lo_write(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let fd = c_int_of(cx, args[1])?;
        let bytes = cx.to_bytes(args[2])?;
        // SAFETY: an open connection and the bytes.
        let n = blocking(raw, || unsafe { ffi::lo_write(raw, fd, bytes.as_ptr().cast(), bytes.len()) });
        Ok(int(cx, n))
    }

    /// `conn_lo_read(conn, fd, len)`: `lo_read`, in blocking mode: the bytes
    /// read, or the Integer it answered when it read none.
    fn conn_lo_read(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let fd = c_int_of(cx, args[1])?;
        let len = usize::try_from(c_int_of(cx, args[2])?).unwrap_or(0);
        let mut buffer = vec![0u8; len];
        // SAFETY: an open connection and a buffer of `len` bytes.
        let n = blocking(raw, || unsafe { ffi::lo_read(raw, fd, buffer.as_mut_ptr().cast(), len) });
        if n <= 0 {
            return Ok(int(cx, n));
        }
        Ok(cx.string_from_bytes(&buffer[..usize::try_from(n).unwrap_or(0)]))
    }

    /// `conn_lo_lseek(conn, fd, offset, whence)`: `lo_lseek`, in blocking
    /// mode.
    fn conn_lo_lseek(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let (fd, offset, whence) = (c_int_of(cx, args[1])?, c_int_of(cx, args[2])?, c_int_of(cx, args[3])?);
        // SAFETY: an open connection.
        let n = blocking(raw, || unsafe { ffi::lo_lseek(raw, fd, offset, whence) });
        Ok(int(cx, n))
    }

    /// `conn_lo_tell(conn, fd)`: `lo_tell`, in blocking mode.
    fn conn_lo_tell(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let fd = c_int_of(cx, args[1])?;
        // SAFETY: an open connection.
        let n = blocking(raw, || unsafe { ffi::lo_tell(raw, fd) });
        Ok(int(cx, n))
    }

    /// `conn_lo_truncate(conn, fd, len)`: `lo_truncate`, in blocking mode.
    fn conn_lo_truncate(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let fd = c_int_of(cx, args[1])?;
        // As the pg gem converts it: a C int, then a size_t.
        let len = c_int_of(cx, args[2])? as usize;
        // SAFETY: an open connection.
        let n = blocking(raw, || unsafe { ffi::lo_truncate(raw, fd, len) });
        Ok(int(cx, n))
    }

    /// `conn_lo_close(conn, fd)`: `lo_close`, in blocking mode.
    fn conn_lo_close(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let fd = c_int_of(cx, args[1])?;
        // SAFETY: an open connection.
        let n = blocking(raw, || unsafe { ffi::lo_close(raw, fd) });
        Ok(int(cx, n))
    }

    /// `conn_lo_unlink(conn, oid)`: `lo_unlink`, in blocking mode.
    fn conn_lo_unlink(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let oid = oid_of(cx, args[1])?;
        // SAFETY: an open connection.
        let n = blocking(raw, || unsafe { ffi::lo_unlink(raw, oid) });
        Ok(int(cx, n))
    }

    /// `res_free(res)`: clear the result and free its box. Its holder's
    /// finalizer calls it; nothing uses the handle after.
    fn res_free(cx, args) {
        let res = res(cx, args[0])?;
        clear(res);
        res.tag = 0;
        // SAFETY: a box `result_handle` made, freed once.
        drop(unsafe { Box::from_raw(ptr::from_mut(res)) });
        Ok(cx.nil())
    }

    /// `res_clear(res)`: `PQclear`, once.
    fn res_clear(cx, args) {
        clear(res(cx, args[0])?);
        Ok(cx.nil())
    }

    /// `res_cleared?(res)`: whether it was cleared.
    fn res_is_cleared(cx, args) {
        let cleared = res(cx, args[0])?.raw.is_null();
        Ok(cx.boolean(cleared))
    }

    /// `res_status(res)`: `PQresultStatus`.
    fn res_status(cx, args) {
        let raw = pgresult(cx, args[0])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQresultStatus(raw) }))
    }

    /// `res_error_message(res)`: `PQresultErrorMessage`.
    fn res_error_message(cx, args) {
        let raw = pgresult(cx, args[0])?;
        // SAFETY: a result.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQresultErrorMessage(raw) }))
    }

    /// `res_verbose_error_message(res, verbosity, show_context)`:
    /// `PQresultVerboseErrorMessage`, or nil when out of memory.
    fn res_verbose_error_message(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let (verbosity, context) = (c_int_of(cx, args[1])?, c_int_of(cx, args[2])?);
        // SAFETY: a result.
        let text = unsafe { ffi::PQresultVerboseErrorMessage(raw, verbosity, context) };
        Ok(owned_c_str(cx, text))
    }

    /// `res_error_field(res, code)`: `PQresultErrorField`, or nil.
    fn res_error_field(cx, args) {
        let raw = res(cx, args[0])?.raw;
        let code = c_int_of(cx, args[1])?;
        // SAFETY: a result or null, for which libpq answers null.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQresultErrorField(raw, code) }))
    }

    /// `res_ntuples(res)`: `PQntuples`.
    fn res_ntuples(cx, args) {
        let raw = pgresult(cx, args[0])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQntuples(raw) }))
    }

    /// `res_nfields(res)`: `PQnfields`.
    fn res_nfields(cx, args) {
        let raw = pgresult(cx, args[0])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQnfields(raw) }))
    }

    /// `res_binary_tuples(res)`: `PQbinaryTuples`.
    fn res_binary_tuples(cx, args) {
        let raw = pgresult(cx, args[0])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQbinaryTuples(raw) }))
    }

    /// `res_fname(res, i)`: `PQfname`.
    fn res_fname(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a result; libpq answers null for a column not in it.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQfname(raw, i) }))
    }

    /// `res_fnumber(res, name)`: `PQfnumber`.
    fn res_fnumber(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let name = c_string(cx, args[1])?;
        // SAFETY: a result and a C string.
        Ok(int(cx, unsafe { ffi::PQfnumber(raw, name.as_ptr()) }))
    }

    /// `res_ftable(res, i)`: `PQftable`.
    fn res_ftable(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQftable(raw, i) }))
    }

    /// `res_ftablecol(res, i)`: `PQftablecol`.
    fn res_ftablecol(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQftablecol(raw, i) }))
    }

    /// `res_fformat(res, i)`: `PQfformat`.
    fn res_fformat(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQfformat(raw, i) }))
    }

    /// `res_ftype(res, i)`: `PQftype`.
    fn res_ftype(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQftype(raw, i) }))
    }

    /// `res_fmod(res, i)`: `PQfmod`.
    fn res_fmod(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQfmod(raw, i) }))
    }

    /// `res_fsize(res, i)`: `PQfsize`.
    fn res_fsize(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQfsize(raw, i) }))
    }

    /// `res_getvalue(res, row, column)`: the value's bytes, or nil for SQL
    /// NULL (`PQgetisnull`).
    fn res_getvalue(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let (row, column) = (c_int_of(cx, args[1])?, c_int_of(cx, args[2])?);
        Ok(match value_at(raw, row, column) {
            Some(bytes) => cx.string_from_bytes(bytes),
            None => cx.nil(),
        })
    }

    /// `res_getisnull(res, row, column)`: `PQgetisnull`.
    fn res_getisnull(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let (row, column) = (c_int_of(cx, args[1])?, c_int_of(cx, args[2])?);
        // SAFETY: a result.
        Ok(cx.boolean(unsafe { ffi::PQgetisnull(raw, row, column) } != 0))
    }

    /// `res_getlength(res, row, column)`: `PQgetlength`.
    fn res_getlength(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let (row, column) = (c_int_of(cx, args[1])?, c_int_of(cx, args[2])?);
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQgetlength(raw, row, column) }))
    }

    /// `res_nparams(res)`: `PQnparams`.
    fn res_nparams(cx, args) {
        let raw = pgresult(cx, args[0])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQnparams(raw) }))
    }

    /// `res_paramtype(res, i)`: `PQparamtype`.
    fn res_paramtype(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQparamtype(raw, i) }))
    }

    /// `res_cmd_status(res)`: `PQcmdStatus`.
    fn res_cmd_status(cx, args) {
        let raw = pgresult(cx, args[0])?;
        // SAFETY: a result.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQcmdStatus(raw) }))
    }

    /// `res_cmd_tuples(res)`: `PQcmdTuples`, the string libpq answers.
    fn res_cmd_tuples(cx, args) {
        let raw = pgresult(cx, args[0])?;
        // SAFETY: a result.
        Ok(c_str_or_nil(cx, unsafe { ffi::PQcmdTuples(raw) }))
    }

    /// `res_oid_value(res)`: `PQoidValue`.
    fn res_oid_value(cx, args) {
        let raw = pgresult(cx, args[0])?;
        // SAFETY: a result.
        Ok(int(cx, unsafe { ffi::PQoidValue(raw) }))
    }

    /// `res_fnames(res)`: every column's `PQfname`, packed.
    fn res_fnames(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let mut packer = Packer::default();
        // SAFETY: a result, and its columns.
        unsafe {
            for i in 0..ffi::PQnfields(raw) {
                packer.c_str(ffi::PQfname(raw, i));
            }
        }
        Ok(packer.done(cx))
    }

    /// `res_formats(res)`: every column's `PQfformat`, one byte each.
    fn res_formats(cx, args) {
        let raw = pgresult(cx, args[0])?;
        // SAFETY: a result, and its columns.
        let formats: Vec<u8> = unsafe { (0..ffi::PQnfields(raw)).map(|i| u8::from(ffi::PQfformat(raw, i) != 0)).collect() };
        Ok(cx.string_from_bytes(&formats))
    }

    /// `res_row(res, row)`: the row's values, packed, nil for SQL NULL.
    fn res_row(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let row = c_int_of(cx, args[1])?;
        let mut packer = Packer::default();
        // SAFETY: a result.
        for column in 0..unsafe { ffi::PQnfields(raw) } {
            packer.bytes(value_at(raw, row, column));
        }
        Ok(packer.done(cx))
    }

    /// `res_rows(res)`: every row's values, row after row, packed.
    fn res_rows(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let mut packer = Packer::default();
        // SAFETY: a result.
        let (rows, columns) = unsafe { (ffi::PQntuples(raw), ffi::PQnfields(raw)) };
        for row in 0..rows {
            for column in 0..columns {
                packer.bytes(value_at(raw, row, column));
            }
        }
        Ok(packer.done(cx))
    }

    /// `res_column(res, column)`: the column's values, packed.
    fn res_column(cx, args) {
        let raw = pgresult(cx, args[0])?;
        let column = c_int_of(cx, args[1])?;
        let mut packer = Packer::default();
        // SAFETY: a result.
        for row in 0..unsafe { ffi::PQntuples(raw) } {
            packer.bytes(value_at(raw, row, column));
        }
        Ok(packer.done(cx))
    }

    /// `cancon_new(conn)`: `PQcancelCreate` (libpq 17), in a new box's
    /// handle, or nil when libpq made none.
    fn cancon_new(cx, args) {
        let raw = pgconn(cx, args[0])?;
        let cancon = call_optional!(c"PQcancelCreate" as unsafe extern "C" fn(*mut ffi::PGconn) -> *mut ffi::PGcancelConn, raw);
        if cancon.is_null() {
            return Ok(cx.nil());
        }
        Ok(handle(cx, Box::new(Cancon { tag: CANCON_TAG, raw: cancon })))
    }

    /// `cancon_free(cancon)`: finish it and free its box.
    fn cancon_free(cx, args) {
        let cancon = cancon(cx, args[0])?;
        cancon_close(cancon)?;
        cancon.tag = 0;
        // SAFETY: a box `cancon_new` made, freed once.
        drop(unsafe { Box::from_raw(ptr::from_mut(cancon)) });
        Ok(cx.nil())
    }

    /// `cancon_open?(cancon)`: whether it holds a `PGcancelConn`.
    fn cancon_is_open(cx, args) {
        let open = !cancon(cx, args[0])?.raw.is_null();
        Ok(cx.boolean(open))
    }

    /// `cancon_blocking(cancon)`: `PQcancelBlocking`.
    fn cancon_blocking(cx, args) {
        let raw = pgcancon(cx, args[0])?;
        let n = call_optional!(c"PQcancelBlocking" as CanconCall, raw);
        Ok(int(cx, n))
    }

    /// `cancon_start(cancon)`: `PQcancelStart`.
    fn cancon_start(cx, args) {
        let raw = pgcancon(cx, args[0])?;
        let n = call_optional!(c"PQcancelStart" as CanconCall, raw);
        Ok(int(cx, n))
    }

    /// `cancon_poll(cancon)`: `PQcancelPoll`.
    fn cancon_poll(cx, args) {
        let raw = pgcancon(cx, args[0])?;
        let n = call_optional!(c"PQcancelPoll" as CanconCall, raw);
        Ok(int(cx, n))
    }

    /// `cancon_status(cancon)`: `PQcancelStatus`.
    fn cancon_status(cx, args) {
        let raw = pgcancon(cx, args[0])?;
        let n = call_optional!(c"PQcancelStatus" as unsafe extern "C" fn(*const ffi::PGcancelConn) -> c_int, raw);
        Ok(int(cx, n))
    }

    /// `cancon_socket(cancon)`: `PQcancelSocket`.
    fn cancon_socket(cx, args) {
        let raw = cancon(cx, args[0])?.raw;
        let n = call_optional!(c"PQcancelSocket" as unsafe extern "C" fn(*const ffi::PGcancelConn) -> c_int, raw);
        Ok(int(cx, n))
    }

    /// `cancon_error_message(cancon)`: `PQcancelErrorMessage`.
    fn cancon_error_message(cx, args) {
        let raw = pgcancon(cx, args[0])?;
        let text = call_optional!(c"PQcancelErrorMessage" as CanconText, raw);
        Ok(c_str_or_nil(cx, text))
    }

    /// `cancon_reset(cancon)`: `PQcancelReset`.
    fn cancon_reset(cx, args) {
        let raw = pgcancon(cx, args[0])?;
        call_optional!(c"PQcancelReset" as unsafe extern "C" fn(*mut ffi::PGcancelConn), raw);
        Ok(cx.nil())
    }

    /// `cancon_finish(cancon)`: `PQcancelFinish`, once.
    fn cancon_finish(cx, args) {
        cancon_close(cancon(cx, args[0])?)?;
        Ok(cx.nil())
    }
}

/// `PQfinish`, once, closing the trace stream after.
fn finish(conn: &mut Conn) {
    if !conn.raw.is_null() {
        // SAFETY: an open connection, which nothing uses after this.
        unsafe { ffi::PQfinish(conn.raw) };
        conn.raw = ptr::null_mut();
    }
    if !conn.trace.is_null() {
        // SAFETY: the stream `conn_trace` opened, which libpq no longer
        // writes to.
        unsafe { libc::fclose(conn.trace.cast()) };
        conn.trace = ptr::null_mut();
    }
}

/// `PQuntrace` and close the stream, when tracing.
fn untrace(conn: &mut Conn) {
    if conn.trace.is_null() {
        return;
    }
    if !conn.raw.is_null() {
        // SAFETY: an open connection.
        unsafe { ffi::PQuntrace(conn.raw) };
    }
    // SAFETY: the stream `conn_trace` opened, which libpq no longer writes to.
    unsafe { libc::fclose(conn.trace.cast()) };
    conn.trace = ptr::null_mut();
}

/// `PQclear`, once.
fn clear(res: &mut Res) {
    if !res.raw.is_null() {
        // SAFETY: a result, which nothing uses after this.
        unsafe { ffi::PQclear(res.raw) };
        res.raw = ptr::null_mut();
    }
}

/// `PQcancelFinish`, once.
fn cancon_close(cancon: &mut Cancon) -> Result<()> {
    if !cancon.raw.is_null() {
        let raw = cancon.raw;
        cancon.raw = ptr::null_mut();
        call_optional!(
            c"PQcancelFinish" as unsafe extern "C" fn(*mut ffi::PGcancelConn),
            raw
        );
    }
    Ok(())
}

/// A `describe_*` call: the connection, then a name or nil.
fn name_call<'cx>(
    cx: &mut Context<'cx>,
    args: &[Value<'cx>],
    call: NameCall,
) -> Result<Value<'cx>> {
    let raw = pgconn(cx, args[0])?;
    let name = c_string_or_null(cx, args[1])?;
    // SAFETY: an open connection and a C string or null.
    let result = unsafe { call(raw, as_ptr(&name)) };
    Ok(result_handle(cx, result))
}

/// A `send_describe_*` call: the connection, then a name or nil.
fn send_name_call<'cx>(
    cx: &mut Context<'cx>,
    args: &[Value<'cx>],
    call: SendNameCall,
) -> Result<Value<'cx>> {
    let raw = pgconn(cx, args[0])?;
    let name = c_string_or_null(cx, args[1])?;
    // SAFETY: an open connection and a C string or null.
    let sent = unsafe { call(raw, as_ptr(&name)) };
    Ok(int(cx, sent))
}
