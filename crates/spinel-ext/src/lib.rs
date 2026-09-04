//! Public, versioned API for native extensions.
//!
//! Standalone on purpose: extension gems depend on this crate and nothing else of
//! Spinel's internals. There is no CRuby C API and no `extconf.rb`.
//! Lands in phase 5.
