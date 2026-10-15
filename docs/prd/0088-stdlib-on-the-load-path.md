# PRD 0088 — The vendored stdlib on the load path

Issue: [#47](https://github.com/ar4mirez/spinel/issues/47) · Phase 3 ·
`area:stdlib`

## Objective

`require "optparse"`, `"ipaddr"`, `"net/http"` — any file of the pure-Ruby
standard library vendored in `stdlib/` — works from the `spinel` binary
alone: no network, no gem install, no checkout beside it. Until now
`stdlib/` was in the tree but nothing put it on `$LOAD_PATH`, so every one
was a `LoadError`.

## Design

**Packed at build time.** `crates/spinel-cli/build.rs` packs every file
under `stdlib/`, sorted, into a length-prefixed archive, compresses it with
zstd (the `zstd` crate, level 19: the 6.7 MB tree becomes 0.8 MB of
binary), and writes a digest of it beside.

**Extracted once.** `crates/spinel-cli/src/stdlib.rs` extracts it on the
first run to `~/.spinel/stdlib/<version>-<digest>/`, or under
`$SPINEL_HOME` when that is set. The digest names the directory, so a binary
built from another `stdlib/` never reads this one's copy, and a later run
finds the directory and does nothing else. First runs that race each write a
staging directory of their own and `rename(2)` it into place: the copy
appears whole or not at all, and a run that loses the race takes the
winner's and removes its own. A home that cannot be written costs only the
stdlib.

**On the load path last**, after `-I` and `RUBYLIB`, where CRuby has its
`rubylibdir`.

Running the vendored files found two gaps, fixed here: `String.try_convert`
(optparse calls it), and an `Array#[]=` that panicked when it spliced
nothing into an Array that had never held anything.

`ponytail:` extraction writes to disk. An in-memory filesystem behind
`require`, `File.read` and `__dir__` would avoid it, at the cost of every
path primitive learning about it; the disk copy is what CRuby's own layout
looks like, so files that read their siblings work unchanged.

## Results

Checked by `crates/spinel-cli/tests/stdlib_runtime.rs`: a program requiring
`ipaddr` and `optparse` runs from a fresh `$SPINEL_HOME` and again from the
copy it left; eight first runs started at once all succeed and leave one
copy and no staging; a home that cannot be written still runs a program
that needs no stdlib.

Corpus 25,114 to **25,251** passing, 0 failed. Compared example by example
with `main`, 136 blocked examples pass — `library/socket`'s `ipaddr` ones,
`library/optparse`, `library/ipaddr` and others whose files only needed to
load — and none that passed fails. Making the stdlib loadable also made
`pp` and `rubygems` loadable to the fixtures that probe for them:
`require "rubygems"` is kept a LoadError until RubyGems can load (#319), and
the examples whose files now run surfaced four fixes, made here —
`system`'s `exception:` check, a shebang-less script run with `/bin/sh`,
`Socket.gethostbyaddr` refusing an address of the other family, and the
default separator of `String#lines` converted to a non-ASCII encoding — and
one tag, `Pathname#===`, for the builtin half of `Pathname` (#326). Every pass
agrees with ruby 4.0.6.
