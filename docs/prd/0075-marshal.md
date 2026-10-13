# PRD 0075 — `Marshal`

Issue: [#37](https://github.com/ar4mirez/spinel/issues/37) · Phase 2 ·
`area:core-lib`, `area:engine`

## Objective

`Marshal.dump` and `Marshal.load` in CRuby's wire format, 4.8, so that what
one engine writes the other reads. `core/marshal/` goes from 217 examples that
could not start to passing.

## Design

**`core/marshal.rb`, in Ruby.** A `Writer` and a `Reader`, each one method per
tag, following `marshal.c` case by case — because the order in which objects
and symbols enter their tables is part of the format, and that order is in
the C and nowhere else. A `_dump` String's instance variables are numbered
before the object that produced it; a `marshal_dump` object is numbered before
its state. Each such case has a ruby/spec example and is written to it.

It asks an object nothing it could answer wrongly. The class, the instance
variables, `freeze` and `extend` are Kernel's own methods bound to the object,
so a BasicObject can be written and a class that defines `name`, `class` or
`freeze` is not believed. A class is written under its real name, and only if
that name still leads to it.

**Instance variables on a String, an Array, a Regexp.** The format needs them
— a dumped Time is a String with the zone on it, and the fixtures set one on
a String before any example runs — and those objects had nowhere to put one:
their payload is their own representation. The heap now keeps the storage for
such an object in a side table by address, a header flag marks the object,
and it wears a shape from then on. `docs/engine.md` has the paragraph. `dup`
and `clone` carry them across.

**What the core library keeps on an object is out of sight.** An exception's
message and backtrace were `@message` and `@backtrace`, and a Time's seconds
`@seconds`: all visible to `instance_variables`, and so all would have been
written into the stream as the user's. They are `@__message__` and so on now,
which `instance_variables` leaves out, as it does for the rest of the core
library. An exception made with no message keeps nil rather than its class
name — `to_s` answers the name — which is what CRuby writes.

**A Time** is `_dump`'s eight bytes and the pairs CRuby puts beside them:
`nano_num` and `nano_den`, `submicro`, `offset`, `zone`. Those names have no
`@`, so they are not instance variables in Ruby's sense; the String carries
them in a hidden one and the writer appends them. Read back, a Time keeps the
offset it was written with and the name its zone had.

**Gaps it walked into, closed here.** `a |= b`, `a &= b`, `a ^= b`,
`a <<= b`, `a >>= b` and `a **= b` on a variable or a constant were refused
by the compiler and are a send now. `Proc.new { }` answers the block. A
constant's name may hold characters past ASCII when looked up by name.

**And what `Proc.new` uncovered.** `core/method/fixtures/classes.rb` stopped
at its first `Proc.new` and took every `Method` spec that needed it down as
blocked. It loads now. One example then hung — a mock on
`respond_to_missing?` that `Kernel#method` never called — so `method` asks
it, as Ruby does, and answers a Method that goes through `method_missing`.
Eleven more examples in that directory ran for the first time and failed on
what a Method reports about itself — an `attr_reader`'s arity, the receiver's
class in `inspect` — and are tagged with what each is missing. The same
happened in four other files once a String could be given an instance
variable; five examples tagged there.

## Not done

- A Symbol has no encoding of its own (#19): one with characters past ASCII
  is written as UTF-8, and five examples that ask a loaded Symbol for its
  encoding cannot run.
- A Regexp does not keep a fixed or non-ASCII-compatible source encoding
  (#33). Four examples tagged.
- `d`, a wrapped C pointer, needs `Dir.allocate` to give something
  `initialize` can fill. The reader and writer have the case; the example is
  blocked.
- An exception's `name`, `key` and `receiver` travel as `@name` and so on,
  not under CRuby's names, and `bt_locations` is dropped on read.
- A Float's trailing mantissa bytes from a 1.8 dump are skipped; the digits
  before them already read back as the nearest Float.
- A Time outside 1900..67435 is refused rather than extended.
- `Random`, `StringIO`, `Mutex`, `ObjectSpace`, `Thread.current` and
  `remove_instance_variable` do not exist yet; one example each waits on them.

## Check

`scripts/spec.sh core/marshal`. Ten rows in `crates/spinel-vm/tests/eval.txt`
— every tag written and read, the tables, the errors and their messages, a
Time's pairs, instance variables on built-ins — measured on ruby 4.0.6. The
fixtures in `core/marshal/fixtures/marshal_data.rb` are bytes CRuby wrote,
and the round trip against them is what the load examples are.
