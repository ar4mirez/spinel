# Known answers for the built-in bigdecimal (#109). Each line is a label and
# what the expression gave; cases.out is what ruby 4.0.9 (bigdecimal 4.0.1)
# printed, written by make.rb, and tests/bigdecimal.rs checks Spinel prints
# the same. A BigDecimal is shown as its inspect, its sign and its _dump, so
# the digits, the sign of a zero and the word count bigdecimal.c keeps are
# all compared.
require "bigdecimal"
require "bigdecimal/util"
require "bigdecimal/math"

def show(value)
  case value
  when BigDecimal then "#{value.inspect} sign=#{value.sign} dump=#{value._dump}"
  when Array then "[" + value.map { |v| show(v) }.join(", ") + "]"
  when Float then value.nan? ? "NaN" : "#{value.inspect} (#{[value].pack("G").unpack1("H*")})"
  when String then value.inspect
  else value.inspect
  end
end

def check(label)
  result = begin
    show(yield)
  rescue Exception => e
    "#{e.class}: #{e.message}"
  end
  puts "#{label} => #{result}"
end

def bd(text) = BigDecimal(text)

MODES = [:up, :down, :half_up, :half_down, :ceiling, :floor, :half_even].freeze

# --- construction ----------------------------------------------------------

%w[1 -1 0 -0 1.5 -0.001 123.456e7 1e-20 .5 1. 0e10 1_000.5 +Infinity -Infinity NaN
   #12 3.14159265358979323846264338327950288419716939937510].each do |text|
  check("BigDecimal(#{text.inspect})") { bd(text) }
end
["1.5e", "1__2", "1_.5", "abc", " 1 2", "1\0", "a\tb\"c", "1e99999999999999999999", "1e-99999999999999999999"].each do |text|
  check("BigDecimal(#{text.inspect})") { bd(text) }
  check("BigDecimal(#{text.inspect}, exception: false)") { BigDecimal(text, exception: false) }
end
["1.5e", "abc", "12abc", "1.2.3"].each do |text|
  check("interpret_loosely(#{text.inspect})") { BigDecimal.interpret_loosely(text) }
end
check("BigDecimal(nil)") { BigDecimal(nil) }
check("BigDecimal(Object.new)") { BigDecimal(Object.new) }
check("BigDecimal(Complex(1, 2))") { BigDecimal(Complex(1, 2)) }
check("BigDecimal(Complex(1.5, 0))") { BigDecimal(Complex(1.5, 0)) }
check("BigDecimal(1, -1)") { BigDecimal(1, -1) }
check("BigDecimal.allocate") { BigDecimal.allocate }
check("BigDecimal.new") { BigDecimal.new("1") }
check("frozen, no ivars") { [bd("1").frozen?, bd("1").instance_variables, bd("1").dup.equal?(bd("1"))] }

# --- Integer, Float and Rational -----------------------------------------------

[0, 7, -42, 10**30, -(2**70), 999_999_999, 1_000_000_000].each do |n|
  check("BigDecimal(#{n})") { BigDecimal(n) }
end
[0.1, 1.0 / 3, -2.5, 1e-20, 2.0**70, 0.1 + 0.2, 5e-324, -0.0, 1.7976931348623157e308].each do |f|
  check("BigDecimal(#{f.inspect})") { BigDecimal(f) }
  [1, 3, 10, 16, 17].each { |digits| check("BigDecimal(#{f.inspect}, #{digits})") { BigDecimal(f, digits) } }
end
# Ruby's dtoa rounds these up from its floating-point estimate.
[[2.675, 3], [0.15, 1], [9.995, 3], [1.005, 3], [0.125, 2], [1.45, 2]].each do |f, digits|
  check("BigDecimal(#{f}, #{digits})") { BigDecimal(f, digits) }
end
check("BigDecimal(Float::NAN)") { BigDecimal(Float::NAN) }
check("BigDecimal(-Float::INFINITY)") { BigDecimal(-Float::INFINITY) }
[Rational(1, 3), Rational(-22, 7), Rational(99999999999999999999, 100)].each do |r|
  check("BigDecimal(#{r})") { BigDecimal(r) }
  [0, 5, 40].each { |digits| check("BigDecimal(#{r}, #{digits})") { BigDecimal(r, digits) } }
end
check("to_d") { [1.to_d, 1.5.to_d, 0.1.to_d(3), "3.14xyz".to_d, Rational(1, 3).to_d(10), nil.to_d, Complex(2, 0).to_d] }
check("coerce") { [bd("1.5").coerce(2), bd("1.5").coerce(2.5), bd("1.5").coerce(Rational(1, 3))] }
check("coerce(String)") { bd("1").coerce("1") }

a = bd("1.23456789012345678901234567890")
[3, 0.5, Rational(1, 3), 10**40, -7].each do |b|
  check("#{a} op #{b.inspect}") { [a + b, a - b, a * b, a / b, b + a, b - a, b * a, b / a] }
end
check("BigDecimal + nil") { bd("1") + nil }
check("BigDecimal + Complex") { bd("1") + Complex(1, 2) }
check("to_i, to_f, to_r") { [a.to_i, a.to_f, a.to_r, bd("-1e25").to_i, bd("1e400").to_f, bd("-1e-400").to_f] }
check("Float(), Rational()") { [Float(a), Rational(bd("1.5")), Rational(bd("1.5"), 2)] }
check("Integer(NaN)") { bd("NaN").to_i }
check("Integer(Infinity)") { bd("-Infinity").to_i }

# --- arithmetic at a precision ---------------------------------------------------

x = bd("3.14159265358979323846")
y = bd("2.71828182845904523536")
[1, 5, 10, 25, 40].each do |digits|
  check("add/sub/mult/div #{digits}") { [x.add(y, digits), x.sub(y, digits), x.mult(y, digits), x.div(y, digits)] }
end
check("div 0") { [x.div(y, 0), bd("1").div(3, 0), bd("2").div(bd("3"), 0)] }
check("1/3, 2/3, 1/7") { [bd("1") / 3, bd("2") / 3, bd("1") / 7, bd("-1") / 7] }
check("quo") { [bd("1").quo(3), bd("1").quo(3, 5), bd("1").quo(3, 0)] }
check("div(Integer)") { [bd("7").div(2), bd("-7").div(2), bd("7.5").div(bd("-2"))] }
check("% remainder divmod") do
  [[7, 3], [-7, 3], [7, -3], [-7, -3], ["5.5", "1.25"], ["-5.5", "1.25"]].map do |p, q|
    [bd(p.to_s) % bd(q.to_s), bd(p.to_s).remainder(bd(q.to_s)), bd(p.to_s).divmod(bd(q.to_s))]
  end
end
check("divmod by 0") { bd("1").divmod(0) }
check("% by Infinity") { [bd("1") % BigDecimal::INFINITY, bd("-1") % BigDecimal::INFINITY, bd("-1").divmod(BigDecimal::INFINITY)] }

# --- rounding ---------------------------------------------------------------------

values = %w[2.5 -2.5 3.5 1.15 1.25 -1.35 0.0049 123.456 -0.5 9.999]
values.each do |text|
  v = bd(text)
  check("round #{text}") { MODES.map { |m| v.round(1, m) } + MODES.map { |m| v.round(0, m) } }
  check("round #{text} defaults") { [v.round, v.round(1), v.round(-1), v.round(half: :even), v.round(half: :down), v.round(2, half: :up)] }
  check("floor/ceil/truncate #{text}") { [v.floor, v.ceil, v.truncate, v.floor(1), v.ceil(1), v.truncate(1), v.fix, v.frac] }
end
check("round far left") { [bd("1").round(-9, :up), bd("1").round(-10, :up), bd("1").round(-10, :ceiling), bd("-1").round(-10, :floor)] }
check("round to zero is Rational") { [bd("1e10").round(-20), bd("1e10").round(-20, :half_up).to_i, bd("0e10").to_i, bd("0e5").to_i] }
check("round errors") { [(bd("1").round(1, :nope) rescue $!.message), (bd("1").round(half: :nope) rescue $!.message), (bd("1").round(1, 9) rescue $!.message)] }
MODES.each do |mode|
  check("mode #{mode}") do
    BigDecimal.save_rounding_mode do
      BigDecimal.mode(BigDecimal::ROUND_MODE, mode)
      [bd("2") / 3, bd("-2") / 3, bd("1").div(7, 4), bd("2.5").round, bd("-2.5").round, x.mult(y, 4), x.add(y, 4)]
    end
  end
end

# --- the precision limit ------------------------------------------------------------

check("limit") do
  BigDecimal.save_limit do
    old = BigDecimal.limit(5)
    [old, BigDecimal.limit, x + y, x - y, x * y, x / y, x.add(y, 10), x.mult(y, 10), x + 0, bd("1") / 3, x.sqrt(0)]
  end
end
check("limit restored") { BigDecimal.limit }
check("limit(-1)") { BigDecimal.limit(-1) }

# --- sqrt, power and BigMath ----------------------------------------------------------

check("sqrt") { [bd("2").sqrt(10), bd("2").sqrt(50), bd("0.0001").sqrt(5), bd("1e100").sqrt(3), bd("0").sqrt(5), bd("Infinity").sqrt(5)] }
check("sqrt(-1)") { bd("-1").sqrt(10) }
check("power") { [bd("2") ** 10, bd("2") ** -2, bd("1.1") ** 3, bd("2").power(100, 10), bd("2") ** 0.5, bd("-2") ** 3, bd("0") ** 0, bd("10") ** 1000000000000000000] }
check("BigMath.PI(50)") { BigMath.PI(50) }
check("BigMath") { [BigMath.E(30), BigMath.log(bd("2"), 30), BigMath.exp(bd("1"), 30), BigMath.sqrt(bd("3"), 30), BigMath.sin(bd("1"), 20), BigMath.cos(bd("1"), 20), BigMath.atan(bd("1"), 20)] }

# --- printing -------------------------------------------------------------------------

[bd("1234567.891"), bd("-0.00012345"), bd("12345678901234567890.123"), bd("1e20"), bd("0.5"), bd("-0"), bd("Infinity"), bd("NaN")].each do |v|
  check("to_s #{v.inspect}") { [v.to_s, v.to_s("F"), v.to_s("3F"), v.to_s("+5E"), v.to_s(" 2F"), v.to_s(4), v.inspect, v.split, v.to_digits] }
end
check("to_s(0)") { bd("1").to_s(0) }
check("encodings") { [bd("1.5").to_s, bd("1.5").to_s("F"), bd("1.5").inspect, bd("1.5").split[1], bd("1.5")._dump].map { |s| s.encoding.to_s } }
check("exponent, precision, scale") do
  %w[0 1 1.5 -1e20 1e-20 123.456 0.00012345 Infinity NaN].map do |t|
    v = bd(t)
    [v.exponent, v.precision, v.scale, v.precision_scale, v.n_significant_digits]
  end
end

# --- NaN, Infinity and the exception modes ---------------------------------------------

nan = BigDecimal::NAN
inf = BigDecimal::INFINITY
check("specials") { [nan + 1, inf + 1, inf - inf, inf * 0, bd("1") / 0, bd("-1") / 0, bd("0") / 0, bd("1") / inf, -inf, inf.abs] }
check("special predicates") { [nan.nan?, inf.infinite?, (-inf).infinite?, bd("1").infinite?, nan.finite?, bd("0").zero?, bd("1").nonzero?, bd("-0").sign] }
check("special compare") { [nan == nan, nan <=> 1, inf > bd("1e1000"), -inf < 0, inf == Float::INFINITY, (nan < 1)] }
check("compare") { [bd("1") == 1, bd("1") == 1.0, bd("0.1") == 0.1, bd("1") <=> "x", [0] == [bd("0")], bd("1").eql?(1), bd("1").hash == bd("1.0").hash] }
check("compare with String") { bd("1") < "x" }
[BigDecimal::EXCEPTION_NaN, BigDecimal::EXCEPTION_INFINITY, BigDecimal::EXCEPTION_ZERODIVIDE, BigDecimal::EXCEPTION_UNDERFLOW].each do |flag|
  check("exception mode #{flag}") do
    BigDecimal.save_exception_mode do
      BigDecimal.mode(flag, true)
      [-> { nan + 1 }, -> { inf + 1 }, -> { bd("1") / 0 }, -> { bd("0") / 0 }, -> { bd("NaN") },
       -> { bd("1") * inf }, -> { bd("1") / inf }, -> { bd("1e-1000000000000000000") * bd("1e-1000000000000000000") },
       -> { bd("1e-400").to_f }, -> { inf.round(1) }].map do |f|
        begin
          show(f.call)
        rescue Exception => e
          "#{e.class}: #{e.message}"
        end
      end
    end
  end
end
check("mode reads") { [BigDecimal.mode(BigDecimal::EXCEPTION_ALL), BigDecimal.mode(BigDecimal::ROUND_MODE)] }
check("mode errors") { [(BigDecimal.mode(BigDecimal::EXCEPTION_NaN, 1) rescue $!.message), (BigDecimal.mode(0x200) rescue $!.message), (BigDecimal.mode(BigDecimal::ROUND_MODE, :nope) rescue $!.message)] }
check("overflow") { [bd("10") ** (2**62), bd("0.1") ** (2**62)] }

# --- Marshal ---------------------------------------------------------------------------

[bd("1.5"), bd("-1.5e-30"), bd("12345678901234567890.0987654321"), bd("0"), bd("-0"), inf, nan, bd("1") / 0, bd("1") / 3].each do |v|
  check("Marshal #{v.inspect}") do
    dumped = Marshal.dump(v)
    loaded = Marshal.load(dumped)
    [dumped.unpack1("H*"), loaded, loaded.class, loaded == v || (loaded.nan? && v.nan?)]
  end
end
check("_load") { [BigDecimal._load("18:0.15e1"), (BigDecimal._load("x") rescue $!.message)] }
