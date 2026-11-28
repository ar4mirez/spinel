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
fn integer_multiplication_division_and_modulo_floor_and_bail_past_the_range() {
    agree_compiled(
        "def ops(a, b) = [a * b, a / b, a % b, -a]\n\
         [ops(7, 2), ops(-7, 2), ops(7, -2), ops(-7, -2), ops(6, 3), ops(0, 5),\n\
          ops(4611686018427387903, 1), ops(-4611686018427387904, 1)]",
        80,
    );
    // Past the fixnum range: a Bignum, which the interpreter makes.
    agree_compiled(
        "def mul(a, b) = a * b\ndef div(a, b) = a / b\ndef neg(a) = -a\n\
         [mul(4611686018427387903, 2), mul(3037000499, 3037000499), mul(-2147483648, 2147483648),\n\
          div(-4611686018427387904, -1), neg(-4611686018427387904), neg(4611686018427387903)]",
        12,
    );
    // A zero divisor raises in the interpreter.
    agree_compiled(
        "def div(a, b) = a / b\ndef mod(a, b) = a % b\n\
         [(div(1, 0) rescue $!.message), (mod(1, 0) rescue $!.class), div(9, 4), mod(-9, 4)]",
        6,
    );
    // A loop of them, compiled through.
    agree_compiled(
        "def digits(n)\n  s = 0\n  while n > 0\n    s = s * 3 + n % 10\n    n = n / 10\n  end\n  s\nend\n\
         [digits(123456789), digits(4611686018427387903)]",
        200,
    );
}

#[test]
fn float_arithmetic_on_flonums_and_the_doubles_that_need_the_heap() {
    agree_compiled(
        "def ops(a, b) = [a + b, a - b, a * b, a / b, a < b, a <= b, a > b, a >= b, a == b, a != b, -a]\n\
         [ops(1.5, 2.25), ops(-3.0, 0.5), ops(2.0, 2.0), ops(1.0e10, 3.0e-5)]",
        60,
    );
    // Answers no flonum holds: zero, minus zero, an infinity, NaN, the
    // extremes. Bail-outs, to the interpreter's heap `Float`.
    agree_compiled(
        "def sub(a, b) = a - b\ndef mul(a, b) = a * b\ndef div(a, b) = a / b\ndef neg(a) = -a\n\
         [sub(1.5, 1.5), neg(sub(1.5, 1.5)), mul(1.0e200, 1.0e200), div(1.0, 3.0e-300),\n\
          mul(1.0e-200, 1.0e-200), (div(0.5, 0.0) rescue $!.class), neg(2.5), mul(-1.5, 0.0)]",
        8,
    );
    // Mixed operands, `%`, and Floats past the band are the interpreter's.
    agree_compiled(
        "def add(a, b) = a + b\ndef mod(a, b) = a % b\ndef lt(a, b) = a < b\n\
         [add(1, 2.5), add(2.5, 1), mod(7.5, 2.0), lt(1.0, 2), lt(1.0e300, 1.0e301), add(1.0e300, 1.0)]",
        2,
    );
    agree_compiled(
        "def norm(x, y, z)\n  i = 0\n  s = 0.0\n  while i < 100\n    s = s + x * x + y * y - z / 3.0\n    i += 1\n  end\n  s\nend\nnorm(1.5, 2.5, 0.75)",
        2_000,
    );
}

#[test]
fn a_redefined_float_or_integer_operator_is_heard_by_compiled_code() {
    agree_compiled(
        "def f(a, b) = [a * b, a / b, -a]\nbefore = [f(1.5, 2.0), f(6, 4)]\n\
         class Float\n  def *(o) = :times\n  def -@ = :neg\nend\n\
         class Integer\n  def /(o) = :over\nend\n[before, f(1.5, 2.0), f(6, 4)]",
        8,
    );
}

#[test]
fn symbols_floats_and_not_are_immediates() {
    agree_compiled(
        "def f(x) = [:a, :b, 2.5, -0.5, !x, !!x, x ? :yes : :no]\n\
         [f(nil), f(false), f(0), f(:s), f(1.5), f('str'), f([])]",
        20,
    );
    agree_compiled(
        "class Odd\n  def ! = :odd\nend\ndef f(x) = !x\n[f(Odd.new), f(nil), f(1)]",
        2,
    );
}

#[test]
fn a_blocks_enclosing_locals_are_read_and_written_in_place() {
    agree("def sum(a)\n  s = 0\n  a.each { |x| s = s + x }\n  s\nend\nsum([1, 2, 3, 40])");
    agree(
        "def nest(n)\n  t = 0\n  n.times { |i| n.times { |j| t = t + i * j } }\n  t\nend\nnest(20)",
    );
    agree(
        "def capture\n  x = 1\n  inc = -> { x = x + 1 }\n  get = -> { x }\n  inc.call\n  inc.call\n  [get.call, x]\nend\ncapture",
    );
    // A heap value written to an enclosing scope, then used there.
    agree(
        "def collect(a)\n  last = nil\n  a.each { |x| last = x }\n  [last, last.size]\nend\ncollect(['a', 'bb', 'ccc'])",
    );
    let stats = agree(
        "def total(n)\n  t = 0\n  i = 0\n  [1] .each { while i < n\n    t = t + i\n    i = i + 1\n  end }\n  t\nend\ntotal(1_000)",
    );
    assert!(
        stats.ran > 4_000,
        "the block's loop ran compiled: {stats:?}"
    );
}

#[test]
fn instance_variables_through_the_inline_caches() {
    let point = "class P\n  def initialize(x, y)\n    @x = x\n    @y = y\n  end\n\
                 def norm = @x * @x + @y * @y\n  def move(d)\n    @x = @x + d\n    @y = @y - d\n    self\n  end\n\
                 def x = @x\nend\n";
    agree_compiled(
        &format!("{point}p = P.new(3, 4)\n[p.norm, p.move(1).norm, p.move(1).move(2).norm, p.x]"),
        20,
    );
    // A second shape at the same sites, a heap value written, an ivar never
    // assigned, and a frozen receiver: each the interpreter's.
    agree_compiled(
        &format!(
            "{point}class Q < P\n  def initialize(x, y)\n    @z = 0\n    super\n  end\nend\n\
             a = P.new(1, 2)\nb = Q.new(1, 2)\n\
             r = [a.norm, b.norm, a.norm, b.move(3).norm, a.move(1.5).norm]\n\
             c = P.new('s', 't')\nr << (c.move(1) rescue $!.class)\n\
             f = P.new(1, 1).freeze\nr << (f.move(1) rescue $!.class) << f.norm\nr"
        ),
        20,
    );
    agree_compiled(
        "class C\n  def get = @never\n  def set(v)\n    @v = v\n    @v\n  end\nend\nc = C.new\n\
         [c.get, c.set(1), c.set(:a), c.set('s'), c.set(2.5), c.set(nil), C.new.set(nil)]",
        4,
    );
    // The first call fills the caches, at the bail-outs; the next run
    // compiled through.
    agree_compiled(
        "class Acc\n  def initialize\n    @n = 0\n  end\n  def run(k)\n    i = 0\n    while i < k\n      @n = @n + i\n      i += 1\n    end\n    @n\n  end\nend\n\
         a = Acc.new\n[a.run(1), a.run(500), a.run(10)]",
        3_000,
    );
    // Every kind of immediate is written in place: a flonum, a symbol, nil.
    agree_compiled(
        "class W\n  def initialize\n    @f = 0.0\n    @s = nil\n  end\n  def run(k)\n    i = 0\n    while i < k\n      @f = @f + 0.5\n      @s = :x\n      @s = nil\n      i += 1\n    end\n    [@f, @s]\n  end\nend\n\
         w = W.new\n[w.run(1), w.run(300)]",
        3_000,
    );
}

#[test]
fn a_heap_value_compiled_code_stores_in_a_local_outlives_a_collection() {
    // `x` is the only reference left to the String once `@a` is cleared, and
    // compiled code put it there; under `GC.stress` the next allocation
    // collects, before the loop has rooted anything else.
    agree_compiled(
        "class H\n  def initialize(s)\n    @a = s\n  end\n  def take\n    x = @a\n    @a = nil\n    y = 'q' * 64\n    [x, x.size]\n  end\nend\n\
         H.new('w' * 50).take\nh = H.new('z' * 100)\n50_000.times { |i| [i] }\n\
         GC.stress = true\nr = h.take\nGC.stress = false\n[r[0] == 'z' * 100, r[1]]",
        2,
    );
    // The same through an enclosing scope's local.
    agree_compiled(
        "def outer\n  o = 'k' * 80\n  r = nil\n  50_000.times { |i| [i] }\n  GC.stress = true\n  [1].each do\n    x = o\n    o = nil\n    y = 'q' * 64\n    r = [x == 'k' * 80, x.size]\n  end\n  GC.stress = false\n  r\nend\n\
         [outer, outer]",
        2,
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
