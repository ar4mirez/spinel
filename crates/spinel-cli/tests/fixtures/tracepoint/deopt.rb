# Code that runs on every fast path the interpreter has — superinstructions
# (`i < n`, `i += 1`, `sum += @v`), inline-cached sends, an attr_reader, a
# yield, light stretches — warmed up for thousands of turns, then traced from
# inside: the TracePoint is enabled by a call the loop makes partway through,
# and must see every event from the next instruction on, including the
# :return of frames entered before it was enabled.
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
end

EVENTS = []
# Only this file's: CRuby writes `TracePoint#disable` in Ruby, and traces it.
TRACE = TracePoint.new(:line, :call, :return) do |tp|
  next unless tp.path == __FILE__
  EVENTS << [tp.event, tp.lineno, tp.method_id, (tp.event == :return ? tp.return_value : nil)]
end

def start_tracing
  TRACE.enable
end

def hot(box, n, trace_at)
  sum = 0
  i = 0
  while i < n
    sum += box.v
    sum = box.add(sum)
    box.each_twice { |k| sum += k }
    start_tracing if i == trace_at
    i += 1
  end
  sum
end

box = Box.new(3)
p hot(box, 5000, -1)
p hot(box, 5000, -1)
result = hot(box, 5003, 5000)
TRACE.disable
p result
EVENTS.each { |event| p event }
p EVENTS.size
