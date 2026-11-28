//! The JIT against the interpreter (#120): the same programs, run once with
//! no JIT and once with it forced on, must answer the same.
//!
//! The interpreter is the reference, so nothing here says what a program
//! answers, only that the two agree. What each case adds is a path through
//! compiled code: a bail-out at a send, at an overflow, at a redefined
//! operator, at a non-fixnum operand, for want of fuel. The JIT's own counts
//! show compiled code ran, so agreement is not two interpreters agreeing.
//!
//! Skipped under Miri, which cannot run Prism's C or Cranelift's code.
#![cfg(not(miri))]

use spinel_vm::jit::Stats;
use spinel_vm::{Heap, compile, interp};

/// What `source` answers, rendered, with the JIT forced on or not, and what
/// the JIT did.
fn run(source: &str, jit: bool, budget: Option<u64>) -> (Result<String, String>, Option<Stats>) {
    let parsed = spinel_parse::parse_file("(differential)", source.as_bytes());
    assert!(parsed.errors.is_empty(), "{source:?}: {:?}", parsed.errors);
    let iseq = compile::program(&parsed.program).expect("compiles");
    let mut heap = Heap::new();
    let mut scope = heap.scope();
    scope.bootstrap();
    spinel_core::boot(&mut scope);
    if jit {
        spinel_jit::install(&mut scope);
    }
    if let Some(budget) = budget {
        scope.set_budget(Some(budget));
    }
    let mut frame = interp::Frame::new(iseq.locals.len());
    let answer = interp::eval_in(&mut scope, &mut frame, &iseq)
        .map(|value| interp::inspect(&mut scope, value))
        .map_err(|error| error.to_string());
    (answer, scope.jit_stats())
}

/// Run `source` both ways, check they agree, and answer the JIT's counts.
fn agree(source: &str) -> Stats {
    let (expected, none) = run(source, false, None);
    assert_eq!(none, None, "no JIT unless one is installed");
    let (got, stats) = run(source, true, None);
    assert_eq!(got, expected, "{source}");
    stats.expect("installed")
}

/// Agree, having run at least `at_least` instructions compiled.
fn agree_compiled(source: &str, at_least: u64) -> Stats {
    let stats = agree(source);
    assert!(
        stats.compiled > 0 && stats.ran >= at_least,
        "{source}\n  ran too little compiled: {stats:?}"
    );
    stats
}

const FIB: &str =
    "def fib(n)\n  if n < 2\n    n\n  else\n    fib(n - 1) + fib(n - 2)\n  end\nend\n";
const COUNT: &str =
    "def count(n)\n  i = 0\n  s = 0\n  while i < n\n    s = s + i\n    i += 1\n  end\n  s\nend\n";

#[test]
fn recursion_and_loops_run_compiled() {
    // `fib`'s test and branch are compiled, and it bails at each send.
    let stats = agree_compiled(&format!("{FIB}fib(15)"), 5_000);
    assert!(stats.entered >= 1_973, "a frame per call: {stats:?}");
    // The whole loop is compiled. It runs compiled until the first turn
    // check, a slice of 4,096 instructions, and the interpreter finishes it,
    // since compiled code is entered only at a body's first instruction.
    agree_compiled(&format!("{COUNT}count(10_000)"), 4_000);
    agree_compiled(
        "def depth(n) = n == 0 ? 0 : depth(n - 1) + 1\ndepth(3_000)",
        3_000,
    );
    agree_compiled(
        "def f(n)\n  i = 0\n  while true\n    i += 1\n    next if i < 5\n    break if i > n\n  end\n  i\nend\nf(100)",
        500,
    );
}

#[test]
fn an_answer_past_the_fixnum_range_bails_to_the_interpreters_bignum() {
    agree_compiled(
        "def inc(a) = a + 1\ndef dec(a) = a - 1\n\
         [inc(4611686018427387903), dec(-4611686018427387904), inc(1), dec(1)]",
        6,
    );
    // Overflow part-way through a compiled loop.
    agree_compiled(
        "def grow(x)\n  i = 0\n  while i < 70\n    x = x + x\n    i += 1\n  end\n  x\nend\ngrow(1)",
        300,
    );
}

#[test]
fn operands_that_are_not_two_fixnums_bail_with_their_operands() {
    agree_compiled(
        "def add(a, b) = a + b\ndef lt(a, b) = a < b\n\
         [add(1, 2), add('a', 'b'), add(1.5, 2), add([1], [2]), add(1, 2.5),\n\
          lt(1, 2), lt(1.0, 2), lt('a', 'b'), (lt(1, nil) rescue $!.class)]",
        6,
    );
}

#[test]
fn every_comparison_agrees_and_a_bail_writes_back_every_operand() {
    // The six answers are on the stack when `NewArray` bails.
    agree_compiled(
        "def cmp(a, b) = [a < b, a <= b, a > b, a >= b, a == b, a != b]\n\
         [cmp(1, 2), cmp(2, 1), cmp(2, 2), cmp(-5, 3), cmp(1, 'x')]",
        48,
    );
}

#[test]
fn a_redefined_operator_is_heard_by_compiled_code() {
    agree_compiled(
        "def plus(a) = a + 1\ndef lt(a) = a < 1\nbefore = [plus(1), lt(0)]\n\
         class Integer\n  def +(other) = 42\n  def <(other) = :less\nend\n\
         [before, plus(1), lt(0)]",
        4,
    );
    agree_compiled(
        "def same(a, b) = [a == b, a != b]\nbefore = same(1, 2)\n\
         class Integer\n  def ==(other) = true\nend\n[before, same(1, 2)]",
        4,
    );
}

#[test]
fn defaults_safe_navigation_and_short_circuits() {
    agree_compiled(
        "def opt(a, b = a + 1, c = b + 1) = [a, b, c]\n[opt(1), opt(1, 5), opt(1, 5, 9)]",
        6,
    );
    agree("def kw(a:, b: a + 2) = a + b\n[kw(a: 1), kw(a: 1, b: 3)]");
    agree_compiled(
        "def safe(x) = x&.succ\n[safe(nil), safe(1), safe(false)]",
        3,
    );
    agree_compiled(
        "def both(a, b) = a && b\ndef either(a, b) = a || b\n\
         [both(1, nil), both(nil, 1), both(1, 2), both(false, 1),\n\
          either(nil, 2), either(false, nil), either(1, 2)]",
        14,
    );
    agree_compiled(
        "def pick(a)\n  if a\n    1\n  elsif a.nil?\n    2\n  else\n    3\n  end\nend\n\
         [pick(true), pick(nil), pick(false), pick(0)]",
        4,
    );
}

#[test]
fn locals_holding_objects_move_as_the_interpreter_moves_them() {
    agree_compiled(
        "def swap(a, b)\n  t = a\n  a = b\n  b = t\n  [a, b]\nend\n\
         r = swap('x', [1])\nr[0] << 2\n[r, swap(nil, :s), swap(self, 1.5)]",
        10,
    );
    agree_compiled(
        "def chain(a)\n  x = y = a\n  [x, y]\nend\n[chain(1), chain('s')]",
        4,
    );
}

#[test]
fn blocks_exceptions_ensure_and_early_returns() {
    agree("[1, 2, 3].map { |x| x + 1 }");
    agree("def outer(k)\n  [1, 2].map { |x| x + k }\nend\nouter(10)");
    agree_compiled(
        "def early(n)\n  return n if n > 3\n  n + 10\nend\n[early(1), early(5)]",
        4,
    );
    agree(
        "def div(a, b) = a / b\nbegin\n  div(1, 0)\nrescue ZeroDivisionError => e\n  e.message\nend",
    );
    agree(
        "$log = []\ndef ens(a)\n  begin\n    a + 1\n  ensure\n    $log << a - 1\n  end\nend\n[ens(1), ens(2), $log]",
    );
    agree("def boom(a)\n  a + 1\n  raise 'no'\nend\n(boom(1) rescue $!.message)");
    agree(
        "def c(x)\n  case x\n  when 1 then :one\n  when 2 then :two\n  else :many\n  end\nend\n[c(1), c(2), c(3)]",
    );
}

#[test]
fn threads_take_turns_through_compiled_loops() {
    agree_compiled(
        &format!(
            "{COUNT}threads = (1..3).map {{ |k| Thread.new {{ count(20_000 * k) }} }}\n\
             [count(5_000), threads.map(&:value)]"
        ),
        10_000,
    );
}

#[test]
fn compiled_code_charges_the_budget_instruction_for_instruction() {
    // A loop that never ends is stopped by the budget either way.
    let spin = "def spin\n  i = 0\n  while true\n    i += 1\n  end\nend\nspin";
    assert_eq!(
        run(spin, false, Some(100_000)).0,
        run(spin, true, Some(100_000)).0
    );
    assert!(run(spin, true, Some(100_000)).0.is_err());

    // The fewest instructions a program needs is the same compiled: compiled
    // code runs one exactly when the interpreter would have.
    let source = format!("{COUNT}count(300)");
    let passes = |jit: bool, budget: u64| run(&source, jit, Some(budget)).0.is_ok();
    let (mut low, mut high) = (1u64, 1_000_000u64);
    assert!(passes(false, high));
    while low < high {
        let middle = u64::midpoint(low, high);
        if passes(false, middle) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    assert!(
        passes(true, low),
        "the JIT needs more than the {low} the interpreter does"
    );
    assert!(
        !passes(true, low - 1),
        "the JIT ran in fewer than the {low} the interpreter needs"
    );
    let (_, stats) = run(&source, true, Some(low));
    assert!(stats.unwrap().ran > 2_000);
}

/// The measured table `spinel-vm`'s own end-to-end test holds the interpreter
/// to, with the JIT forced on: every row's methods, and the core library's
/// methods each calls, compiled on first entry.
#[test]
fn the_eval_table_answers_the_same_with_the_jit_forced() {
    const TABLE: &str = include_str!("../../spinel-vm/tests/eval.txt");
    let mut checked = 0;
    let mut ran = 0;
    let mut wrong = Vec::new();
    for (index, line) in TABLE.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (source, want) = line.split_once("  #=> ").expect("a measured row");
        checked += 1;
        let (got, stats) = run(source, true, None);
        ran += stats.expect("installed").ran;
        if got.as_deref() != Ok(want) {
            wrong.push(format!(
                "eval.txt:{}: {source}\n  ruby: {want}\n  jit:  {got:?}",
                index + 1
            ));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    assert!(checked > 1_500, "the table lost its rows: {checked}");
    assert!(
        ran > 100_000,
        "compiled code barely ran: {ran} instructions"
    );
}
