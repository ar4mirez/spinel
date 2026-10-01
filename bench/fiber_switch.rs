//! What one fiber switch costs (#16), against system `ruby --yjit`.
//!
//! A fiber here is its own value stack and frame list, and `resume` /
//! `Fiber.yield` swap which pair the interpreter loop runs on — two
//! `mem::take`s and two moves, plus the Ruby wrappers in `core/fiber.rb` and the
//! sends that reach them. This times a round trip (one `resume`, one `yield`)
//! inside a Ruby loop, and subtracts the same loop with the fiber taken out, so
//! the number is the switching rather than the loop around it.
//!
//! Not criterion, for the reason `method_cache.rs` gives: the question is the
//! order of magnitude, and `Instant` answers it. Numbers go in the PR.

use std::process::Command;
use std::time::{Duration, Instant};

use spinel_vm::{Heap, Iseq, compile, interp};

/// Min of five runs: the time with nothing else interfering.
fn time(mut body: impl FnMut()) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let start = Instant::now();
        body();
        best = best.min(start.elapsed());
    }
    best
}

fn compiled(source: &str) -> Iseq {
    let parsed = spinel_parse::parse(source.as_bytes());
    assert!(parsed.errors.is_empty(), "the benchmark source parses");
    compile::program(&parsed.program).expect("the benchmark source compiles")
}

/// One evaluation on a fresh, booted heap. Boot is in the time; `main`
/// subtracts a run of an empty program to take it out.
fn run(iseq: &Iseq) -> Duration {
    time(|| {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        scope.bootstrap();
        spinel_core::boot(&mut scope);
        let mut frame = interp::Frame::new(iseq.locals.len());
        std::hint::black_box(interp::eval_in(&mut scope, &mut frame, iseq).expect("the loop runs"));
    })
}

const ROUND_TRIPS: u32 = 20_000;

fn main() {
    // The same loop twice: once switching, once calling a block instead, so
    // what differs is the switch.
    let switching = compiled(&format!(
        "f = Fiber.new {{ loop {{ Fiber.yield 1 }} }}
         i = 0
         while i < {ROUND_TRIPS}
           f.resume
           i = i + 1
         end"
    ));
    let baseline = compiled(&format!(
        "b = proc {{ 1 }}
         i = 0
         while i < {ROUND_TRIPS}
           b.call
           i = i + 1
         end"
    ));
    let boot_only = compiled("nil");

    let boot = run(&boot_only);
    let with_fiber = run(&switching).saturating_sub(boot);
    let with_call = run(&baseline).saturating_sub(boot);
    let per_trip = with_fiber.saturating_sub(with_call) / ROUND_TRIPS;

    println!("fiber round trip (resume + yield), {ROUND_TRIPS} of them");
    println!("  spinel      {per_trip:>9.1?} per round trip, net of a block call");

    // The same measurement on system Ruby with YJIT, as `CLAUDE.md` asks.
    let script = format!(
        "f = Fiber.new {{ loop {{ Fiber.yield 1 }} }}
         b = proc {{ 1 }}
         t = ->(&body) {{ (1..5).map {{ s = Process.clock_gettime(Process::CLOCK_MONOTONIC); body.call; Process.clock_gettime(Process::CLOCK_MONOTONIC) - s }}.min }}
         fiber = t.call {{ {ROUND_TRIPS}.times {{ f.resume }} }}
         call = t.call {{ {ROUND_TRIPS}.times {{ b.call }} }}
         per = (fiber - call) / {ROUND_TRIPS}
         puts format('  ruby --yjit %9.1fns per round trip, net of a block call', per * 1e9)"
    );
    match Command::new("ruby")
        .args(["--yjit", "-e", &script])
        .output()
    {
        Ok(out) if out.status.success() => print!("{}", String::from_utf8_lossy(&out.stdout)),
        _ => println!("  ruby --yjit (not available)"),
    }
}
