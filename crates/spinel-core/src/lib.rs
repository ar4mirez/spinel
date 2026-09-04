//! The Rust primitives underneath Ruby's core classes.
//!
//! Core classes are written in Ruby in `core/*.rb`. Code lives here only when Ruby
//! cannot express the operation: raw bytes, allocation, syscalls, dispatch.
//! A `build.rs` will compile `core/*.rb` into the bytecode image. See `docs/engine.md`.
