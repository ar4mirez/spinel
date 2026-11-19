# Ractors running in parallel, each on an OS thread of its own (#119): CPU-bound
# work through every path a running Ractor reads the shared class tables by —
# method calls, blocks, attr_readers, Hash literals, respond_to?, sends to
# several classes from one site — while the tables are written: the main
# Ractor defines methods on the classes the work calls through, and each
# Ractor defines a method of its own on a module they share.
Warning[:experimental] = false

class Shape
  attr_reader :size

  def initialize(size)
    @size = size
  end

  def area = size * size
end

class Circle < Shape
  def area = 3 * size * size
end

module Shared
end

def work(k)
  shapes = (1..40).map { |i| i.even? ? Shape.new(i + k) : Circle.new(i + k) }
  total = 0
  300.times do |round|
    shapes.each { |shape| total += shape.area if shape.respond_to?(:area) }
    total += {round => k}.size
  end
  total
end

ractors = (1..4).map do |k|
  Ractor.new(k) do |k|
    Shared.module_eval("def from_#{k} = #{k * 10}")
    [k, work(k), Object.new.extend(Shared).public_send(:"from_#{k}")]
  end
end
100.times { |i| Shape.define_method(:"extra_#{i}") { i } }
ractors.each { |r| p r.value }
p Shared.instance_methods(false).sort
p Circle.new(2).extra_99
p work(0)
