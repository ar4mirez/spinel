# PRD 0025 — method visibility: `public`, `private`, `protected` in the method table

Issue: [#161](https://github.com/ar4mirez/spinel/issues/161) · Phase 1 · `area:engine`

## Objective

The method table had no notion of public, private or protected. Named as a gap
by #13 when it landed `defined?`, and reached again by #15, which needed it for
`Kernel#respond_to?`. Three things were wrong, and only the first was visible as
a failure:

- `defined?(obj.m)` answered `"method"` for a private method. Ruby answers `nil`.
- `defined?(Object.print)` could not be right either — `print` is private on
  `Kernel`, so Ruby answers `nil`.
- `respond_to?` ignored its `include_all` argument, so `respond_to?(:puts)` was
  true where Ruby says false.

## Baseline

Measured on this branch at e175753 — that is, after #164, whose unblocking is
what several of the numbers below move against.

| | |
|---|---|
| Rust tests | 275 passing · 0 failing |
| ruby/spec corpus | 25,624 examples · 1,577 passed · 0 failed |
| `verify-passes.rb` | 900 re-run on ruby 4.0.6, all agree |
| tags naming #161 | 2, in `language/def_tags.txt` and `language/defined_tags.txt` |

## Decisions

### Every rule was measured, and three of them were not what the docs suggest

`CLAUDE.md` answers behaviour questions with ruby/spec and then with CRuby.
Every rule here was run on ruby 4.0.6 first, and three came back different from
what a reading would have produced:

- **`method_defined?`'s second argument is `inherit`, not `include_all`.** The
  issue calls it `include_all`, which is `respond_to?`'s. They are different
  questions asked with the same shape — one selects on the ancestor chain, the
  other on visibility — and `lookup_inherited` keeps them apart.
- **`defined?(self.priv)` is `nil` while `self.priv` *runs*.** The `self.m`
  exception Ruby carved out for private calls is not carved out for `defined?`.
  So `visibility_refusal_at` takes a `self_receiver_ok` flag, and the two
  askers pass different values rather than one guessing the other's answer.
- **A top-level `def` is a private instance method of `Object`.** `def m; end;
  Object.new.m` raises. Missing that left `language/def_spec.rb`'s tagged
  example failing after the tag came off.

### Visibility rides on the `Method`, and `set_visibility` bumps the serial

#169's inline caches memoise `(ClassId, serial, Method)` and re-check two
integers per send. Putting visibility inside the memoised value keeps a warm
site free; the price is that `private :m` on an already-called method must bump
the class serial the way `define_method` does, or a site that called `obj.m`
while it was public keeps calling it — and so does every already-warm site in
the heap, which would look intermittent and receiver-dependent rather than
reproducible. `Classes::set_visibility` calls `invalidate`, and
`callcache.rs`'s `narrowing_visibility_misses` is the proof, written to the
shape of the existing `a_bumped_serial_misses`.

### The scope visibility is on the frame, not the lexical scope

A bare `private` sets what the `def`s below it get. It was on the `CrefNode`
first, which is wrong in a way only measurement finds:

```ruby
class A
  private
  [1].each { def in_block; end }   # private: a block shares the scope
  def outer; def nested; end; end  # public: a method body resets it
end
```

A method's cref *is* the class body's cref, so a cref-based rule made `nested`
private too. On the frame both lines are right: a method body starts public, and
a block takes the value from the frame its `home` link already names — which is
the frame it was written in, not the one that called it. `[1].each { }` calls
the block from inside `Array#each`, so "inherit from the caller" would have been
wrong in the other direction.

Two frames are class bodies rather than method bodies and start public
explicitly: `Insn::OpenClass`, and the block of `Class.new { }`, which Ruby
runs as a class body. Missing the second one gave `Class.new { attr_writer :a }`
a private setter and cost five examples.

### `module_function`, because visibility alone cannot explain `Kernel.puts`

Marking `puts` private on `Kernel` made `defined?(Object.print)` nil, and broke
`defined?(Kernel.puts)`, which Ruby answers `"method"`. Both are true at once
because `module_function` makes *two* definitions: a private instance method and
a public copy on the module's singleton. So `Module#module_function` is
implemented, with arguments, and `core/kernel.rb` uses it. The bare form sets
the mode for the rest of the body and needs a second piece of frame state; it
is refused rather than silently ignored.

### A hook that would fire is refused, not skipped

`private :m` on an *inherited* method defines it here, and Ruby fires
`method_added`; `module_function` fires `singleton_method_added`. Spinel has
neither hook (#28). Both go through the existing `hook_refusal`, so the two
`method_added_spec.rb` examples that check them are reported blocked rather than
failed — the answer the rest of this engine already gives for a hook it cannot
fire.

## Plan

1. `Visibility` on `Method` and in the method table; `set_visibility` invalidates. ✅
2. `private`/`public`/`protected`, bare and with arguments, returning the arguments. ✅
3. The dispatch check, with `self.m` and the protected family rule. ✅
4. `defined?` refuses what the site could not call, without #39's `Unknowable`. ✅
5. The `*_method_defined?` family, `method_defined?`'s `inherit`, `respond_to?`'s `include_all`. ✅
6. `public_send`, which refuses protected as well as private. ✅
7. `module_function`, and the `Kernel` module functions. ✅
8. `attr_*` take the current visibility; the scope visibility moves to the frame. ✅
9. Oracle rows, a cache-invalidation test, both tags removed. ✅

## Results

### ruby/spec delta

| | before | after |
|---|---|---|
| corpus passed | 1,577 | **1,606** |
| corpus failed | 0 | **0** |
| `verify-passes.rb` | 900 agree | **920 agree** |
| tags naming #161 | 2 | **0** |

`core/module/` 69 → 74, `core/kernel/` gained `public_send`, and both tagged
examples now pass rather than being skipped. `language/def_spec.rb` does not
regress.

### The definition of done

- [x] `Method` carries a visibility, set by `private`, `public`, `protected` —
      bare and with arguments — and by `private def m`, which returns the symbol
- [x] `Module#private_method_defined?`, `#public_method_defined?`,
      `#protected_method_defined?`, and `#method_defined?`'s second argument
- [x] `defined?` of a private method with a receiver is `nil`; the
      `defined_spec.rb` entry has left `spec/tags/`
- [x] `respond_to?` honours `include_all`
- [x] `core/module/` visibility specs, and `language/def_spec.rb` does not regress

`respond_to_missing?` is *not* done and is listed below rather than ticked:
nothing dispatches to it yet, so honouring `include_all` there has nothing to
honour.

### Two disagreements surfaced in subsystems this slice never set out to touch

Both were found by unblocking, both reproduced in a plain `.rb` file, and both
fixed here rather than tagged:

- **`break` in a lambda raised `LocalJumpError`.** `-> { break :v }.call` should
  be `:v`. A lambda is a method body for `return` and the `Links` said so, but
  `breaks` was left pointing at whatever passed the block. One line, and the two
  `break_spec.rb` examples behind it pass. They had been blocked because
  `fixtures/break.rb` calls `private`, so the fixture never loaded.
- **`public_send` reached protected and private methods.** `implicit_self:
  false` is not the same question: an ordinary `obj.m` still reaches a protected
  method from inside the family and a private one through `self`.
  `public_only` is the strict form and is now its own flag.

### Left for later

- **`respond_to_missing?`** — nothing dispatches to it, so `include_all` there
  is a parameter with no caller. It belongs with `method_missing` in #28.
- **Bare `module_function`** — refused, not ignored. Needs the same frame state
  the scope visibility uses, plus a branch in `Insn::DefineMethod`.
- **`private :m` on an inherited method copies the body** rather than storing a
  `ZSUPER` entry, so `B#m` does not track a later redefinition of `A#m`. Marked
  `// ponytail:` on `set_visibility` with the upgrade path.
