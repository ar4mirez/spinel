# Engine

The VM that runs Ruby. Every choice below is either a **decision** (settled with the owner, listed in README) or a **default** (a reasonable starting point that a slice may replace if ruby/spec or a benchmark says so). Defaults are marked.

**Language version.** Spinel implements one Ruby language version at a time, the latest stable release (4.0 at launch), and `RUBY_VERSION` reports it. ruby/spec is checked out at the matching branch so `ruby_version_is` guards select the right specs. See cli.md for the full identity table.

## Pipeline

```
source ──Prism──▶ spinel_ast ──resolve──▶ bytecode ──▶ interpreter (+ inline caches)
                              (locals,                       │
                               scopes,                       ▼ phase 6
                               constants)              Cranelift JIT
```

- **Parse.** `spinel-parse` lowers Prism's tree into `spinel_ast`. Error-tolerant: syntax errors become nodes with diagnostics, so tooling can still format or lint a broken file.
- **Resolve.** Ruby needs a pass that knows which bare identifiers are locals versus method calls, assigns slot indexes to locals per scope, and marks variables captured by blocks. Prism already does the local-versus-call decision; Spinel keeps its own pass so the AST stays parser-independent.
- **Compile.** One bytecode function per method, block, or top-level script. Blocks compile to their own function with a pointer to the enclosing frame's environment. Bytecode is immutable and position-independent (symbols by name, relinked on load), so it can be cached on disk and shared between Ractors.
- **Execute.** A non-recursive interpreter loop. A Ruby-to-Ruby call pushes a frame and continues the same loop; it does not recurse on the Rust stack. This matters for fibers and for deep recursion limits that match Ruby's.

## Values (default)

64-bit tagged word, CRuby's proven scheme:

| tag | meaning |
|---|---|
| `...1` | fixnum, 63-bit |
| `..10` | flonum (most doubles, rotated bits), so floats rarely allocate |
| `x..00` | heap pointer, 8-byte aligned |
| small constants | `nil`, `true`, `false`, `undef`, static symbols |

`Integer` promotes to a heap bignum on overflow. Bignum arithmetic is a primitive over a pure-Rust bigint crate.

## Heap and GC

**Decision:** one `Heap` per Ractor. `&mut Heap` is passed explicitly through the VM; there is no global.

**Default GC:** precise, non-moving mark-sweep with size-class free lists and a large-object space. Precise because the compiler knows every root: the VM stack, frames, the current exception, per-heap tables, and Rust-side handles. Rust code holds `Value`s only inside a `HandleScope`, so a GC in the middle of a primitive cannot lose them. Non-moving first because it is the simplest correct thing. Because there is no C API pinning objects, a moving generational collector is a contained upgrade later; `spinel-ext` uses handles from day one so extensions survive that upgrade.

Object header (default, 16 bytes): class pointer, shape id, flags (frozen, ractor-shareable, mark bit, age). Instance variables live in a slots array indexed by shape; the shape tree is per heap.

## Shapes and inline caches

Instance variables use hidden-class shapes (V8, YJIT). Method lookup uses a per-class method table plus a per-heap global cache keyed by `(class, method symbol)` and invalidated by a class serial that bumps on any method definition in the class or its ancestors. Call sites carry a monomorphic inline cache `(class serial, target)`. Because bytecode is shared across Ractors, inline caches do not live in the bytecode; each heap owns a side table indexed by call-site id. Class serials are atomic integers on the shared class object so every heap sees an invalidation. These are day-one features because retrofitting them into a VM that assumed direct table lookups is painful.

## Calling convention

Ruby's full argument protocol from the start, because ruby/spec's `language/` directory exercises all of it and everything else depends on it: required, optional, splat, post-required, keyword, keyword splat, `**nil`, block argument, block pass, anonymous `*`/`**`/`&` forwarding, `...`. Arity errors match Ruby's messages exactly; the spec checks the text.

## Frames, blocks, exceptions, fibers

- Frames hold locals, an environment pointer for captured variables (environments are heap-allocated only when a block captures them; default: a `captured` bit from the resolve pass decides), the receiver, the method entry, and a catch table.
- `break`, `next`, `redo`, `return` from blocks, and `throw`/`catch` are all non-local exits through the same unwinding path as exceptions, with catch tables per bytecode range, like YARV and the JVM.
- Fibers own a VM stack. Switching fibers is switching which VM stack the interpreter loop uses. Because the interpreter is non-recursive and the core library is in Ruby, almost no fiber switch happens with Rust frames in between. Where a primitive must call back into Ruby (a sort comparator, `Hash#each` primitive fallback), the re-entrant call runs on a `corosensei` coroutine so the fiber can still be suspended. `HandleScope`s form a per-thread linked list that includes suspended coroutine stacks, so the GC still sees every handle. `Enumerator#next` and the Fiber scheduler API come from this for free.

## Core library

**Decision:** core classes are Ruby on primitives. `core/string.rb` defines `String#upcase` in Ruby using primitives like `Primitive.string_bytes(self)`. The rule for what becomes a primitive: raw memory, allocation, encoding tables, syscalls, dispatch, and anything the JIT needs as an intrinsic. Everything else is Ruby.

Boot order:

1. `spinel-vm` creates the heap and the bootstrap classes `BasicObject`, `Object`, `Module`, `Class`, `Kernel`, `Symbol`, `String`, `Integer`, `Array`, `Hash`, `Proc`, `Exception` as empty shells with the right ancestry.
2. Rust primitives are registered under a hidden `Primitive` module.
3. `core.image` (precompiled `core/*.rb`) is loaded and executed, filling in every method.
4. Core classes are marked shareable. Reopening them from user code is allowed only from the main Ractor, as in Ruby 4 (see "Ractors and threads").

`core/*.rb` is compiled to bytecode at `cargo build` time by running the compiler on the host, so boot does not parse.

Strings (default): byte vector plus encoding plus cached character count. UTF-8, US-ASCII, ASCII-8BIT first; other encodings via transcoding tables in a later phase. No ropes.

Hash (default): insertion-ordered open addressing, keyed on `#hash`/`#eql?` with fast paths for Symbol, String, and fixnum keys.

Regex is the open question (see below).

## Metaprogramming

Supported from the phase it is needed, never designed out: `send`, `respond_to?`, `method_missing`, `define_method`, `instance_variable_get/set`, `instance_eval`, `class_eval`, `Module#include/prepend/extend` with correct ancestor linearization, `const_missing`, `inherited`/`included`/`method_added` hooks, `ObjectSpace.each_object` for classes only at first. String `eval` works because Prism is in the binary. `binding` and `Binding#local_variable_get` come with it; the resolve pass keeps a name-to-slot map per frame for that purpose. `caller`, `caller_locations`, `Method#source_location`, and `__method__` come from frame metadata. `ObjectSpace::WeakMap`, `WeakRef`, and `ObjectSpace.define_finalizer` need weak references in the GC and are phase 2. `TracePoint` for `:class`, `:call`, `:return`, `:line`, and `:raise` is phase 7 because Zeitwerk and debuggers need it. Refinements are late-phase.

## Ractors and threads

**Decision:** Ractor-native. A Ractor is a heap plus a lock plus one or more OS threads. Threads in the same Ractor take turns under the lock, releasing it around blocking IO, like a per-Ractor GVL. Ractors run in parallel.

Shared, immutable, append-only tables live in `spinel-vm/src/shared/`: the symbol table and frozen string literals. Classes and modules are also shared objects, but they are not immutable: their method tables, constants, and class-level instance variables may be written only by the main Ractor, under one class lock, and read by every Ractor. Non-main Ractors get `Ractor::IsolationError` on write, which is Ruby 4's rule and is what `core/ractor/` specs check. Everything else is per heap. A thread that does not hold its Ractor's lock is parked, so the GC can treat parked threads' VM stacks as roots without stopping the world.

The lock is released around blocking IO, `sleep`, `Queue#pop`, `Mutex#lock`, `IO.select`, and FFI calls declared `blocking: true`. Sending an object to another Ractor copies it or moves it; shareable objects (frozen deep, or immutable tables) pass by reference. This is the same rule set as Ruby 4, so `Ractor` specs apply.

## JIT (phase 6)

Bytecode to Cranelift IR, method at a time, after a call count threshold, with inline caches read at compile time and deoptimization back to the interpreter when a guard fails. JIT frames emit stack maps so the precise GC keeps working. ZJIT's design is public reference material for this shape. Not started until the interpreter passes the bulk of ruby/spec, because the JIT can only be as correct as the semantics it copies.

## Native extensions and FFI

**Decision:** `spinel-ext` crate, no CRuby C API.

- Extensions are `cdylib` crates built with `spinel-ext`, exporting `spinel_ext_init`. `require "foo"` finds `foo.spinel` the way it finds `foo.so` today. The boundary is a versioned `extern "C"` function table with `#[repr(C)]` handles, because Rust's own ABI is not stable across compiler versions; the `spinel-ext` crate is a safe wrapper over that table. A version mismatch is a clean error.
- Values are opaque handles rooted in a `HandleScope`. Extensions never see raw pointers, so the GC is free to move.
- `Spinel::FFI` (default: built on `libffi`) covers the `fiddle`/`ffi` gem use case: load a C library, declare functions, call them.

**Built-in gems** (the default gems whose C parts become Spinel primitives, shipped inside the binary at pinned versions): `json`, `digest`, `zlib`, `socket`, `io-console`, `date`, `bigdecimal`, `openssl` (bound to the `openssl` crate; large surface, phased), `psych` (a YAML crate behind Psych's API), `etc`, `strscan`, `stringio`, `monitor`, `racc`, `fiddle` (as `Spinel::FFI`), and `ripper` replaced by a Prism-backed shim. Their Ruby halves are the upstream gems' Ruby files. Lockfiles that pin these resolve to the built-in copies (package-manager.md, section 6).

**Spinel-native gems** for popular third-party C extensions, published to rubygems.org under the `<cpu>-<os>-spinel` platform and substituted at install time: `pg` on `rust-postgres`, `sqlite3` on `rusqlite`, `nio4r` and `puma` on `mio`/Rust IO, `bcrypt`, `msgpack`. Each presents the upstream gem's Ruby API. `nokogiri` is the hard one and is out of scope until phase 7.

## Conformance

- `spec/ruby` is a git submodule of ruby/spec. `mspec` is Ruby, so it runs on Spinel itself once the engine is far enough along. Before that, `spec/harness/` is a tiny Rust runner that understands `describe`/`it`/`should ==` and a handful of matchers, enough to drive the `language/` specs.
- CI publishes `bench/spec-status.md`: pass/fail/skip counts per directory. That table is the project's progress bar.
- Skips live in `spec/tags/` with a reason. There are no expected-failure markers in code.

## Performance targets

| milestone | target |
|---|---|
| interpreter, phase 3 | within 1.5× of `ruby --disable-yjit` on the yjit-bench headline set |
| JIT, phase 6 | faster than `ruby --yjit` on the same set |
| boot | `spinel -e ''` under 10 ms; a 500-file app with warm bytecode cache under 100 ms |

## Open questions to settle when their slice arrives

- **Regex.** Ruby's regex dialect is Onigmo with lookbehind, named captures, backreferences, `\p{}` and encoding awareness. A Rust regex crate does not cover backreferences or lookaround. Options: bind Onigmo (C, and the one piece of CRuby lineage that would be allowed since it is an independent library), write a backtracking engine in Rust, or start with the `fancy-regex` crate and measure. Decide at the end of phase 1, because `language/regexp/` and most of `core/string/` depend on it.
- **Encoding coverage.** Beyond UTF-8/ASCII/binary, which of Ruby's 100+ encodings matter. Probably the transcoding set for Shift_JIS, EUC-JP, ISO-8859-x, UTF-16/32 first.
- **Generational GC timing.** Only after allocation profiles from real gems exist.
