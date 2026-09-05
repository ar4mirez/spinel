# PRD 0019 — A failed dispatch raises a rescuable `NoMethodError`

Issue: [#170](https://github.com/ar4mirez/spinel/issues/170) · Phase 1 · `area:engine`

## Objective

Make a call to a method the heap does not have an ordinary Ruby raise, so
`rescue NoMethodError` catches it.

Today it is `Error::NoSuchMethod`, a Rust-level error that unwinds past every
Ruby handler and ends the program. `begin; obj.nope; rescue NoMethodError; end`
does not catch, where CRuby does. That is a compatibility bug — the fixed
decision in `CLAUDE.md` is that behaviour questions are answered by ruby/spec
and then by CRuby, never by what is convenient for the report.

It is also load-bearing for the corpus. The top two blocked reasons are both a
`NoMethodError` out of an mspec helper, 1,606 examples between them, and
`respond_to?`-shaped guards in ruby/spec routinely rescue one.

## Baseline

Measured on `fix/170-rescuable-nomethoderror` at 70304d7, before any change.

| | |
|---|---|
| Rust tests | 235 passing · 0 failing |
| ruby/spec | 3835 files · 25624 examples · 1248 passed · 0 failed · 22515 blocked · 1861 skipped · 4.1s |
| `rescue NoMethodError` on a missing method | does not catch; program ends |
| `NameError#name`, `NameError#receiver` | do not exist |
| Top blocked reason | 850 · `would raise NoMethodError: undefined method 'tmp' for an instance of Object` |
| Second | 756 · `would raise NoMethodError: undefined method 'mock' for an instance of Object` |

Three sync-conflict copies (`callcache 2.rs`, `inline_cache 2.rs`,
`0018-… 2.md`) were byte-identical to their originals and broke `cargo test`
outright — Cargo reads `tests/inline_cache 2.rs` as a target and rejects the
space in the crate name. Moved out of the tree, not deleted.

## Decisions

### The exception is built at the dispatch site, not in the interpreter loop

`Error::Raise` becomes an object in one place, the loop's `Err` arm, which is
why every site that emits one became catchable without being touched. A
`NoMethodError` cannot use that seam: it needs `@receiver`, and an `Error` holds
only `String`s. Putting a `Value` in an `Error` would leave the receiver
unrooted for the whole unwind, which is a collector bug waiting for the first
`rescue` that allocates.

`dispatch` already returns `Result<Option<Unwind>, Error>` and all four of its
callers turn `Some(unwind)` into `Step::Unwind`. So the miss builds the object
where the receiver is live and rooted, and returns `Ok(Some(Unwind::Exception))`.
No new plumbing, and the object is rooted by the scope from the moment it exists.

### `Error::NoSuchMethod` is deleted rather than kept alongside

Nothing else produced it, and keeping it would mean two ways to fail the same
way. Its doc comment argued *against* this change — that a catchable
`NoMethodError` lets `core/array/reject_spec.rb` swallow "Spinel has no
`reject!`" and carry on down a branch Ruby never takes. That argument is real
and this slice accepts the cost: it is the report getting worse in exchange for
the language getting right, and the fixed decisions rank those in that order.

What is *not* given up is the diagnostic. See below.

### The "core library is still minimal" hint moves to the uncaught case

The hint was attached to the error variant. With the variant gone it attaches to
the outcome the issue asked to keep it for: a `NoMethodError` that reached the
top level with no handler. `describe_error` keys on
`Error::Uncaught { class: "NoMethodError", .. }`.

This is strictly what the issue's second checkbox asks and nothing more. A
program whose own `NoMethodError` goes unhandled now also sees the hint, which
is correct — the wording is "this *may* be Spinel".

### The message is measured per receiver kind, not per class

CRuby does not have one message. Measured on ruby 4.0.6:

| receiver | message |
|---|---|
| `Object.new` | `undefined method 'nope' for an instance of Object` |
| `nil` | `undefined method 'nope' for nil` |
| `true` / `false` | `undefined method 'nope' for true` / `… for false` |
| `Object` | `undefined method 'nope' for class Object` |
| `Comparable` | `undefined method 'nope' for module Comparable` |

Spinel says `for an instance of {class}` for all five. Three of them are wrong
text a spec can assert on. The rows go in `crates/spinel-vm/tests/eval.txt`, so
`scripts/eval-oracle.rb` re-checks them against a real Ruby and no new oracle
script is needed.

Anonymous receivers are the one case not tabled: CRuby writes
`for class #<Class:0x000000010…>` and the address differs per run, so neither
Ruby nor Spinel can be held to it. Spinel keeps its existing
`an anonymous class` wording.

### `name` and `receiver` are Ruby, reading ivars the VM wrote

`core/exception.rb` already reads `@message` and `@backtrace` as ordinary Ruby.
`NameError#name` and `NameError#receiver` are two more of the same, over
`@name` and `@receiver`, which the dispatch miss writes.

`NameError.new` is refused (`# own initialize` in `exceptions.txt`), so the only
`NameError`s that exist are the VM's own — there is no user-constructed one
whose `receiver` should be CRuby's `ArgumentError`.

### Scope: dispatch only, not every `NameError`

The VM raises `NameError` for an uninitialized constant and for a bad ivar name
too, through `Error::raise`, which carries no name. Those keep `@name` unset and
answer `nil` where CRuby answers a symbol.

Fixing them means either a `Value` in an `Error` (rejected above) or threading
`Unwind` out of `const_base`/`uninitialized`, four call sites deep. The issue
asks for the dispatch failure; this slice does the dispatch failure and files
the rest.

## Plan

1. Move the three sync-conflict files out of the tree so `cargo test` builds.
2. Measure CRuby's message, `name`, and `receiver` per receiver kind.
3. `EXC_NAME` / `EXC_RECEIVER` ivar constants; `no_method_error` builds the object.
4. Dispatch miss returns `Ok(Some(Unwind::Exception(…)))`.
5. Delete `Error::NoSuchMethod`, its `Display` arm, and its doc comment.
6. `describe_error` keys the hint on uncaught `NoMethodError`.
7. `NameError#name` / `#receiver` in `core/exception.rb`.
8. Measured rows in `crates/spinel-vm/tests/eval.txt`; a CLI test for the hint.
9. `cargo test` (debug and release), `scripts/eval-oracle.rb --check`, `scripts/spec.sh`.

## Results

| | before | after |
|---|---|---|
| Rust tests | 235 · 0 failing | 236 · 0 failing (237 in debug) |
| ruby/spec passed | 1248 | **1291** (+43) |
| ruby/spec failed | 0 | **0** |
| ruby/spec blocked | 22515 | 22472 |
| `rescue NoMethodError` on a missing method | does not catch | catches |
| `NameError#name` / `#receiver` | do not exist | measured against CRuby |
| Oracles (`eval`, `exceptions`, `ancestors`, `anonymous`, `regexp`) | agree | agree |
| `scripts/verify-passes.rb` | — | 974 passing examples re-run on ruby 4.0.6, all agree |

### ruby/spec delta

Every directory that moved, and they sum to the +43 above:

| directory | before | after |
|---|---|---|
| `spec/ruby/language` | 670 | 688 (+18) |
| `spec/ruby/core/array` | 127 | 139 (+12) |
| `spec/ruby/core/module` | 51 | 60 (+9) |
| `spec/ruby/core/kernel` | 79 | 81 (+2) |
| `spec/ruby/library` | 4 | 6 (+2) |

### Verified by mutation, not just by green

Each half broken deliberately, with the narrowest check that should notice:

| mutation | caught by |
|---|---|
| `nil`/`true`/`false` wording collapses to "an instance of" | `eval.txt` |
| `@name` never written | `eval.txt` |
| `@receiver` never written | `eval.txt` |
| the miss is unrescuable again — the original bug | `eval.txt` |
| the gap is not recorded — the five swallowed failures return | `scripts/spec.sh` |
| the uncaught-`NoMethodError` diagnostic is dropped | `spinel-cli` tests |

A seventh was **not** caught, and that was the useful one. The first draft
answered the swallowed-gap problem twice: once generally, in `run`, and once
with a dedicated `NoMethodError` case in the `raises` matcher for the
unswallowed shape. Reverting the matcher case changed nothing — `0 failed` held
— because the general rule already converted that example. Two mechanisms where
one sufficed, and the mutation is what said so rather than review. Deleted; the
surviving rule is reworded so it is true whether the example caught the raise or
a matcher saw it.

The harness that ran these was wrong on its first attempt, in a way worth
naming: `set -o pipefail` with `cargo test … | grep -q` hands back *cargo's*
non-zero exit rather than grep's, so every detector inverted and all seven
mutations read as "not caught". A mutation suite that reports everything vacuous
is reporting on itself.

### All the passes that moved are re-run on CRuby

`scripts/verify-passes.rb` defaults to `language/` — 688 examples, which is not
the same as all 1291. Every directory this slice moved was run through it
explicitly:

| directory | passes re-run on ruby 4.0.6 |
|---|---|
| `language` | 688 · agree |
| `core/array` | 139 · agree |
| `core/module` | 60 · agree |
| `core/kernel` | 81 · agree |
| `library` | 6 · agree |

That matters more than usual here. 43 examples pass *while a missing method was
raised during them* — measured by temporarily reporting that combination as
blocked, which moved the count 1291 → 1248, exactly the slice's delta. Most are
specs that assert `NoMethodError` on purpose (`no_such_method`, `oops`); some
raise for a real gap (`sort_by!`, `binding`) and pass anyway because the
assertion did not depend on it.

The harness cannot tell those apart — a `should raise_error(StandardError)` that
Spinel satisfies with a `NoMethodError` and CRuby satisfies with something else
would pass on both. That is a limit of corpus-as-oracle rather than anything
this slice introduced, but re-running every moved directory on CRuby is the
check available, and it agrees.

### The unblock revealed five old failures, as expected

Turning a missing method into a catchable raise made five examples *fail* that
had been blocked — four in `core/array/` (`reject!`, `delete_if`, `fill`,
`map!`) and one in `core/kernel/define_singleton_method_spec.rb`. They are the
exact shape the deleted `Error::NoSuchMethod` doc comment named:

```ruby
a.reject! { |x| raise StandardError if x == 2; x == 1 }  # Spinel has no reject!
rescue StandardError                                     # ... which this eats
a.should == [1, 3, 4]                                    # ... so this fails
```

The failure is not a disagreement with Ruby — Ruby has `reject!`, so the example
never ran the code under test. `0 failed` is the corpus ratchet, so this had to
be answered rather than tolerated.

#### Answered by recording the gap, not by hiding it

`Heap::missing_method` records the first `NoMethodError` a dispatch raised for.
The VM does not try to tell a Spinel gap from a program's own `NoMethodError` —
it cannot — so it records that one happened and lets the harness decide. An
example that then **fails** is reported blocked, naming the method; an example
that **passes** is untouched, and `scripts/verify-passes.rb` re-runs those on
CRuby either way.

That is the same judgement `Error::Unknowable` encodes elsewhere: a failure a
missing method could explain is not evidence of a disagreement, so do not report
it as one.

The unswallowed shape — an example asking for `SyntaxError` that got
`undefined method 'eval'` — is the same rule and needs no second one. The first
draft added a dedicated case in the `raises` matcher for it; see the mutation
section for why it was deleted.

### Left for later

- `NameError#name` and `#receiver` on an *uninitialized constant* or a bad ivar
  name still answer `nil` where CRuby answers a symbol. Those raises go through
  `Error::raise`, which carries no name. Filed.
- `method_missing` is not consulted before the raise. Filed.
- The four `core/array/` examples above stay blocked until `reject!`,
  `delete_if`, `fill`, and `map!` exist. They are now blocked with the method
  named, which is what puts them in the report that chooses the next slice.
