# `super` in a block whose method's frame is not on the stack the block
# runs on: a fiber's first frame, a thread's, a Proc called after its method
# returned. Puma's `FiberPerRequest#handle_request` is the first.
class A
  def m(x) = "A#{x}"
end

module InFiber
  def m(x) = Fiber.new { super }.resume
end

module InThread
  def m(x) = Thread.new { super }.value
end

module Escaped
  def m(x) = @kept = proc { super }
end

class B < A; include InFiber; end
class C < A; include InThread; end
class D < A; include Escaped; end

p B.new.m(1)
p C.new.m(2)
d = D.new
d.m(3)
p d.instance_variable_get(:@kept).call
