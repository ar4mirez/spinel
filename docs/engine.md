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
- **Resolve.** Ruby needs to know which bare identifiers are locals versus method calls, which slot each local gets, and which variables a block captures. The first is decided in `spinel-parse`, because Prism decides it and the lowering keeps the answer as `VarRef::Local` versus a `Call` with `variable_call`. The second is done by the compiler as it walks, since a scope's slot map is exactly what the compiler is already building. Landed that way in [#10](https://github.com/ar4mirez/spinel/issues/10): a separate pass had nothing left to do that the walk was not doing anyway. Capture analysis has no home yet and gets one with blocks, in [#11](https://github.com/ar4mirez/spinel/issues/11) — that is the pass this line originally described, and the only part of it that is a real pass.
- **Compile.** One bytecode function per method, block, or top-level script. Blocks compile to their own function with a pointer to the enclosing frame's environment. Bytecode is immutable and position-independent (symbols by name, relinked on load), so it can be cached on disk and shared between Ractors.
- **Execute.** A non-recursive interpreter loop. A Ruby-to-Ruby call pushes a frame and continues the same loop; it does not recurse on the Rust stack. This matters for fibers and for deep recursion limits that match Ruby's.

### Bytecode

Landed in [#10](https://github.com/ar4mirez/spinel/issues/10). An `Iseq` is a
`Vec<Insn>` — a Rust enum, not a packed byte buffer, because the decode step a
buffer buys is the `match` an enum already does and `Insn` is `Copy` and 16
bytes either way. The on-disk form phase 3 caches is therefore a *serialisation*
of the enum rather than the enum's own bytes.

Three rules make an `Iseq` position-independent, which is what lets it be cached
on disk and shared between Ractors:

- **Jumps are relative**, counted from the instruction after the jump, so an
  `Iseq` never needs relocating.
- **Symbols are names.** The pool holds `Box<str>` and instructions index it;
  `Iseq::link` interns the pool against the process table. Two processes agree
  about symbols without agreeing about the order they first saw them in.
- **Literals are descriptions**, never `Value`s, because a `Value` can be a
  pointer into one heap. Materialising is per heap, which is also what makes a
  string literal a fresh object per evaluation, as Ruby requires.

Arithmetic and comparison are instructions rather than sends — `opt_plus` and
friends, as YARV has them: a fast path for fixnums and flonums with a real send
behind it. The send behind it arrives with the calling convention in #11. Until
then an operand off the fast path is *not yet dispatchable* rather than wrong,
and the spec that needed it stays blocked. Integer overflow is the same answer:
Ruby promotes to a bignum, there is no bignum, and a wrapped result would be a
wrong answer where a blocked spec is merely an incomplete one.

## Values

Landed in [#6](https://github.com/ar4mirez/spinel/issues/6). A 64-bit tagged word,
CRuby's proven scheme:

| low bits | the rest of the word | kind |
|---|---|---|
| `1` | 63-bit signed integer | fixnum |
| `10` | double, rotated left by three | flonum |
| `0100` | ordinal | `nil`, `false`, `true`, `undef` |
| `1100` | symbol id | static symbol |
| `000` | 8-byte-aligned pointer | heap object |

The zero word is deliberately not a `Value`. `Option<Value>` is therefore one word as
well, and a zeroed slot is a detectable bug rather than a plausible object.

Flonums cover the doubles whose top three exponent bits are `011` or `100`: magnitudes
between roughly 1.7e-77 and 1.8e77, which is every float a normal program holds. NaN,
the infinities, `-0.0`, and the extremes allocate instead. Excluding those three is also
what makes bitwise equality on `Value` exactly Ruby's `equal?`, which the interpreter,
the method cache, and the GC all rely on.

`Integer` promotes to a heap bignum past ±2^62. Bignum arithmetic is a primitive over a
pure-Rust bigint crate.

## Heap and GC

**Decision:** one `Heap` per Ractor. `&mut Heap` is passed explicitly through the VM; there is no global.

**Default GC:** precise, non-moving mark-sweep with size-class free lists and a large-object space. Precise because the compiler knows every root: the VM stack, frames, the current exception, per-heap tables, and Rust-side handles. Rust code holds `Value`s only inside a `HandleScope`, so a GC in the middle of a primitive cannot lose them. Non-moving first because it is the simplest correct thing. Because there is no C API pinning objects, a moving generational collector is a contained upgrade later; `spinel-ext` uses handles from day one so extensions survive that upgrade.

Landed in [#7](https://github.com/ar4mirez/spinel/issues/7). The object header is 16 bytes:

| offset | field | notes |
|---|---|---|
| 0 | class pointer | `Option<Value>`; also the free-list link while the cell is free |
| 8 | `len` | `Value` slots, or bytes |
| 12 | flags | mark bit; frozen and ractor-shareable join it with #8 |
| 13 | payload kind | slots or bytes — whether the collector descends |
| 14 | reserved | #8's shape id |

Instance variables live in a slots array indexed by the shape; the shape tree is per heap.

Cells come from 64 KiB blocks, one size class per block: 32, 64, 128, 256, and 512 bytes, holding 2, 6, 14, 30, and 62 slots. Anything larger gets its own allocation in the large-object list. Sweeping rebuilds every free list from the unmarked cells, so a dead object and a cell that has never been used take the same path; blocks are zeroed on arrival so that an unused cell reads as unmarked rather than as uninitialised memory.

A collection runs inside an allocation, when bytes since the last one cross a threshold — 1 MiB, then twice the live bytes, floored at 1 MiB — or when a caller asks. There is no collector thread and no interior mutability, so every collection point is a `&mut Heap`, which is what makes "a GC in the middle of a primitive" something the compiler can see.

`HandleScope` is the discipline, and it is enforced rather than documented: `Heap` has no method that allocates. `HandleScope::alloc` returns a `Handle` — an index into the heap's root stack — and never a bare pointer, so an object the collector cannot see is a program that does not compile. A scope pops its own handles on drop, and a nested scope's handle cannot escape into its parent because `Handle` is covariant in the scope's lifetime. Today the root set is exactly that stack; the VM stack, frames, the current exception, and the per-heap tables each plug into the same mark phase as their slice lands.

## Classes and ancestor chains

Landed in [#8](https://github.com/ar4mirez/spinel/issues/8). Every class and module owns a **run** of the ancestor chain: the modules prepended to it, then the class itself, then the modules included in it. A class's ancestry is its own run followed by its superclass's, so `include` on a superclass is visible to every subclass with nothing propagated, and `ancestors` is a walk.

The run is maintained by `include` and `prepend` rather than recomputed from a list of mixins, because the order depends on the state of the chain at the moment of each call: `include M; include B; include A` where `A` also includes `M` gives `[C, A, B, M]`, and no replay of `[M, B, A]` against `A`'s final contents produces that. Ruby 3.0's [Feature #9573](https://bugs.ruby-lang.org/issues/9573) — a module gaining an include reaches back into everything that already mixed it in — is a flat list of includers per module, patched at each includer's own copy of the module.

These rules are CRuby's, and they were measured rather than read: `crates/spinel-vm/tests/ancestors.txt` is 42 hierarchies with the ancestors a real Ruby computes for them, `scripts/ancestors-oracle.rb` is what re-measures it in CI, and `tests/ancestors.rs` is what holds Spinel to it.

Singleton classes are allocated on first ask, with one exception: the `class` keyword builds a class's metaclass as it defines it, because `class B < A` with a `def self.m` on `A` reaches `m` through `#<Class:B> < #<Class:A>` and a `B` still pointing at `Class` would miss it. That is CRuby's `rb_define_class`, and `HandleScope::define_class` stays lazy for everything else. A class's singleton inherits from its superclass's, `BasicObject`'s inherits from `Class`, and a module's inherits from `Module` — the twist that puts `Class`, `Module`, `Object`, `Kernel`, and `BasicObject` at the end of every metaclass's ancestors. An ordinary object's singleton *becomes* its class, as in Ruby, so the header write is the whole mechanism.

Method tables are Rust hash maps hanging off the class table, not object slots, so the class table is a root source in `Heap::mark`: it holds every class object, every method body, and every constant.

## Constants and lexical scope

Landed in [#13](https://github.com/ar4mirez/spinel/issues/13). A constant belongs to exactly one module and lives in a hash map on that module's class-table entry, beside its methods and rooted by the same walk.

Lookup needs the chain of `class`/`module` bodies a reference was **written** inside, which is not the chain it **runs** inside:

```ruby
module A
  X = 1
  class B
    def m = X          # A::X, though B.ancestors never reaches A
  end
end
```

The compiler cannot supply that chain — it knows `B` is one level deeper, but `B` is a `ClassId` that does not exist until the body runs — so it is built at runtime, as CRuby's cref is. A `CrefId` indexes a per-heap arena of `(class, parent)` nodes. It is an arena index rather than a heap object because a cref is unreachable from Ruby, never outlives the modules it names, and is read on the hot path of every constant reference; the `ClassId`s in it are already rooted by the class table.

Three things carry one, because a reference resolves where it was written: a frame carries the scope that pushed it, a `Method` carries the scope its `def` appeared in, and a `Proc` carries the scope it was created in as a sixth slot.

A bare `X` searches the cref chain innermost-first (own tables only), then the innermost scope's ancestors, then `Object` if the ancestors did not already reach it — which they do for a class body and do not for a module body. `A::X` is the ancestor walk alone, rooted at `A`, with no lexical scope and no `Object` fallback; Ruby removed that fallback in 2.5, so `Sub::TOP` for a top-level `TOP` is a `NameError`. `::X` is the ancestor walk rooted at `Object`.

These rules are documented nowhere and read wrong from `variable.c`, so like the ancestor chain they are measured rather than believed: `crates/spinel-vm/tests/eval.txt` holds them and `scripts/eval-oracle.rb` re-measures them in CI.

`defined?` reads the same tables and answers a table of Ruby's own strings. Where it *misses*, it does not answer Ruby's `nil`: a fresh bootstrap heap cannot tell "undefined" from "defined in a file `require` has not landed to load", and `nil` would pass a spec the VM had no right to. A miss is `Error::Unknowable` until [#39](https://github.com/ar4mirez/spinel/issues/39).

## Shapes and inline caches

Instance variables use hidden-class shapes (V8, YJIT). Method lookup uses a per-class method table plus a per-heap global cache keyed by `(class, method symbol)`, landed with #8; misses are cached too, so a `method_missing` dispatch does not re-walk the chain per call. Invalidation is a serial that bumps on any method definition, removal, `include`, or `prepend`. Today that serial is one per class *table* rather than one per class: correct, and coarser than it needs to be — a definition anywhere evicts every cached lookup. Per-class serials that bump on a definition in the class or its ancestors need a subclass list and a descendant walk per definition, and the benchmark that would justify writing them arrives with the JIT.

Call sites carry a monomorphic inline cache `(class serial, target)`. Because bytecode is shared across Ractors, inline caches do not live in the bytecode; each heap owns a side table indexed by call-site id. Class serials are atomic integers on the shared class object so every heap sees an invalidation. These are day-one features because retrofitting them into a VM that assumed direct table lookups is painful.

## Calling convention

Ruby's full argument protocol from the start, because ruby/spec's `language/` directory exercises all of it and everything else depends on it: required, optional, splat, post-required, keyword, keyword splat, `**nil`, block argument, block pass, anonymous `*`/`**`/`&` forwarding, `...`. Arity errors match Ruby's messages exactly; the spec checks the text.

Landed in [#11](https://github.com/ar4mirez/spinel/issues/11), less the parts that need a class the VM does not have yet: `**kwrest` collects into a `Hash` and there is no `Hash`, and `&`/`...` forwarding waits with it. One `bind` fills a frame's parameter slots from a call's arguments, and the caller never inspects the callee's parameters — which is what makes an ordinary call, `send`, `yield`, and `Proc#call` the same code path with a different receiver.

A method or lambda and a block differ in exactly two places, and both are the binder's: a lambda checks arity and raises `ArgumentError`, and a block pads with `nil`, drops extras, and spreads a single `Array` across its parameters when it has room for more than one value. `{ |a| }` and `{ |*a| }` take the array whole; `{ |a, b| }` and `{ |a,| }` spread it.

A call site is a table entry rather than an instruction operand: `Insn::Send(u32)` indexes `Iseq::call_sites`, which carries the name, the positional count, the keyword names, and what block the call passes. That is where the inline caches above go, since they are per heap and bytecode is not.

Optional and keyword defaults are code at the top of the body rather than something the binder evaluates. The binder writes an undef marker into the slots a call left out, and each default is guarded by a `JumpUnlessUndef`. That handles a default that calls a method, and it handles keywords — which are independent rather than a fall-through chain, so a per-parameter entry offset would not.

## Frames, blocks, exceptions, fibers

- Frames hold locals, an environment pointer for captured variables (environments are heap-allocated only when a block captures them; default: a `captured` bit from the resolve pass decides), the receiver, the method entry, and a catch table.
- As landed in #11, a frame is a value in a `Vec` that the loop pushes and pops, so a Ruby-to-Ruby call never grows the Rust stack. Locals live in a heap environment whose first slot links to the enclosing one, and `GetLocal` carries a depth that walks it. The environment is a plain slots object, so the collector traces a captured local with no code that knows what an environment is.
- That environment is allocated for *every* frame rather than only for the ones a block captures, which is the `ponytail:` note in `interp.rs`. The escape analysis this section describes needs a resolve pass that does not exist yet, and the slice's check is a spec delta rather than a benchmark. It is a cost, not a wrong answer.
- A method body is a definition id — a fixnum indexing a per-heap table of `Iseq`s and primitives — rather than a heap object, because the heap has no payload kind that holds a Rust `Iseq` and no finaliser to drop one. A fixnum body is also one the collector never traces.
- `break`, `next`, `redo`, `return` from blocks, and `throw`/`catch` are all non-local exits through the same unwinding path as exceptions, with catch tables per bytecode range, like YARV and the JVM. As landed in #12: one `Unwind` value travels up the frame stack and one search decides where it stops — a `rescue` range whose handler accepts the class, the frame `catch` opened for a tag, or the frame a `break` or `return` named by id. Frame *ids*, not depths, because a `Proc` can outlive its call and a depth would be reused; a target that is no longer on the stack is `LocalJumpError`, which is Ruby.
- An `ensure` body is compiled **once**, not twice. YARV emits one copy inline for the normal path and another as a handler, and pays for it with two versions of any `break` or `return` written inside one. Spinel enters it through the same door either way: `EnterEnsure` parks the reason on the frame — a value on the normal path, an unwind in flight otherwise — and `LeaveEnsure` either restores the value or resumes the unwind. "Runs on every exit path" is then a property of the search rather than of keeping two copies in step.
- A jump *within* a frame that leaves an `ensure` goes out through the unwinder too (`Insn::Goto`), because a plain jump to a loop's end would step straight over the `ensure` body. Only the ones actually being left are run: the search skips any entry whose range still covers the jump's target, which is what keeps `begin; while c; next; end; ensure; E; end` from running `E` once per iteration.
- A raise becomes an exception object at exactly one place, where `Error::Raise` leaves an instruction. What does *not* become one is `Error::NoDispatch`, `Error::Budget`, `Error::Unknowable`, and `Error::NoSuchMethod`: those say the VM cannot run the program rather than that Ruby raises, and a `rescue` that caught them would let an example carry on down a branch Ruby never takes. The exception hierarchy itself is measured from CRuby by `scripts/exceptions-oracle.rb` into `src/exceptions.txt`, including which classes define an `initialize` of their own — Spinel implements only `Exception#initialize`, so `new` on one of the others refuses rather than answering as though it had validated arguments it never looked at.
- Fibers own a VM stack. Switching fibers is switching which VM stack the interpreter loop uses. Because the interpreter is non-recursive and the core library is in Ruby, almost no fiber switch happens with Rust frames in between. Where a primitive must call back into Ruby (a sort comparator, `Hash#each` primitive fallback), the re-entrant call runs on a `corosensei` coroutine so the fiber can still be suspended. The heap's root stack is what the GC scans, and a suspended coroutine keeps its own region of it, so the GC still sees every handle. `Enumerator#next` and the Fiber scheduler API come from this for free.

## Core library

**Decision:** core classes are Ruby on primitives. `core/string.rb` defines `String#upcase` in Ruby using primitives like `Primitive.string_bytes(self)`. The rule for what becomes a primitive: raw memory, allocation, encoding tables, syscalls, dispatch, and anything the JIT needs as an intrinsic. Everything else is Ruby.

Boot order:

1. `spinel-vm` creates the heap and the bootstrap classes `BasicObject`, `Object`, `Module`, `Class`, `Kernel`, `Comparable`, `Enumerable`, `Numeric`, `Symbol`, `String`, `Integer`, `Array`, `Hash`, `Proc`, `Exception` as empty shells with the right ancestry. The three modules and `Numeric` are on the list because of "the right ancestry": without them `Integer.ancestors` is wrong from the first commit, and `core/*.rb` cannot fix an ancestry it is loaded into.
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

- `spec/ruby` is a git submodule of ruby/spec. `mspec` is Ruby, so it runs on Spinel itself once the engine is far enough along. Before that, `spec/harness/` is a tiny Rust runner that reads a spec file's syntax tree: it finds the `describe`/`it` structure and evaluates the `ruby_version_is` and `platform_is` guards against the target. Since [#10](https://github.com/ar4mirez/spinel/issues/10) it also *runs* the examples it can: an example whose every construct compiles is executed and its `.should ==` expectations checked, and one that mentions anything the compiler cannot mean yet is reported `blocked` rather than passed or failed. The `blocked` column shrinks slice by slice and disappears with the harness itself. Each run ends by ranking what the blocking constructs were, in order of how many examples each accounts for, which is how the next slice gets chosen from data rather than from a guess about which corner of Ruby matters. Run it with `scripts/spec.sh [dir]`.
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
