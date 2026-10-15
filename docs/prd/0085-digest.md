# PRD 0085 — `digest`

Issue: [#58](https://github.com/ar4mirez/spinel/issues/58) · Phase 3 ·
`area:gems`

## Objective

`require "digest"` and its algorithms behave as CRuby's, at the version
CRuby 4.0.6 ships (3.2.1). `library/digest/` was all blocked, and RubyGems,
Bundler and most of what checks a file's integrity hash with it.

## Design

**One primitive, the RustCrypto crates.** `Kernel#__digest__(algorithm,
string)` answers the digest of the bytes — MD5, SHA1, SHA256, SHA384,
SHA512, RMD160 — from `md-5`, `sha1`, `sha2` and `ripemd`, the boring
choice `CLAUDE.md` asks for.

**`lib/digest.rb`, in Ruby,** after `digest.c`: `Digest::Instance` gives any
object with `update`, `finish` and `reset` the rest — `digest` with and
without `!`, `hexdigest`, `base64digest`, `==` against a digest or a String
of either length, `file`, `new` — and names the missing method as CRuby
does. `Digest::Class` adds the one-call class methods, and `Digest::Base`
backs the six algorithms. `Digest::SHA2` and `Digest()` are upstream's Ruby
in shape. `digest/md5` and the rest load `digest`, and `digest/bubblebabble`
is `bubblebabble.c` in Ruby.

`ponytail:` an algorithm keeps what it was fed and digests it all at the
end, so its memory is the size of its input and `file` reads the whole
file; an incremental hasher behind the primitive is the upgrade.

## Results

`library/digest` 0 to **128** of 129, the last skipped by its guard.
Corpus 23,093 to **23,222** passing, 0 failed; compared example by example
with `main`, 113 blocked examples pass, and the frozen-string-literal one
#314 handed to #58 too, and none that passed fails.
