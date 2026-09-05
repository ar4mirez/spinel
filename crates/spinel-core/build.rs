//! Tell Cargo that `core/*.rb` is a source file.
//!
//! `lib.rs` embeds every one of them with `include_str!`, and Cargo does not
//! track those paths on its own: editing `core/hash.rb` rebuilt `spinel-cli`
//! and left *this* crate — the one holding the text — untouched, so the binary
//! kept running the previous core library. A stale core library that still
//! builds and still runs is the worst shape a build bug can take, and it cost
//! two debugging passes in #151 before it was believed.
//!
//! Watching the directory rather than each file also catches a new `core/*.rb`
//! that `SOURCES` has just learned about.

fn main() {
    println!("cargo:rerun-if-changed=../../core");
}
