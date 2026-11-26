# TracePoints that wait only for :class, :call or :raise, which fire where a
# frame is pushed or an exception starts, enabled around code that runs on
# every fast path the interpreter has — superinstructions, inline-cached
# sends, calls and returns taken without the trip through the loop, a yield,
# light stretches — warmed up first, and then a :line one enabled from inside
# a loop already running, which must still see every line from then on.
EVENTS = []
def note(*values) = EVENTS << values

class Box
  attr_reader :v

  def initialize(v)
    @v = v
  end

  def add(x)
    x + @v
  end

  def each_twice
    yield 1
    yield 2
  end

  def deep(n)
    n.zero? ? @v : deep(n - 1) + 1
  end
end

def hot(box, n)
  sum = 0
  i = 0
  while i < n
    sum += box.v
    sum = box.add(sum)
    box.each_twice { |k| sum += k }
    sum += box.deep(3)
    i += 1
  end
  sum
end

box = Box.new(3)
p hot(box, 5000)

# :class alone, as Zeitwerk keeps one enabled: a class body reached from a
# method the hot loop calls, after an `inherited` hook written in Ruby that
# makes calls of its own and ends in one, and a module's. The event comes
# before the body's first line: its `a` is not set yet.
classes = TracePoint.new(:class) do |tp|
  next unless tp.path == __FILE__
  body = tp.binding
  note(:class, tp.lineno, tp.self, tp.method_id, body.local_variable_defined?(:a) && body.local_variable_get(:a))
end

def helper(x) = x + 1

class Base
  def self.inherited(sub)
    super
    note(:inherited, sub.name, Box.new(1).deep(4), [1, 2].sum)
    x = 1
    x + helper(2)
  end
end

def open_classes(i)
  Class.new(Base)
  if i == 2
    eval(<<~RUBY, binding, __FILE__, __LINE__ + 1)
      class Made < Base
        X = Box.new(2).deep(2)
      end
      module Mixed
      end
    RUBY
  end
  i
end

classes.enable do
  p hot(box, 5000)
  sum = 0
  4.times { |i| sum += open_classes(i) + hot(box, 10) }
  p sum
  class Late < Base
    def self.made = :late
  end
  4.times do |i|
    eval(<<~RUBY, binding, __FILE__, __LINE__ + 1)
      class Warm#{i} < Base
        a = 1
        b = a + 2
        note(:body, a, b)
      end
    RUBY
  end
end
p Made::X

# :raise alone, as an error tracker keeps one enabled: raised in a method the
# fast paths called, deep down, in the core library and rescued by the
# program, rescued by the core library itself, and raised again from a
# `rescue`; the hook makes calls of its own.
def thrower(n)
  n.zero? ? raise(ArgumentError, "deep") : thrower(n - 1)
end

def catcher(box, n)
  hot(box, 10)
  thrower(n)
rescue ArgumentError => e
  e.message
end

raises = TracePoint.new(:raise) do |tp|
  note(:raise, tp.lineno, tp.raised_exception.class, tp.method_id, hot(Box.new(1), 3))
end
raises.enable do
  p hot(box, 5000)
  p catcher(box, 5)
  p [1].fetch(9) rescue p $!.class
  p Integer("12", exception: false), Integer("x", exception: false)
  begin
    begin
      raise "inner"
    rescue => e
      raise TypeError, e.message
    end
  rescue TypeError => e
    p e.message
  end
  p 3.times.map { |i| catcher(box, i) }
end

# :call alone.
calls = TracePoint.new(:call) do |tp|
  note(:call, tp.lineno, tp.method_id) if tp.path == __FILE__ && tp.method_id != :note
end
calls.enable do
  p hot(box, 2)
end

# All three, and then :line from a call the loop makes partway through.
lines = TracePoint.new(:line) do |tp|
  note(:line, tp.lineno) if tp.path == __FILE__
end

def start_lines(lines)
  lines.enable
end

def traced(box, n, at, lines)
  sum = 0
  i = 0
  while i < n
    sum += box.add(i)
    start_lines(lines) if i == at
    i += 1
  end
  sum
end

few_calls = TracePoint.new(:call) do |tp|
  note(:call, tp.lineno, tp.method_id) if %i[start_lines catcher thrower].include?(tp.method_id)
end
classes.enable
raises.enable
few_calls.enable
p traced(box, 5003, 5000, lines)
lines.disable
p catcher(box, 1)
few_calls.disable
raises.disable
classes.disable

EVENTS.each { |event| p event }
p EVENTS.size
