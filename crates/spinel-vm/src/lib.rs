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

/// The date of this release, reported as `RUBY_RELEASE_DATE`.
///
/// Spinel's, not CRuby's: the day [`ENGINE_VERSION`] was cut. Set it together
/// with the workspace version.
pub const RELEASE_DATE: &str = "2026-09-03";

/// Reported as `RUBY_COPYRIGHT`, shaped like CRuby's
/// `ruby - Copyright (C) 1993-2025 Yukihiro Matsumoto`. The holder is `LICENSE`'s.
pub const COPYRIGHT: &str = "spinel - Copyright (C) 2026 Angel Ramirez";

/// The platform this build is for, as CRuby and RubyGems spell it:
/// `x86_64-linux`, `aarch64-linux`, `arm64-darwin24`. Reported as `RUBY_PLATFORM`,
/// and through `RbConfig::CONFIG["arch"]` it is what `Gem::Platform.local` parses.
/// Decided at build time, as CRuby decides it; `build.rs` says how.
pub const PLATFORM: &str = env!("SPINEL_RUBY_PLATFORM");

/// The one-line banner printed by `spinel --version`, and `RUBY_DESCRIPTION`.
///
/// Shaped like `ruby -v` so a Ruby developer can read it without being told how:
/// `spinel 0.0.1 (ruby 4.0.0) [arm64-darwin24]`.
pub fn description() -> String {
    format!("{ENGINE} {ENGINE_VERSION} (ruby {LANGUAGE_VERSION}) [{PLATFORM}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_names_engine_language_and_platform() {
        let d = description();
        assert!(d.starts_with("spinel 0."), "{d}");
        assert!(d.contains("(ruby 4.0.0)"), "{d}");
        assert!(d.ends_with(&format!("[{PLATFORM}]")), "{d}");
    }

    #[test]
    fn platform_names_cpu_and_os() {
        let (cpu, os) = PLATFORM.split_once('-').expect("cpu-os");
        assert!(!cpu.is_empty() && !os.is_empty(), "{PLATFORM}");
        assert!(
            !PLATFORM.contains("macos"),
            "RubyGems says darwin: {PLATFORM}"
        );
    }

    #[test]
    fn release_date_is_a_date() {
        let parts: Vec<&str> = RELEASE_DATE.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [4, 2, 2]);
        assert!(parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())));
    }
}

pub mod bignum;
pub mod bytecode;
pub mod callcache;
pub mod class;
pub mod compile;
pub mod cp5022x;
pub mod encoding_table;
pub mod errno;
pub mod etc;
pub mod euckr;
pub mod euckr_table;
pub mod float;
pub mod heap;
pub mod interp;
pub mod iso2022jp;
pub mod jis;
pub mod jis_table;
pub mod json;
pub mod method;
pub mod mt;
pub mod regexp;
pub mod sbc;
pub mod shape;
pub mod shared;
pub mod signal;
pub mod socket;
pub mod strings;
pub mod transcode;
pub mod value;
pub mod zlib;
pub use bytecode::{BinOp, Insn, Iseq, Literal};
pub use class::{Builtin, ClassId, Classes, CrefId, Kind, Method, Mixin, MixinError};
pub use compile::Unsupported;
pub use heap::{Handle, HandleScope, Heap, ParseFailure, ParsedSource, Parser, Payload, Stats};
pub use interp::{Error, Frame};
pub use method::{
    BindingOp, BitOp, Definition, Definitions, FiberOp, FsOp, IvarOp, Native, ReflectOp, StrOp,
    SysOp,
};
pub use regexp::Regexps;
pub use shape::{ShapeId, Shapes};
pub use value::{SymbolId, Unpacked, Value};
