# Fiddle::Closure called from C (#105): qsort and bsearch calling back into
# Ruby, a closure called through a Function, a block that raises out of C,
# a callback that makes a C call with a closure of its own, and the closure
# lifecycle. Each line is compared with CRuby's.
require "fiddle"
require "fiddle/import"

include Fiddle

libc = Fiddle.dlopen(nil)
qsort = Function.new(libc["qsort"], [TYPE_VOIDP, TYPE_SIZE_T, TYPE_SIZE_T, TYPE_VOIDP], TYPE_VOID)
bsearch = Function.new(libc["bsearch"],
  [TYPE_VOIDP, TYPE_VOIDP, TYPE_SIZE_T, TYPE_SIZE_T, TYPE_VOIDP], TYPE_VOIDP)

def ints(pointer, count) = pointer[0, count * SIZEOF_INT].unpack("i*")

numbers = [42, -7, 19, 0, 3, 3, -100, 8]
buffer = Pointer.malloc(numbers.size * SIZEOF_INT, RUBY_FREE)
buffer[0, numbers.size * SIZEOF_INT] = numbers.pack("i*")
calls = 0
compare = Closure::BlockCaller.new(TYPE_INT, [TYPE_VOIDP, TYPE_VOIDP]) do |a, b|
  calls += 1
  a[0, SIZEOF_INT].unpack1("i") <=> b[0, SIZEOF_INT].unpack1("i")
end
qsort.call(buffer, numbers.size, SIZEOF_INT, compare)
puts "qsort: #{ints(buffer, numbers.size).inspect} (#{calls.positive?})"

descending = Closure::BlockCaller.new(TYPE_INT, [TYPE_VOIDP, TYPE_VOIDP]) do |a, b|
  b[0, SIZEOF_INT].unpack1("i") <=> a[0, SIZEOF_INT].unpack1("i")
end
qsort.call(buffer, numbers.size, SIZEOF_INT, descending)
puts "qsort descending: #{ints(buffer, numbers.size).inspect}"

qsort.call(buffer, numbers.size, SIZEOF_INT, compare)
key = Pointer.malloc(SIZEOF_INT, RUBY_FREE)
[19, 5].each do |wanted|
  key[0, SIZEOF_INT] = [wanted].pack("i")
  found = bsearch.call(key, buffer, numbers.size, SIZEOF_INT, compare)
  shown = found.null? ? "not found" : "found #{found[0, SIZEOF_INT].unpack1('i')} at #{(found.to_i - buffer.to_i) / SIZEOF_INT}"
  puts "bsearch #{wanted}: #{shown}"
end

# A closure's address is a C function, so a Function calls it through C.
twice = Closure::BlockCaller.new(TYPE_LONG, [TYPE_LONG, TYPE_DOUBLE]) { |n, x| n * 2 + x.floor }
puts "through C: #{Function.new(twice, [TYPE_LONG, TYPE_DOUBLE], TYPE_LONG).call(20, 2.75)}"
puts "argument types: #{twice.args.inspect} -> #{twice.ctype}"

# Every argument type a callback converts.
types = [TYPE_CHAR, TYPE_UCHAR, TYPE_SHORT, TYPE_USHORT, TYPE_INT, TYPE_UINT,
  TYPE_LONG, TYPE_ULONG, TYPE_LONG_LONG, TYPE_ULONG_LONG, TYPE_FLOAT, TYPE_DOUBLE,
  TYPE_BOOL, TYPE_CONST_STRING]
seen = nil
echo = Closure::BlockCaller.new(TYPE_VOID, types) { |*args| seen = args }
Function.new(echo, types, TYPE_VOID).call(-5, 250, -300, 65000, -70000, 4_000_000_000,
  -(2**40), 2**63, -(2**50), 2**64 - 1, 1.5, -2.25, true, "text")
puts "callback arguments: #{seen.inspect}"

{TYPE_CHAR => 300, TYPE_UCHAR => -1, TYPE_SHORT => 70000, TYPE_UINT => -1,
 TYPE_DOUBLE => 3, TYPE_FLOAT => 0.1, TYPE_BOOL => :yes, TYPE_CONST_STRING => "back"}.each do |type, value|
  give = Closure::BlockCaller.new(type, []) { value }
  answer = Function.new(give, [], type).call
  puts "callback returns #{type}: #{answer.inspect}"
end
pointer_back = Closure::BlockCaller.new(TYPE_VOIDP, []) { buffer }
puts "callback returns a pointer: #{Function.new(pointer_back, [], TYPE_VOIDP).call == buffer}"

# A block that raises or throws leaves the C call at once, as CRuby's longjmp
# does, and the next call is unaffected.
compared = 0
raising = Closure::BlockCaller.new(TYPE_INT, [TYPE_VOIDP, TYPE_VOIDP]) do |a, b|
  compared += 1
  raise ArgumentError, "no comparing today" if compared == 2
  0
end
begin
  qsort.call(buffer, numbers.size, SIZEOF_INT, raising)
rescue ArgumentError => error
  puts "raised out of qsort: #{error.message} after #{compared} comparisons"
end
caught = catch(:out) do
  thrower = Closure::BlockCaller.new(TYPE_INT, [TYPE_VOIDP, TYPE_VOIDP]) { throw :out, :thrown }
  qsort.call(buffer, numbers.size, SIZEOF_INT, thrower)
  :finished
end
puts "throw out of a callback: #{caught.inspect}"
qsort.call(buffer, numbers.size, SIZEOF_INT, compare)
puts "after: #{ints(buffer, numbers.size).inspect}"

# A callback that makes a C call with a closure of its own.
rows = [[3, 1, 2], [9, 8], [5, 4, 6, 7]]
inner = Closure::BlockCaller.new(TYPE_INT, [TYPE_VOIDP, TYPE_VOIDP]) do |a, b|
  a[0, SIZEOF_INT].unpack1("i") <=> b[0, SIZEOF_INT].unpack1("i")
end
row_buffers = rows.map do |row|
  memory = Pointer.malloc(row.size * SIZEOF_INT, RUBY_FREE)
  memory[0, row.size * SIZEOF_INT] = row.pack("i*")
  [memory, row.size]
end
table = Pointer.malloc(rows.size * SIZEOF_VOIDP, RUBY_FREE)
table[0, rows.size * SIZEOF_VOIDP] = row_buffers.map { |memory, _| memory.to_i }.pack("J*")
sizes = row_buffers.to_h { |memory, size| [memory.to_i, size] }
outer = Closure::BlockCaller.new(TYPE_INT, [TYPE_VOIDP, TYPE_VOIDP]) do |a, b|
  left, right = a.ptr, b.ptr
  [left, right].each { |row| qsort.call(row, sizes[row.to_i], SIZEOF_INT, inner) }
  sizes[left.to_i] <=> sizes[right.to_i]
end
qsort.call(table, rows.size, SIZEOF_VOIDP, outer)
sorted = table[0, rows.size * SIZEOF_VOIDP].unpack("J*").map { |address| ints(Pointer.new(address), sizes[address]) }
puts "nested: #{sorted.inspect}"

# Fiddle::Importer's bind makes the closure for a C signature.
module LibC
  extend Fiddle::Importer
  dlload Fiddle.dlopen(nil)
  extern "void qsort(void *, size_t, size_t, void *)"
  bind("int compare_desc(void *, void *)") do |a, b|
    b[0, Fiddle::SIZEOF_INT].unpack1("i") <=> a[0, Fiddle::SIZEOF_INT].unpack1("i")
  end
end
LibC.qsort(buffer, numbers.size, SIZEOF_INT, LibC["compare_desc"])
puts "Importer bind: #{ints(buffer, numbers.size).inspect}"

made = nil
Closure.create(TYPE_INT, [TYPE_INT]) { |closure| made = closure; puts "created freed?: #{closure.freed?}" }
puts "create frees after the block: #{made.freed?}"
begin
  made.to_i
rescue ArgumentError => error
  puts "to_i after free: #{error.message.sub(/0x\h+/, 'ADDR')}"
end
compare.free
compare.free
puts "free twice: #{compare.freed?}"
begin
  Closure.new(TYPE_INT, TYPE_INT)
rescue TypeError => error
  puts "args not an Array: #{error.message}"
end
begin
  Closure.new(TYPE_INT, [99])
rescue RuntimeError => error
  puts "unknown type: #{error.message}"
end
