# PRD 0047 — Reflection, definition hooks, `define_method` and `method_missing`

Issue: [#28](https://github.com/ar4mirez/spinel/issues/28) · Phase 2 ·
`area:core-lib`

## Objective

`Module`/`Class` reflection as ruby/spec describes it: `define_method`,
`instance_eval`/`instance_exec`/`class_eval`/`class_exec`, `method_missing`
with `respond_to_missing?`, and the definition hooks — `method_added`,
`singleton_method_added`, `method_removed`, `method_undefined`, `inherited`,
`included`/`extended`/`prepended`, `append_features`/`extend_object`/
`prepend_features`, `const_added`. PRD 0038 gated the hooks on #16; this is
the half that waited.

## Baseline

Corpus 4300 passed, 0 failed, with #16 and #26. Every hook site in
`interp.rs` went through `hook_refusal`, which raised `Unknowable` when a
program defined the hook, so a class with an `inherited` was a blocked
example rather than a wrong one.

## Design: a hook is a frame

The interpreter does not recurse, and #16 showed that a native can push a
frame and return. A hook is the same: `fire_hook` looks the hook up on the
receiver and, unless the method found is the core library's default, pushes
its frame on top of the instruction that fired it, marked `discards_value`
so the definition's own answer — already on the stack below — is what the
instruction leaves. Several hooks from one instruction are pushed in reverse,
so they run in CRuby's order (`method_added` before a module function's
`singleton_method_added`, arguments left to right). No coroutine is needed:
`corosensei` stays reserved for primitives that must *read* a Ruby answer
mid-operation.

- **Defaults are real methods.** `core/module.rb`, `core/class.rb` and
  `core/basic_object.rb` define every hook as a private no-op, so reflection
  sees them and `super` from an override reaches one. `fire_hook` skips a
  default owned by `BasicObject`, `Module`, `Class` or `Kernel` — the common
  case, a class with no hook, costs one cached lookup. A hook the program
  `undef`s goes on to `method_missing` or a NoMethodError, as in CRuby; one
  that was never defined is the core library still loading.
- **Mixing in is Ruby.** `include`, `prepend` and `extend` check every
  argument, then call `append_features`/`prepend_features`/`extend_object`
  and `included`/`prepended`/`extended`, right to left; the splice is the
  `__include__`/`__prepend__`/`__extend__` primitive. Overriding
  `append_features` now decides the splice, as it does in Ruby. They loop
  with `while` over `size` and `[]` because the core library includes modules
  before `Array`'s Ruby half has loaded.
- **`define_method` is a `Definition::Proc`.** The body runs with the
  receiver as `self`, a method's arity and `return`, and the method's owner
  for `super`. The `*_eval`/`*_exec` family pushes an eval cref — the definee
  without being the lexical scope — so constants and class variables resolve
  where the block was written, and `def` inside lands on the singleton (or,
  for an Integer, Float or Symbol, raises CRuby's `can't define singleton`).
- **`method_missing`** is reached on a miss and on a visibility refusal when
  the program defines one; `respond_to?` consults a non-default
  `respond_to_missing?` through a frame marked `booleanizes_value`.
- **Singleton classes know what they are attached to**, which is what
  `singleton_method_*` hooks are sent to, what `Class#attached_object`
  answers, and how `to_s` prints `#<Class:#<Foo:0x…>>`. A singleton class's
  own metaclass is made on first dispatch, CRuby's `ENSURE_EIGENCLASS`, so
  `k.singleton_class` answers `K`'s class methods.

## Also in this slice

Reflection that ruby/spec reached once the hooks stopped blocking it:
`const_get` paths (qualified past the first segment, `"::"` refused),
`const_set` firing `const_added`, `Module.constants`, `constants` stopping at
`Object`, `deprecate_constant`'s check, `BasicObject::BasicObject`,
`Module.nesting`, `private_class_method`/`public_class_method`,
`remove_method` firing `method_removed`, `undef_method` of several names,
`Module#<`/`<=`/`>`/`>=` answering nil for unrelated modules, frozen checks on
`define_method`/`alias_method`/`undef_method`/`append_features`/
`extend_object`, `initialize`-family names private however they are defined,
`method_defined?(m, false)` meaning the class's own table, a module's
`alias`/`public` finding `Object`'s methods, class variables through a
singleton class and in definition order, `main` per heap with `to_s`,
`include`, `define_method`, `public` and `private`, `Kernel#Array`/`Hash`/
`String`/`sleep`/`__method__`/`__callee__`/`__dir__` as module functions,
`initialize_copy`'s checks, `instance_variables_to_inspect`'s type check, and
`Regexp#===` honouring `to_str` (in Ruby now) with block-less `grep` leaving
`$~` alone.

## Found on the way

- **`Kernel#is_a?` asked `self.class`** (#202): it now reads the object's
  real ancestry, singleton class first, so a module from `extend` counts and an
  overridden `class` does not. Closes #202.
- **A bare `super` dropped `**rest`.** zsuper forwarded named keywords only;
  it now forwards the keyword rest too, merged as a `**` argument would be.
- **`String#new` on a subclass skips a Ruby `initialize`** — tagged against
  #19, which owns String construction.
- **`Kernel#rand`/`#srand` have no issue**; filed #249 (`Random`).

## Tags

Eighty-six added and five removed, each naming the issue that owns the method: Kernel functions
that are IO (#41), process (#43), loading (#39), Binding (#38), `Random`
(#249), `Rational`/`Complex` (#34), `Float` (#181), `Integer` (#17), tracing
(#127) or signals (#44); `Proc#==` (#27); the predefined constants (#41, #44,
#65, #38); `Queue` (#46). Four `Module` examples read methods the module
fixtures define after an `autoload` of a fixture file (#39). The five removed
are #28's three and #202's two, whose examples pass.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/module` | 122 / 1021 | **443 / 1021** |
| `core/class` | 17 / 54 | **30 / 54** |
| `core/basicobject` | 14 / 108 | **65 / 108** |
| `core/kernel` | 168 / 1277 | **286 / 1277** |
| **corpus** | 4300 | **5011** |

0 failed; all passes re-run on ruby 4.0.7 agree.
