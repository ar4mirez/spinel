# `Ractor.shareable?`: frozen and immutable objects are, and everything a
# frozen object holds must be too.
Warning[:experimental] = false
S = Struct.new(:a)
D = Data.define(:a)
class Holder
  def initialize(value) = @value = value
end
def show(label, value)
  puts "#{label}: #{Ractor.shareable?(value)}"
end
show "Integer", 1
show "Bignum", 2**100
show "Float", 1.5
show "Rational", 1r
show "Complex", 1i
show "Symbol", :a
show "nil", nil
show "true", true
show "String", +"x"
show "frozen String", "x".freeze
show "frozen Array of mutable", [+"x"].freeze
show "frozen Array of frozen", ["x".freeze].freeze
show "frozen Array of mutable Array", [[1]].freeze
show "frozen Array of frozen Array", [[1].freeze].freeze
show "frozen Hash", { a: 1 }.freeze
show "frozen Hash, mutable value", { a: +"x" }.freeze
show "frozen Hash, mutable Array value", { a: [1] }.freeze
show "frozen Hash, mutable default", Hash.new(+"x").freeze
show "Range of mutable Strings", Range.new(+"a", +"b")
show "Range of Arrays", ([1]..[2]).freeze
show "Struct", S.new(1)
show "frozen Struct", S.new(1).freeze
show "frozen Struct, mutable Array", S.new([1]).freeze
show "Data", D.new(1)
show "Data, mutable member", D.new(+"x")
show "Class", String
show "Module", Kernel
show "frozen Object", Object.new.freeze
show "frozen Object, mutable ivar", Holder.new(+"x").freeze
show "frozen Object, frozen ivar", Holder.new("x".freeze).freeze
show "Regexp literal", /a/
show "frozen Exception, mutable message", Exception.new(+"x").freeze
show "frozen Exception, frozen message", Exception.new("x".freeze).freeze
show "Proc", proc {}
show "frozen Proc", proc {}.freeze
show "lambda", -> {}
show "Thread", Thread.current
show "Ractor", Ractor.current
show "Port", Ractor::Port.new
show "Encoding", Encoding::UTF_8
show "frozen Time", Time.at(0).freeze
