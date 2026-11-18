//! An extension whose header says it was built against ABI 0, which no
//! `spinel` loads: the ABI before the first API. Its init aborts the process,
//! so a test that survives loading it proves the init was never called.

use spinel_ext::abi::{Header, RawContext, Status};

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static spinel_ext_header: Header = Header {
    abi: 0,
    ..Header::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
};

/// # Safety
///
/// Never safe to call: a `spinel` that reads the header never does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spinel_ext_init(_cx: *mut RawContext) -> Status {
    std::process::abort()
}
