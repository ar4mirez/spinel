# Fiddle::Pointer (#105): allocation and freeing, byte access, arithmetic,
# comparison, dereferencing, strings, and C functions writing through a
# pointer. Each line is compared with CRuby's.
require "fiddle"

include Fiddle

libc = Fiddle.dlopen(nil)
memcpy = Function.new(libc["memcpy"], [TYPE_VOIDP, TYPE_VOIDP, TYPE_SIZE_T], TYPE_VOIDP)
strcpy = Function.new(libc["strcpy"], [TYPE_VOIDP, TYPE_CONST_STRING], TYPE_VOIDP)
snprintf = Function.new(libc["snprintf"], [TYPE_VOIDP, TYPE_SIZE_T, TYPE_CONST_STRING, TYPE_VARIADIC], TYPE_INT)

buffer = Pointer.malloc(16, RUBY_FREE)
puts "malloc: size #{buffer.size} zeroed #{buffer[0, 16] == "\0" * 16} null? #{buffer.null?} freed? #{buffer.freed?}"
puts "free function: #{buffer.free.class} #{buffer.free.to_i == RUBY_FREE}"
buffer[0, 5] = "hello"
buffer[5] = 33
puts "bytes: #{buffer[0, 6].inspect} #{buffer[0]} #{buffer[5]}"
buffer[6] = 200
puts "a byte reads back signed: #{buffer[6]}"
buffer[6] = 0
puts "to_s: #{buffer.to_s.inspect} to_s(3): #{buffer.to_s(3).inspect} to_str: #{buffer.to_str.bytesize} to_str(2): #{buffer.to_str(2).inspect}"
puts "encodings: #{buffer.to_s.encoding} #{buffer[0, 2].encoding}"

# Arithmetic: a new pointer at the offset, its size moved the other way.
later = buffer + 2
puts "plus: #{later.to_i - buffer.to_i} size #{later.size} #{later.to_s.inspect} free #{later.free.inspect}"
back = later - 1
puts "minus: #{back.to_i - buffer.to_i} size #{back.size} #{back[0, 4].inspect}"
puts "compare: #{[buffer <=> later, later <=> buffer, buffer <=> buffer + 0, buffer <=> 1].inspect}"
puts "equal: #{[buffer == buffer + 0, buffer == later, buffer.eql?(Pointer.new(buffer.to_i)), buffer == buffer.to_i].inspect}"
puts "to_i and to_int: #{buffer.to_i == buffer.to_int} #{Integer === buffer.to_i}"
puts "negative and wrapped: #{[Pointer.new(-1).to_i, Pointer.new(2**64 - 1).to_i, (Pointer.new(1) - 2).to_i, Pointer.new(1 << 62).to_i].inspect}"
begin
  Pointer.new(2**64)
rescue RangeError => error
  puts "too big: #{error.message}"
end

# A pointer stored in memory, read back with ptr, and ref's round trip.
cell = Pointer.malloc(SIZEOF_VOIDP, RUBY_FREE)
cell[0, SIZEOF_VOIDP] = [buffer.to_i].pack("J")
puts "ptr: #{cell.ptr == buffer} #{(+cell).to_s.inspect}"
puts "ref: #{buffer.ref.ptr == buffer} #{(-buffer).ptr.to_s.inspect}"

# C functions writing through pointers and into a String.
copy = Pointer.malloc(16, RUBY_FREE)
answer = memcpy.call(copy, buffer, 6)
puts "memcpy: #{answer == copy} #{copy.to_s.inspect} #{answer.class}"
strcpy.call(copy + 6, " world")
puts "strcpy: #{copy.to_s.inspect}"
string = "\0" * 32
written = snprintf.call(string, 32, "%s=%d %.2f %c", :const_string, "pi", :int, 3, :double, 3.14159, :int, 33)
puts "snprintf into a String: #{written} #{string.unpack1('Z*').inspect} #{string.bytesize}"
begin
  snprintf.call(string, 32, "%c", :char, 33)
rescue RuntimeError => error
  puts "a char is no variadic argument: #{error.message}"
end
from_string = Pointer["chars"]
puts "Pointer[String]: #{from_string.to_s.inspect} size #{from_string.size}"
puts "Pointer[Integer]: #{Pointer[buffer.to_i] == buffer} #{Pointer[buffer.to_i].size}"
puts "Pointer[Pointer]: #{Pointer[buffer] == buffer}"
wrapper = Object.new
def wrapper.to_ptr = Fiddle::Pointer.new(1234)
puts "Pointer[to_ptr]: #{Pointer[wrapper].to_i}"
odd = Object.new
def odd.to_ptr = 1
begin
  Pointer[odd]
rescue DLError => error
  puts "to_ptr not a Pointer: #{error.message}"
end
Pointer.write(copy.to_i, "ABC")
puts "Pointer.read: #{Pointer.read(copy.to_i, 5).inspect}"

# size=, NULL, and freeing.
buffer.size = 4
puts "size=: #{buffer.size} #{buffer.to_str.inspect}"
puts "NULL: #{NULL.null?} #{NULL.to_i} #{NULL.size} #{Pointer.new(0, 10).size}"
begin
  NULL[0]
rescue DLError => error
  puts "NULL[0]: #{error.message}"
end
begin
  NULL[0] = 1
rescue DLError => error
  puts "NULL[0]=: #{error.message}"
end
puts "inspect: #{buffer.inspect.sub(/:0x\h+ ptr=0x\h+/, ':ADDR ptr=ADDR').sub(/free=0x\h+/, 'free=FREE')}"
puts "NULL inspect: #{NULL.inspect.sub(/:0x\h+/, ':ADDR')}"
buffer.call_free
puts "call_free: #{buffer.freed?}"
buffer.call_free
puts "call_free twice: #{buffer.freed?}"
plain = Pointer.malloc(8)
puts "no free function: #{plain.free.inspect} #{plain.freed?}"
plain.free = RUBY_FREE
plain.call_free
puts "free= then call_free: #{plain.freed?}"
Pointer.malloc(8, RUBY_FREE) { |scoped| puts "malloc block: #{scoped.size} #{scoped.freed?}"; plain = scoped }
puts "after the block: #{plain.freed?}"
begin
  Pointer.malloc(8) { }
rescue ArgumentError => error
  puts "malloc block without free: #{error.message}"
end

address = Fiddle.malloc(8)
Pointer.new(address)[0, 3] = "xyz"
address = Fiddle.realloc(address, 64)
puts "Fiddle.malloc and realloc: #{Pointer.new(address).to_s(3).inspect}"
Fiddle.free(address)
puts "dlwrap round trip: #{[Fiddle.dlunwrap(Fiddle.dlwrap(:sym)), Fiddle.dlunwrap(Fiddle.dlwrap(nil)), Fiddle.dlunwrap(Fiddle.dlwrap(42)), Fiddle.dlwrap(true) == Fiddle::Qtrue].inspect}"
pinned = Fiddle::Pinned.new(object = Object.new)
puts "Pinned: #{pinned.ref.equal?(object)} #{pinned.cleared?}"
pinned.clear
begin
  pinned.ref
rescue Fiddle::ClearedReferenceError => error
  puts "cleared: #{pinned.cleared?} #{error.message}"
end
