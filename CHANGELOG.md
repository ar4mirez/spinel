# Changelog

Notable changes to Spinel, newest first. There are no releases yet, so every
entry is under Unreleased. A change marked **Breaking** changes something a
tool outside the engine can see: a command's output, a file format, or the
bytecode a tool reads.

## Unreleased

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
  - Nothing on disk changes, because the bytecode cache `docs/cli.md`
    describes is not built yet. It will be keyed to this format.
  - What each one answers is in `docs/engine.md` (Bytecode), and how to read
    the dump is in `docs/cli.md` (`--dump-bytecode`).
