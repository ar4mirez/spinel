# What a hook sees and what it may do: disable itself on the way through an
# unwind, raise, run for one thread only, run after a newer TracePoint's
# hook, see a frame's binding, `$!` cleared, and the names a method was
# defined and called by.
OUT = []
def note(*values) = OUT << values

class Base
  def self.inherited(sub)
    note(:inherited, sub)
  end

  def greet(name)
    "hi #{name}"
  end
end

class Sub < Base
  def greet(name)
    super
  end
  alias_method :hello, :greet
  define_method(:made) { |x| x * 2 }

  def self.build = new
end

# A :raise hook that disables itself: the exception goes on.
once = TracePoint.new(:raise) do |tp|
  note(:raise_once, tp.raised_exception.message, $!)
  tp.disable
end
once.enable
begin
  raise "first"
rescue => e
  note(:rescued, e.message, $!.message)
end
begin
  raise "second"
rescue => e
  note(:rescued, e.message)
end
note(:once_enabled, once.enabled?)

# A :return hook that disables itself while an exception passes.
def leaves
  raise "out"
end
ret = TracePoint.new(:return) do |tp|
  next unless tp.path == __FILE__
  note(:return_once, tp.method_id, tp.return_value)
  tp.disable
end
ret.enable
begin
  leaves
rescue => e
  note(:after_return, e.message)
end

# A hook that raises: its exception is what the program sees.
def plain = :plain
raising = TracePoint.new(:call) do |tp|
  raise ArgumentError, "from hook" if tp.method_id == :plain
end
begin
  raising.enable { plain }
rescue ArgumentError => e
  note(:hook_raised, e.message, raising.enabled?)
end

# A :line hook that raises before a `begin` body's first line runs: that
# `begin`'s `rescue` takes it, and the backtrace reports the traced line.
stop_line = __LINE__ + 4
liner = TracePoint.new(:line) { |tp| raise "line hook" if tp.lineno == stop_line }
liner.enable
begin
  note(:not_rescued)
rescue => e
  liner.disable
  note(:line_rescued, e.message, e.backtrace)
end

# Two TracePoints for one event: the newer's hook runs first.
order = []
first = TracePoint.new(:call) { |tp| order << [:first, tp.method_id] if tp.path == __FILE__ }
second = TracePoint.new(:call) { |tp| order << [:second, tp.method_id] if tp.path == __FILE__ }
first.enable
second.enable
plain
second.disable
first.disable
note(:order, order)

# Names, classes and bindings.
calls = []
TracePoint.new(:call, :return, :class) do |tp|
  next unless tp.path == __FILE__
  calls << [tp.event, tp.method_id, tp.callee_id, tp.defined_class, tp.self.class, tp.binding.local_variables]
end.enable do
  s = Sub.build
  s.hello("x")
  s.made(4)
  class Sub; end
  class Fresh < Base; end
end
calls.each { |call| note(*call) }

# One thread's, from `enable` with a block.
seen = []
TracePoint.new(:call) { |tp| seen << Thread.current.name if tp.path == __FILE__ }.enable do
  plain
  t = Thread.new { plain }
  t.name = "other"
  t.join
end
note(:threads, seen)

# `target:`: only the target's own events, and its `:line`s on `target_line:`.
def target_me
  a = 1
  b = plain
  a
end
targeted = []
tp = TracePoint.new(:call, :return, :line) { |t| targeted << [t.event, t.method_id, t.lineno] }
tp.enable(target: method(:target_me)) { target_me; plain }
lined = []
line_tp = TracePoint.new(:line) { |t| lined << t.lineno }
line = method(:target_me).source_location[1] + 2
line_tp.enable(target: method(:target_me), target_line: line) { target_me }
note(:targeted, targeted)
note(:lined, lined, line)

OUT.each { |values| p values }
