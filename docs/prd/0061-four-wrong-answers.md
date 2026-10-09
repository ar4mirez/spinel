# PRD 0061 — four wrong answers

Issues: [#242](https://github.com/ar4mirez/spinel/issues/242),
[#173](https://github.com/ar4mirez/spinel/issues/173),
[#207](https://github.com/ar4mirez/spinel/issues/207),
[#203](https://github.com/ar4mirez/spinel/issues/203) · Phase 2 ·
`area:engine`, `area:core-lib`

## Objective

Four small issues with one thing in common: each is an answer that was wrong
rather than missing, so nothing refused and nothing was reported blocked. One
slice, because none of them is large and all of them were measured the same
way.

## Baseline

Corpus 16384 passed, 0 failed, on `main` after #281.

## What was wrong, and what it is now

**An Integer against a Float is compared exactly (#242).** The issue named
`<=>`. Measuring it found the operators wrong too, on the fixnum side:
`2**54 + 1 > (2**54).to_f` was false, because `binop` widened the fixnum to an
`f64` and the two landed on the same double. #238 had fixed this for a bignum
and left the fixnum arm as it was. `int_cmp_float` is the ordering that
`int_eq_float` was already the equality of: the float is split at its decimal
point, the integer parts are compared as integers, and the fraction breaks a
tie. `<=>` needed no change of its own — `Numeric#<=>` is written over `<`,
`>` and `==`, and is right once they are. NaN is still unordered.

**`String#match` is a send (#203).** `str.match(pattern)` is
`pattern.match(str)` in Ruby, so a Regexp whose `match` is its own is the one
asked. It is now a Ruby method that does exactly that, after making a Regexp
of a String or of what converts with `to_str`. Two things came with it:

- `Regexp.new` on a subclass answered a plain `Regexp`, so there was no
  subclass instance to ask. It answers one of the subclass now.
- `str =~ obj` is `obj =~ str` when `obj` is neither a Regexp nor a String,
  and `str.match?("b")` takes a String. Both refused before.

`match?` and `=~` against a Regexp do not dispatch, override or not. Measured:
the issue expected they might, and CRuby does not.

**A `NameError` knows its name and receiver (#173).** For an uninitialized
constant, an uninitialized class variable, and a name that is not an ivar or
class-variable name. An `Error` still holds no `Value`. The seam is the heap:
the raising site leaves the two values in a slot the collector marks, keyed by
the message, and `exception_new` — the one place an `Error::Raise` becomes an
object — takes them back. The key is what makes a note left by an error that
Rust swallowed harmless: a later raise with other text does not inherit it.

Measuring this found two more wrong answers, fixed here:

- A bare name that is neither a local nor a method raised `NoMethodError`
  with "undefined method". It is a `NameError`, "undefined local variable or
  method 'x' for main". The call site records that it was a bare name
  (`CallSite::variable_call`), which Prism already says.
- The message named `main` as "an instance of an anonymous class", and any
  object with a singleton class the same way. It says `main`, and looks past
  the singleton class.
- A bare constant missing inside a class names the class: `uninitialized
  constant K::Nope`.
- A private constant reached through a subclass names the class that holds
  it, in the message and as the receiver.

`FrozenError#receiver` rides the same note, with no name in it: the VM's five
frozen checks leave the object that refused, and the core library's own
raises pass `receiver: self`.

**An ivar name is converted before it is judged (#207).**
`instance_variable_get`, `_set` and `_defined?` take what answers `to_str`,
and a converted `"c"` is the `NameError` a literal `"c"` is. The conversion is
three Ruby wrappers over the primitives.

## Not done

- A `Regexp` subclass's `initialize` is not called by `new`: the pattern is
  compiled before there is an object. `// ponytail:` at the site.
- `NoMethodError#args` is its own tag and stays.
- A constant assigned in a frozen class is not refused at all, so it has no
  `FrozenError` to give a receiver to. Found measuring; not this slice.
- `e.name` for an ivar name that came through `to_str` is the converted
  String; CRuby's is too.

## Check

Rows in `crates/spinel-vm/tests/eval.txt` for each of the four, measured on
ruby 4.0.6. The ruby/spec delta is in the pull request.
