//! The native half of the sqlite3-spinel gem: SQLite's C API as singleton
//! methods of `SQLite3::Spinel`, one SQLite call each, answering what SQLite
//! answers. Everything the sqlite3 gem's C extension decides on top of
//! those calls — which exception a result code raises, what a row is, how a
//! Ruby value binds — is Ruby, in `lib/sqlite3/sqlite3_native.rb`.
//!
//! A connection and a statement are boxes the Ruby side holds by an Integer
//! handle, the box's address: [`Db`] around a `sqlite3*`, [`Stmt`] around a
//! `sqlite3_stmt*`. `*_new` makes one, `*_free` (from the holder's
//! finalizer) frees it. A call that can fail answers SQLite's result code;
//! the message is the connection's `errmsg`, read next.
//!
//! ponytail: a handle is an address, so a forged or freed one passed back
//! is undefined behavior, which a tag in each box only catches when the
//! address still holds a box. Only `sqlite3_native.rb` passes them, from
//! instance variables a finalizer frees after. Upgrade: an object with
//! native data and a free function in `spinel-ext`, as CRuby's typed data.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

use libsqlite3_sys as ffi;
use spinel_ext::{Context, Error, Result, Value};

spinel_ext::extension!(init);

// Two functions the amalgamation exports that libsqlite3-sys's bundled
// bindings leave out.
unsafe extern "C" {
    fn sqlite3_close_v2(db: *mut ffi::sqlite3) -> c_int;
    fn sqlite3_bind_text16(
        stmt: *mut ffi::sqlite3_stmt,
        index: c_int,
        data: *const c_void,
        len: c_int,
        destructor: ffi::sqlite3_destructor_type,
    ) -> c_int;
}

/// What a [`Db`] box starts with, so that a statement's handle passed as a
/// connection's is refused.
const DB_TAG: u64 = 0x5350_494e_454c_4442;
/// What a [`Stmt`] box starts with.
const STMT_TAG: u64 = 0x5350_494e_454c_5354;

/// A connection: null before `db_open` and after `db_close`.
struct Db {
    tag: u64,
    raw: *mut ffi::sqlite3,
    /// The message of the last call whose error SQLite does not leave in
    /// `errmsg`: `sqlite3_load_extension`'s.
    message: Option<Vec<u8>>,
}

/// A statement: null before `stmt_prepare`, after `stmt_finalize`, and for
/// SQL with no statement in it.
struct Stmt {
    tag: u64,
    raw: *mut ffi::sqlite3_stmt,
    /// The byte offset in the prepared SQL where the first statement ended.
    tail: usize,
}

/// One method of `SQLite3::Spinel`: `$cx`, and the arguments as `$args`.
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

fn init(cx: &mut Context<'_>) -> Result<()> {
    // SAFETY: called once, as a program's first use of SQLite is.
    unsafe { ffi::sqlite3_initialize() };
    let sqlite3 = cx.define_module("SQLite3")?;
    let spinel = cx.define_module_under(sqlite3, "Spinel")?;
    let table: &[(&str, i32, spinel_ext::Method)] = &[
        ("libversion", 0, libversion),
        ("libversion_number", 0, libversion_number),
        ("threadsafe", 0, threadsafe),
        ("status", 2, status),
        ("complete", 1, complete),
        ("db_new", 0, db_new),
        ("db_free", 1, db_free),
        ("db_open", 4, db_open),
        ("db_close", 1, db_close),
        ("db_discard", 1, db_discard),
        ("db_open?", 1, db_is_open),
        ("db_errcode", 1, db_errcode),
        ("db_errmsg", 1, db_errmsg),
        ("db_error_offset", 1, db_error_offset),
        ("db_message", 1, db_message),
        ("db_changes", 1, db_changes),
        ("db_total_changes", 1, db_total_changes),
        ("db_last_insert_rowid", 1, db_last_insert_rowid),
        ("db_autocommit", 1, db_autocommit),
        ("db_busy_timeout", 2, db_busy_timeout),
        ("db_extended_result_codes", 2, db_extended_result_codes),
        ("db_interrupt", 1, db_interrupt),
        ("db_filename", 2, db_filename),
        ("db_disable_dqs", 1, db_disable_dqs),
        ("db_enable_load_extension", 2, db_enable_load_extension),
        ("db_load_extension", 2, db_load_extension),
        ("stmt_new", 0, stmt_new),
        ("stmt_free", 1, stmt_free),
        ("stmt_prepare", 3, stmt_prepare),
        ("stmt_tail", 1, stmt_tail),
        ("stmt_finalize", 1, stmt_finalize),
        ("stmt_open?", 1, stmt_is_open),
        ("stmt_step", 1, stmt_step),
        ("stmt_reset", 1, stmt_reset),
        ("stmt_clear_bindings", 1, stmt_clear_bindings),
        ("stmt_busy", 1, stmt_busy),
        ("stmt_column_count", 1, stmt_column_count),
        ("stmt_column_type", 2, stmt_column_type),
        ("stmt_column_int64", 2, stmt_column_int64),
        ("stmt_column_double", 2, stmt_column_double),
        ("stmt_column_text", 2, stmt_column_text),
        ("stmt_column_blob", 2, stmt_column_blob),
        ("stmt_column_name", 2, stmt_column_name),
        ("stmt_column_decltype", 2, stmt_column_decltype),
        ("stmt_column_database_name", 2, stmt_column_database_name),
        ("stmt_bind_parameter_count", 1, stmt_bind_parameter_count),
        ("stmt_bind_parameter_index", 2, stmt_bind_parameter_index),
        ("stmt_bind_parameter_name", 2, stmt_bind_parameter_name),
        ("stmt_bind_null", 2, stmt_bind_null),
        ("stmt_bind_int64", 3, stmt_bind_int64),
        ("stmt_bind_double", 3, stmt_bind_double),
        ("stmt_bind_text", 3, stmt_bind_text),
        ("stmt_bind_text16", 3, stmt_bind_text16),
        ("stmt_bind_blob", 3, stmt_bind_blob),
        ("stmt_sql", 1, stmt_sql),
        ("stmt_expanded_sql", 1, stmt_expanded_sql),
        ("stmt_status", 3, stmt_status),
    ];
    for &(name, arity, method) in table {
        cx.define_singleton_method(spinel, name, arity, method)?;
    }
    Ok(())
}

/// The box `value`, a handle `db_new` answered.
fn db<'cx, 'a>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<&'a mut Db> {
    let address = cx.to_i64(value)?;
    let db = usize::try_from(address).unwrap_or(0) as *mut Db;
    // SAFETY: the handle is the address of a live box (the module notes);
    // its tag says it is a connection's.
    if db.is_null() || unsafe { (*db).tag } != DB_TAG {
        return Err(Error::new("ArgumentError", "not a database handle"));
    }
    // SAFETY: as above, and the Ruby side uses a box from one call at a time.
    Ok(unsafe { &mut *db })
}

/// The box `value`, a handle `stmt_new` answered.
fn stmt<'cx, 'a>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<&'a mut Stmt> {
    let address = cx.to_i64(value)?;
    let stmt = usize::try_from(address).unwrap_or(0) as *mut Stmt;
    // SAFETY: as for `db`.
    if stmt.is_null() || unsafe { (*stmt).tag } != STMT_TAG {
        return Err(Error::new("ArgumentError", "not a statement handle"));
    }
    // SAFETY: as for `db`.
    Ok(unsafe { &mut *stmt })
}

/// `value`, a `String`, as a C string: an `ArgumentError` for one with a NUL
/// in it, as `StringValueCStr` raises.
fn c_string<'cx>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<CString> {
    CString::new(cx.to_bytes(value)?)
        .map_err(|_| Error::new("ArgumentError", "string contains null byte"))
}

/// `value` as a C `int`.
fn c_int_of<'cx>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<c_int> {
    let n = cx.to_i64(value)?;
    c_int::try_from(n).map_err(|_| Error::new("RangeError", format!("integer {n} too big for int")))
}

/// A NUL-terminated C string SQLite answered, as a `String`, or nil for
/// none.
fn c_str_or_nil<'cx>(cx: &mut Context<'cx>, text: *const c_char) -> Value<'cx> {
    if text.is_null() {
        cx.nil()
    } else {
        // SAFETY: SQLite answers NUL-terminated strings that live until the
        // next call on the same object, and this copies it at once.
        let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
        cx.string_from_bytes(bytes)
    }
}

/// `len` bytes SQLite answered at `data`, as a `String`.
fn bytes_at<'cx>(cx: &mut Context<'cx>, data: *const c_void, len: c_int) -> Value<'cx> {
    match usize::try_from(len) {
        Ok(len) if !data.is_null() && len > 0 => {
            // SAFETY: SQLite answered `len` bytes at `data`, valid until the
            // next call on the statement, and this copies them at once.
            let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len) };
            cx.string_from_bytes(bytes)
        }
        _ => cx.string_from_bytes(&[]),
    }
}

/// A result code, as an `Integer`.
fn code<'cx>(cx: &mut Context<'cx>, rc: c_int) -> Value<'cx> {
    cx.integer(i64::from(rc))
}

/// A box's address, as the `Integer` handle the Ruby side keeps.
fn handle<'cx, T>(cx: &mut Context<'cx>, boxed: Box<T>) -> Value<'cx> {
    let address = Box::into_raw(boxed) as usize;
    cx.integer(i64::try_from(address).expect("a user-space address fits in i64"))
}

/// `sqlite3_close_v2`: a connection with statements still open is closed
/// once the last is finalized.
fn close(db: &mut Db) {
    if !db.raw.is_null() {
        // SAFETY: an open connection, which nothing uses after this.
        unsafe { sqlite3_close_v2(db.raw) };
        db.raw = std::ptr::null_mut();
    }
}

/// `sqlite3_finalize`, once.
fn finalize(stmt: &mut Stmt) {
    if !stmt.raw.is_null() {
        // SAFETY: a prepared statement, which nothing uses after this.
        unsafe { ffi::sqlite3_finalize(stmt.raw) };
        stmt.raw = std::ptr::null_mut();
    }
}

/// The prepared statement in `stmt`, which the Ruby side only asks for once
/// it has checked there is one.
fn prepared(stmt: &Stmt) -> Result<*mut ffi::sqlite3_stmt> {
    if stmt.raw.is_null() {
        Err(Error::new(
            "SQLite3::Exception",
            "cannot use a closed statement",
        ))
    } else {
        Ok(stmt.raw)
    }
}

/// The open connection in `db`, which the Ruby side only asks for once it
/// has checked there is one.
fn open(db: &Db) -> Result<*mut ffi::sqlite3> {
    if db.raw.is_null() {
        Err(Error::new(
            "SQLite3::Exception",
            "cannot use a closed database",
        ))
    } else {
        Ok(db.raw)
    }
}

/// A bind of `bytes` with `bind`, `SQLITE_TOOBIG` for more than a C `int`
/// counts.
fn bind_bytes(
    raw: *mut ffi::sqlite3_stmt,
    index: c_int,
    bytes: &[u8],
    bind: unsafe extern "C" fn(
        *mut ffi::sqlite3_stmt,
        c_int,
        *const c_void,
        c_int,
        ffi::sqlite3_destructor_type,
    ) -> c_int,
) -> c_int {
    let Ok(len) = c_int::try_from(bytes.len()) else {
        return ffi::SQLITE_TOOBIG;
    };
    // SAFETY: a prepared statement; SQLITE_TRANSIENT makes SQLite copy the
    // bytes before this returns.
    unsafe {
        bind(
            raw,
            index,
            bytes.as_ptr().cast(),
            len,
            ffi::SQLITE_TRANSIENT(),
        )
    }
}

unsafe extern "C" fn bind_text(
    stmt: *mut ffi::sqlite3_stmt,
    index: c_int,
    data: *const c_void,
    len: c_int,
    destructor: ffi::sqlite3_destructor_type,
) -> c_int {
    // SAFETY: the caller's.
    unsafe { ffi::sqlite3_bind_text(stmt, index, data.cast(), len, destructor) }
}

unsafe extern "C" fn bind_text16(
    stmt: *mut ffi::sqlite3_stmt,
    index: c_int,
    data: *const c_void,
    len: c_int,
    destructor: ffi::sqlite3_destructor_type,
) -> c_int {
    // SAFETY: the caller's.
    unsafe { sqlite3_bind_text16(stmt, index, data, len, destructor) }
}

unsafe extern "C" fn bind_blob(
    stmt: *mut ffi::sqlite3_stmt,
    index: c_int,
    data: *const c_void,
    len: c_int,
    destructor: ffi::sqlite3_destructor_type,
) -> c_int {
    // SAFETY: the caller's.
    unsafe { ffi::sqlite3_bind_blob(stmt, index, data, len, destructor) }
}

methods! {
    /// `libversion`: `sqlite3_libversion()`, the loaded library's version.
    fn libversion(cx, _args) {
        // SAFETY: a static string.
        let text = unsafe { ffi::sqlite3_libversion() };
        Ok(c_str_or_nil(cx, text))
    }

    /// `libversion_number`: `sqlite3_libversion_number()`.
    fn libversion_number(cx, _args) {
        // SAFETY: no arguments.
        let n = unsafe { ffi::sqlite3_libversion_number() };
        Ok(cx.integer(i64::from(n)))
    }

    /// `threadsafe`: `sqlite3_threadsafe()`, the `SQLITE_THREADSAFE` it was
    /// compiled with.
    fn threadsafe(cx, _args) {
        // SAFETY: no arguments.
        let n = unsafe { ffi::sqlite3_threadsafe() };
        Ok(cx.integer(i64::from(n)))
    }

    /// `status(op, reset)`: `sqlite3_status`'s current and highwater values,
    /// as two little-endian 64-bit integers in one String.
    fn status(cx, args) {
        let op = c_int_of(cx, args[0])?;
        let reset = c_int::from(cx.is_truthy(args[1]));
        let (mut current, mut highwater) = (0, 0);
        // SAFETY: two valid places for the answers.
        unsafe { ffi::sqlite3_status(op, &raw mut current, &raw mut highwater, reset) };
        let mut bytes = i64::from(current).to_le_bytes().to_vec();
        bytes.extend_from_slice(&i64::from(highwater).to_le_bytes());
        Ok(cx.string_from_bytes(&bytes))
    }

    /// `complete(sql)`: `sqlite3_complete`.
    fn complete(cx, args) {
        let sql = c_string(cx, args[0])?;
        // SAFETY: a NUL-terminated string.
        let complete = unsafe { ffi::sqlite3_complete(sql.as_ptr()) } != 0;
        Ok(cx.boolean(complete))
    }

    /// `db_new`: an empty connection box's handle.
    fn db_new(cx, _args) {
        let db = Box::new(Db { tag: DB_TAG, raw: std::ptr::null_mut(), message: None });
        Ok(handle(cx, db))
    }

    /// `db_free(db)`: close the connection and free its box. Its holder's
    /// finalizer calls it; nothing uses the handle after.
    fn db_free(cx, args) {
        let db = db(cx, args[0])?;
        close(db);
        db.tag = 0;
        // SAFETY: a box `db_new` made, freed once.
        drop(unsafe { Box::from_raw(std::ptr::from_mut(db)) });
        Ok(cx.nil())
    }

    /// `db_open(db, filename, flags, vfs)`: `sqlite3_open_v2`. On failure
    /// the connection stays, for `db_errmsg`, until `db_close`.
    fn db_open(cx, args) {
        let db = db(cx, args[0])?;
        let filename = c_string(cx, args[1])?;
        let flags = c_int_of(cx, args[2])?;
        let vfs = if cx.is_nil(args[3]) { None } else { Some(c_string(cx, args[3])?) };
        close(db);
        // SAFETY: NUL-terminated strings, and a place for the connection.
        let rc = unsafe {
            ffi::sqlite3_open_v2(
                filename.as_ptr(),
                &raw mut db.raw,
                flags,
                vfs.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            )
        };
        Ok(code(cx, rc))
    }

    /// `db_close(db)`: `sqlite3_close_v2`.
    fn db_close(cx, args) {
        let db = db(cx, args[0])?;
        close(db);
        Ok(cx.nil())
    }

    /// `db_discard(db)`: let go of a connection carried across a `fork`
    /// without the writes closing it would make, as the sqlite3 gem's
    /// `discard_db` does: release its memory, close its files, and forget
    /// it, never calling `sqlite3_close`.
    fn db_discard(cx, args) {
        let db = db(cx, args[0])?;
        if !db.raw.is_null() {
            // SAFETY: an open connection; each file pointer SQLite answers
            // is its own open file, closed once, and the connection is never
            // used again.
            unsafe {
                ffi::sqlite3_db_release_memory(db.raw);
                let mut index = 0;
                loop {
                    let name = ffi::sqlite3_db_name(db.raw, index);
                    if name.is_null() {
                        break;
                    }
                    close_file(db.raw, name, ffi::SQLITE_FCNTL_FILE_POINTER);
                    index += 1;
                }
                close_file(db.raw, std::ptr::null(), ffi::SQLITE_FCNTL_JOURNAL_POINTER);
            }
            db.raw = std::ptr::null_mut();
        }
        Ok(cx.nil())
    }

    /// `db_open?(db)`: whether the connection is open.
    fn db_is_open(cx, args) {
        let db = db(cx, args[0])?;
        let open = !db.raw.is_null();
        Ok(cx.boolean(open))
    }

    /// `db_errcode(db)`: `sqlite3_errcode`.
    fn db_errcode(cx, args) {
        let raw = db(cx, args[0])?.raw;
        // SAFETY: an open connection or null, which answers SQLITE_NOMEM.
        let rc = unsafe { ffi::sqlite3_errcode(raw) };
        Ok(code(cx, rc))
    }

    /// `db_errmsg(db)`: `sqlite3_errmsg`.
    fn db_errmsg(cx, args) {
        let raw = db(cx, args[0])?.raw;
        // SAFETY: an open connection or null, which answers "out of memory".
        let text = unsafe { ffi::sqlite3_errmsg(raw) };
        Ok(c_str_or_nil(cx, text))
    }

    /// `db_error_offset(db)`: `sqlite3_error_offset`, -1 for none.
    fn db_error_offset(cx, args) {
        let raw = open(db(cx, args[0])?)?;
        // SAFETY: an open connection.
        let offset = unsafe { ffi::sqlite3_error_offset(raw) };
        Ok(cx.integer(i64::from(offset)))
    }

    /// `db_message(db)`: the message of the last `db_load_extension` that
    /// failed, or nil.
    fn db_message(cx, args) {
        let message = db(cx, args[0])?.message.take();
        Ok(match message {
            Some(bytes) => cx.string_from_bytes(&bytes),
            None => cx.nil(),
        })
    }

    /// `db_changes(db)`: `sqlite3_changes`.
    fn db_changes(cx, args) {
        let raw = open(db(cx, args[0])?)?;
        // SAFETY: an open connection.
        let n = unsafe { ffi::sqlite3_changes(raw) };
        Ok(cx.integer(i64::from(n)))
    }

    /// `db_total_changes(db)`: `sqlite3_total_changes`.
    fn db_total_changes(cx, args) {
        let raw = open(db(cx, args[0])?)?;
        // SAFETY: an open connection.
        let n = unsafe { ffi::sqlite3_total_changes(raw) };
        Ok(cx.integer(i64::from(n)))
    }

    /// `db_last_insert_rowid(db)`: `sqlite3_last_insert_rowid`.
    fn db_last_insert_rowid(cx, args) {
        let raw = open(db(cx, args[0])?)?;
        // SAFETY: an open connection.
        let id = unsafe { ffi::sqlite3_last_insert_rowid(raw) };
        Ok(cx.integer(id))
    }

    /// `db_autocommit(db)`: whether `sqlite3_get_autocommit` says no
    /// transaction is open.
    fn db_autocommit(cx, args) {
        let raw = open(db(cx, args[0])?)?;
        // SAFETY: an open connection.
        let autocommit = unsafe { ffi::sqlite3_get_autocommit(raw) } != 0;
        Ok(cx.boolean(autocommit))
    }

    /// `db_busy_timeout(db, ms)`: `sqlite3_busy_timeout`.
    fn db_busy_timeout(cx, args) {
        let raw = open(db(cx, args[0])?)?;
        let ms = c_int_of(cx, args[1])?;
        // SAFETY: an open connection.
        let rc = unsafe { ffi::sqlite3_busy_timeout(raw, ms) };
        Ok(code(cx, rc))
    }

    /// `db_extended_result_codes(db, on)`: `sqlite3_extended_result_codes`.
    fn db_extended_result_codes(cx, args) {
        let raw = open(db(cx, args[0])?)?;
        let on = c_int::from(cx.is_truthy(args[1]));
        // SAFETY: an open connection.
        let rc = unsafe { ffi::sqlite3_extended_result_codes(raw, on) };
        Ok(code(cx, rc))
    }

    /// `db_interrupt(db)`: `sqlite3_interrupt`.
    fn db_interrupt(cx, args) {
        let raw = open(db(cx, args[0])?)?;
        // SAFETY: an open connection.
        unsafe { ffi::sqlite3_interrupt(raw) };
        Ok(cx.nil())
    }

    /// `db_filename(db, name)`: `sqlite3_db_filename`, or nil.
    fn db_filename(cx, args) {
        let raw = open(db(cx, args[0])?)?;
        let name = c_string(cx, args[1])?;
        // SAFETY: an open connection and a NUL-terminated name.
        let text = unsafe { ffi::sqlite3_db_filename(raw, name.as_ptr()) };
        Ok(c_str_or_nil(cx, text))
    }

    /// `db_disable_dqs(db)`: turn off double-quoted string literals, in DDL
    /// and DML, as the `strict:` option does.
    fn db_disable_dqs(cx, args) {
        let raw = open(db(cx, args[0])?)?;
        // SAFETY: an open connection; these two options take an int and a
        // null out-pointer.
        unsafe {
            ffi::sqlite3_db_config(raw, ffi::SQLITE_DBCONFIG_DQS_DDL, 0, std::ptr::null_mut::<c_int>());
            ffi::sqlite3_db_config(raw, ffi::SQLITE_DBCONFIG_DQS_DML, 0, std::ptr::null_mut::<c_int>());
        }
        Ok(cx.boolean(true))
    }

    /// `db_enable_load_extension(db, onoff)`: `sqlite3_enable_load_extension`.
    fn db_enable_load_extension(cx, args) {
        let raw = open(db(cx, args[0])?)?;
        let on = c_int_of(cx, args[1])?;
        // SAFETY: an open connection.
        let rc = unsafe { ffi::sqlite3_enable_load_extension(raw, on) };
        Ok(code(cx, rc))
    }

    /// `db_load_extension(db, path)`: `sqlite3_load_extension` with its
    /// default entry point; its message, if it fails, is `db_message`'s.
    fn db_load_extension(cx, args) {
        let db = db(cx, args[0])?;
        let raw = open(db)?;
        let path = c_string(cx, args[1])?;
        let mut message: *mut c_char = std::ptr::null_mut();
        // SAFETY: an open connection, a NUL-terminated path, and a place for
        // a message SQLite allocates, freed here once copied.
        let rc = unsafe {
            let rc = ffi::sqlite3_load_extension(raw, path.as_ptr(), std::ptr::null(), &raw mut message);
            if !message.is_null() {
                db.message = Some(CStr::from_ptr(message).to_bytes().to_vec());
                ffi::sqlite3_free(message.cast());
            }
            rc
        };
        Ok(code(cx, rc))
    }

    /// `stmt_new`: an empty statement box's handle.
    fn stmt_new(cx, _args) {
        let stmt = Box::new(Stmt { tag: STMT_TAG, raw: std::ptr::null_mut(), tail: 0 });
        Ok(handle(cx, stmt))
    }

    /// `stmt_free(stmt)`: finalize the statement and free its box. Its
    /// holder's finalizer calls it; nothing uses the handle after.
    fn stmt_free(cx, args) {
        let stmt = stmt(cx, args[0])?;
        finalize(stmt);
        stmt.tag = 0;
        // SAFETY: a box `stmt_new` made, freed once.
        drop(unsafe { Box::from_raw(std::ptr::from_mut(stmt)) });
        Ok(cx.nil())
    }

    /// `stmt_prepare(stmt, db, sql)`: `sqlite3_prepare_v2` of the first
    /// statement in `sql`; `stmt_tail` is where it ended.
    fn stmt_prepare(cx, args) {
        let stmt = stmt(cx, args[0])?;
        let raw = open(db(cx, args[1])?)?;
        let sql = cx.to_bytes(args[2])?;
        let Ok(len) = c_int::try_from(sql.len()) else {
            return Ok(code(cx, ffi::SQLITE_TOOBIG));
        };
        finalize(stmt);
        let mut tail: *const c_char = std::ptr::null();
        // SAFETY: an open connection, `len` bytes of SQL, and places for the
        // statement and the tail, which points into `sql`.
        let rc = unsafe {
            ffi::sqlite3_prepare_v2(raw, sql.as_ptr().cast(), len, &raw mut stmt.raw, &raw mut tail)
        };
        stmt.tail = if tail.is_null() {
            sql.len()
        } else {
            (tail as usize).saturating_sub(sql.as_ptr() as usize).min(sql.len())
        };
        Ok(code(cx, rc))
    }

    /// `stmt_tail(stmt)`: the byte offset where the prepared statement
    /// ended in its SQL.
    fn stmt_tail(cx, args) {
        let tail = stmt(cx, args[0])?.tail;
        Ok(cx.integer(i64::try_from(tail).unwrap_or(i64::MAX)))
    }

    /// `stmt_finalize(stmt)`: `sqlite3_finalize`.
    fn stmt_finalize(cx, args) {
        let stmt = stmt(cx, args[0])?;
        finalize(stmt);
        Ok(cx.nil())
    }

    /// `stmt_open?(stmt)`: whether there is a prepared statement.
    fn stmt_is_open(cx, args) {
        let open = !stmt(cx, args[0])?.raw.is_null();
        Ok(cx.boolean(open))
    }

    /// `stmt_step(stmt)`: `sqlite3_step`.
    fn stmt_step(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        // SAFETY: a prepared statement.
        let rc = unsafe { ffi::sqlite3_step(raw) };
        Ok(code(cx, rc))
    }

    /// `stmt_reset(stmt)`: `sqlite3_reset`.
    fn stmt_reset(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        // SAFETY: a prepared statement.
        let rc = unsafe { ffi::sqlite3_reset(raw) };
        Ok(code(cx, rc))
    }

    /// `stmt_clear_bindings(stmt)`: `sqlite3_clear_bindings`.
    fn stmt_clear_bindings(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        // SAFETY: a prepared statement.
        let rc = unsafe { ffi::sqlite3_clear_bindings(raw) };
        Ok(code(cx, rc))
    }

    /// `stmt_busy(stmt)`: `sqlite3_stmt_busy`.
    fn stmt_busy(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        // SAFETY: a prepared statement.
        let busy = unsafe { ffi::sqlite3_stmt_busy(raw) } != 0;
        Ok(cx.boolean(busy))
    }

    /// `stmt_column_count(stmt)`: `sqlite3_column_count`.
    fn stmt_column_count(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        // SAFETY: a prepared statement.
        let n = unsafe { ffi::sqlite3_column_count(raw) };
        Ok(cx.integer(i64::from(n)))
    }

    /// `stmt_column_type(stmt, i)`: `sqlite3_column_type`.
    fn stmt_column_type(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a prepared statement; SQLite answers NULL out of range.
        let kind = unsafe { ffi::sqlite3_column_type(raw, i) };
        Ok(cx.integer(i64::from(kind)))
    }

    /// `stmt_column_int64(stmt, i)`: `sqlite3_column_int64`.
    fn stmt_column_int64(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a prepared statement.
        let n = unsafe { ffi::sqlite3_column_int64(raw, i) };
        Ok(cx.integer(n))
    }

    /// `stmt_column_double(stmt, i)`: `sqlite3_column_double`, as the 8
    /// little-endian bytes of the double (`unpack1("E")`).
    fn stmt_column_double(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a prepared statement.
        let x = unsafe { ffi::sqlite3_column_double(raw, i) };
        Ok(cx.string_from_bytes(&x.to_le_bytes()))
    }

    /// `stmt_column_text(stmt, i)`: `sqlite3_column_text`'s bytes.
    fn stmt_column_text(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a prepared statement; the text, then its length, as
        // SQLite says to ask for them.
        let (data, len) = unsafe {
            let data = ffi::sqlite3_column_text(raw, i);
            (data, ffi::sqlite3_column_bytes(raw, i))
        };
        Ok(bytes_at(cx, data.cast(), len))
    }

    /// `stmt_column_blob(stmt, i)`: `sqlite3_column_blob`'s bytes.
    fn stmt_column_blob(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: as for `stmt_column_text`.
        let (data, len) = unsafe {
            let data = ffi::sqlite3_column_blob(raw, i);
            (data, ffi::sqlite3_column_bytes(raw, i))
        };
        Ok(bytes_at(cx, data, len))
    }

    /// `stmt_column_name(stmt, i)`: `sqlite3_column_name`, or nil.
    fn stmt_column_name(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a prepared statement; null out of range.
        let text = unsafe { ffi::sqlite3_column_name(raw, i) };
        Ok(c_str_or_nil(cx, text))
    }

    /// `stmt_column_decltype(stmt, i)`: `sqlite3_column_decltype`, or nil.
    fn stmt_column_decltype(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a prepared statement; null out of range or for none.
        let text = unsafe { ffi::sqlite3_column_decltype(raw, i) };
        Ok(c_str_or_nil(cx, text))
    }

    /// `stmt_column_database_name(stmt, i)`: `sqlite3_column_database_name`,
    /// or nil.
    fn stmt_column_database_name(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a prepared statement; null out of range or for none.
        let text = unsafe { ffi::sqlite3_column_database_name(raw, i) };
        Ok(c_str_or_nil(cx, text))
    }

    /// `stmt_bind_parameter_count(stmt)`: `sqlite3_bind_parameter_count`.
    fn stmt_bind_parameter_count(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        // SAFETY: a prepared statement.
        let n = unsafe { ffi::sqlite3_bind_parameter_count(raw) };
        Ok(cx.integer(i64::from(n)))
    }

    /// `stmt_bind_parameter_index(stmt, name)`:
    /// `sqlite3_bind_parameter_index`, 0 for no such parameter.
    fn stmt_bind_parameter_index(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let Ok(name) = CString::new(cx.to_bytes(args[1])?) else {
            return Ok(cx.integer(0));
        };
        // SAFETY: a prepared statement and a NUL-terminated name.
        let index = unsafe { ffi::sqlite3_bind_parameter_index(raw, name.as_ptr()) };
        Ok(cx.integer(i64::from(index)))
    }

    /// `stmt_bind_parameter_name(stmt, i)`: `sqlite3_bind_parameter_name`,
    /// or nil.
    fn stmt_bind_parameter_name(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a prepared statement; null out of range or for `?`.
        let text = unsafe { ffi::sqlite3_bind_parameter_name(raw, i) };
        Ok(c_str_or_nil(cx, text))
    }

    /// `stmt_bind_null(stmt, i)`: `sqlite3_bind_null`.
    fn stmt_bind_null(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        // SAFETY: a prepared statement.
        let rc = unsafe { ffi::sqlite3_bind_null(raw, i) };
        Ok(code(cx, rc))
    }

    /// `stmt_bind_int64(stmt, i, n)`: `sqlite3_bind_int64`.
    fn stmt_bind_int64(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        let n = cx.to_i64(args[2])?;
        // SAFETY: a prepared statement.
        let rc = unsafe { ffi::sqlite3_bind_int64(raw, i, n) };
        Ok(code(cx, rc))
    }

    /// `stmt_bind_double(stmt, i, bytes)`: `sqlite3_bind_double` of the
    /// double whose 8 little-endian bytes are `bytes` (`[x].pack("E")`).
    fn stmt_bind_double(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        let bytes: [u8; 8] = cx
            .to_bytes(args[2])?
            .try_into()
            .map_err(|_| Error::new("ArgumentError", "a double is 8 bytes"))?;
        // SAFETY: a prepared statement.
        let rc = unsafe { ffi::sqlite3_bind_double(raw, i, f64::from_le_bytes(bytes)) };
        Ok(code(cx, rc))
    }

    /// `stmt_bind_text(stmt, i, bytes)`: `sqlite3_bind_text`, UTF-8.
    fn stmt_bind_text(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        let bytes = cx.to_bytes(args[2])?;
        let rc = bind_bytes(raw, i, &bytes, bind_text);
        Ok(code(cx, rc))
    }

    /// `stmt_bind_text16(stmt, i, bytes)`: `sqlite3_bind_text16`, UTF-16 in
    /// the byte order its BOM says, or the machine's.
    fn stmt_bind_text16(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        let bytes = cx.to_bytes(args[2])?;
        let rc = bind_bytes(raw, i, &bytes, bind_text16);
        Ok(code(cx, rc))
    }

    /// `stmt_bind_blob(stmt, i, bytes)`: `sqlite3_bind_blob`.
    fn stmt_bind_blob(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let i = c_int_of(cx, args[1])?;
        let bytes = cx.to_bytes(args[2])?;
        let rc = bind_bytes(raw, i, &bytes, bind_blob);
        Ok(code(cx, rc))
    }

    /// `stmt_sql(stmt)`: `sqlite3_sql`.
    fn stmt_sql(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        // SAFETY: a prepared statement.
        let text = unsafe { ffi::sqlite3_sql(raw) };
        Ok(c_str_or_nil(cx, text))
    }

    /// `stmt_expanded_sql(stmt)`: `sqlite3_expanded_sql`, or nil.
    fn stmt_expanded_sql(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        // SAFETY: a prepared statement; SQLite allocates the text, freed
        // here once copied.
        let text = unsafe { ffi::sqlite3_expanded_sql(raw) };
        let value = c_str_or_nil(cx, text);
        // SAFETY: SQLite's allocation, or null.
        unsafe { ffi::sqlite3_free(text.cast()) };
        Ok(value)
    }

    /// `stmt_status(stmt, op, reset)`: `sqlite3_stmt_status`.
    fn stmt_status(cx, args) {
        let raw = prepared(stmt(cx, args[0])?)?;
        let op = c_int_of(cx, args[1])?;
        let reset = c_int::from(cx.is_truthy(args[2]));
        // SAFETY: a prepared statement.
        let n = unsafe { ffi::sqlite3_stmt_status(raw, op, reset) };
        Ok(cx.integer(i64::from(n)))
    }
}

/// Close the file `sqlite3_file_control(db, name, op)` answers, if it is
/// open.
///
/// # Safety
///
/// `db` is an open connection and `name` a database name of it or null.
unsafe fn close_file(db: *mut ffi::sqlite3, name: *const c_char, op: c_int) {
    let mut file: *mut ffi::sqlite3_file = std::ptr::null_mut();
    // SAFETY: the caller's; `op` answers a file pointer.
    let rc = unsafe { ffi::sqlite3_file_control(db, name, op, (&raw mut file).cast()) };
    if rc == ffi::SQLITE_OK && !file.is_null() {
        // SAFETY: an open file of the connection, whose methods close it.
        unsafe {
            if let Some(methods) = (*file).pMethods.as_ref()
                && let Some(close) = methods.xClose
            {
                close(file);
            }
        }
    }
}
