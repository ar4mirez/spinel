//! Spinel's own Ruby syntax tree.
//!
//! Every crate above the parser consumes this tree and never a Prism node, so a
//! hand-written parser can replace Prism later without touching anything else.
//! See `docs/architecture.md`.
//!
//! Node types land in issue #2.
