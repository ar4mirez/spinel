# PRD 0091 — `net/http`, and what it stands on

Issue: [#53](https://github.com/ar4mirez/spinel/issues/53) · Phase 3 ·
`area:stdlib`

## Objective

`require "net/http"` loads the vendored `net/http` — with `uri` and
`net/protocol` under it — and works over the `socket` of #61. Both were in
`stdlib/` since #47, but each stopped on something Spinel lacked: `io/wait`
as a feature, the `\g<name>` calls URI's RFC 3986 grammar is written in,
`autoload`, and the `/n` modifier.

## Design

**`io/wait`** is a built-in feature: in Ruby 4.0 its methods are `IO`'s own
(#61 wrote them), and requiring it still succeeds.

**Subexpression calls** (`\g<name>`, `\g'name'`, `\g<1>`) in `spinel-regex`.
The parser keeps a call as written until the whole pattern is read, since it
may call a group defined after it, then replaces it with a copy of the group
it calls — a capturing copy, so the group holds what the call matched when
the call matched last, as in Onigmo. A call that reaches its own group is
recursion, which a copy cannot express, and is refused. Every call the
standard library makes is to a group that does not recurse.

**`autoload`** (#39's): `Module#autoload`, `#autoload?`, `Kernel#autoload`
and `Kernel.autoload`, registering on the caller's module — a method's own
class, or the lexical scope. A constant the VM does not find is no longer a
`NameError` at once: `GetConst` calls `Module#__const_missing__`, in Ruby,
which requires an autoload registered for the name along the lexical scopes,
the ancestors and `Object`, looks again where the reference would have
looked, and failing that calls the program's own `const_missing` — which the
VM did not call at all before — and failing that has the VM raise the
`NameError` it always raised, so every message is unchanged. `const_get`,
`const_defined?` and `constants` see autoloads, `const_added` fires for one,
and an autoload of the file being required is ignored, as CRuby has it.

**`/n` and `/u`** (#222's): `/u` fixes a pattern to UTF-8; `/n` on an ASCII
pattern is one that matches in any ASCII-compatible encoding, says so in its
options and `inspect`, and is equal to the pattern without it.

`ponytail:` `/n` against a UTF-8 subject with bytes past ASCII matches
characters where CRuby matches bytes, warning; `/n` with non-ASCII bytes and
`/e` and `/s` stay refused — #222 and #19. A recursive call is refused (#33).

## Results

`library/net-http` and `library/uri` from blocked to **583** of 583 passing;
`core/module/autoload_spec.rb` and `core/kernel/autoload_spec.rb` to 67
passing and 0 failing, the rest of autoload — `defined?`, autoloads in
reopened classes — left to #39.

Corpus 26,355 to **26,980** passing, 0 failed: compared example by example
with `main`, 564 blocked examples pass, and none that passed fails. The
examples that newly run and fail are tagged: two for `/n` against a UTF-8
subject (#222), one for `Module#public` copying an inherited method (#28),
and one `linear_time?` example guarded for engines that are not CRuby's
(#33). Every pass agrees with ruby 4.0.6.
