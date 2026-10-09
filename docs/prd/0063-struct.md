# PRD 0063 — `Struct`

Issue: [#30](https://github.com/ar4mirez/spinel/issues/30) · Phase 2 ·
`area:core-lib`

## Objective

`Struct` did not exist: 162 of `core/struct`'s 163 examples stopped at
`uninitialized constant Struct`, and so did every fixture elsewhere that
defines one.

## Baseline

Corpus 16461 passed, 0 failed (on the triage branch, PRD 0062). `core/struct`
1 of 163.

## Design

**It is Ruby.** `core/struct.rb`, with no new primitive. A struct's values
are an Array in a hidden instance variable, so `instance_variables` is empty
as Ruby's is, and its members are an Array on the class.

**`Struct.new` defines; the defined class's `new` instantiates.** In Ruby one
method does both, by receiver. Here `Struct.new` only ever defines a class,
and gives it a singleton `new` that allocates and calls `initialize`. A class
that inherits from a struct class inherits that `new`; a plain subclass of
`Struct` inherits the defining one, which is Ruby's behaviour for both.

**Members are found up the chain.** `class Car < Struct.new(:make)` has no
members of its own, so the class asks its ancestors for the first that was
defined as a struct.

**Values are made on first use.** A subclass may assign a member before its
`initialize` reaches `Struct#initialize`, which then sets every member — to
nil where nothing was given. Both are what ruby/spec checks.

**Keywords follow `keyword_init`.** `true`: keywords only, or one positional
Hash. `nil`, the default: keywords when keywords are all that was given,
positional otherwise. `false`: a keyword Hash is one positional value.

**Recursion.** A struct that holds itself inspects as `#<struct P:...>` and
compares equal to itself. Its hash is its class's alone, and so is the hash
of whatever struct the walk started from — CRuby's `rb_exec_recursive_outer`
— which is what gives two `eql?` recursive structs one hash.

**The name is the class's real one.** `inspect` does not call a `name` the
struct class defines for itself, and prints none for a class nested in an
anonymous one.

## Came with it

- `Kernel#<=>`, which every object has: 0 for itself or what it is `==` to,
  nil otherwise. It was missing, and `Struct#<=>` is it.
- `Module#<=>`. Without it the new `Kernel#<=>` would have answered nil for
  `Class <=> Module`, where Ruby says -1.
- `Integer.__index__` refuses a `to_int` that does not answer an Integer,
  with CRuby's message.

## Not done

- `Data` (`core/data`, 88 examples) is a separate class and a separate slice.
- Three `core/struct` examples stay blocked, none on `Struct`: `Module#dup`,
  `to_sym` on a non-UTF-8 String, and one whose expected message is ruby
  4.1's wording where this prints 4.0's.
- A redefined named struct warns at the line of `Struct.new`'s caller; the
  constant is replaced, not checked for equality first.

## Check

Rows in `crates/spinel-vm/tests/eval.txt`, measured on ruby 4.0.6. The
ruby/spec delta is in the pull request.
