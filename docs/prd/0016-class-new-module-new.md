# PRD 0016 — `Class.new`, `Module.new`: anonymous classes and modules

Tracks [#162](https://github.com/ar4mirez/spinel/issues/162). Milestone: Phase 1: a VM that runs `language/`. `P0`, `size:M`, `area:engine`.

## Objective

Make `Class.new` and `Module.new` real, so the corpus stops being blocked on a *fixture*.

The two are the largest remaining `allocate` refusals, and almost none of the examples behind them are reflection specs. `language/` and `core/` open an anonymous class to have something to test *on* — `c = Class.new { def foo; end }` — and then check something with nothing to do with `Class.new`. Every one of those is blocked at the first line.

## Baseline

Measured on `cf21fb6`, before this slice:

```
language/    2735 examples ·  619 passed · 0 failed · 2067 blocked ·   49 skipped
corpus      25624 examples · 1112 passed · 0 failed · 22652 blocked · 1860 skipped
```

The two reasons this slice removes:

```
             language/   corpus
 Class.new         63      266
 Module.new        21      231
             ---------   ------
                   84      497
```

`Class` is the third-largest blocker in `language/` after instance variables (482) and `eval` (134); together the pair is the largest reason corpus-wide that is a single missing operation rather than a whole subsystem.

## What Ruby actually does

Measured, not read — `scripts/anonymous-oracle.rb` regenerates `crates/spinel-vm/tests/anonymous.txt`, and every rule below is a line in it.

```ruby
Class.new.superclass          # Object
Class.new(String).superclass  # String
Class.new(Comparable)         # TypeError: superclass must be an instance of Class (given an instance of Module)
Class.new(o.singleton_class)  # TypeError: can't make subclass of singleton class
Class.new(Class)              # TypeError: can't make subclass of Class
Class.new(Object, Object)     # ArgumentError: wrong number of arguments (given 2, expected 0..1)
Module.new(Object)            # ArgumentError: wrong number of arguments (given 1, expected 0)
Class.new.name                # nil
Class.new.to_s                # "#<Class:0x000000010c0e4b80>"
Module.new.to_s               # "#<Module:0x000000010c0e4b80>"
Foo = Class.new; Foo.name     # "Foo"        — assignment names it
X1 = Class.new; X2 = X1       # X1.name is still "X1" — only the first assignment names
NS::Inner = Class.new         # "NS::Inner"  — the path, not the leaf
```

### The block is `module_eval`, and that is not one thing

`Class.new { ... }` runs the block with `self` set to the new class. What that changes, and what it does not, is the part a plausible implementation gets wrong:

| inside `Class.new(P) { ... }` | goes to | measured |
|---|---|---|
| `def foo` | the new class | `k.instance_methods(false) == [:foo]` |
| `[1].each { def foo }` | the new class | `[:foo]` — the definee reaches nested blocks |
| `CV` (a constant on `P`) | **NameError** | `class C < P` finds it; the block does not |
| `ASSIGNED = 1` | **top level** | `k.const_defined?(:ASSIGNED, false)` is false |
| `class DeepFoo; end` | **top level** | `DeepFoo.name == "DeepFoo"` |

So the *definee* moves to the new class and the *lexical scope* does not. Spinel had one `CrefId` doing both jobs.

### `inherited` fires before the block

```ruby
klass = Class.new(D) { ScratchPad << self }   # ScratchPad == [D, klass]
```

`core/class/new_spec.rb` calls this "runs the inherited hook after yielding the block"; the recorded order says otherwise, and `D.inherited` pushes `self`, so `D` is first. Confirmed separately: `instance_methods(false)` is empty inside the hook.

## Decisions

### The cref learns CRuby's `pushed_by_eval` flag

A `CrefNode` gains one `bool`. `def` reads the innermost node either way; constant lookup, constant assignment, and the `cbase` a bare `class Foo` defines into all skip nodes that carry it. That is the mechanism CRuby uses — `CREF_PUSHED_BY_EVAL` — and it reproduces all five rows of the table above without a second field on `Call`, without a ninth `Proc` slot, and without teaching blocks to carry a definee of their own.

The alternative considered was leaving the cref alone and adding `definee: Option<ClassId>` to `Call`. It is wrong at the third row (a nested block would lose the definee unless `Proc` grew a slot to capture it) and it leaves constant scope right only by accident.

This is the seam `module_eval`, `class_eval`, and `instance_eval` need in #28. It is not built for them — it is what `Class.new { }` needs — but it is deliberately the same flag.

### `inherited` refuses rather than firing

`Class.new(P)` where `P` has a user-defined `inherited` raises `Error::Unknowable`, and so does `class C < P`. It does not silently skip the hook.

This is #15's precedent for `singleton_method_added`, quoted in the issue: a program whose hook never runs reports a state it never reached, which is worse than reporting that the VM cannot get there yet. Firing it needs a primitive to push two frames in sequence — the hook, then the block — which this interpreter's natives cannot do; #28 owns hooks and can.

The refusal is added to `class C < P` in the same commit even though the issue only asks for `Class.new`. Both run through `define_or_reopen`, and a VM that refuses one path while silently skipping the other is harder to reason about than one that refuses both. It costs pass count: see *Checks*.

### `Class.allocate` keeps refusing

`Class.allocate` in Ruby answers a half-built class that raises `TypeError: uninitialized class` from `superclass` and `can't instantiate uninitialized class` from `new`. `Module.allocate` is a `NoMethodError`. Neither is what the 497 blocked examples want, and an uninitialised class is exactly the bare-object-wearing-a-class hazard #13 closed the door on. The refusal stays; only its `needs` text changes, because "before `Class.new`" is no longer true.

### `Module#to_s` grows an address, in Ruby

`#<Class:0x...>` needs an object id in the text, which #15 left alone rather than invent. `Object#object_id` is already a primitive and `Integer#to_s(16)` is already `core/integer.rb`'s, so the whole thing is four lines of Ruby in `core/module.rb` and no new native.

`Kernel#to_s` is `"#<" + self.class.name + ">"`, which raises `TypeError` the moment `self.class` is anonymous. It becomes `self.class.to_s`. The address is still missing from an *instance*'s `to_s` — `#<Foo>` where Ruby says `#<Foo:0x...>` — which is #15's gap, unchanged and not this slice's.

## Non-goals

- `Class#allocate` on `Class` or `Module` — uninitialised classes, above.
- Firing `inherited`, `included`, `extended`, `method_added` — #28.
- `module_eval` / `class_eval` / `instance_eval` as methods. This slice adds the cref flag they need and no surface.
- Naming anonymous classes reachable *through* a named one. Ruby does not either: `Outer = Class.new { Inner = Class.new }` leaves `Inner` at top level.
- An address in an instance's `to_s`.

## What ships

### `crates/spinel-vm/src/class.rs`

- `CrefNode.pushed_by_eval`, `Classes::push_eval_cref`, `Classes::cref_base`.
- `const_get` skips eval nodes, in the lexical walk and when choosing the innermost scope for the ancestor step.
- `Classes::name_if_anonymous` — the write behind `Foo = Class.new`.

### `crates/spinel-vm/src/interp.rs`

- `Native::New` answers `Class` and `Module` receivers before `allocate_instance` sees them: arity, superclass validation, `define_class`/`define_module` with no name, then the block as a frame whose `self` is the new class and whose cref is an eval cref.
- `const_base` and `open_class` resolve their cbase through `cref_base`.
- `Insn::SetConst` names an anonymous class or module.
- `inherited_refusal` on the two definition paths.
- `allocate_refusal`'s `needs` text for `Class` and `Module`.

### `core/module.rb`, `core/kernel.rb`, `core/object.rb`

`Module#to_s` for an anonymous module; `to_s` that does not raise on one.

### `scripts/anonymous-oracle.rb`, `crates/spinel-vm/tests/anonymous.txt`, `crates/spinel-vm/tests/anonymous.rs`

The measurement, the table, and the test that holds Spinel to it.

## Checks, and what they measured

```
             before                                    after
language/    2735 ex ·  619 pass · 0 fail · 2067 blk   2735 ex ·  626 pass · 0 fail · 2060 blk
corpus      25624 ex · 1112 pass · 0 fail · 22652 blk  25624 ex · 1144 pass · 0 fail · 22619 blk
```

The two reasons are gone from `language/` entirely, and all but three from the corpus:

```
                        language/       corpus
 `allocate` on Class     63 →  0       266 →  3
 `allocate` on Module    21 →  0       231 →  0
```

The three that remain are `core/class/allocate_spec.rb` asking for an *uninitialised* class, which is the question this slice deliberately left open.

Per directory:

```
core/class    2 → 10 passed        core/class/new_spec.rb    0/15 → 7/15
core/module  28 → 44 passed        core/module/new_spec.rb   0/4  → 2/4
                                   core/class/superclass_spec.rb  0/3 → 1/3
```

+32 corpus passes against 497 examples unblocked is the expected shape: most of those examples were blocked on `Class.new` *and* on something else, and removing the first blocker reveals the second. It shows in the ranking — instance variables 482 → 483, multiple assignment 102 → 109 — which is the point of the ranking.

`scripts/verify-passes.rb` re-ran all 626 `language/` and 514 `core/` passes on CRuby: all agree, so nothing here is a false pass.

### The slice found four silently-skipped hooks, and one wrong answer

`Class.new` made 9 examples run far enough to *fail* rather than be blocked. None was caused by this slice; each was a pre-existing bug that an anonymous-class fixture had been hiding. What they were, and what was done:

| examples | what | done |
|---|---|---|
| 4 | `const_added`, `included`, `prepended`, `prepend_features` silently skipped | refuse, like `inherited` |
| 1 | `rescue X` ignored a user-defined `X.===` | refuse |
| 2 | `(a.b = v)` answered what `b=` returned, not `v` | fixed |
| 1 | `Float#<=>` answered nil where `coerce` misbehaves | `spec/tags/skip.txt` |
| 1 | `const_added` on the `class A::C` path | refuse |

> Since [#146](https://github.com/ar4mirez/spinel/issues/146) this file is
> `spec/tags/<path>_tags.txt`, one `fails(reason):description` per line in
> mspec's own format. The entries named here moved with it, unchanged apart
> from reasons holding a parenthesis, which mspec's tag parser cannot carry.
> See `spec/tags/README.md`.

The hook refusals generalise `inherited`'s decision into one `hook_refusal` helper covering every site where Spinel would define something Ruby would announce. They cost 14 blocked examples corpus-wide and no passes, and they are the list #28 deletes.

`(a.b = v)` is Ruby's rule that an assignment-shaped call evaluates to the value assigned — `def b=(*) = 1` still makes `(a.b = "x")` answer `"x"`, while `a.send(:b=, "x")` answers 1. Prism already distinguishes them (`CallNode#is_attribute_write?`), so the fix is that flag plus stashing the value in a hidden `%attrN` local across the send. A stack rotate would have been tidier and is a new instruction; the local is not.

### What this slice found, and fixed, elsewhere

`class_name` — what Ruby's messages mean by "an instance of X" — was answering with a *singleton* class when the value had one: `Class.new(Comparable)` said "given an instance of `#<Class:Comparable>`". It had never shown, because nothing built a singleton for a module until `hook_refusal` did. Two fixes, both kept: `class_name` skips singletons the way `Object#class` does, and `hook_refusal` reads `singleton()` rather than `singleton_class()` so that asking whether a hook exists cannot create one.

`Kernel#to_s` and `Object#to_s` were `"#<" + self.class.name + ">"`, a `TypeError` on the first instance of an anonymous class. Now `self.class.to_s`.

### What is skipped, and why

`Float#<=> raises TypeError when #coerce misbehaves` is in `spec/tags/skip.txt`. It needs `Numeric#coerce` and the coerce-then-retry path in the numeric operators — a subsystem, not a special case in `<=>`.
