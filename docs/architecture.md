# Architecture

## Shape

```
┌────────────────────────────────── spinel (one static binary) ──────────────────────────────────┐
│  CLI (clap): run · x · init · install · add · remove · update · test · build · parse · spec      │
│                                                                                                │
│  ┌── tooling (spinel-cli) ───────────────────────────────────────────────────────────────────┐ │
│  │ gems: index · resolver · lockfile · store · ext loader   test: pool · report   build: pack │ │
│  └───────────────────────────────────────────────────────────────────────────────────────────┘ │
│  ┌── engine ─────────────────────────────────────────────────────────────────────────────────┐ │
│  │ spinel-parse   Prism → spinel_ast                                                          │ │
│  │ spinel-ast     Spinel's own tree (the only AST anyone sees)                                │ │
│  │ spinel-vm      values · heap · GC · shapes · bytecode compiler · interpreter · Ractors      │ │
│  │ spinel-core    Rust primitives + core/*.rb (String, Array, Hash, ...) precompiled at build  │ │
│  │ spinel-jit     Cranelift backend (phase 6)                                                 │ │
│  │ spinel-ext     public API for native extensions (separate crate, versioned ABI)            │ │
│  └───────────────────────────────────────────────────────────────────────────────────────────┘ │
│  embedded assets: core bytecode image · vendored pure-Ruby stdlib (zstd) · Ruby shims          │
└────────────────────────────────────────────────────────────────────────────────────────────────┘
                     │ writes                                       │ reads
                     ▼                                              ▼
   ~/.spinel/                                        project/
     stdlib/<spinel-ver>/          extracted once      Gemfile, Gemfile.lock      (Bundler-compatible)
     store/gems/<name>-<ver>/                          .spinel/gem_home/          (standard RubyGems layout)
     store/ext/<name>-<ver>-<abi>/                     .spinel/bytecode/          (compiled .rb cache)
     index/<host>/
```

## Crates

A Cargo workspace. More than one crate because the engine and the tooling have different compile profiles and because `spinel-ext` must be a standalone public crate.

| crate | contents | depends on |
|---|---|---|
| `spinel-ast` | AST types, spans, and the Prism coverage table. No parser. | nothing |
| `spinel-parse` | `ruby-prism` in, `spinel_ast` out. The only place Prism is imported. The lowering matches Prism's node enum exhaustively, so a new upstream node kind is a build failure here rather than a run-time surprise. | ast |
| `spinel-vm` | `Value`, `Heap`, GC, shapes, symbols, bytecode, compiler, interpreter, frames, exceptions, fibers, Ractors | ast |
| `spinel-core` | Rust primitives + `core/*.rb`, plus a `build.rs` that compiles `core/*.rb` to a bytecode image (so `spinel-vm` is also a build-dependency; the image is little-endian and target-independent) | vm |
| `spinel-ext` | Handle-based API for extensions: `Value`, `Heap` access, method definition macros, ABI version | vm |
| `spinel-jit` | Cranelift lowering from bytecode (phase 6) | vm |
| `spinel-cli` | the binary: subcommands, package manager, test runner, build packer | all |

Rule from `CLAUDE.md` restated: nothing outside `spinel-parse` sees Prism. That is what keeps "write our own parser later" a contained project.

`spinel parse <dir>` sweeps a corpus through that lowering and fails only on a
node it does not handle, never on invalid Ruby. The `sweep` CI job runs it
against ruby/spec and ruby/ruby's `lib/`, which is how the boundary is kept
honest as Ruby grows syntax.

## Binary layout

The release binary is Rust code plus an embedded zstd archive:

- `core.image`: the precompiled bytecode of `core/*.rb`, loaded at boot instead of parsing (this is the startup trick; there is no snapshot of a heap, only of bytecode)
- `stdlib/**/*.rb`: the pure-Ruby standard library, vendored from Ruby's repository (Ruby license / BSD-2, per-file terms in `stdlib/LICENSE/LEGAL`). `erb`, `optparse`, `fileutils`, `json`'s Ruby half, `net/http`, and so on. `set` is not among them: it is a core class as of Ruby 4.0. Extracted once to `~/.spinel/stdlib/<ver>/`.
- `shims/*.rb`: Spinel's setup, test worker, Gemfile evaluator (cli.md)

Default gems whose parts are C in CRuby (`json` parser, `openssl`, `zlib`, `socket`, `io-console`, `digest`, `date`, `bigdecimal`, `psych`, `etc`, `fiddle`, `monitor`) ship as built-in gems: their Ruby halves in the asset archive, their C halves as Spinel primitives. See engine.md, "Built-in gems".

Target size: under 30 MB. No libruby, no OpenSSL unless we link it for the `openssl` extension.

## Dependencies

| crate | used for |
|---|---|
| `ruby-prism` | parsing |
| `cranelift-codegen`, `cranelift-frontend` | JIT (phase 6) |
| `corosensei` | stackful coroutines for fibers when a primitive must re-enter the interpreter |
| `libffi` | `Spinel::FFI` |
| `pubgrub`, `reqwest`, `tokio`, `serde`, `notify`, `clap`, `zstd` | tooling |
| Onigmo (direct C binding, not the `onig` crate, which wraps Oniguruma) or a Rust engine | Ruby regex semantics (see engine.md, open questions) |

## Repo layout

```
Cargo.toml                 workspace
crates/                    one dir per crate above
core/                      Ruby source for core classes, compiled into core.image
stdlib/                    vendored pure-Ruby stdlib: ruby/ruby `lib/` at a pinned tag, flattened so stdlib/ is a $LOAD_PATH root, with UPSTREAM and its LICENSE/ files
shims/                     setup.rb, test_worker.rb, gemfile_eval.rb
spec/
  ruby/                    git submodule: ruby/spec
  tags/                    skipped specs with reasons
  harness/                 minimal runner used before mspec itself runs on Spinel (a workspace member, and the only crate outside crates/: it ships to nobody and is deleted in phase 2)
scripts/                   spec.sh, bench.sh, release.sh, vendor-stdlib.sh
crates/spinel-cli/tests/   tooling integration tests + fixtures/
bench/                     yjit-bench subset, spec-status.md (CI-generated)
docs/
```

Tooling integration tests live inside `spinel-cli` rather than a top-level `tests/`
package. Cargo only sets `CARGO_BIN_EXE_spinel`, and only guarantees the binary is
rebuilt before the test runs, for tests in the binary's own package; a separate
package has no dependency edge to `spinel-cli`, so `cargo test` will happily run the
suite against a stale binary from an earlier build. `spinel-cli` is where the tooling
lives anyway, so this is also where its tests belong.
