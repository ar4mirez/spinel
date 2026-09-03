# Spinel: notes for AI coding sessions

Read `README.md`, then `docs/engine.md` if touching the VM, or the subsystem doc otherwise. `docs/roadmap.md` lists work as small verifiable slices. Do one slice per session.

## Fixed decisions (settled with the owner; do not relitigate in a session)

- Engine from scratch in Rust. Never link, vendor, or call into CRuby, mruby, or any other Ruby VM. Reusing Ruby's pure-Ruby stdlib files and ruby/spec is fine; they are Ruby code, not an engine.
- Full Ruby compatibility is the target. Behavior questions are answered by ruby/spec, then by what CRuby does, in that order. Never by what is convenient.
- Prism is the parser, but nothing outside `crates/spinel-parse` may see a Prism node. Everything consumes `spinel_ast`.
- Native extensions use the `spinel-ext` Rust API or FFI. There is no CRuby C API, and no `extconf.rb`.
- Core classes live in `core/*.rb`. Rust primitives exist only where Ruby cannot express the operation (raw bytes, allocation, syscalls, dispatch). If a method can be written in Ruby, it is.
- One `Heap` per Ractor. There is no process-global mutable VM state. `static mut`, `lazy_static` with interior mutability, and thread-locals holding VM objects are forbidden. The only exceptions live in `crates/spinel-vm/src/shared/`: immutable append-only tables (symbols, frozen literals) and shared class objects whose tables are writable only by the main Ractor under the class lock.
- Compat contract for tooling: Gemfile, Gemfile.lock, and the `GEM_HOME` on-disk layout stay interoperable with Bundler and RubyGems.

## Conventions

- Definition of done for engine work is a ruby/spec delta: name the spec files or directories that newly pass. `scripts/spec.sh core/array` runs one directory. Never mark a spec as "expected failure" to make a slice green; skip it with a reason in `spec/tags/`.
- Definition of done for tooling work is an integration test under `tests/` shelling out to the built `spinel` binary against a fixture in `tests/fixtures/`.
- Prefer boring: `clap`, `reqwest` + `tokio`, `pubgrub`, `serde`, `notify`, `cranelift` (later).
- Mark deliberate shortcuts with a `// ponytail:` comment naming the ceiling and the upgrade path.
- Benchmarks live in `bench/` and compare against system `ruby --yjit`. Numbers go in the PR, not in code.
- When a slice reveals a doc is wrong, fix the doc in the same PR.

## Build

```sh
cargo build --release          # target/release/spinel
cargo test                     # Rust unit tests + tooling integration tests
scripts/spec.sh [dir]          # ruby/spec: spec/harness before phase 2, mspec on Spinel after (submodule: spec/ruby)
scripts/bench.sh               # yjit-bench subset vs system ruby
```
