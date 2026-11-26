//! The Fiber scheduler's IO hooks, each handed an `IO::Buffer`, on the
//! `spinel` binary (#797).
//!
//! ruby/spec has examples for `Fiber.set_scheduler`, `Fiber.schedule` and
//! `kernel_sleep`, and none for what a scheduler is there for: `io_read`,
//! `io_write`, `io_wait`, `io_pread` and `io_pwrite`, called with a buffer
//! over the bytes an IO reads or writes, and the scheduler closed — so run —
//! when its thread or the program ends. These fixtures drive a scheduler
//! after the one CRuby's own tests use (`fixtures/fiber_scheduler/scheduler.rb`)
//! that logs each hook and what the buffer it got says about itself.
//!
//! Each expected output is what ruby 3.4.10 printed for the same fixture on
//! Linux, the newest CRuby on the machine that recorded them. Nothing these
//! hooks are asked changed in 4.0; the 4.0.9 run is left to whoever next
//! touches them on a machine that has it.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fiber_scheduler")
        .join(name)
}

fn run(name: &str, args: &[&std::ffi::OsStr]) -> Output {
    Command::new(SPINEL)
        .arg("run")
        .arg(fixture(name))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spinel runs")
}

fn stdout_of(output: &Output) -> String {
    assert!(
        output.status.success(),
        "status {:?}, stderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).expect("UTF-8 output")
}

/// A reader that waits on an empty pipe for a writer that sleeps first: the
/// reader's `readpartial` is `io_read` with a locked buffer of the five bytes
/// it asked for, whose `-EAGAIN` is `io_wait`; the writer's `write` is
/// `io_write` with a locked, read-only buffer over its bytes; `IO::Buffer#read`
/// and `#write` hand over the buffer itself; `read_nonblock` is `io_read` too,
/// as CRuby's `io_getpartial` makes it. `Fiber.blocking` hides the scheduler,
/// which is how the scheduler's own reads are made, and closing it runs both.
#[test]
fn the_io_hooks_get_buffers_over_what_is_read_and_written() {
    let output = run("io_hooks.rb", &[]);
    assert_eq!(
        stdout_of(&output),
        "[:scheduled]\n\
         [:write, 5]\n\
         [:buffer_write, 3]\n\
         [:readpartial, \"hello\"]\n\
         [:buffer_read, 3, \"abc\"]\n\
         [:read_nonblock, :wait_readable]\n\
         [:current, true, nil]\n\
         [:fiber]\n\
         [:io_read, IO, [IO::Buffer, 5, :locked?], 0, 0]\n\
         [:read, -11]\n\
         [:io_wait, IO, 1, nil]\n\
         [:fiber]\n\
         [:kernel_sleep, 0.01]\n\
         [:close]\n\
         [:io_write, IO, [IO::Buffer, 5, :locked?, :readonly?], 0, 0]\n\
         [:write, 5]\n\
         [:io_write, IO, [IO::Buffer, 3, :readonly?, :external?], 3, 0]\n\
         [:write, 3]\n\
         [:io_read, IO, [IO::Buffer, 5, :locked?], 0, 0]\n\
         [:read, 5]\n\
         [:io_read, IO, [IO::Buffer, 3, :internal?], 3, 0]\n\
         [:read, 3]\n\
         [:io_read, IO, [IO::Buffer, 10, :locked?], 0, 0]\n\
         [:read, -11]\n\
         nil\n"
    );
}

/// `IO#pread` and `#pwrite` are `io_pread` and `io_pwrite` at the file
/// offset asked for, and so are `IO::Buffer#pread` and `#pwrite`; `p` inside
/// the fiber is not `io_write`, since standard output that is not a terminal
/// holds what it writes, and its buffer is sent with `write(2)` itself.
#[test]
fn pread_and_pwrite_are_the_schedulers_at_the_offset_asked_for() {
    let dir = std::env::temp_dir().join(format!("spinel-fiber-pread-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let output = run("pread.rb", &[dir.as_os_str()]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        stdout_of(&output),
        "\"3456\"\n\
         2\n\
         4\n\
         \"6789\"\n\
         4\n\
         \"6789456789\"\n\
         [:fiber]\n\
         [:io_pread, File, [IO::Buffer, 4, :locked?], 3, 0, 0]\n\
         [:io_pwrite, File, [IO::Buffer, 2, :locked?, :readonly?], 1, 0, 0]\n\
         [:io_pread, File, [IO::Buffer, 4, :internal?], 6, 4, 0]\n\
         [:io_pwrite, File, [IO::Buffer, 4, :internal?], 0, 2, 0]\n\
         [:close]\n"
    );
}

/// A scheduler left set is closed when its thread's block returns, and the
/// main thread's before the `at_exit` blocks, which runs what each scheduled:
/// CRuby's `thread_do_start` and `rb_ec_fiber_scheduler_finalize`.
#[test]
fn a_scheduler_left_set_runs_when_its_thread_or_the_program_ends() {
    let output = run("closes_at_end.rb", &[]);
    assert_eq!(
        stdout_of(&output),
        "thread block done\n\
         thread fiber done\n\
         thread value :value\n\
         main script done\n\
         main fiber done\n\
         at_exit, scheduler nil\n"
    );
}
