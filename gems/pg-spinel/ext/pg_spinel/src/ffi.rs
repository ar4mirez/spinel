//! The part of libpq's C API (`libpq-fe.h`, `libpq/libpq-fs.h`) this
//! extension calls, as PostgreSQL 10's libpq has it, the oldest the pg gem
//! supports. What later releases added is looked up by name when called
//! (`crate::optional`), so these are all that is linked.

use std::ffi::{c_char, c_int, c_uchar, c_uint, c_void};

/// `PGconn`.
#[repr(C)]
pub struct PGconn {
    _private: [u8; 0],
}

/// `PGresult`.
#[repr(C)]
pub struct PGresult {
    _private: [u8; 0],
}

/// `PGcancel`.
#[repr(C)]
pub struct PGcancel {
    _private: [u8; 0],
}

/// `PGcancelConn` (libpq 17).
#[repr(C)]
pub struct PGcancelConn {
    _private: [u8; 0],
}

/// `FILE`.
#[repr(C)]
pub struct CFile {
    _private: [u8; 0],
}

pub type Oid = c_uint;

/// `PQconninfoOption`.
#[repr(C)]
pub struct PQconninfoOption {
    pub keyword: *mut c_char,
    pub envvar: *mut c_char,
    pub compiled: *mut c_char,
    pub val: *mut c_char,
    pub label: *mut c_char,
    pub dispchar: *mut c_char,
    pub dispsize: c_int,
}

/// `PGnotify`.
#[repr(C)]
pub struct PGnotify {
    pub relname: *mut c_char,
    pub be_pid: c_int,
    pub extra: *mut c_char,
    pub next: *mut PGnotify,
}

#[link(name = "pq")]
unsafe extern "C" {
    pub fn PQlibVersion() -> c_int;
    pub fn PQisthreadsafe() -> c_int;
    pub fn PQinitSSL(do_init: c_int);
    pub fn PQinitOpenSSL(do_ssl: c_int, do_crypto: c_int);
    pub fn pg_encoding_to_char(encoding: c_int) -> *const c_char;

    pub fn PQconnectdb(conninfo: *const c_char) -> *mut PGconn;
    pub fn PQconnectStart(conninfo: *const c_char) -> *mut PGconn;
    pub fn PQconnectPoll(conn: *mut PGconn) -> c_int;
    pub fn PQfinish(conn: *mut PGconn);
    pub fn PQreset(conn: *mut PGconn);
    pub fn PQresetStart(conn: *mut PGconn) -> c_int;
    pub fn PQresetPoll(conn: *mut PGconn) -> c_int;
    pub fn PQping(conninfo: *const c_char) -> c_int;
    pub fn PQconndefaults() -> *mut PQconninfoOption;
    pub fn PQconninfoParse(
        conninfo: *const c_char,
        errmsg: *mut *mut c_char,
    ) -> *mut PQconninfoOption;
    pub fn PQconninfo(conn: *mut PGconn) -> *mut PQconninfoOption;
    pub fn PQconninfoFree(options: *mut PQconninfoOption);

    pub fn PQdb(conn: *const PGconn) -> *mut c_char;
    pub fn PQuser(conn: *const PGconn) -> *mut c_char;
    pub fn PQpass(conn: *const PGconn) -> *mut c_char;
    pub fn PQhost(conn: *const PGconn) -> *mut c_char;
    pub fn PQport(conn: *const PGconn) -> *mut c_char;
    pub fn PQoptions(conn: *const PGconn) -> *mut c_char;
    pub fn PQstatus(conn: *const PGconn) -> c_int;
    pub fn PQtransactionStatus(conn: *const PGconn) -> c_int;
    pub fn PQparameterStatus(conn: *const PGconn, name: *const c_char) -> *const c_char;
    pub fn PQprotocolVersion(conn: *const PGconn) -> c_int;
    pub fn PQserverVersion(conn: *const PGconn) -> c_int;
    pub fn PQerrorMessage(conn: *const PGconn) -> *mut c_char;
    pub fn PQsocket(conn: *const PGconn) -> c_int;
    pub fn PQbackendPID(conn: *const PGconn) -> c_int;
    pub fn PQconnectionNeedsPassword(conn: *const PGconn) -> c_int;
    pub fn PQconnectionUsedPassword(conn: *const PGconn) -> c_int;
    pub fn PQclientEncoding(conn: *const PGconn) -> c_int;
    pub fn PQsetClientEncoding(conn: *mut PGconn, encoding: *const c_char) -> c_int;
    pub fn PQsslInUse(conn: *mut PGconn) -> c_int;
    pub fn PQsslAttribute(conn: *mut PGconn, name: *const c_char) -> *const c_char;
    pub fn PQsslAttributeNames(conn: *mut PGconn) -> *const *const c_char;
    pub fn PQsetErrorVerbosity(conn: *mut PGconn, verbosity: c_int) -> c_int;
    pub fn PQsetErrorContextVisibility(conn: *mut PGconn, show_context: c_int) -> c_int;
    pub fn PQtrace(conn: *mut PGconn, stream: *mut CFile);
    pub fn PQuntrace(conn: *mut PGconn);

    pub fn PQgetCancel(conn: *mut PGconn) -> *mut PGcancel;
    pub fn PQfreeCancel(cancel: *mut PGcancel);
    pub fn PQcancel(cancel: *mut PGcancel, errbuf: *mut c_char, errbufsize: c_int) -> c_int;

    pub fn PQexec(conn: *mut PGconn, query: *const c_char) -> *mut PGresult;
    pub fn PQexecParams(
        conn: *mut PGconn,
        command: *const c_char,
        n_params: c_int,
        param_types: *const Oid,
        param_values: *const *const c_char,
        param_lengths: *const c_int,
        param_formats: *const c_int,
        result_format: c_int,
    ) -> *mut PGresult;
    pub fn PQprepare(
        conn: *mut PGconn,
        name: *const c_char,
        query: *const c_char,
        n_params: c_int,
        param_types: *const Oid,
    ) -> *mut PGresult;
    pub fn PQexecPrepared(
        conn: *mut PGconn,
        name: *const c_char,
        n_params: c_int,
        param_values: *const *const c_char,
        param_lengths: *const c_int,
        param_formats: *const c_int,
        result_format: c_int,
    ) -> *mut PGresult;
    pub fn PQdescribePrepared(conn: *mut PGconn, name: *const c_char) -> *mut PGresult;
    pub fn PQdescribePortal(conn: *mut PGconn, name: *const c_char) -> *mut PGresult;
    pub fn PQsendQuery(conn: *mut PGconn, query: *const c_char) -> c_int;
    pub fn PQsendQueryParams(
        conn: *mut PGconn,
        command: *const c_char,
        n_params: c_int,
        param_types: *const Oid,
        param_values: *const *const c_char,
        param_lengths: *const c_int,
        param_formats: *const c_int,
        result_format: c_int,
    ) -> c_int;
    pub fn PQsendPrepare(
        conn: *mut PGconn,
        name: *const c_char,
        query: *const c_char,
        n_params: c_int,
        param_types: *const Oid,
    ) -> c_int;
    pub fn PQsendQueryPrepared(
        conn: *mut PGconn,
        name: *const c_char,
        n_params: c_int,
        param_values: *const *const c_char,
        param_lengths: *const c_int,
        param_formats: *const c_int,
        result_format: c_int,
    ) -> c_int;
    pub fn PQsendDescribePrepared(conn: *mut PGconn, name: *const c_char) -> c_int;
    pub fn PQsendDescribePortal(conn: *mut PGconn, name: *const c_char) -> c_int;
    pub fn PQgetResult(conn: *mut PGconn) -> *mut PGresult;
    pub fn PQisBusy(conn: *mut PGconn) -> c_int;
    pub fn PQconsumeInput(conn: *mut PGconn) -> c_int;
    pub fn PQnotifies(conn: *mut PGconn) -> *mut PGnotify;
    pub fn PQputCopyData(conn: *mut PGconn, buffer: *const c_char, nbytes: c_int) -> c_int;
    pub fn PQputCopyEnd(conn: *mut PGconn, errormsg: *const c_char) -> c_int;
    pub fn PQgetCopyData(conn: *mut PGconn, buffer: *mut *mut c_char, is_async: c_int) -> c_int;
    pub fn PQsetnonblocking(conn: *mut PGconn, arg: c_int) -> c_int;
    pub fn PQisnonblocking(conn: *const PGconn) -> c_int;
    pub fn PQflush(conn: *mut PGconn) -> c_int;
    pub fn PQsetSingleRowMode(conn: *mut PGconn) -> c_int;
    pub fn PQmakeEmptyPGresult(conn: *mut PGconn, status: c_int) -> *mut PGresult;

    pub fn PQresultStatus(res: *const PGresult) -> c_int;
    pub fn PQresStatus(status: c_int) -> *mut c_char;
    pub fn PQresultErrorMessage(res: *const PGresult) -> *mut c_char;
    pub fn PQresultVerboseErrorMessage(
        res: *const PGresult,
        verbosity: c_int,
        show_context: c_int,
    ) -> *mut c_char;
    pub fn PQresultErrorField(res: *const PGresult, fieldcode: c_int) -> *mut c_char;
    pub fn PQntuples(res: *const PGresult) -> c_int;
    pub fn PQnfields(res: *const PGresult) -> c_int;
    pub fn PQbinaryTuples(res: *const PGresult) -> c_int;
    pub fn PQfname(res: *const PGresult, field_num: c_int) -> *mut c_char;
    pub fn PQfnumber(res: *const PGresult, field_name: *const c_char) -> c_int;
    pub fn PQftable(res: *const PGresult, field_num: c_int) -> Oid;
    pub fn PQftablecol(res: *const PGresult, field_num: c_int) -> c_int;
    pub fn PQfformat(res: *const PGresult, field_num: c_int) -> c_int;
    pub fn PQftype(res: *const PGresult, field_num: c_int) -> Oid;
    pub fn PQfsize(res: *const PGresult, field_num: c_int) -> c_int;
    pub fn PQfmod(res: *const PGresult, field_num: c_int) -> c_int;
    pub fn PQcmdStatus(res: *mut PGresult) -> *mut c_char;
    pub fn PQoidValue(res: *const PGresult) -> Oid;
    pub fn PQcmdTuples(res: *mut PGresult) -> *mut c_char;
    pub fn PQgetvalue(res: *const PGresult, tup_num: c_int, field_num: c_int) -> *mut c_char;
    pub fn PQgetlength(res: *const PGresult, tup_num: c_int, field_num: c_int) -> c_int;
    pub fn PQgetisnull(res: *const PGresult, tup_num: c_int, field_num: c_int) -> c_int;
    pub fn PQnparams(res: *const PGresult) -> c_int;
    pub fn PQparamtype(res: *const PGresult, param_num: c_int) -> Oid;
    pub fn PQclear(res: *mut PGresult);
    pub fn PQfreemem(ptr: *mut c_void);

    pub fn PQescapeStringConn(
        conn: *mut PGconn,
        to: *mut c_char,
        from: *const c_char,
        length: usize,
        error: *mut c_int,
    ) -> usize;
    pub fn PQescapeString(to: *mut c_char, from: *const c_char, length: usize) -> usize;
    pub fn PQescapeLiteral(conn: *mut PGconn, str: *const c_char, len: usize) -> *mut c_char;
    pub fn PQescapeIdentifier(conn: *mut PGconn, str: *const c_char, len: usize) -> *mut c_char;
    pub fn PQescapeByteaConn(
        conn: *mut PGconn,
        from: *const c_uchar,
        from_length: usize,
        to_length: *mut usize,
    ) -> *mut c_uchar;
    pub fn PQescapeBytea(
        from: *const c_uchar,
        from_length: usize,
        to_length: *mut usize,
    ) -> *mut c_uchar;
    pub fn PQunescapeBytea(strtext: *const c_uchar, retbuflen: *mut usize) -> *mut c_uchar;
    pub fn PQencryptPassword(passwd: *const c_char, user: *const c_char) -> *mut c_char;
    pub fn PQencryptPasswordConn(
        conn: *mut PGconn,
        passwd: *const c_char,
        user: *const c_char,
        algorithm: *const c_char,
    ) -> *mut c_char;

    pub fn lo_creat(conn: *mut PGconn, mode: c_int) -> Oid;
    pub fn lo_create(conn: *mut PGconn, lobj_id: Oid) -> Oid;
    pub fn lo_import(conn: *mut PGconn, filename: *const c_char) -> Oid;
    pub fn lo_export(conn: *mut PGconn, lobj_id: Oid, filename: *const c_char) -> c_int;
    pub fn lo_open(conn: *mut PGconn, lobj_id: Oid, mode: c_int) -> c_int;
    pub fn lo_write(conn: *mut PGconn, fd: c_int, buf: *const c_char, len: usize) -> c_int;
    pub fn lo_read(conn: *mut PGconn, fd: c_int, buf: *mut c_char, len: usize) -> c_int;
    pub fn lo_lseek(conn: *mut PGconn, fd: c_int, offset: c_int, whence: c_int) -> c_int;
    pub fn lo_tell(conn: *mut PGconn, fd: c_int) -> c_int;
    pub fn lo_truncate(conn: *mut PGconn, fd: c_int, len: usize) -> c_int;
    pub fn lo_close(conn: *mut PGconn, fd: c_int) -> c_int;
    pub fn lo_unlink(conn: *mut PGconn, lobj_id: Oid) -> c_int;
}
