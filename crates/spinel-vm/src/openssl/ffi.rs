//! The OpenSSL 3 calls `tls.rs` makes that `openssl-sys` does not bind, or
//! binds behind a feature this crate does not turn on. Each is declared as
//! OpenSSL 3.0's headers declare it; OpenSSL's macros over `SSL_ctrl` and
//! `SSL_CTX_ctrl` are spelled out in `tls.rs` with the numbers below.

#![allow(non_camel_case_types, non_snake_case)]

use std::ffi::{c_char, c_int, c_long, c_uchar, c_uint, c_ulong, c_void};

use openssl_sys as sys;

/// `OSSL_DECODER_CTX`, opaque.
pub enum OSSL_DECODER_CTX {}
/// `OPENSSL_STACK`, opaque: every `STACK_OF(T)` is one.
pub enum OPENSSL_STACK {}
/// `X509_LOOKUP`, opaque.
pub enum X509_LOOKUP {}
/// `X509_LOOKUP_METHOD`, opaque.
pub enum X509_LOOKUP_METHOD {}
/// `X509V3_CTX`: `X509V3_set_ctx` fills it in, and nothing here reads its
/// fields, so it is room enough for OpenSSL 3's struct (seven pointers and
/// an int), with some to spare.
#[repr(C)]
pub struct X509V3_CTX {
    pub room: [usize; 16],
}

pub type VerifyCallback = unsafe extern "C" fn(c_int, *mut sys::X509_STORE_CTX) -> c_int;
pub type NewSessionCallback = unsafe extern "C" fn(*mut sys::SSL, *mut sys::SSL_SESSION) -> c_int;
pub type KeylogCallback = unsafe extern "C" fn(*const sys::SSL, *const c_char);
pub type ClientHelloCallback =
    unsafe extern "C" fn(*mut sys::SSL, *mut c_int, *mut c_void) -> c_int;
pub type AlpnSelectCallback = unsafe extern "C" fn(
    *mut sys::SSL,
    *mut *const c_uchar,
    *mut c_uchar,
    *const c_uchar,
    c_uint,
    *mut c_void,
) -> c_int;
pub type ClientCertCallback =
    unsafe extern "C" fn(*mut sys::SSL, *mut *mut sys::X509, *mut *mut sys::EVP_PKEY) -> c_int;

unsafe extern "C" {
    // crypto
    pub fn OPENSSL_sk_new_null() -> *mut OPENSSL_STACK;
    pub fn OPENSSL_sk_push(st: *mut OPENSSL_STACK, data: *const c_void) -> c_int;
    pub fn OPENSSL_sk_num(st: *const OPENSSL_STACK) -> c_int;
    pub fn OPENSSL_sk_value(st: *const OPENSSL_STACK, i: c_int) -> *mut c_void;
    pub fn OPENSSL_sk_free(st: *mut OPENSSL_STACK);
    pub fn CRYPTO_free(ptr: *mut c_void, file: *const c_char, line: c_int);
    pub fn ERR_peek_last_error() -> c_ulong;
    pub fn ERR_peek_error() -> c_ulong;
    pub fn ERR_clear_error();

    // BIO
    pub fn BIO_new(method: *const sys::BIO_METHOD) -> *mut sys::BIO;
    pub fn BIO_s_mem() -> *const sys::BIO_METHOD;
    pub fn BIO_new_mem_buf(buf: *const c_void, len: c_int) -> *mut sys::BIO;
    pub fn BIO_free(bio: *mut sys::BIO) -> c_int;
    pub fn BIO_ctrl(bio: *mut sys::BIO, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;

    // BN and ASN1
    pub fn BN_hex2bn(bn: *mut *mut sys::BIGNUM, hex: *const c_char) -> c_int;
    pub fn BN_bn2hex(bn: *const sys::BIGNUM) -> *mut c_char;
    pub fn BN_free(bn: *mut sys::BIGNUM);
    pub fn BN_new() -> *mut sys::BIGNUM;
    pub fn BN_check_prime(bn: *const sys::BIGNUM, ctx: *mut c_void, cb: *mut c_void) -> c_int;
    pub fn BN_generate_prime_ex(
        ret: *mut sys::BIGNUM,
        bits: c_int,
        safe: c_int,
        add: *const sys::BIGNUM,
        rem: *const sys::BIGNUM,
        cb: *mut c_void,
    ) -> c_int;
    pub fn ASN1_INTEGER_to_BN(
        ai: *const sys::ASN1_INTEGER,
        bn: *mut sys::BIGNUM,
    ) -> *mut sys::BIGNUM;
    pub fn BN_to_ASN1_INTEGER(
        bn: *const sys::BIGNUM,
        ai: *mut sys::ASN1_INTEGER,
    ) -> *mut sys::ASN1_INTEGER;
    pub fn ASN1_INTEGER_free(ai: *mut sys::ASN1_INTEGER);
    pub fn ASN1_TIME_to_tm(s: *const sys::ASN1_TIME, tm: *mut Tm) -> c_int;
    pub fn ASN1_TIME_free(s: *mut sys::ASN1_TIME);
    pub fn X509_time_adj_ex(
        s: *mut sys::ASN1_TIME,
        offset_day: c_int,
        offset_sec: c_long,
        in_tm: *mut i64,
    ) -> *mut sys::ASN1_TIME;
    pub fn ASN1_STRING_print(bio: *mut sys::BIO, v: *const sys::ASN1_STRING) -> c_int;
    pub fn ASN1_STRING_get0_data(x: *const sys::ASN1_STRING) -> *const c_uchar;
    pub fn ASN1_STRING_length(x: *const sys::ASN1_STRING) -> c_int;
    pub fn ASN1_STRING_type(x: *const sys::ASN1_STRING) -> c_int;
    pub fn ASN1_OCTET_STRING_new() -> *mut sys::ASN1_OCTET_STRING;
    pub fn ASN1_OCTET_STRING_set(
        s: *mut sys::ASN1_OCTET_STRING,
        data: *const c_uchar,
        len: c_int,
    ) -> c_int;
    pub fn ASN1_OCTET_STRING_free(s: *mut sys::ASN1_OCTET_STRING);
    pub fn i2a_ASN1_OBJECT(bio: *mut sys::BIO, a: *const sys::ASN1_OBJECT) -> c_int;
    pub fn i2t_ASN1_OBJECT(buf: *mut c_char, len: c_int, a: *const sys::ASN1_OBJECT) -> c_int;

    // objects
    pub fn OBJ_txt2obj(s: *const c_char, no_name: c_int) -> *mut sys::ASN1_OBJECT;
    pub fn OBJ_obj2nid(o: *const sys::ASN1_OBJECT) -> c_int;
    pub fn OBJ_nid2sn(n: c_int) -> *const c_char;
    pub fn OBJ_nid2ln(n: c_int) -> *const c_char;
    pub fn OBJ_ln2nid(s: *const c_char) -> c_int;
    pub fn OBJ_sn2nid(s: *const c_char) -> c_int;
    pub fn OBJ_obj2txt(
        buf: *mut c_char,
        len: c_int,
        a: *const sys::ASN1_OBJECT,
        no_name: c_int,
    ) -> c_int;
    pub fn ASN1_OBJECT_free(a: *mut sys::ASN1_OBJECT);

    // keys
    pub fn OSSL_DECODER_CTX_new_for_pkey(
        pkey: *mut *mut sys::EVP_PKEY,
        input_type: *const c_char,
        input_struct: *const c_char,
        keytype: *const c_char,
        selection: c_int,
        libctx: *mut c_void,
        propquery: *const c_char,
    ) -> *mut OSSL_DECODER_CTX;
    pub fn OSSL_DECODER_CTX_set_passphrase(
        ctx: *mut OSSL_DECODER_CTX,
        kstr: *const c_uchar,
        klen: usize,
    ) -> c_int;
    pub fn OSSL_DECODER_CTX_set_input_type(
        ctx: *mut OSSL_DECODER_CTX,
        input_type: *const c_char,
    ) -> c_int;
    pub fn OSSL_DECODER_CTX_set_selection(ctx: *mut OSSL_DECODER_CTX, selection: c_int) -> c_int;
    pub fn OSSL_DECODER_from_bio(ctx: *mut OSSL_DECODER_CTX, bio: *mut sys::BIO) -> c_int;
    pub fn OSSL_DECODER_CTX_free(ctx: *mut OSSL_DECODER_CTX);
    pub fn d2i_RSAPublicKey_bio(bio: *mut sys::BIO, rsa: *mut *mut sys::RSA) -> *mut sys::RSA;
    pub fn PEM_read_bio_RSAPublicKey(
        bio: *mut sys::BIO,
        rsa: *mut *mut sys::RSA,
        cb: *mut c_void,
        u: *mut c_void,
    ) -> *mut sys::RSA;
    pub fn RSA_free(rsa: *mut sys::RSA);
    pub fn EVP_PKEY_new() -> *mut sys::EVP_PKEY;
    pub fn EVP_PKEY_assign(pkey: *mut sys::EVP_PKEY, kind: c_int, key: *mut c_void) -> c_int;
    pub fn EVP_PKEY_free(pkey: *mut sys::EVP_PKEY);
    pub fn EVP_PKEY_up_ref(pkey: *mut sys::EVP_PKEY) -> c_int;
    pub fn EVP_PKEY_get_base_id(pkey: *const sys::EVP_PKEY) -> c_int;
    pub fn EVP_PKEY_get_id(pkey: *const sys::EVP_PKEY) -> c_int;
    pub fn EVP_PKEY_get0_type_name(pkey: *const sys::EVP_PKEY) -> *const c_char;
    pub fn EVP_PKEY_get_bn_param(
        pkey: *const sys::EVP_PKEY,
        name: *const c_char,
        bn: *mut *mut sys::BIGNUM,
    ) -> c_int;
    pub fn EVP_PKEY_eq(a: *const sys::EVP_PKEY, b: *const sys::EVP_PKEY) -> c_int;
    pub fn EVP_PKEY_print_private(
        bio: *mut sys::BIO,
        pkey: *const sys::EVP_PKEY,
        indent: c_int,
        pctx: *mut c_void,
    ) -> c_int;
    pub fn EVP_PKEY_print_public(
        bio: *mut sys::BIO,
        pkey: *const sys::EVP_PKEY,
        indent: c_int,
        pctx: *mut c_void,
    ) -> c_int;
    pub fn EVP_PKEY_CTX_new_from_name(
        libctx: *mut c_void,
        name: *const c_char,
        propq: *const c_char,
    ) -> *mut sys::EVP_PKEY_CTX;
    pub fn EVP_PKEY_CTX_new(
        pkey: *mut sys::EVP_PKEY,
        engine: *mut c_void,
    ) -> *mut sys::EVP_PKEY_CTX;
    pub fn EVP_PKEY_CTX_free(ctx: *mut sys::EVP_PKEY_CTX);
    pub fn EVP_PKEY_keygen_init(ctx: *mut sys::EVP_PKEY_CTX) -> c_int;
    pub fn EVP_PKEY_paramgen_init(ctx: *mut sys::EVP_PKEY_CTX) -> c_int;
    pub fn EVP_PKEY_keygen(ctx: *mut sys::EVP_PKEY_CTX, pkey: *mut *mut sys::EVP_PKEY) -> c_int;
    pub fn EVP_PKEY_paramgen(ctx: *mut sys::EVP_PKEY_CTX, pkey: *mut *mut sys::EVP_PKEY) -> c_int;
    pub fn EVP_PKEY_CTX_ctrl_str(
        ctx: *mut sys::EVP_PKEY_CTX,
        kind: *const c_char,
        value: *const c_char,
    ) -> c_int;
    pub fn EVP_MD_CTX_new() -> *mut sys::EVP_MD_CTX;
    pub fn EVP_MD_CTX_free(ctx: *mut sys::EVP_MD_CTX);
    pub fn EVP_DigestSignInit(
        ctx: *mut sys::EVP_MD_CTX,
        pctx: *mut *mut sys::EVP_PKEY_CTX,
        md: *const sys::EVP_MD,
        e: *mut c_void,
        pkey: *mut sys::EVP_PKEY,
    ) -> c_int;
    pub fn EVP_DigestSign(
        ctx: *mut sys::EVP_MD_CTX,
        sig: *mut c_uchar,
        siglen: *mut usize,
        tbs: *const c_uchar,
        tbslen: usize,
    ) -> c_int;
    pub fn EVP_DigestVerifyInit(
        ctx: *mut sys::EVP_MD_CTX,
        pctx: *mut *mut sys::EVP_PKEY_CTX,
        md: *const sys::EVP_MD,
        e: *mut c_void,
        pkey: *mut sys::EVP_PKEY,
    ) -> c_int;
    pub fn EVP_DigestVerify(
        ctx: *mut sys::EVP_MD_CTX,
        sig: *const c_uchar,
        siglen: usize,
        tbs: *const c_uchar,
        tbslen: usize,
    ) -> c_int;
    pub fn i2d_PrivateKey_bio(bio: *mut sys::BIO, pkey: *const sys::EVP_PKEY) -> c_int;
    pub fn i2d_PUBKEY_bio(bio: *mut sys::BIO, pkey: *const sys::EVP_PKEY) -> c_int;
    pub fn i2d_PKCS8PrivateKey_bio(
        bio: *mut sys::BIO,
        pkey: *const sys::EVP_PKEY,
        enc: *const sys::EVP_CIPHER,
        kstr: *const c_char,
        klen: c_int,
        cb: *mut c_void,
        u: *mut c_void,
    ) -> c_int;
    pub fn PEM_write_bio_PrivateKey_traditional(
        bio: *mut sys::BIO,
        pkey: *const sys::EVP_PKEY,
        enc: *const sys::EVP_CIPHER,
        kstr: *const c_uchar,
        klen: c_int,
        cb: *mut c_void,
        u: *mut c_void,
    ) -> c_int;
    pub fn PEM_write_bio_PKCS8PrivateKey(
        bio: *mut sys::BIO,
        pkey: *const sys::EVP_PKEY,
        enc: *const sys::EVP_CIPHER,
        kstr: *const c_char,
        klen: c_int,
        cb: *mut c_void,
        u: *mut c_void,
    ) -> c_int;
    pub fn PEM_write_bio_PUBKEY(bio: *mut sys::BIO, pkey: *const sys::EVP_PKEY) -> c_int;
    pub fn EVP_get_digestbyname(name: *const c_char) -> *const sys::EVP_MD;

    // certificates
    pub fn X509_new() -> *mut sys::X509;
    pub fn X509_free(x: *mut sys::X509);
    pub fn X509_up_ref(x: *mut sys::X509) -> c_int;
    pub fn X509_dup(x: *const sys::X509) -> *mut sys::X509;
    pub fn d2i_X509_bio(bio: *mut sys::BIO, x: *mut *mut sys::X509) -> *mut sys::X509;
    pub fn PEM_read_bio_X509(
        bio: *mut sys::BIO,
        x: *mut *mut sys::X509,
        cb: *mut c_void,
        u: *mut c_void,
    ) -> *mut sys::X509;
    pub fn i2d_X509(x: *const sys::X509, out: *mut *mut c_uchar) -> c_int;
    pub fn PEM_write_bio_X509(bio: *mut sys::BIO, x: *const sys::X509) -> c_int;
    pub fn X509_print(bio: *mut sys::BIO, x: *const sys::X509) -> c_int;
    pub fn X509_get_version(x: *const sys::X509) -> c_long;
    pub fn X509_set_version(x: *mut sys::X509, version: c_long) -> c_int;
    pub fn X509_get_serialNumber(x: *mut sys::X509) -> *mut sys::ASN1_INTEGER;
    pub fn X509_set_serialNumber(x: *mut sys::X509, serial: *mut sys::ASN1_INTEGER) -> c_int;
    pub fn X509_get_subject_name(x: *const sys::X509) -> *mut sys::X509_NAME;
    pub fn X509_get_issuer_name(x: *const sys::X509) -> *mut sys::X509_NAME;
    pub fn X509_set_subject_name(x: *mut sys::X509, name: *const sys::X509_NAME) -> c_int;
    pub fn X509_set_issuer_name(x: *mut sys::X509, name: *const sys::X509_NAME) -> c_int;
    pub fn X509_get0_notBefore(x: *const sys::X509) -> *const sys::ASN1_TIME;
    pub fn X509_get0_notAfter(x: *const sys::X509) -> *const sys::ASN1_TIME;
    pub fn X509_set1_notBefore(x: *mut sys::X509, tm: *const sys::ASN1_TIME) -> c_int;
    pub fn X509_set1_notAfter(x: *mut sys::X509, tm: *const sys::ASN1_TIME) -> c_int;
    pub fn X509_get_pubkey(x: *mut sys::X509) -> *mut sys::EVP_PKEY;
    pub fn X509_set_pubkey(x: *mut sys::X509, pkey: *mut sys::EVP_PKEY) -> c_int;
    pub fn X509_sign(x: *mut sys::X509, pkey: *mut sys::EVP_PKEY, md: *const sys::EVP_MD) -> c_int;
    pub fn X509_verify(x: *mut sys::X509, pkey: *mut sys::EVP_PKEY) -> c_int;
    pub fn X509_check_private_key(x: *const sys::X509, pkey: *const sys::EVP_PKEY) -> c_int;
    pub fn X509_cmp(a: *const sys::X509, b: *const sys::X509) -> c_int;
    pub fn X509_get0_signature(
        sig: *mut *const sys::ASN1_BIT_STRING,
        alg: *mut *const sys::X509_ALGOR,
        x: *const sys::X509,
    );
    pub fn X509_ALGOR_get0(
        obj: *mut *const sys::ASN1_OBJECT,
        ptype: *mut c_int,
        ppval: *mut *const c_void,
        alg: *const sys::X509_ALGOR,
    );
    pub fn X509_get_ext_count(x: *const sys::X509) -> c_int;
    pub fn X509_get_ext(x: *const sys::X509, loc: c_int) -> *mut sys::X509_EXTENSION;
    pub fn X509_delete_ext(x: *mut sys::X509, loc: c_int) -> *mut sys::X509_EXTENSION;
    pub fn X509_add_ext(x: *mut sys::X509, ex: *mut sys::X509_EXTENSION, loc: c_int) -> c_int;

    // names
    pub fn X509_NAME_new() -> *mut sys::X509_NAME;
    pub fn X509_NAME_free(name: *mut sys::X509_NAME);
    pub fn d2i_X509_NAME(
        a: *mut *mut sys::X509_NAME,
        pp: *mut *const c_uchar,
        length: c_long,
    ) -> *mut sys::X509_NAME;
    pub fn i2d_X509_NAME(name: *const sys::X509_NAME, out: *mut *mut c_uchar) -> c_int;
    pub fn X509_NAME_add_entry_by_txt(
        name: *mut sys::X509_NAME,
        field: *const c_char,
        kind: c_int,
        bytes: *const c_uchar,
        len: c_int,
        loc: c_int,
        set: c_int,
    ) -> c_int;
    pub fn X509_NAME_oneline(
        name: *const sys::X509_NAME,
        buf: *mut c_char,
        size: c_int,
    ) -> *mut c_char;
    pub fn X509_NAME_print_ex(
        out: *mut sys::BIO,
        name: *const sys::X509_NAME,
        indent: c_int,
        flags: c_ulong,
    ) -> c_int;
    pub fn X509_NAME_entry_count(name: *const sys::X509_NAME) -> c_int;
    pub fn X509_NAME_get_entry(
        name: *const sys::X509_NAME,
        loc: c_int,
    ) -> *mut sys::X509_NAME_ENTRY;
    pub fn X509_NAME_ENTRY_get_object(ne: *const sys::X509_NAME_ENTRY) -> *mut sys::ASN1_OBJECT;
    pub fn X509_NAME_ENTRY_get_data(ne: *const sys::X509_NAME_ENTRY) -> *mut sys::ASN1_STRING;
    pub fn X509_NAME_cmp(a: *const sys::X509_NAME, b: *const sys::X509_NAME) -> c_int;
    pub fn X509_NAME_hash_ex(
        name: *const sys::X509_NAME,
        libctx: *mut c_void,
        propq: *const c_char,
        ok: *mut c_int,
    ) -> c_ulong;
    pub fn X509_NAME_hash_old(name: *const sys::X509_NAME) -> c_ulong;

    // extensions
    pub fn d2i_X509_EXTENSION(
        a: *mut *mut sys::X509_EXTENSION,
        pp: *mut *const c_uchar,
        length: c_long,
    ) -> *mut sys::X509_EXTENSION;
    pub fn i2d_X509_EXTENSION(ext: *const sys::X509_EXTENSION, out: *mut *mut c_uchar) -> c_int;
    pub fn X509_EXTENSION_free(ext: *mut sys::X509_EXTENSION);
    pub fn X509_EXTENSION_create_by_OBJ(
        ext: *mut *mut sys::X509_EXTENSION,
        obj: *const sys::ASN1_OBJECT,
        crit: c_int,
        data: *mut sys::ASN1_OCTET_STRING,
    ) -> *mut sys::X509_EXTENSION;
    pub fn X509_EXTENSION_get_object(ext: *mut sys::X509_EXTENSION) -> *mut sys::ASN1_OBJECT;
    pub fn X509_EXTENSION_get_data(ext: *mut sys::X509_EXTENSION) -> *mut sys::ASN1_OCTET_STRING;
    pub fn X509_EXTENSION_get_critical(ext: *const sys::X509_EXTENSION) -> c_int;
    pub fn X509V3_EXT_print(
        out: *mut sys::BIO,
        ext: *mut sys::X509_EXTENSION,
        flag: c_ulong,
        indent: c_int,
    ) -> c_int;
    pub fn X509V3_set_ctx(
        ctx: *mut X509V3_CTX,
        issuer: *mut sys::X509,
        subject: *mut sys::X509,
        req: *mut c_void,
        crl: *mut c_void,
        flags: c_int,
    );
    pub fn X509V3_set_nconf(ctx: *mut X509V3_CTX, conf: *mut c_void);
    pub fn X509V3_EXT_nconf_nid(
        conf: *mut c_void,
        ctx: *mut X509V3_CTX,
        ext_nid: c_int,
        value: *const c_char,
    ) -> *mut sys::X509_EXTENSION;

    // stores
    pub fn X509_STORE_new() -> *mut sys::X509_STORE;
    pub fn X509_STORE_free(store: *mut sys::X509_STORE);
    pub fn X509_STORE_up_ref(store: *mut sys::X509_STORE) -> c_int;
    pub fn X509_STORE_add_cert(store: *mut sys::X509_STORE, x: *mut sys::X509) -> c_int;
    pub fn X509_STORE_add_lookup(
        store: *mut sys::X509_STORE,
        m: *const X509_LOOKUP_METHOD,
    ) -> *mut X509_LOOKUP;
    pub fn X509_LOOKUP_file() -> *const X509_LOOKUP_METHOD;
    pub fn X509_LOOKUP_hash_dir() -> *const X509_LOOKUP_METHOD;
    pub fn X509_LOOKUP_ctrl(
        ctx: *mut X509_LOOKUP,
        cmd: c_int,
        argc: *const c_char,
        argl: c_long,
        ret: *mut *mut c_char,
    ) -> c_int;
    pub fn X509_STORE_set_default_paths(store: *mut sys::X509_STORE) -> c_int;
    pub fn X509_STORE_set_flags(store: *mut sys::X509_STORE, flags: c_ulong) -> c_int;
    pub fn X509_STORE_set_purpose(store: *mut sys::X509_STORE, purpose: c_int) -> c_int;
    pub fn X509_STORE_set_trust(store: *mut sys::X509_STORE, trust: c_int) -> c_int;
    pub fn X509_STORE_get0_param(store: *const sys::X509_STORE) -> *mut sys::X509_VERIFY_PARAM;
    pub fn X509_VERIFY_PARAM_set_time(param: *mut sys::X509_VERIFY_PARAM, t: i64);
    pub fn X509_STORE_CTX_new() -> *mut sys::X509_STORE_CTX;
    pub fn X509_STORE_CTX_free(ctx: *mut sys::X509_STORE_CTX);
    pub fn X509_STORE_CTX_init(
        ctx: *mut sys::X509_STORE_CTX,
        store: *mut sys::X509_STORE,
        x509: *mut sys::X509,
        chain: *mut OPENSSL_STACK,
    ) -> c_int;
    pub fn X509_STORE_CTX_cleanup(ctx: *mut sys::X509_STORE_CTX);
    pub fn X509_STORE_CTX_set_verify_cb(ctx: *mut sys::X509_STORE_CTX, cb: VerifyCallback);
    pub fn X509_STORE_CTX_set_ex_data(
        ctx: *mut sys::X509_STORE_CTX,
        idx: c_int,
        data: *mut c_void,
    ) -> c_int;
    pub fn X509_STORE_CTX_get_ex_data(ctx: *const sys::X509_STORE_CTX, idx: c_int) -> *mut c_void;
    pub fn X509_STORE_CTX_get_error(ctx: *const sys::X509_STORE_CTX) -> c_int;
    pub fn X509_STORE_CTX_set_error(ctx: *mut sys::X509_STORE_CTX, s: c_int);
    pub fn X509_STORE_CTX_get_error_depth(ctx: *const sys::X509_STORE_CTX) -> c_int;
    pub fn X509_STORE_CTX_get_current_cert(ctx: *const sys::X509_STORE_CTX) -> *mut sys::X509;
    pub fn X509_STORE_CTX_get0_chain(ctx: *const sys::X509_STORE_CTX) -> *mut OPENSSL_STACK;
    pub fn X509_STORE_CTX_set_flags(ctx: *mut sys::X509_STORE_CTX, flags: c_ulong);
    pub fn X509_STORE_CTX_set_purpose(ctx: *mut sys::X509_STORE_CTX, purpose: c_int) -> c_int;
    pub fn X509_STORE_CTX_set_trust(ctx: *mut sys::X509_STORE_CTX, trust: c_int) -> c_int;
    pub fn X509_STORE_CTX_set_time(ctx: *mut sys::X509_STORE_CTX, flags: c_ulong, t: i64);
    pub fn X509_verify_cert(ctx: *mut sys::X509_STORE_CTX) -> c_int;
    pub fn X509_verify_cert_error_string(n: c_long) -> *const c_char;
    pub fn X509_get_default_cert_file() -> *const c_char;
    pub fn X509_get_default_cert_dir() -> *const c_char;
    pub fn X509_get_default_cert_file_env() -> *const c_char;
    pub fn X509_get_default_cert_dir_env() -> *const c_char;
    pub fn X509_get_default_cert_area() -> *const c_char;
    pub fn X509_get_default_private_dir() -> *const c_char;

    // TLS
    pub fn TLS_method() -> *const sys::SSL_METHOD;
    pub fn SSL_CTX_new(method: *const sys::SSL_METHOD) -> *mut sys::SSL_CTX;
    pub fn SSL_CTX_free(ctx: *mut sys::SSL_CTX);
    pub fn SSL_CTX_ctrl(
        ctx: *mut sys::SSL_CTX,
        cmd: c_int,
        larg: c_long,
        parg: *mut c_void,
    ) -> c_long;
    pub fn SSL_CTX_get_options(ctx: *const sys::SSL_CTX) -> u64;
    pub fn SSL_CTX_set_options(ctx: *mut sys::SSL_CTX, op: u64) -> u64;
    pub fn SSL_CTX_clear_options(ctx: *mut sys::SSL_CTX, op: u64) -> u64;
    pub fn SSL_CTX_set_cipher_list(ctx: *mut sys::SSL_CTX, s: *const c_char) -> c_int;
    pub fn SSL_CTX_set_ciphersuites(ctx: *mut sys::SSL_CTX, s: *const c_char) -> c_int;
    pub fn SSL_CTX_get_ciphers(ctx: *const sys::SSL_CTX) -> *mut OPENSSL_STACK;
    pub fn SSL_CTX_get_security_level(ctx: *const sys::SSL_CTX) -> c_int;
    pub fn SSL_CTX_set_security_level(ctx: *mut sys::SSL_CTX, level: c_int);
    pub fn SSL_CTX_set_cert_store(ctx: *mut sys::SSL_CTX, store: *mut sys::X509_STORE);
    pub fn SSL_CTX_use_certificate(ctx: *mut sys::SSL_CTX, x: *mut sys::X509) -> c_int;
    pub fn SSL_CTX_use_PrivateKey(ctx: *mut sys::SSL_CTX, pkey: *mut sys::EVP_PKEY) -> c_int;
    pub fn SSL_CTX_check_private_key(ctx: *const sys::SSL_CTX) -> c_int;
    pub fn SSL_CTX_add_client_CA(ctx: *mut sys::SSL_CTX, x: *mut sys::X509) -> c_int;
    pub fn SSL_CTX_load_verify_file(ctx: *mut sys::SSL_CTX, file: *const c_char) -> c_int;
    pub fn SSL_CTX_load_verify_dir(ctx: *mut sys::SSL_CTX, dir: *const c_char) -> c_int;
    pub fn SSL_CTX_set_verify(ctx: *mut sys::SSL_CTX, mode: c_int, cb: Option<VerifyCallback>);
    pub fn SSL_CTX_set_timeout(ctx: *mut sys::SSL_CTX, t: c_long) -> c_long;
    pub fn SSL_CTX_get_timeout(ctx: *const sys::SSL_CTX) -> c_long;
    pub fn SSL_CTX_set_verify_depth(ctx: *mut sys::SSL_CTX, depth: c_int);
    pub fn SSL_CTX_set_alpn_protos(
        ctx: *mut sys::SSL_CTX,
        protos: *const c_uchar,
        len: c_uint,
    ) -> c_int;
    pub fn SSL_CTX_set_session_id_context(
        ctx: *mut sys::SSL_CTX,
        sid: *const c_uchar,
        len: c_uint,
    ) -> c_int;
    pub fn SSL_CTX_sess_set_new_cb(ctx: *mut sys::SSL_CTX, cb: Option<NewSessionCallback>);
    pub fn SSL_CTX_set_keylog_callback(ctx: *mut sys::SSL_CTX, cb: Option<KeylogCallback>);
    pub fn SSL_CTX_add_session(ctx: *mut sys::SSL_CTX, s: *mut sys::SSL_SESSION) -> c_int;
    pub fn SSL_CTX_remove_session(ctx: *mut sys::SSL_CTX, s: *mut sys::SSL_SESSION) -> c_int;
    pub fn SSL_CTX_flush_sessions(ctx: *mut sys::SSL_CTX, t: c_long);
    pub fn SSL_CTX_set_client_hello_cb(
        ctx: *mut sys::SSL_CTX,
        cb: Option<ClientHelloCallback>,
        arg: *mut c_void,
    );
    pub fn SSL_CTX_set_alpn_select_cb(
        ctx: *mut sys::SSL_CTX,
        cb: Option<AlpnSelectCallback>,
        arg: *mut c_void,
    );
    pub fn SSL_CTX_set_client_cert_cb(ctx: *mut sys::SSL_CTX, cb: Option<ClientCertCallback>);
    pub fn SSL_client_hello_get0_ext(
        s: *mut sys::SSL,
        kind: c_uint,
        out: *mut *const c_uchar,
        outlen: *mut usize,
    ) -> c_int;
    pub fn SSL_set_SSL_CTX(ssl: *mut sys::SSL, ctx: *mut sys::SSL_CTX) -> *mut sys::SSL_CTX;
    pub fn SSL_new(ctx: *mut sys::SSL_CTX) -> *mut sys::SSL;
    pub fn SSL_free(ssl: *mut sys::SSL);
    pub fn SSL_ctrl(ssl: *mut sys::SSL, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    pub fn SSL_set_fd(ssl: *mut sys::SSL, fd: c_int) -> c_int;
    pub fn SSL_get_rbio(ssl: *const sys::SSL) -> *mut sys::BIO;
    pub fn SSL_set_ex_data(ssl: *mut sys::SSL, idx: c_int, data: *mut c_void) -> c_int;
    pub fn SSL_get_ex_data(ssl: *const sys::SSL, idx: c_int) -> *mut c_void;
    pub fn SSL_get_ex_data_X509_STORE_CTX_idx() -> c_int;
    pub fn SSL_is_server(ssl: *const sys::SSL) -> c_int;
    pub fn SSL_connect(ssl: *mut sys::SSL) -> c_int;
    pub fn SSL_accept(ssl: *mut sys::SSL) -> c_int;
    pub fn SSL_read(ssl: *mut sys::SSL, buf: *mut c_void, num: c_int) -> c_int;
    pub fn SSL_write(ssl: *mut sys::SSL, buf: *const c_void, num: c_int) -> c_int;
    pub fn SSL_shutdown(ssl: *mut sys::SSL) -> c_int;
    pub fn SSL_get_error(ssl: *const sys::SSL, ret: c_int) -> c_int;
    pub fn SSL_pending(ssl: *const sys::SSL) -> c_int;
    pub fn SSL_get_version(ssl: *const sys::SSL) -> *const c_char;
    pub fn SSL_state_string(ssl: *const sys::SSL) -> *const c_char;
    pub fn SSL_state_string_long(ssl: *const sys::SSL) -> *const c_char;
    pub fn SSL_get_current_cipher(ssl: *const sys::SSL) -> *const sys::SSL_CIPHER;
    pub fn SSL_CIPHER_get_name(c: *const sys::SSL_CIPHER) -> *const c_char;
    pub fn SSL_CIPHER_get_version(c: *const sys::SSL_CIPHER) -> *const c_char;
    pub fn SSL_CIPHER_get_bits(c: *const sys::SSL_CIPHER, alg_bits: *mut c_int) -> c_int;
    pub fn SSL_session_reused(ssl: *const sys::SSL) -> c_int;
    pub fn SSL_get_verify_result(ssl: *const sys::SSL) -> c_long;
    pub fn SSL_get1_peer_certificate(ssl: *const sys::SSL) -> *mut sys::X509;
    pub fn SSL_get_peer_cert_chain(ssl: *const sys::SSL) -> *mut OPENSSL_STACK;
    pub fn SSL_get_certificate(ssl: *const sys::SSL) -> *mut sys::X509;
    pub fn SSL_get0_alpn_selected(
        ssl: *const sys::SSL,
        data: *mut *const c_uchar,
        len: *mut c_uint,
    );
    pub fn SSL_get_finished(ssl: *const sys::SSL, buf: *mut c_void, count: usize) -> usize;
    pub fn SSL_get_peer_finished(ssl: *const sys::SSL, buf: *mut c_void, count: usize) -> usize;
    pub fn SSL_get1_session(ssl: *mut sys::SSL) -> *mut sys::SSL_SESSION;
    pub fn SSL_set_session(ssl: *mut sys::SSL, session: *mut sys::SSL_SESSION) -> c_int;
    pub fn SSL_export_keying_material(
        ssl: *mut sys::SSL,
        out: *mut c_uchar,
        olen: usize,
        label: *const c_char,
        llen: usize,
        context: *const c_uchar,
        contextlen: usize,
        use_context: c_int,
    ) -> c_int;
    pub fn SSL_SESSION_free(s: *mut sys::SSL_SESSION);
    pub fn SSL_SESSION_get_time(s: *const sys::SSL_SESSION) -> c_long;
    pub fn SSL_SESSION_set_time(s: *mut sys::SSL_SESSION, t: c_long) -> c_long;
    pub fn SSL_SESSION_get_timeout(s: *const sys::SSL_SESSION) -> c_long;
    pub fn SSL_SESSION_set_timeout(s: *mut sys::SSL_SESSION, t: c_long) -> c_long;
    pub fn SSL_SESSION_get_id(s: *const sys::SSL_SESSION, len: *mut c_uint) -> *const c_uchar;
    pub fn SSL_SESSION_set1_id(s: *mut sys::SSL_SESSION, sid: *const c_uchar, len: c_uint)
    -> c_int;
    pub fn SSL_SESSION_get_protocol_version(s: *const sys::SSL_SESSION) -> c_int;
    pub fn i2d_SSL_SESSION(s: *const sys::SSL_SESSION, pp: *mut *mut c_uchar) -> c_int;
    pub fn d2i_SSL_SESSION(
        a: *mut *mut sys::SSL_SESSION,
        pp: *mut *const c_uchar,
        length: c_long,
    ) -> *mut sys::SSL_SESSION;
    pub fn PEM_read_bio_SSL_SESSION(
        bio: *mut sys::BIO,
        x: *mut *mut sys::SSL_SESSION,
        cb: *mut c_void,
        u: *mut c_void,
    ) -> *mut sys::SSL_SESSION;
    pub fn PEM_write_bio_SSL_SESSION(bio: *mut sys::BIO, x: *const sys::SSL_SESSION) -> c_int;
    pub fn SSL_SESSION_print(bio: *mut sys::BIO, x: *const sys::SSL_SESSION) -> c_int;
}

/// `struct tm`, as `ASN1_TIME_to_tm` fills it in. glibc and the BSDs add a
/// UTC offset and a zone name after the nine ints; room is kept for both.
#[repr(C)]
#[derive(Default)]
pub struct Tm {
    pub tm_sec: c_int,
    pub tm_min: c_int,
    pub tm_hour: c_int,
    pub tm_mday: c_int,
    pub tm_mon: c_int,
    pub tm_year: c_int,
    pub tm_wday: c_int,
    pub tm_yday: c_int,
    pub tm_isdst: c_int,
    pub tm_gmtoff: c_long,
    pub tm_zone: usize,
}

// `BIO_ctrl` commands.
pub const BIO_CTRL_RESET: c_int = 1;
pub const BIO_CTRL_EOF: c_int = 2;
pub const BIO_CTRL_INFO: c_int = 3;
pub const BIO_C_FILE_TELL: c_int = 133;

// `SSL_ctrl` and `SSL_CTX_ctrl` commands, the numbers OpenSSL 3's
// `ssl.h` gives the macros over them.
pub const SSL_CTRL_EXTRA_CHAIN_CERT: c_int = 14;
pub const SSL_CTRL_MODE: c_int = 33;
pub const SSL_CTRL_SET_SESS_CACHE_SIZE: c_int = 42;
pub const SSL_CTRL_GET_SESS_CACHE_SIZE: c_int = 43;
pub const SSL_CTRL_SET_SESS_CACHE_MODE: c_int = 44;
pub const SSL_CTRL_GET_SESS_CACHE_MODE: c_int = 45;
pub const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
pub const SSL_CTRL_CHAIN: c_int = 88;
pub const SSL_CTRL_SET_GROUPS_LIST: c_int = 92;
pub const SSL_CTRL_SET_SIGALGS_LIST: c_int = 98;
pub const SSL_CTRL_SET_CLIENT_SIGALGS_LIST: c_int = 102;
pub const SSL_CTRL_GET_PEER_TMP_KEY: c_int = 109;
pub const SSL_CTRL_SET_DH_AUTO: c_int = 118;
pub const SSL_CTRL_SET_MIN_PROTO_VERSION: c_int = 123;
pub const SSL_CTRL_SET_MAX_PROTO_VERSION: c_int = 124;
/// `SSL_set_retry_verify`'s command (OpenSSL 3.0).
pub const SSL_CTRL_SET_RETRY_VERIFY: c_int = 136;

// `SSL_CTX_set_mode`'s modes CRuby sets on every context.
pub const SSL_MODE_ENABLE_PARTIAL_WRITE: c_long = 0x1;
pub const SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER: c_long = 0x2;
pub const SSL_MODE_RELEASE_BUFFERS: c_long = 0x10;

// `SSL_get_error`'s answers.
pub const SSL_ERROR_NONE: c_int = 0;
pub const SSL_ERROR_WANT_READ: c_int = 2;
pub const SSL_ERROR_WANT_WRITE: c_int = 3;
pub const SSL_ERROR_SYSCALL: c_int = 5;
pub const SSL_ERROR_ZERO_RETURN: c_int = 6;
pub const SSL_ERROR_WANT_X509_LOOKUP: c_int = 4;
pub const SSL_ERROR_WANT_CLIENT_HELLO_CB: c_int = 11;
pub const SSL_ERROR_WANT_RETRY_VERIFY: c_int = 12;

// The client hello callback's answers, and the TLS alert it sends.
pub const SSL_CLIENT_HELLO_SUCCESS: c_int = 1;
pub const SSL_CLIENT_HELLO_ERROR: c_int = 0;
pub const SSL_CLIENT_HELLO_RETRY: c_int = -1;
pub const SSL_AD_INTERNAL_ERROR: c_int = 80;
pub const SSL_TLSEXT_ERR_OK: c_int = 0;
pub const SSL_TLSEXT_ERR_ALERT_FATAL: c_int = 2;
pub const SSL_TLSEXT_ERR_NOACK: c_int = 3;
pub const TLSEXT_TYPE_SERVER_NAME: c_uint = 0;
pub const TLSEXT_TYPE_ALPN: c_uint = 16;

pub const ERR_LIB_SSL: c_ulong = 20;
pub const SSL_R_CERTIFICATE_VERIFY_FAILED: c_ulong = 134;

pub const X509_FILETYPE_PEM: c_long = 1;
pub const X509_L_FILE_LOAD: c_int = 1;
pub const X509_L_ADD_DIR: c_int = 2;

pub const EVP_PKEY_RSA: c_int = 6;
/// `EVP_PKEY_KEYPAIR`, the selection a private key's decoder asks for.
pub const EVP_PKEY_KEYPAIR: c_int = 0x87;

pub const TLSEXT_NAMETYPE_HOST_NAME: c_long = 0;
