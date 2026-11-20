# `raise exception, message` raises what `exception.exception(message)`
# answers: a copy with the message, backtrace and cause as asked.
class MyError < StandardError; end

begin
  begin
    raise MyError, "first"
  rescue => e
    raise e, "second"
  end
rescue => again
  p [again.class, again.message, again.equal?(e), e.message]
end

begin
  raise RuntimeError.new("a"), "b", ["here:1"]
rescue => e
  p [e.message, e.backtrace]
end

begin
  begin
    raise "inner"
  rescue
    raise ArgumentError.new("o"), "o2", cause: nil
  end
rescue => e
  p [e.message, e.cause]
end

def m = raise(IOError.new("q"), "r")
begin
  m
rescue => e
  p [e.message, e.backtrace.first]
end

class Odd < StandardError
  def exception(message) = "not an exception"
end
begin
  raise Odd.new, "x"
rescue TypeError => e
  p [e.class, e.message]
end
