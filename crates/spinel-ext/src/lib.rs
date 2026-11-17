//! Public, versioned API for native extensions.
//!
//! Standalone on purpose: extension gems depend on this crate and nothing else of
//! Spinel's internals. There is no CRuby C API and no `extconf.rb`.
//! Lands in phase 5.

/// The extension ABI a `spinel` loads, recorded in every compiled app's
/// `manifest.json` (docs/build.md) so a payload's `.spinel` extensions can be
/// checked against the binary that runs them. 0 until the API lands in
/// phase 5: nothing is built against it yet, and the first API is 1.
pub const ABI: u32 = 0;
