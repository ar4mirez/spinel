//! Cranelift backend: bytecode in, machine code out.
//!
//! Lands in phase 6. The interpreter is the reference; the JIT must agree with it
//! on every ruby/spec file. See `docs/engine.md`.
