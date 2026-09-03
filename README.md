# Spinel

**A Ruby engine and toolchain built from scratch, in one binary.**

Spinel is to Ruby what Bun set out to be for JavaScript, taken one step further: not only the package manager, test runner, and app compiler are new, the engine underneath is too. Ruby's syntax and semantics stay. Everything that executes them is written fresh, in Rust, designed around what we know now about fast dynamic-language VMs.

```sh
curl -fsSL https://spinel.dev/install | sh   # one binary, no Ruby needed

spinel init                 # Gemfile + .ruby-version + test/
spinel add sinatra          # resolves + installs in seconds, writes Gemfile.lock
spinel install              # reads any existing Gemfile.lock
spinel run app.rb           # Spinel's own VM
spinel test                 # minitest/rspec, forked workers, --watch
spinel x rubocop            # run a gem's executable without installing it
spinel build --compile app.rb -o app   # single self-contained executable
```

## Decisions (settled, see `CLAUDE.md` for the why-nots)

| Decision | Choice |
|---|---|
| Engine | From scratch. No CRuby code in the execution path. |
| Compatibility | Full Ruby, reached incrementally. [ruby/spec](https://github.com/ruby/spec) is the definition of done. |
| Host language | Rust |
| Parser | Prism now, lowered into Spinel's own AST so a hand-written parser can replace it later |
| Native extensions | A Rust extension API plus an FFI. Key extensions are reimplemented, not emulated. |
| Core classes | Written mostly in Ruby on a small set of Rust primitives; the JIT makes them fast |
| Concurrency | Isolated heaps with a lock per heap, Ractor-native from the first commit |
| Language version | One Ruby language version at a time, the latest stable (4.0 at launch). `RUBY_ENGINE` is `"spinel"`. See docs/cli.md |

## Why from scratch

CRuby carries thirty years of decisions that a new engine does not have to: a C API that freezes the object layout, thousands of process-wide globals that made Ractors a decade-long retrofit, a conservative GC that cannot move objects, and core classes in C that only a few people can change. Spinel starts with shapes for instance variables, a precise per-heap GC that is free to move, a Ractor-native heap design, and a core library in Ruby that anyone can read and improve. The bet is that a modern design, plus a JIT built on Cranelift, ends up faster than CRuby with YJIT on real programs, while running the same code.

## Non-goals

- Running CRuby C extensions unmodified. Gems like `pg` get a Spinel-native implementation behind the same Ruby API.
- Windows in the first releases. macOS (arm64, x64) and Linux (x64, arm64) first.
- Rails on day one. Rails is the phase 7 milestone, not a phase 1 constraint.

## Docs

- [Architecture](docs/architecture.md): crates, binary layout, dependencies
- [Engine](docs/engine.md): values, heap, GC, bytecode, interpreter, core library, Ractors, JIT, conformance
- [CLI and runtime plumbing](docs/cli.md): `spinel run`, how Spinel identifies itself, bytecode cache, Bundler interop
- [Package manager](docs/package-manager.md): Gemfile/lock compat, index, resolver, store, extensions
- [Test runner](docs/test-runner.md)
- [Build / compile](docs/build.md): single-executable packaging
- [Roadmap](docs/roadmap.md): phases as one-session slices, each with a named check

## Status

Pre-code. Progress is measured one way: ruby/spec pass counts per directory, published in `bench/spec-status.md` by CI.
