dir = ARGV.fetch(0)
path = File.join(dir, "ungetc_plain.txt")
File.binwrite(path, "hello\nwörld\nlast")

def show(label, value)
  puts "#{label}: #{value.inspect}"
end

def line(label, value)
  puts "#{label}: #{value.inspect} #{value&.encoding.inspect}"
end

File.open(path) do |f|
  c = f.getc
  show "getc", c
  f.ungetc(c)
  line "gets after ungetc(getc)", f.gets
  show "lineno", f.lineno
  show "$.", $.
  line "gets", f.gets
  line "gets", f.gets
  line "gets at end", f.gets
end

File.open(path) do |f|
  f.ungetc("é")
  line "gets after ungetc(é)", f.gets
  f.ungetc("éxy")
  f.each_line { |l| line "each_line after ungetc(éxy)", l }
  show "lineno", f.lineno
end

File.open(path) do |f|
  f.ungetc(0x41)
  line "gets after ungetc(0x41)", f.gets
  f.ungetc(0xe9)
  line "gets after ungetc(0xe9)", f.gets
end

File.open(path) do |f|
  f.ungetbyte(0x5a)
  line "gets after ungetbyte(0x5a)", f.gets
  f.ungetbyte("pq\n")
  show "read(2) after ungetbyte(pq\\n)", f.read(2)
  line "gets", f.gets
  f.ungetbyte("\xC3")
  line "gets after ungetbyte(\\xC3)", f.gets
  f.ungetbyte("r\ns")
  f.each_line { |l| line "each_line after ungetbyte(r\\ns)", l }
  show "lineno", f.lineno
end

File.open(path) do |f|
  f.getc
  f.ungetc("abc\n")
  line "gets after getc, ungetc(abc\\n)", f.gets
  show "read(3)", f.read(3)
  line "gets", f.gets
  show "lineno", f.lineno
end

File.open(path) do |f|
  f.ungetc("xy\nz")
  show "read(2) before any read", f.read(2)
  line "gets", f.gets
  line "gets", f.gets
  show "read(4)", f.read(4)
  f.each_line { |l| line "each_line", l }
  show "lineno", f.lineno
  show "$.", $.
end

File.open(path) do |f|
  f.ungetc("q\n" * 3)
  f.each_line { |l| line "each_line after ungetc(q\\n * 3)", l }
  show "lineno", f.lineno
end

File.open(path, "r:ISO-8859-1") do |f|
  f.ungetc("ab\n")
  line "gets ISO-8859-1", f.gets
  line "gets ISO-8859-1", f.gets
end
