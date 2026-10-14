# PRD 0076 — `StringIO`, the first built-in gem

Issue: [#56](https://github.com/ar4mirez/spinel/issues/56) · Phase 3 ·
`area:gems`

## Objective

`require "stringio"` loads a `StringIO` that behaves as CRuby's does, at the
version CRuby 4.0.6 ships (3.2.0). `library/stringio/` goes from all blocked
to passing, and every spec elsewhere that captures output through a StringIO
can run.

## Design

**Libraries shipped inside the binary.** `docs/engine.md` names the built-in
gems — default gems whose C half becomes Spinel's — and nothing loaded one
yet. `lib/*.rb` holds their Spinel halves; `spinel_core` embeds them and
installs a lookup on the heap beside the parser. `require` asks it first:
a hit is loaded from the embedded source under the path
`<internal:stringio>` and recorded in `$LOADED_FEATURES` as `stringio.so`,
the name CRuby records for its extension. A second `require` answers false.
`load_file` takes the source as an optional fourth argument instead of
reading the path.

**`lib/stringio.rb`, in Ruby.** CRuby's StringIO is `ext/stringio/stringio.c`.
Here it is a class over a String, a byte position, a line number and which
sides are open, following the C function by function: modes as Strings,
Integer flags or keywords, a frozen String opening read-only, writes that
overwrite at the position and pad with NULs past the end, `ungetc` writing
back into the String, paragraph mode taking every newline that ends a
paragraph, limits that never split a character.

**`String#-@` and `#dedup`**, which StringIO's specs reached and the core
library lacked: a frozen String with the same content, the same object each
time.

## Not done

- A write copies the whole String: there is no `String#bytesplice` yet.
  Marked `ponytail:`.
- `-@` keeps what it interns for good, where CRuby's table is weak (#36). A
  frozen String *literal* is not the interned one; tagged.
- `getch` and `getpass` wait on `io/console`; five examples on `Thread`;
  sixteen on `format` conversions that `Kernel#format` does not have.

## Check

`scripts/spec.sh library/stringio`, and one row in
`crates/spinel-vm/tests/eval.txt` measured on ruby 4.0.6.
