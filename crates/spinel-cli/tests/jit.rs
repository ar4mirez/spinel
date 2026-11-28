//! `SPINEL_JIT=force` (#120): `spinel run` compiles every body the first time
//! it runs, and a program prints what it prints without it. The fixture's
//! methods each leave compiled code a different way: at a send, an overflow
//! into a Bignum, a non-fixnum operand, a `NewArray` with six answers on the
//! stack, an exception, a turn check in a loop. That the JIT ran and how is
//! `spinel-jit`'s own differential test; this is the switch.

use std::path::Path;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn run(jit: Option<&str>) -> String {
    let program = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jit/program.rb");
    let mut command = Command::new(SPINEL);
    command.arg(&program).env_remove("SPINEL_JIT");
    if let Some(mode) = jit {
        command.env("SPINEL_JIT", mode);
    }
    let out = command.output().expect("spinel runs");
    let stdout = String::from_utf8(out.stdout).expect("utf-8");
    assert!(
        out.status.success(),
        "SPINEL_JIT={jit:?}\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

#[test]
fn a_program_prints_the_same_with_the_jit_forced() {
    let interpreted = run(None);
    assert!(
        interpreted.starts_with("2584\n1249975000\n4611686018427387904\n"),
        "{interpreted}"
    );
    assert_eq!(run(Some("force")), interpreted);
    // Only `force` turns it on; anything else is the interpreter alone.
    assert_eq!(run(Some("1")), interpreted);
}
