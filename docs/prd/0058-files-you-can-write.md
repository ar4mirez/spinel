# PRD 0058 — files you can write

Issue: [#41](https://github.com/ar4mirez/spinel/issues/41), first slice ·
pulled forward from Phase 3 · `area:runtime`

## Objective

The largest row in the blocked ranking was one missing method: 2,569
examples stopped at `File.umask`. It is the first thing mspec's `tmp` helper
calls, and `tmp` is how a spec gets a file to work on. Behind `umask` the
helpers want `Dir.mkdir`, `File.stat`, a `File.open` that can write,
`File.delete` and `Dir.rmdir`. This slice is that set: enough of #41 for a
spec to make a file, use it, and clean up.

## Baseline

Corpus 14314 passed, 0 failed. `core/file` 137 of 913, `core/io` 61 of 1595,
`core/dir` 3 of 332. A `File` was its whole contents read at open and a
position, and any write mode refused.

## Design

**A `File` is a descriptor.** `open(2)` with the flags its mode means, and
then `read`, `write`, `lseek` and `close` on what came back. Five
primitives, each one system call. Everything else is Ruby in `core/file.rb`.

**Reads are buffered, writes are not.** A read fills a buffer a block at a
time, which is what lets `gets` look for a separator without a system call
per byte. A write goes straight out. Because the descriptor is ahead of the
reader by whatever is buffered, `pos` subtracts the buffer, a relative
`seek` adjusts for it, and a write after a read seeks back first.

**`gets` has Ruby's rules**, measured: a limit, a nil separator, paragraph
mode with its leading blank lines skipped and its `chomp` taking only the
separating one, and a limit that lands inside a UTF-8 character taking the
rest of it, up to sixteen bytes. It sets `$_` and `$.`.

**Errors are the OS's.** Every primitive answers an Integer errno on
failure and something that is not an Integer on success, so a count or a
position comes back in a one-element Array. `File.__check__` raises
`SystemCallError.new(path, errno, function)`, which reads `No such file or
directory @ rb_sysopen - path` as CRuby's does. Until now the function name
was pasted into the message and the `@` was missing.

**Beside it:** `File::Stat` from one `stat` primitive that answers sixteen
Integers; `File.umask`, `delete`, `rename`, `symlink`, `readlink`, `chmod`,
`size`, `zero?`, `write`; `Dir.mkdir` and `rmdir`; `Kernel#open`, `printf`
and `putc`.

## The spec run cleans up after itself

An example that blocks part way never reaches its own cleanup, so once specs
could write files every run left a `rubyspec_temp/` in the working directory.
`scripts/spec.sh` now gives each mspec process a temp directory of its own
under `target/spec-tmp/` and removes it when the run ends. That needed
`File.realdirpath`, which mspec calls on the directory it is handed.

## Not in this slice

No write buffer, so `sync` is still only recorded. No transcoding between an
external and an internal encoding in either direction. No `flock`,
`truncate`, times on `File::Stat` (they need `Time.at`, #32), `IO.pipe`,
`IO.popen`, `IO.sysopen`, `Dir.open` or `File.fnmatch`. `open("|cmd")` is a
process (#43). Those are the rest of #41 and the next rows of the ranking.

## Results

| | passed | failed | blocked |
|---|---:|---:|---:|
| before | 14314 | 0 | 16347 |
| this slice | **15053** | **0** | 15561 |

53 examples that had been blocked now run and disagree, and are tagged with
the issue that owns each: 31 are `Dir.glob` edge cases that were always
there and could not be reached (#42), 14 are transcoding between an IO's
encodings (#41), 2 are directory entries in an internal encoding (#42), 2
are `require` through a symlink (#39), and one each a path's encoding (#41),
a write's encoding (#41, two examples) and a backtrace location for a file
since removed (#29).
