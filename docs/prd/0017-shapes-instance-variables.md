# PRD 0017 — Shapes: hidden-class instance variables

Tracks [#151](https://github.com/ar4mirez/spinel/issues/151). Milestone: Phase 1: a VM that runs `language/`. `P0`, `size:M`, `area:engine`.

## Objective

Give objects somewhere to put an instance variable, so the corpus stops being blocked on the thing every fixture is *built from*.

`docs/engine.md` has committed to shapes since #7 reserved two header bytes for the id. Nothing has been written. Meanwhile four slices in a row have parked something on this one: #8 left a class's table id in a fixed slot, #12 left an exception's message in a fixed slot, #13 refused `@a` rather than reach for a per-object `HashMap`, and #15 refused `attr_accessor` rather than ship one that is right by accident. Each of those is a second storage scheme that only exists because there is no first one.

## Baseline

Measured on `5f80637`, before this slice:

```
language/    2735 examples ·  626 passed · 0 failed · 2060 blocked ·   49 skipped
corpus      25624 examples · 1144 passed · 0 failed · 22619 blocked · 1861 skipped
```

The two reasons this slice removes:

```
                                  language/   corpus
 assigning an instance variable       483      8521
 an instance variable                  17       308
```

8,829 of the corpus's 22,619 blocked examples — 39% — are the first of those, an order of magnitude ahead of anything else in any milestone. #162 is the precedent for what to expect from a number that size: it unblocked 497 examples and gained 32 passes, because a fixture is what an example is blocked on *first*, not only.

## What Ruby actually does

An instance variable is not a slot number the compiler can pick. Two objects of the same class can hold different sets:

```ruby
class C
  def initialize(flag) = flag ? @a = 1 : @b = 2
end
```

So the mapping from name to storage index belongs to the object, not the class — and putting a hash table in every object costs a word and a lookup on every read. V8's hidden classes and CRuby 3.2's shapes both answer this the same way: objects that were *built the same way* share one description of their layout, and the object holds an id for it.

The tree is keyed by insertion order, which is the part that has to be right. `@a` then `@b` is a different shape from `@b` then `@a`, because a shape is a path from the root and each edge adds one name at one index. That they diverge is not an implementation detail to be smoothed over — it is what makes an index constant per shape, and it is what the unit tests in the definition of done pin.

## Decisions

### Ivars live behind one slot, the way an `Array`'s elements already do

A heap cell cannot grow. `interp.rs` already says so, above `ARRAY_STORAGE`: mark-sweep with size classes hands out a fixed cell, so a growable thing lives in a *separate* storage object that gets replaced, and the object's own address never moves.

Instance variables get the same treatment and for the same reason. Slot 0 of an ivar-capable object holds either `nil` or a classless `Payload::Slots` object of `capacity` values; the shape says which index within it a name is. Growth doubles from 2 and rewrites slot 0.

The alternative — ivars directly in the object's own slots, indexed by shape — reads better in a diagram and cannot be made to work here: an object allocated with room for two ivars that acquires a third would have to become a different cell, and Ruby would see `equal?` change. The indirection is one deref, and it is the deref `Array` already pays.

`Heap::mark` needs no change, which is what the issue predicted: the storage object hangs off a traced slot of a `Payload::Slots` object. The definition of done asks for a test rather than an edit, and that is what ships.

### Shape id `0` means "not ivar-capable", not "no ivars yet"

The header's two reserved bytes become a `u16` shape id. Node 0 is a sentinel and node 1 is the root — an object that *can* hold ivars and holds none.

The distinction is load-bearing because slot 0 means something else on an `Array` (its storage) and on a `Proc` (its iseq). Without a way to ask, `@x = 1` on an array would quietly overwrite the array's elements. With it, an object whose class has a representation of its own answers a refusal that names the class, and no reachable code path writes over a native slot.

This uses the field #7 already reserved rather than a new flag bit, so the header stays 16 bytes and the size classes do not shift.

### Three fixed-slot schemes are deleted, not left beside the new one

The issue names one; two more were parked here in the same way and are collected in the same pass, because a second storage scheme that outlives its reason is how a VM ends up with four.

- **A class object's table id** (`SLOT_ID` in `class.rs`) becomes the hidden ivar `@__id__`. It is always the first ivar a class object receives, so its index is 0 and `class_of` stays two derefs on the dispatch path.
- **An exception's message and backtrace** (`EXC_MESSAGE`, `EXC_BACKTRACE` in `interp.rs`) become `@message` and `@backtrace`, and `Exception#message`, `#to_s` and `#backtrace` become three lines of `core/exception.rb` instead of three natives.
- **A `Hash`'s three slots** become `@pairs`, `@default` and `@default_is_proc`, and the six `__pairs__`-style primitives `install_primitives` was carrying are deleted. `core/hash.rb` reads its own ivars like any Ruby class.

### `defined?(@a)` may finally answer `nil`

#13 refused it on purpose: a VM with no instance variables answering `nil` would pass `defined?(@nope).should be_nil` while being unable to represent the question. That reason expires here. `defined?(@a)` now answers `"instance-variable"` or `nil`, and the `nil` is a measurement rather than a coincidence.

### `attr_accessor` addresses a name, not a slot

#15 left it out because `Native::Getter(slot)` addresses a fixed slot and "which slot" is exactly what a shape decides — an `attr_accessor` built on slot 0 would work for a class with one ivar and corrupt the second. The fix is a reader and a writer that carry a *symbol*, so the shape resolves the index per object at call time. `attr_reader`, `attr_writer` and `attr_accessor` are then one primitive that defines them.

This is in the issue as "wanted by #15" rather than in its definition of done. It ships here because it is forty lines on top of what the definition of done already requires, and because `attr_accessor` is in more corpus fixtures than `@a` is.

### A frozen object refuses the write

`@a = 1` on a frozen object is a `FrozenError` in Ruby. The header flag is already there and the check is one line, so it is not deferred: skipping it would make `freeze` silently a no-op for the one kind of state it exists to protect.

## Non-goals

- **Class variables and globals.** `@@a` and `$a` are still `Unsupported`. They are per-class and per-heap tables, not per-object storage, and neither shares this mechanism.
- **Ivars on objects with a native representation.** `@x` on an `Array`, `String`, `Proc`, `Regexp` or `MatchData` refuses with a reason naming the class. Ruby allows it; supporting it means giving those objects an ivar slot too, which costs a word on every array in the corpus to serve a case ruby/spec barely exercises. The refusal is honest and the upgrade is one constant per representation.
- **Ivar inline caches.** `docs/engine.md` describes a call-site cache keyed by shape. Reading an ivar walks the shape's parent chain today — a handful of links, since the chain is as long as the object has ivars. The benchmark that justifies the cache arrives with the JIT.
- **`remove_instance_variable`.** Removing a name means a shape transition backwards, which is a second tree edge and no corpus pressure.

## What ships

### `crates/spinel-vm/src/shape.rs` (new)

The per-heap shape tree. `Shapes` owns a `Vec<Shape>`; each node is a parent, the name it adds, the index that name lands at, the ivar count, and a map of child transitions. `transition` walks or extends the tree, `index_of` walks parents for a name, `names` reconstructs a shape's ivars in insertion order for `instance_variables`. No `Value`s, so it is not a mark root.

### `crates/spinel-vm/src/heap.rs`

`_reserved: [u8; 2]` becomes `shape: ShapeId`. `HandleScope` grows `shape`/`set_shape`, and `Heap` grows `shapes`/`shapes_mut` beside `classes`.

### `crates/spinel-vm/src/interp.rs`

`ivar_get`, `ivar_set`, `ivar_defined` and `ivar_names` over the shape tree, plus the storage growth. `Insn::GetIvar`/`SetIvar`/`DefinedIvar` handlers. Exceptions become ivar-bearing objects. `Hash` allocation sets three ivars. `Native::IvarReader`/`IvarWriter`/`AttrDefine`/`InstanceVariableGet`/`InstanceVariableSet`/`InstanceVariableDefined`/`InstanceVariables`.

### `crates/spinel-vm/src/compile.rs`

`Slot` — the local-or-ivar an assignment writes through — replaces the `(slot, depth)` pair, so `@a = 1`, `@a += 1`, `@a ||= 1` and `rescue => @e` all come out of the same three call sites that already handled locals. `defined?(@a)` stops refusing.

### `crates/spinel-vm/src/class.rs`

`SLOT_ID` becomes `@__id__`. `class_id_of` and `class_of` read it through the shape; the round-trip through `Classes::object` that rules out impostors is unchanged.

### `core/exception.rb`, `core/hash.rb`, `core/module.rb`

`message`/`to_s`/`backtrace` become Ruby. `Hash` reads `@pairs`, `@default`, `@default_is_proc`. `Module` gains nothing — `attr_accessor` is installed as a primitive on `Module` from Rust, because it defines methods.

### `crates/spinel-vm/tests/shapes.rs`

The definition of done's unit tests: same names in the same order share a shape, the same names in a different order do not, an index is stable across objects of one shape, storage growth preserves earlier values, and a collection through a chain of ivars keeps them all alive.

## Checks, and what they measured

```
             before                                        after
language/    2735 ·  626 passed · 0 failed · 2060 blocked   2735 ·  667 passed · 0 failed · 2019 blocked
corpus      25624 · 1144 passed · 0 failed · 22619 blocked  25624 · 1243 passed · 0 failed · 22520 blocked
```

`+41` in `language/`, `+99` across the corpus, and both instance-variable reasons are gone from the ranking entirely — not reduced, absent. `grep -i "instance variable"` over a full `scripts/spec.sh` run returns nothing.

The blocked count fell by 99 against 8,829 examples unblocked, which is #162's shape exactly and for its reason: a fixture is what an example is blocked on *first*, not only. What it bought is visible in the ranking rather than the totals. Before, one reason held 39% of everything blocked and the second-largest was 621. After, the largest is 850 and the top fifteen span 850 to 259 — the corpus finally says what is actually next.

Rust tests: `cargo test` green in debug and release, `cargo clippy --all-targets` clean. `shape.rs` has six unit tests over the tree in isolation; `crates/spinel-vm/tests/shapes.rs` has eight through the VM, including the two the issue's definition of done named and the collection test it predicted would be a test rather than a change. The prediction held — `Heap::mark` is untouched.

### Four failures the slice revealed, and what each one was

Unblocking 8,829 examples runs code that has never run. Four of them disagreed with Ruby, none in the shape tree, all pre-existing and none previously reachable. They are listed because "0 failed" is the invariant and each was fixed rather than tagged.

- **`dup` kept the singleton class, and shared the original's ivars.** `singleton_class_spec.rb` and `metaclass_spec.rb` both assert that a constant on an object's singleton does not survive `dup`; `dup_value` copied the header class, and an object's singleton *is* its header class. It now walks past the singleton. The shape and a *copy* of the storage travel with it, so `a.dup` and `a` do not write through each other — which nothing could have noticed before there were instance variables to write.
- **`lambda { |a,| }` accepted two arguments.** Prism spells the trailing comma `ImplicitRestNode` and the lowering folded it into a rest parameter, which made the arity unbounded. It is not a rest parameter: it turns off a block's auto-splat and leaves the arity what the named parameters say. `RestParam::implicit` and `ParamSpec::trailing_comma` separate the two, measured against CRuby for all four combinations of proc/lambda and one/two arguments.
- **`MatchData#inspect` answered the matched text.** It fell through to `Object#inspect`, which calls `to_s`, which is the whole match — a plausible String that is not what `inspect` means. `MatchData#names` is a new primitive over the group names the engine already recorded, and `inspect` is now eight lines of `core/match_data.rb` agreeing with CRuby on both the numbered and the named form.
- **`Float#<=>` gave up before asking `#coerce`.** Newly reachable because `attr_reader` inside `Class.new do ... end` is what the example is built from. Ruby asks a non-`Numeric` to coerce itself, and raises `TypeError: coerce must return [x, y]` when it misbehaves; both were measured against CRuby rather than read.

### One harness bug, which mattered more than any of them

`struct_group_spec.rb` puts its `before :all` inside `platform_is_not`, and `discover.rs` collected hooks only from `describe` bodies. The examples below it therefore ran with `@g` never assigned — `(@g == nil).should == false` failed against a `nil` that was the harness's doing, not Ruby's.

That is worse than a failure: the same hole would as happily have produced a *pass*. `Walk::nested` now stacks hooks for guard bodies and for any other block it descends into, which is the same treatment `describe` already got. This is the second time a harness oracle has needed to move with a slice, and it is the reason `spec/harness/tests/harness.rs`'s deliberately-blocked example is a class variable now rather than an instance variable.

### `eval.txt` measures the language, not the VM

`eval.rs` bootstrapped a heap without `core/*.rb`, so moving `Exception#message` into Ruby broke seven of its rows. The fix is one line — the test boots the core library, as `anonymous.rs` and `bytecode.rs` already do — and it is the right direction anyway: a table that measured the VM without its core library was measuring a language nobody runs. `spinel-core` was already a dev-dependency for exactly this.

### Editing `core/*.rb` did not rebuild the crate that embeds it

`spinel-core` pulls every `core/*.rb` in with `include_str!`, and Cargo does not track those paths: editing `core/hash.rb` rebuilt `spinel-cli` and left `spinel-core` alone, so the binary went on running the *previous* core library. It cost two debugging passes here — a method moved into Ruby simply did not exist — before it was believed rather than worked around with a `touch`.

`crates/spinel-core/build.rs` is five lines and one `cargo:rerun-if-changed`. It watches the directory rather than each file, so a new `core/*.rb` counts too. A stale core library that still builds and still runs is the worst shape a build bug can take, and it will bite harder as more of the language moves into Ruby.

### What is skipped, and why

- **Ivars on `Array`, `String`, `Proc`, `Regexp`, `MatchData`.** A slot per object to serve a case ruby/spec barely exercises. The refusal names the class, so the ranking will say when one starts to matter.
- **The inline cache keyed by shape.** `index_of` walks the parent chain, which is as long as the object has ivars. `docs/engine.md` describes the cache; the benchmark that justifies it arrives with the JIT.
- **`remove_instance_variable`.** A backwards transition, a second kind of tree edge, and no corpus pressure.
- **A shape id wider than `u16`.** 65,534 shapes per heap. Past that, `ivar_set` refuses and names #7, because widening the field pushes the header to 24 bytes and shifts every size class — a thing to decide with a workload rather than in advance.
