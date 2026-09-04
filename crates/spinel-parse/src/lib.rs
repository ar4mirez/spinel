//! Ruby source in, [`spinel_ast`] out.
//!
//! The only crate permitted to import Prism. Nothing here may leak a Prism type
//! through its public API. See `docs/architecture.md`.
//!
//! The Prism dependency and the lowering land in issue #3.
