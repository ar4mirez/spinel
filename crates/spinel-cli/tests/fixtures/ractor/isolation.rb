Warning[:experimental] = false
S = "s"
H = {}
FZ = "fz".freeze
FA = [1, "two".freeze, [3].freeze].freeze
FH = { a: 1, "b" => [2].freeze }.freeze
class C
  @@v = 1
  def self.v = @@v
  @civ = [1]
  @sciv = 5
  class << self
    attr_reader :civ, :sciv
  end
end
Point = Struct.new(:x, :y)
r = Ractor.new do
  out = []
  out << (S rescue $!.message)
  out << Object.const_defined?(:H)
  out << (Object.const_get(:H) rescue $!.message)
  out << [FZ, FZ.frozen?, FZ.equal?(FZ)]
  out << [FA, FA.frozen?, FA[2].frozen?]
  out << [FH, FH[:a], FH["b"]]
  out << (begin; X = +"x"; rescue => e; e.message; end)
  XF = "xf".freeze
  out << (C.v rescue $!.message)
  out << (C.civ rescue $!.message)
  out << C.sciv
  out << (begin; class C; @x = 1; end; rescue => e; e.message; end)
  out << ($g rescue $!.message)
  out << (begin; $g = 1; rescue => e; e.message; end)
  out << ["x".encoding.equal?(Encoding::UTF_8), "x".encoding.to_s]
  out << Point.new(1, 2).x
  class Bar; def hi = :hi; end
  o = Object.new
  def o.hi = 2
  out << o.hi
  Object.define_method(:zz) { 1 }
  out << zz
  out << [1, 2, 3].map { |i| i * 2 }.sum
  out << { b: 1 }.merge(c: 2)
  out << "%05.2f" % 3.14159
  out << [3, 1, 2].sort.inspect
  out << (1..10).each_slice(3).to_a
  out << Integer("42") + Float("1.5")
  out << :upcase.to_proc.call("str")
  out << [Ractor.current.name, Ractor.main?]
  out
end
r.value.each { |v| p v }
p XF, Bar.new.hi
p (zz rescue $!)
