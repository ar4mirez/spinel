# Fiddle's struct layout against the platform's C ABI (#105): sizes, member
# offsets and alignment as `Fiddle::Importer#struct` and `union` lay them out,
# values written and read back through memory, and structs a C function
# fills — `gmtime_r`'s `struct tm`, `gettimeofday`'s `struct timeval` — read
# at the offsets C wrote them. Each line is compared with CRuby's.
require "fiddle"
require "fiddle/import"

module C
  extend Fiddle::Importer
  dlload Fiddle.dlopen(nil)

  Mixed = struct ["char c", "int i", "double d", "short s", "char *name"]
  Packedish = struct ["char a", "char b", "short c", "int d"]
  Arrays = struct ["char tag[3]", "int values[4]", "double weight"]
  Tail = struct ["double d", "char c"]
  Unsigned = struct ["unsigned char uc", "unsigned short us", "unsigned int ui",
    "unsigned long ul", "long long ll", "unsigned long long ull", "float f"]
  Choice = union ["char c", "int i", "double d", "long l[2]"]
  Nested = struct ["char head", {inner: ["int x", "char y"]}, "double tail"]
  Tm = struct ["int tm_sec", "int tm_min", "int tm_hour", "int tm_mday",
    "int tm_mon", "int tm_year", "int tm_wday", "int tm_yday", "int tm_isdst",
    "long tm_gmtoff", "const char *tm_zone"]
  Timeval = struct ["long tv_sec", "long tv_usec"]

  extern "void *gmtime_r(void *, void *)"
  extern "int gettimeofday(void *, void *)"
  extern "void *memset(void *, int, size_t)"
end

[C::Mixed, C::Packedish, C::Arrays, C::Tail, C::Unsigned, C::Choice, C::Nested, C::Tm, C::Timeval].each do |type|
  name = type.name.sub("C::", "")
  puts "#{name}: size #{type.size} alignment #{type.alignment} members #{type.members.inspect}"
end

layout = Fiddle::CStructEntity.new(Fiddle::NULL, C::Mixed.types)
puts "Mixed offsets: #{layout.instance_variable_get(:@offset).inspect}"
puts "Tm offsets: #{Fiddle::CStructEntity.new(Fiddle::NULL, C::Tm.types).instance_variable_get(:@offset).inspect}"
puts "Arrays offsets: #{Fiddle::CStructEntity.new(Fiddle::NULL, C::Arrays.types).instance_variable_get(:@offset).inspect}"

C::Mixed.malloc(Fiddle::RUBY_FREE) do |mixed|
  name = Fiddle::Pointer.malloc(8, Fiddle::RUBY_FREE)
  name[0, 6] = "fiddle"
  mixed.c = -3
  mixed.i = 123_456
  mixed.d = 2.5
  mixed.s = -2
  mixed.name = name
  puts "Mixed values: #{[mixed.c, mixed.i, mixed.d, mixed.s, mixed.name.to_s].inspect}"
  bytes = mixed.to_ptr[0, C::Mixed.size]
  puts "Mixed bytes: c=#{bytes[0].unpack1('c')} i=#{bytes[4, 4].unpack1('i')} d=#{bytes[8, 8].unpack1('d')} s=#{bytes[16, 2].unpack1('s')}"
  puts "Mixed [] and to_h: #{mixed['i']} #{mixed.to_h.reject { |k, _| k == 'name' }.inspect}"
end

C::Arrays.malloc(Fiddle::RUBY_FREE) do |arrays|
  arrays.tag = [65, 66, 67]
  arrays.values = [10, -20, 30, -40]
  arrays.weight = 0.125
  puts "Arrays values: #{[arrays.tag, arrays.values, arrays.weight].inspect}"
  puts "Arrays bytes: #{arrays.to_ptr[0, 3].inspect} #{arrays.to_ptr[4, 16].unpack('i*').inspect}"
  arrays.values[2] = 99
  puts "Arrays element write: #{arrays.values.inspect}"
end

C::Choice.malloc(Fiddle::RUBY_FREE) do |choice|
  choice.d = 1.0
  puts "Choice as long: #{choice.l.inspect}"
  choice.i = 0x41424344
  puts "Choice overlaps: #{[choice.c, choice.i].inspect}"
end

C::Nested.malloc(Fiddle::RUBY_FREE) do |nested|
  nested.head = 1
  nested.inner.x = 77
  nested.inner.y = 8
  nested.tail = 9.75
  bytes = nested.to_ptr[0, C::Nested.size]
  puts "Nested: #{[nested.head, nested.inner.x, nested.inner.y, nested.tail].inspect} x at 4: #{bytes[4, 4].unpack1('i')} tail at 16: #{bytes[16, 8].unpack1('d')}"
end

C::Unsigned.malloc(Fiddle::RUBY_FREE) do |u|
  u.uc = 255
  u.us = 65535
  u.ui = 4_294_967_295
  u.ul = 18_446_744_073_709_551_615
  u.ll = -9_223_372_036_854_775_808
  u.ull = 18_446_744_073_709_551_615
  u.f = 0.1
  puts "Unsigned: #{[u.uc, u.us, u.ui, u.ul, u.ll, u.ull, u.f].inspect}"
end

time = Fiddle::Pointer.malloc(Fiddle::SIZEOF_LONG, Fiddle::RUBY_FREE)
time[0, Fiddle::SIZEOF_LONG] = [951_782_400 + 3 * 3600 + 25 * 60 + 9].pack("q")
C::Tm.malloc(Fiddle::RUBY_FREE) do |tm|
  C.memset(tm, 0, C::Tm.size)
  answer = C.gmtime_r(time, tm)
  puts "gmtime_r: #{answer == tm.to_ptr} #{[tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_wday, tm.tm_yday, tm.tm_isdst, tm.tm_gmtoff].inspect} zone #{%w[GMT UTC].include?(tm.tm_zone.to_s)}"
end

C::Timeval.malloc(Fiddle::RUBY_FREE) do |tv|
  status = C.gettimeofday(tv, nil)
  puts "gettimeofday: #{status} #{(tv.tv_sec - Time.now.to_i).abs <= 2} #{tv.tv_usec.between?(0, 999_999)}"
end

# A struct array sorted by C's qsort, comparing one member.
Point = C.struct ["int x", "int y"]
points = [[3, 30], [1, 10], [2, 20]]
memory = Fiddle::Pointer.malloc(Point.size * points.size, Fiddle::RUBY_FREE)
points.each_with_index do |(x, y), index|
  point = Point.new(memory + Point.size * index)
  point.x = x
  point.y = y
end
by_x = Fiddle::Closure::BlockCaller.new(Fiddle::TYPE_INT, [Fiddle::TYPE_VOIDP, Fiddle::TYPE_VOIDP]) do |a, b|
  Point.new(a).x <=> Point.new(b).x
end
qsort = Fiddle::Function.new(Fiddle::Handle::DEFAULT["qsort"],
  [Fiddle::TYPE_VOIDP, Fiddle::TYPE_SIZE_T, Fiddle::TYPE_SIZE_T, Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOID)
qsort.call(memory, points.size, Point.size, by_x)
puts "qsort structs: #{points.size.times.map { |i| point = Point.new(memory + Point.size * i); [point.x, point.y] }.inspect}"
puts "sizeof: #{[C.sizeof("int"), C.sizeof("double"), C.sizeof("char*"), C.sizeof(C::Mixed), C.sizeof(C::Tm)].inspect}"
