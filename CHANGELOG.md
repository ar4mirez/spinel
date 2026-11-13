# Changelog

Notable changes to Spinel, newest first. There are no releases yet, so every
entry is under Unreleased. A change marked **Breaking** changes something a
tool outside the engine can see: a command's output, a file format, or the
bytecode a tool reads.

## Unreleased

### Added

- **The bytecode cache** ([#40](https://github.com/ar4mirez/spinel/issues/40)).
  Inside a project, a required file's compiled bytecode is kept in
  `.spinel/bytecode/<sha256 of source>.sbc` and read back on the next run
  instead of parsing. The file format is new and private to one build of
  Spinel: its header carries a fingerprint of the compiler, and a file
  written by another build is ignored and rewritten. `docs/cli.md` has the
  rest.

### Breaking: bytecode format

- **Superinstructions** ([#746](https://github.com/ar4mirez/spinel/issues/746),
  [#750](https://github.com/ar4mirez/spinel/issues/750)). `Insn` has five new
  variants: `GetLocalPushIntBinOp`, `GetLocalGetLocalBinOp`,
  `GetLocalGetIvarBinOp`, `GetLocalBinOp` and `PushSelfGetLocal`. Each is
  named for the run of instructions it stands for. Once a body is compiled,
  `bytecode::fuse` writes each over the first instruction of its run, and
  leaves the rest of the run in place after it, so no `pc` moves.
  - `spinel run --dump-bytecode` prints them, followed by the instructions they
    stand for.
  - A tool that reads `Iseq::insns` must read a superinstruction as its run's
    first instruction (`Insn::head`), or know the new variants.
  - Nothing on disk changed then, because the bytecode cache `docs/cli.md`
    describes was not built yet. Its files (#40) carry a fingerprint of the
    compiler, so a later change to the format makes them stale rather than
    wrong.
  - What each one answers is in `docs/engine.md` (Bytecode), and how to read
    the dump is in `docs/cli.md` (`--dump-bytecode`).
