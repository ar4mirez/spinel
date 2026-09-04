//! Values, heap, GC, shapes, bytecode, interpreter, Ractors.
//!
//! One [`Heap`] per Ractor and no process-global mutable VM state. See
//! `docs/engine.md`; the exceptions live in `src/shared/`.
//!
//! [`Heap`]: https://github.com/ar4mirez/spinel/issues/6

/// Spinel's own version, reported as `RUBY_ENGINE_VERSION`.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What Ruby code sees in `RUBY_ENGINE`.
pub const ENGINE: &str = "spinel";

/// The Ruby *language* version this build implements, reported as `RUBY_VERSION`.
///
/// Spinel targets one language version at a time. ruby/spec's `ruby_version_is`
/// guards and gems' version checks read this. See `docs/cli.md`.
pub const LANGUAGE_VERSION: &str = "4.0.0";

/// The host platform, as RubyGems spells it. Reported as `RUBY_PLATFORM`.
///
// ponytail: RubyGems writes an OS version suffix on darwin ("arm64-darwin24") and
// uses the raw arch on linux ("aarch64-linux"). Getting that exactly right needs
// host-triple detection, which arrives with the real `RUBY_PLATFORM` in phase 3.
// This is good enough for a version banner and wrong for nothing else yet.
pub fn platform() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" if cfg!(target_os = "macos") => "arm64",
        arch => arch,
    };
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    };
    format!("{arch}-{os}")
}

/// The one-line banner printed by `spinel --version`, and later `RUBY_DESCRIPTION`.
///
/// Shaped like `ruby -v` so a Ruby developer can read it without being told how:
/// `spinel 0.0.1 (ruby 4.0.0) [arm64-darwin]`.
pub fn description() -> String {
    format!(
        "{ENGINE} {ENGINE_VERSION} (ruby {LANGUAGE_VERSION}) [{}]",
        platform()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_names_engine_language_and_platform() {
        let d = description();
        assert!(d.starts_with("spinel 0."), "{d}");
        assert!(d.contains("(ruby 4.0.0)"), "{d}");
        assert!(d.ends_with(&format!("[{}]", platform())), "{d}");
    }

    #[test]
    fn platform_has_both_halves() {
        let p = platform();
        assert_eq!(p.split('-').count(), 2, "{p}");
        assert!(!p.starts_with('-') && !p.ends_with('-'), "{p}");
    }
}

pub mod bytecode;
pub mod class;
pub mod compile;
pub mod heap;
pub mod interp;
pub mod method;
pub mod shared;
pub mod value;
pub use bytecode::{BinOp, Insn, Iseq, Literal};
pub use class::{Builtin, ClassId, Classes, Kind, Method, Mixin, MixinError};
pub use compile::Unsupported;
pub use heap::{Handle, HandleScope, Heap, Payload, Stats};
pub use interp::{Error, Frame};
pub use method::{Definition, Definitions, Native};
pub use value::{SymbolId, Unpacked, Value};
