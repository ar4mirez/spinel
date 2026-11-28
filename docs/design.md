# Design: why Spinel is built from scratch

The settled decisions and the reasoning behind them. `CLAUDE.md` holds the same decisions as rules for working sessions, with the why-nots; this page is the short version for readers.

## Decisions

| Decision | Choice |
|---|---|
| Engine | Written from scratch in Rust. No CRuby, mruby or other Ruby VM code is linked, vendored or called. Ruby's pure-Ruby standard library and ruby/spec are reused: they are Ruby code, not an engine. |
| Compatibility | Full Ruby, reached incrementally. [ruby/spec](https://github.com/ruby/spec) is the definition of done; where it is silent, CRuby's behavior decides. |
| Parser | [Prism](https://github.com/ruby/prism), lowered into Spinel's own AST (`spinel_ast`) inside `crates/spinel-parse`, so nothing else sees a Prism node and a hand-written parser could replace it later. |
| Native extensions | A Rust extension API (`spinel-ext`, with a versioned ABI) plus an FFI (`Spinel::FFI`, under `fiddle`). There is no CRuby C API: gems such as `pg` get a Spinel-native implementation behind the same Ruby API. |
| Core classes | Written in Ruby (`core/*.rb`) on a small set of Rust primitives, used only where Ruby cannot express the operation (raw bytes, allocation, syscalls, dispatch). |
| Concurrency | Ractor-native: one heap per Ractor, with a lock per heap that its threads take turns under. No process-global mutable VM state. |
| Language version | One Ruby language version at a time, the latest stable: 4.0 today. `RUBY_ENGINE` is `"spinel"`. See [cli.md](cli.md), "How Spinel identifies itself". |
| Tooling compatibility | `Gemfile`, `Gemfile.lock` and the `GEM_HOME` layout stay interoperable with Bundler and RubyGems. |

## Why from scratch

CRuby carries thirty years of decisions that a new engine does not have to: a C API that freezes the object layout, thousands of process-wide globals that made Ractors a decade-long retrofit, a conservative GC that cannot move objects, and core classes in C that only a few people can change.

Spinel starts with shapes for instance variables, a precise GC per heap, a Ractor-native heap design, and a core library in Ruby that anyone can read and improve. The GC is non-moving mark-sweep today; a generational, moving GC is planned ([#126](https://github.com/ar4mirez/spinel/issues/126)), and nothing in the design stops it from moving objects.

The bet is that this design, plus a JIT built on Cranelift, ends up faster than CRuby with YJIT on real programs while running the same code. That is a goal, not a result: the JIT has only started and makes nothing faster yet ([phase 6](roadmap.md#phase-6-jit-and-gc)), and the interpreter's numbers against CRuby are in [`bench/README.md`](../bench/README.md).

## Non-goals

- Running CRuby C extensions unmodified.
- Windows in the first releases ([#139](https://github.com/ar4mirez/spinel/issues/139)). Linux (x86_64, aarch64) and macOS (x86_64, arm64) come first.
- Rails as an early constraint. Rails is the phase 7 milestone ([roadmap](roadmap.md#phase-7-rails-and-reach)).
