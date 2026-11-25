# The events of frames an unwind leaves: a method an exception passes through
# returns nil, one a `return` in its block or a `break` from the block it was
# given ends returns what they carry, and a `throw` passes through. A
# `:raise` fires once, where the exception starts, and an exception the core
# library raises and rescues itself is none, as CRuby's C never raises it.
EVENTS = []
TRACE = TracePoint.new(:call, :return, :raise) do |tp|
  next unless tp.path == __FILE__
  value = case tp.event
          when :return then tp.return_value
          when :raise then [tp.raised_exception.class, tp.self, tp.method_id, tp.defined_class]
          end
  EVENTS << [tp.event, tp.lineno, tp.method_id, value]
end

def inner
  raise ArgumentError, "boom"
end

def outer
  inner
  :never
end

def finder
  [1, 2, 3].each { |x| return x * 10 if x == 2 }
  :none
end

def gives
  yield
  :after
end

def breaker
  gives { break :broken }
end

def thrower
  throw :done, 7
end

def ensured
  return :from_body
ensure
  EVENTS << [:ensure_ran]
end

def entered_before
  TRACE.enable
  raise "late"
end

begin
  entered_before
rescue => e
  EVENTS << [:rescued, e.message]
end
begin
  outer
rescue ArgumentError
end
finder
breaker
catch(:done) { thrower }
ensured
Integer("x", exception: false)
Float("y", exception: false)
begin
  [1].fetch(9)
rescue IndexError
end
begin
  1 / 0
rescue ZeroDivisionError
end
TRACE.disable
EVENTS.each { |event| p event }
