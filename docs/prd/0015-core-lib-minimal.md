# PRD 0015 — Minimal `core/*.rb`: Kernel, Object, Integer, String, Array, Hash, Symbol

Tracks [#15](https://github.com/ar4mirez/spinel/issues/15). Milestone: Phase 1: a VM that runs `language/`. `P0`, `size:M`, `area:core-lib`.

## Objective

Every slice so far has been the engine underneath a core library that did not exist. This is the first Ruby the engine runs on itself: `core/*.rb`, loaded into every heap at bootstrap, defining the methods `language/` reaches for while checking something else.

Two things gate on it, and they are the two the corpus complains about loudest.

`new` on a built-in class is answered by a refusal, not a value — 631 examples corpus-wide, 92 in `language/`, fourth-largest single reason in the whole corpus. #13 added `Class#new` and let it allocate only `Object`, `BasicObject`, and the exception classes whose `initialize` is `Exception#initialize`, because a zero-slot object wearing `Proc`'s class made `Proc#lambda?` read past the end of it. Allocation shape per class is this slice's.

`Array` cannot grow. A heap object's `len` is fixed at allocation, so `a << 1` has nowhere to put the element, and `BinOp::from_name("<<")` returns `None` on purpose with a test pinning it. #12's triage named the consequence: all five examples in `redo_spec.rb`, four in `throw_spec.rb`, and three in `break_spec.rb` are blocked on `Array#each`, `#<<`, and `#include?` rather than on anything about `redo`, `throw`, or `break`.

## Baseline

Measured on `ce37cd5`, before this slice:

```
language/    2735 examples ·  545 passed · 0 failed · 2145 blocked · 45 skipped
corpus      25624 examples ·  713 passed · 0 failed · 23056 blocked · 1855 skipped
```

The core-library-shaped reasons in `language/`, ranked, are what this slice is measured against:

```
  92  `new` on a built-in class cannot be answered before `core/*.rb` (#15)
  21  Array#<<        14  String#dup      10  Array#each     10  Kernel#loop
   6  String#frozen?   6  Symbol#should    5  String#*        5  Kernel#freeze
   5  Array#map        3  Integer#times    3  Integer#<<      2  Array#pop
   2  String#length    2  String#b         2  Integer#& ** ^  1  String#+
   1  Array#[]         1  Array#dup        1  Class#allocate
```

The larger reasons above them — `eval` (133), `mock` (95), instance variables (482) — belong to #151 and to the harness, not here.

## Decisions

### Where core lives, and who loads it

**`crates/spinel-core` loads it. `spinel-vm` never sees a `.rb` file.**

`spinel-vm` deliberately does not depend on `spinel-parse`: the manifest says so in a comment, and the rule is that Prism lives in exactly one crate. Compiling `core/*.rb` needs a parser, so the loader cannot live in the VM without dragging Prism into every VM build.

`spinel-core` already exists for exactly this — "the Rust primitives underneath Ruby's core classes" — and is empty. It gains `spinel-parse`, and one function: `spinel_core::boot(&mut scope)`, called after `scope.bootstrap()`. Every embedder — the CLI, the spec harness — calls both, in that order.

**Compiled once per process, executed once per heap.** `include_str!` embeds the sources, so there is no file to find at runtime and no install layout to get wrong. The parse and the compile happen on the first `boot` and the resulting `Arc<Iseq>` is cached in a `OnceLock`; every later heap re-runs the same bytecode. This matters because the spec harness builds a fresh `Heap` per example and there are 25,624 of them: parsing per heap would multiply the corpus run by the cost of the parser, and executing per heap is unavoidable — method tables are per heap.

An `Iseq` is immutable bytecode and holds no `Value`, so the cache is not the process-global mutable VM state `CLAUDE.md` forbids. It is the same category as `shared/symbols`.

`// ponytail:` this is the bytecode image engine.md describes, minus the serialization. The ceiling is process startup: one parse and compile of `core/*.rb` on the first heap. The upgrade path is a `build.rs` that serializes the `Iseq` into the binary, which is worth writing when that first parse shows up in a benchmark — not before.

### What `Array` is

**Two slots: storage and length. The storage object is replaced on growth; the `Array` is not.**

`a << 1` must mutate the object the caller is holding. A heap cell cannot grow — mark-sweep with size classes hands out a fixed cell — so a design where the elements live directly in the `Array`'s own slots can only "grow" by allocating a different object, and Ruby would see the identity change.

So an `Array` is two slots, `[storage, length]`, where `storage` is a separate `Payload::Slots` object holding capacity elements and `length` is a fixnum of how many are live. Growth allocates a bigger storage object, copies, and stores it back into slot 0. The `Array`'s address never changes. This is what CRuby does with `RARRAY`'s pointer and length, for the same reason.

Capacity doubles from 4. The copy is `Vec`'s amortization argument, which is the one every growable array makes.

### What is Rust, and why

engine.md's rule is that a primitive is raw memory, allocation, encoding tables, syscalls, dispatch, or a JIT intrinsic. Everything else is Ruby. Each primitive this slice adds carries its one-line reason, and the reasons are all one of two: *reads or writes raw storage*, or *allocates*.

| Primitive | Why Ruby cannot |
| --- | --- |
| `Array#[]`, `#[]=`, `#size` | reads and writes a raw slot run |
| `Array#push` | writes a raw slot, and reallocates storage when it is full |
| `Array#pop` | writes the length back into a raw slot |
| `Class#allocate` | allocation, and the shape is per class |
| `String#length`, `#bytesize` | reads a byte payload's length |
| `String#+`, `#*`, `#dup` | allocates a byte payload |
| `String#<<` | not shipped — see non-goals |
| `Integer#<<`, `#>>`, `#&`, `#|`, `#^`, `#~`, `#**` | fixnum bit patterns; the JIT wants them as intrinsics |
| `Object#freeze`, `#frozen?` | a header flag bit |
| `Symbol#to_s`, `#length` | reads the shared symbol table |
| `Object#object_id` | the object's address |
| `Object#hash` | reads raw bytes and raw slots |
| `Module#ancestors`, `#method_defined?`, `Class#superclass` | reads the class table |
| `Float#to_s` | shortest round-tripping decimal is an algorithm, not a format |
| `Kernel#__write__` | a syscall |

`each`, `map`, `include?`, `first`, `last`, `empty?`, `join`, `reverse`, `min`, `max`, `sum`, `times`, `upto`, `loop`, `is_a?`, `respond_to?`, `dup` on `Object`, and the whole of `Comparable` are Ruby, on top of those. If a method is missing from `core/*.rb` and its absence is not explained by a missing primitive, it was left out, not blocked.

### `Class#new` stops refusing

`Class#allocate` becomes a primitive that switches on the receiver's `Builtin`: `Array` allocates the two-slot pair with empty storage, `String` allocates a zero-length byte payload, `Hash` allocates its pair-array, `Object` allocates zero slots as it does today. `Class#new` is then `allocate` plus `initialize`, which is what Ruby says it is, and `core/*.rb` writes each class's `initialize` in Ruby.

The classes with no allocation shape — `Proc`, `Symbol`, `Integer`, `NilClass` — keep refusing, because `Proc.new` without a block and `Integer.new` raise in Ruby and a plausible-but-wrong answer is the one thing this project does not ship. The refusal names which class and why, rather than naming this issue.

## Non-goals

- **Instance variables.** `@x` is not compiled — 482 examples in `language/`, 8520 corpus-wide, and it is [#151](https://github.com/ar4mirez/spinel/issues/151)'s shape tree. Everything in `core/*.rb` is therefore written against slots the primitives own, not against ivars. `attr_accessor` is *not* shipped for the same reason: `Native::Getter`/`Setter` address a fixed slot, and which slot an ivar gets is exactly what a shape decides. Shipping `attr_accessor` against slot 0 would work for one ivar and silently corrupt the second.
- **A real `Hash`.** Hash literals are not compiled (#157), so a hash cannot be written down in a spec. `Hash` here is an association list on the pair array: `[]`, `[]=`, `size`, `key?`, `each`, `delete`, linear. `// ponytail:` O(n) lookup, upgrade to an open-addressed table when a spec measures it or #157 makes hashes writable in the corpus. That is phase 2's `core/hash/` slice.
- **Encodings.** `String` is bytes. `#length` is byte length, and it is only right because the corpus's strings here are ASCII. `#b`, `#force_encoding`, `#encoding` are not shipped; the Encoding slice in phase 2 owns them, and 277 examples already wait on it.
- **Bignum.** `Integer#**` overflows to a refusal, not to a bignum. `Literal::BigInt` already refuses; this keeps that story consistent rather than growing a second, worse integer.
- **`Object#inspect` replacing `interp::inspect`.** The Rust one formats values for *harness reports* and is reachable when no Ruby is running. `core/*.rb` gains a real `inspect` for Ruby code to call; the Rust one stays as the reporter. Deleting it is the harness's own retirement, at the end of phase 2.

## What ships

### `core/*.rb` — new directory

`basic_object.rb`, `object.rb`, `kernel.rb`, `comparable.rb`, `module.rb`, `class.rb`, `integer.rb`, `string.rb`, `symbol.rb`, `array.rb`, `hash.rb`, `exception.rb`, `nil_class.rb`, `true_class.rb`, `false_class.rb`. Loaded in that order, which is dependency order: `Comparable` before `Integer` includes it.

### `crates/spinel-core` — the loader

`boot(&mut HandleScope)` compiles the embedded sources once and evaluates them into the heap. A compile error in `core/*.rb` is a panic with the file and the construct named: it is a bug in this repository, not a user's program, and returning a `Result` would only push the panic one frame out.

### `crates/spinel-vm` — the primitives

New `Native` variants, each named in the table above. `Class#allocate`, and `Native::New` delegating to it instead of refusing. `new_array`/`array_elements` rewritten against the two-slot representation, which is every place in the interpreter that builds or reads an array: array literals, `Native::ArrayPlus`, `MatchData#to_a`, `#captures`, `ruby_eq`, and `inspect`.

### `spinel run`

`spinel run file.rb` and `spinel file.rb` parse, compile, boot a heap with core in it, and evaluate. The "this build has no VM yet" message in `main.rs` is deleted. An uncaught exception prints `file:line: message (Class)` on stderr and exits 1, which is Ruby's shape; `--dump-bytecode` prints the `Iseq` instead of running it, which is the debugging window `spinel parse` is for the tree.

### `spec/harness` — `--blocked=N`

The blocked-reason ranking was capped at 15 with no way to see the tail, and the tail is what the next slice is chosen from. `--blocked=0` prints every reason. `scripts/spec.sh`'s header now says that a value-taking flag is spelled `--flag=value`, because a bare word is read as a corpus path and `--blocked 0` used to fail with "no such spec file: 0".

## Checks, and what they measured

`spinel run hello.rb` works, and `crates/spinel-cli/tests/cli.rs` shells out to the built binary against `tests/fixtures/run/hello.rb`. The expected output checked in beside it is **CRuby's**, produced by running the same file on a real Ruby, so the assertion is "Spinel agrees with Ruby" rather than "Spinel agrees with itself".

The spec delta:

```
language/    545 -> 619 passed   (2145 -> 2067 blocked)
corpus       713 -> 1111 passed  (23056 -> 22653 blocked)
```

Per directory, every one of which was at zero before this slice:

```
core/array      115    core/integer     60    core/matchdata   49    core/kernel   45
core/string      43    core/float       34    core/module      27    core/symbol   19
core/proc        16    core/exception    9    core/hash         9
```

`scripts/verify-passes.rb` re-ran all 1,111 of those passes on ruby 4.0.6 and all agree, so none of them is a pass Spinel had no right to.

### Reflection was the gap the numbers did not show

`language/` moved to 619 with `Kernel#is_a?`, `kind_of?`, `respond_to?`, `Module#===` and `Module#<` all *dead*: each is written in Ruby against `Module#ancestors` or `#method_defined?`, and neither existed. Nothing failed, because a missing method is reported blocked — the specs that would have caught it were blocked on something else first. It was found by running a handful of expressions through `spinel run` and diffing against CRuby, which is the check the spec numbers cannot be.

`Module#ancestors`, `Class#superclass`, `Module#method_defined?` and `Object#hash` are the four primitives that answer it, and they took the corpus from 1,023 to 1,111, with `eql?` — `hash`'s partner, and the other half of what a real `Hash` keys on. The same pass then found four wrong answers in code the specs did not yet reach: `Hash#fetch(k, nil)` raised instead of answering nil, `1 <=> 1.5` was nil instead of -1, `BasicObject#!` asked the object's own `==` instead of testing identity, and `Array#initialize` appended where Ruby replaces.

Making `respond_to?` answer also turned five refusals into failures, all of them real: `Array.new([1,2])` did not replace the receiver's contents, `Array.new(a, x)` raised `ArgumentError` where Ruby raises `TypeError`, `Array#shift` took any number of arguments, `Array#min`/`#max` ignored a comparison block, and `Array.new(n) { break :x }` answered the array rather than `:x` — that last one a `Class#new` bug, where the pre-placed object survived a `break` that named the frame.

`cargo test` covers the representation directly: growth preserves object identity across reallocation, a read past the length is `nil` rather than stale storage, every immediate has a class, and `Float#to_s` matches Ruby's shape at the boundaries measured from CRuby.

### What this slice found, and fixed, elsewhere

Making the core library reachable turned three refusals into failures, which is the harness working:

- **`next` from a block skipped its `ensure`.** `next` compiled to `Insn::Leave`, which pops the frame outright and steps over any `ensure` protecting the point — the same bug a plain `Jump` would have, which is why `Insn::Goto` exists. It now leaves through the unwinder when there is an `ensure` open (`Insn::LeaveThroughEnsure`), so the search runs the bodies on the way. Four examples in `next_spec.rb`.
- **`defined?` answered an unfrozen string.** Ruby's is frozen, and `defined_spec.rb` asserts it on every literal. `Literal::FrozenStr` is the one-variant answer.
- **A singleton method could be defined on a frozen object.** `def frozen.m` now raises `FrozenError`, which needed the header bit this slice added anyway.

### What is skipped, and why

Five examples are in `spec/tags/skip.txt`, each naming a gap this slice does not own:

- `defined?` of a private method needs method visibility, which the class table does not have.
- `[[:blank:]]` must match U+1680; `spinel-regex`'s POSIX brackets are still ASCII for that class. Visible rather than blocked now only because `nil.to_a` answers.
- Two in `send_spec.rb`: `m(*args, &args.pop)` must expand the splat before evaluating the block argument, and Spinel expands splats after every argument is on the stack. Fixing it needs a dynamic argument count at the call site, which is the calling convention's ([#160](https://github.com/ar4mirez/spinel/issues/160)).
- `Kernel#to_enum` must exist, and an `Enumerator` needs fibers ([#26](https://github.com/ar4mirez/spinel/issues/26), [#16](https://github.com/ar4mirez/spinel/issues/16)). Blocked before this slice, failing after it only because `respond_to?` can now answer honestly.

None is an expected failure: each is reported *skipped* with its reason, and none was added to make a run green.
