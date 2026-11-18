# Fiddle::Handle and Fiddle::Function (#105): opening libraries, looking up
# symbols, calling C with each argument and return type, variadic calls,
# errno in Fiddle.last_error, and the errors CRuby raises. Each line is
# compared with CRuby's.
require "fiddle"

include Fiddle

libc = Fiddle.dlopen(nil)
puts "dlopen(nil): #{libc.class} #{libc.to_i != 0}"
puts "sym: #{libc.sym('strlen') == libc['strlen']} #{Integer === libc['strlen']}"
puts "Handle::DEFAULT: #{Handle::DEFAULT['strlen'] == libc['strlen']} #{Handle::DEFAULT.frozen?} #{Integer === Handle::DEFAULT.to_i}"
puts "Handle.sym: #{Handle.sym('strlen') == libc['strlen']} #{Handle['strlen'] == Handle.sym('strlen')}"
puts "sym_defined?: #{Integer === libc.sym_defined?('strlen')} #{libc.sym_defined?('no_such_function_here').inspect} #{Handle.sym_defined?('no_such_function_here').inspect}"
begin
  libc["no_such_function_here"]
rescue DLError => error
  puts "unknown symbol: #{error.message}"
end
begin
  libc[:strlen]
rescue TypeError => error
  puts "symbol name: #{error.message}"
end
begin
  Handle.new("doesnotexist.doesnotexist")
rescue DLError => error
  puts "missing library: #{error.class} #{error.message.include?('doesnotexist.doesnotexist')}"
end
begin
  Handle.new(42)
rescue TypeError => error
  puts "library not a String: #{error.message}"
end
puts "close_enabled?: #{libc.close_enabled?} #{libc.enable_close.inspect} #{libc.close_enabled?} #{libc.disable_close.inspect}"
own = Handle.new(nil)
puts "close: #{own.close}"
begin
  own.close
rescue DLError => error
  puts "close twice: #{error.message}"
end
begin
  own["strlen"]
rescue DLError => error
  puts "closed handle: #{error.message}"
end
answered = Handle.new(nil) { |handle| Integer === handle["strlen"] }
puts "block form: #{answered.inspect.sub(/0x\h+/, 'ADDR')}"
puts "Fiddle::Handle constants: #{[Handle::RTLD_GLOBAL, Handle::RTLD_LAZY, Handle::RTLD_NOW] == [RTLD_GLOBAL, RTLD_LAZY, RTLD_NOW]}"

def function(handle, name, args, ret, **options) = Function.new(handle[name], args, ret, **options)

strlen = function(libc, "strlen", [TYPE_CONST_STRING], TYPE_SIZE_T, name: "strlen")
puts "strlen: #{strlen.call('fiddle')} #{strlen.call('')} name #{strlen.name.inspect} #{strlen.name.frozen?}"
puts "Function: #{[strlen.abi == Function::DEFAULT, strlen.ptr == libc['strlen'], strlen.to_i == strlen.ptr, strlen.need_gvl?].inspect}"
puts "to_proc: #{%w[a bb ccc].map(&strlen).inspect}"
begin
  strlen.call("a\0b")
rescue ArgumentError => error
  puts "NUL in a const string: #{error.message}"
end
begin
  strlen.call(1)
rescue TypeError => error
  puts "const string not a String: #{error.message}"
end
begin
  strlen.call
rescue ArgumentError => error
  puts "arity: #{error.message}"
end

abs = function(libc, "abs", [TYPE_INT], TYPE_INT)
labs = function(libc, "labs", [:long], :long)
llabs = function(libc, "llabs", ["long_long"], "long_long")
puts "integers: #{[abs.call(-7), abs.call(-2**31 + 1), labs.call(-2**40), llabs.call(-2**62), abs.call(-3.9), abs.call(2r)].inspect}"
[2**31, -2**31 - 1, 2**64, "x", nil, Float::INFINITY].each do |bad|
  begin
    abs.call(bad)
  rescue RangeError, TypeError => error
    puts "abs(#{bad.inspect}): #{error.class} #{error.message}"
  end
end
toupper = function(libc, "toupper", [TYPE_INT], TYPE_INT)
puts "toupper: #{toupper.call('a'.ord).chr}"
pow = function(libc, "pow", [TYPE_DOUBLE, TYPE_DOUBLE], TYPE_DOUBLE)
puts "doubles: #{pow.call(2, 10)} #{pow.call(2.5, 2)} #{pow.call(9, 0.5)}"
begin
  pow.call("2", 1)
rescue TypeError => error
  puts "double from a String: #{error.message}"
end
strtoul = function(libc, "strtoul", [TYPE_CONST_STRING, TYPE_VOIDP, TYPE_INT], -TYPE_LONG)
puts "unsigned long: #{strtoul.call('18446744073709551615', nil, 10)} #{strtoul.call('ff', nil, 16)}"
end_pointer = Pointer.malloc(SIZEOF_VOIDP, RUBY_FREE)
digits = Pointer["123abc"]
strtol = function(libc, "strtol", [TYPE_VOIDP, TYPE_VOIDP, TYPE_INT], TYPE_LONG)
puts "out parameter: #{strtol.call(digits, end_pointer, 10)} rest #{end_pointer.ptr.to_s.inspect}"
getenv = function(libc, "getenv", [TYPE_CONST_STRING], TYPE_CONST_STRING)
ENV["SPINEL_FIDDLE_PROBE"] = "found it"
setenv = function(libc, "setenv", [TYPE_CONST_STRING, TYPE_CONST_STRING, TYPE_INT], TYPE_INT)
setenv.call("SPINEL_FIDDLE_SET", "from C", 1)
puts "const string return: #{getenv.call('SPINEL_FIDDLE_SET').inspect} #{getenv.call('SPINEL_FIDDLE_SURELY_UNSET').inspect} #{getenv.call('SPINEL_FIDDLE_SET').encoding}"
strdup = function(libc, "strdup", [TYPE_CONST_STRING], TYPE_VOIDP)
copy = strdup.call("duplicate")
puts "pointer return: #{copy.class} #{copy.to_s.inspect} size #{copy.size} free #{copy.free.inspect}"
Fiddle.free(copy.to_i)

sprintf = function(libc, "sprintf", [TYPE_VOIDP, TYPE_CONST_STRING, TYPE_VARIADIC], TYPE_INT)
out = "\0" * 64
count = sprintf.call(out, "%d|%ld|%s|%.3f|%u|%c", :int, -5, :long, 2**40, :const_string, "str",
  :double, 1.0 / 3, -TYPE_INT, 4_000_000_000, :int, 90)
puts "variadic: #{count} #{out.unpack1('Z*').inspect}"
begin
  sprintf.call(out, "%d", :int)
rescue ArgumentError => error
  puts "unpaired variadic: #{error.message}"
end
begin
  sprintf.call
rescue ArgumentError => error
  puts "variadic arity: #{error.message}"
end
begin
  Function.new(libc["sprintf"], [TYPE_VARIADIC, TYPE_INT], TYPE_INT)
rescue ArgumentError => error
  puts "variadic not last: #{error.message}"
end
begin
  Function.new(libc["abs"], [99], TYPE_INT)
rescue RuntimeError => error
  puts "unknown argument type: #{error.message}"
end
begin
  Function.new(libc["abs"], TYPE_INT, TYPE_INT)
rescue TypeError => error
  puts "argument types not an Array: #{error.message}"
end

access = function(libc, "access", [TYPE_CONST_STRING, TYPE_INT], TYPE_INT)
puts "errno: #{access.call('/surely/not/here', 0)} #{Fiddle.last_error}"
puts "errno after success: #{access.call('/', 0)} #{Fiddle.last_error.class}"
Fiddle.last_error = 7
puts "last_error=: #{Fiddle.last_error}"
puts "types: #{[TYPE_VOID, TYPE_VOIDP, TYPE_CHAR, TYPE_UCHAR, TYPE_INT, TYPE_SIZE_T, TYPE_SSIZE_T, TYPE_INTPTR_T, TYPE_UINTPTR_T, TYPE_BOOL, TYPE_VARIADIC, TYPE_CONST_STRING].inspect}"
puts "sizes: #{[SIZEOF_VOIDP, SIZEOF_CHAR, SIZEOF_SHORT, SIZEOF_INT, SIZEOF_LONG, SIZEOF_LONG_LONG, SIZEOF_FLOAT, SIZEOF_DOUBLE, SIZEOF_SIZE_T, SIZEOF_BOOL, SIZEOF_CONST_STRING].inspect}"
puts "alignments: #{[ALIGN_VOIDP, ALIGN_CHAR, ALIGN_SHORT, ALIGN_INT, ALIGN_LONG, ALIGN_LONG_LONG, ALIGN_FLOAT, ALIGN_DOUBLE, ALIGN_SIZE_T, ALIGN_BOOL].inspect}"
puts "WINDOWS: #{WINDOWS} RUBY_FREE: #{Integer === RUBY_FREE} BUILD_RUBY_PLATFORM: #{BUILD_RUBY_PLATFORM == RUBY_PLATFORM}"
puts "errors: #{[DLError.superclass, Fiddle::Error.superclass, ClearedReferenceError.superclass].inspect}"
