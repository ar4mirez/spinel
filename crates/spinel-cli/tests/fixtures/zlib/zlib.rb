# The built-in zlib (#59), against ruby 4.0.9: every line this prints is
# what CRuby prints (zlib.out). Inputs come from a fixed LCG, so both run
# the same bytes; gzip headers get an explicit mtime, so both write the same.
require "zlib"
require "stringio"

def lcg_bytes(n, seed, alphabet = nil)
  x = seed
  out = "".b
  n.times do
    x = (x * 214013 + 2531011) & 0x7fffffff
    out << (alphabet ? alphabet.getbyte((x >> 16) % alphabet.bytesize) : (x >> 16) & 0xff)
  end
  out
end

def hex(s) = s.unpack1("H*")
def slices(s, n) = (0...s.bytesize).step(n).map { |i| s.byteslice(i, n) }
def digest(s) = s.nil? ? "nil" : "#{s.bytesize}:#{Zlib.crc32(s)}:#{s.encoding}"

def try
  r = yield
  r.is_a?(String) ? "=> #{digest(r)}" : "=> #{r.inspect}"
rescue Exception => e
  "!! #{e.class}: #{e.message}"
end

text = ("the quick brown fox jumps over the lazy dog, " * 40) +
  lcg_bytes(3000, 7, "abcdefghij klmnop\n")
INPUTS = {
  "empty" => "".b,
  "ten" => "1" * 10,
  "short" => "hello world, hello world, hello hello world",
  "text" => text * 8,
  "random" => lcg_bytes(40_000, 3),
  "mixed" => lcg_bytes(50_000, 11, "abcdefgh") + lcg_bytes(10_000, 5),
  "runs" => ("a" * 40_000) + ("b" * 30_000) + lcg_bytes(500, 9) * 30,
}

puts "== versions and constants"
p [Zlib::VERSION, Zlib::ZLIB_VERSION, Zlib.zlib_version, Zlib::VERSION.encoding]
p Zlib.constants.sort
p [Zlib::BINARY, Zlib::ASCII, Zlib::TEXT, Zlib::UNKNOWN, Zlib::NO_COMPRESSION, Zlib::BEST_SPEED,
   Zlib::BEST_COMPRESSION, Zlib::DEFAULT_COMPRESSION, Zlib::FILTERED, Zlib::HUFFMAN_ONLY, Zlib::RLE,
   Zlib::FIXED, Zlib::DEFAULT_STRATEGY, Zlib::MAX_WBITS, Zlib::DEF_MEM_LEVEL, Zlib::MAX_MEM_LEVEL,
   Zlib::NO_FLUSH, Zlib::SYNC_FLUSH, Zlib::FULL_FLUSH, Zlib::FINISH, Zlib::OS_CODE, Zlib::OS_UNKNOWN]
p Zlib::ZStream.instance_methods(false).sort
p Zlib::Deflate.instance_methods(false).sort, Zlib::Inflate.instance_methods(false).sort
p Zlib::GzipFile.instance_methods(false).sort
p Zlib::GzipWriter.instance_methods(false).sort, Zlib::GzipReader.instance_methods(false).sort
p Zlib.singleton_methods.sort
p Zlib::GzipFile::Error.ancestors.take(4)
puts try { Zlib::ZStream.new }
puts try { Zlib::GzipFile.new }

puts "== checksums"
["", " ", "123456789", "a" * 100_000, INPUTS["random"]].each do |s|
  p [Zlib.crc32(s), Zlib.adler32(s)]
end
[0, 1, -1, 2**8, 2**32, 2**32 + 5, -2**63, 2**64 - 1, 3.7, ~305419896].each do |sum|
  puts [sum, try { Zlib.crc32("p", sum) }, try { Zlib.adler32("p", sum) },
        try { Zlib.crc32("", sum) }, try { Zlib.adler32("", sum) }, try { Zlib.adler32("ab" * 9, sum) }].join(" ")
end
puts try { Zlib.crc32("a", 2**64) }, try { Zlib.crc32("a", -2**63 - 1) }, try { Zlib.crc32("a", "x") }
puts try { Zlib.crc32(1) }, try { Zlib.crc32("a", 1, 2) }
p [Zlib.crc32, Zlib.adler32, Zlib.crc32(nil, 5), Zlib.adler32(nil, 5), Zlib.crc32("abc", nil)]
p [Zlib.crc32_combine(Zlib.crc32("ab"), Zlib.crc32("cd"), 2), Zlib.crc32("abcd")]
p [Zlib.adler32_combine(Zlib.adler32("ab"), Zlib.adler32("cd"), 2), Zlib.adler32("abcd")]
p [Zlib.crc32_combine(1, 2, 3), Zlib.adler32_combine(1, 2, 3), Zlib.adler32_combine(1, 2, -3),
   Zlib.crc32_combine(0xdeadbeef, 0x12345678, 2**40), Zlib.adler32_combine(0xffffffff, 0xfffffff0, 2**40 + 7)]
table = Zlib.crc_table
p [table.size, table[0], table[1], table[255], table.sum]
File.open(__FILE__, "rb") { |f| p Zlib.crc32(f) == Zlib.crc32(File.binread(__FILE__)) }

puts "== Zlib.deflate at every level and strategy"
INPUTS.each do |name, data|
  (0..9).each do |level|
    out = Zlib::Deflate.deflate(data, level)
    puts "#{name} #{level} #{digest(out)}#{out.bytesize < 80 ? " " + hex(out) : ""}"
  end
  (0..4).each do |strategy|
    [[15, 8], [9, 1], [-15, 9], [31, 8], [12, 3], [8, 8], [-9, 2], [25, 5]].each do |wbits, mem|
      [1, 6, 9].each do |level|
        d = Zlib::Deflate.new(level, wbits, mem, strategy)
        out = d.deflate(data, Zlib::FINISH)
        puts "#{name} s#{strategy} w#{wbits} m#{mem} l#{level} #{digest(out)} #{d.total_in} #{d.total_out} #{d.adler} #{d.data_type}"
        back = Zlib::Inflate.new(wbits < 0 ? wbits : 47).inflate(out)
        raise "round trip #{name}" unless back == data
      end
    end
  end
end
puts try { Zlib::Deflate.new(10) }
puts try { Zlib::Deflate.new(1, 16) }
puts try { Zlib::Deflate.new(1, 15, 0) }
puts try { Zlib::Deflate.new(1, 15, 8, 5) }
puts try { Zlib::Deflate.new(1, -8) }
puts try { Zlib::Deflate.new("1") }
puts try { Zlib::Deflate.new(1.9).deflate("x", Zlib::FINISH) }
puts try { Zlib::Deflate.new(2**40) }

puts "== deflate streams"
data = INPUTS["mixed"]
[0, 1, 6, 9].each do |level|
  d = Zlib::Deflate.new(level)
  outs = []
  slices(data, 7777).each { |chunk| outs << d.deflate(chunk) }
  outs << d.flush
  outs << d.deflate("tail" * 100, Zlib::FULL_FLUSH)
  outs << d.flush(Zlib::NO_FLUSH)
  outs << d.deflate("more", Zlib::SYNC_FLUSH)
  outs << d.finish
  puts "#{level} #{outs.map(&:bytesize).inspect} #{digest(outs.join)} #{d.finished?} #{d.total_in} #{d.total_out}"
  puts try { d.deflate("x") }
  puts try { d.finish }
  d.close
  puts try { d.deflate("x") }
  p [d.closed?, d.ended?]
end
d = Zlib::Deflate.new(Zlib::NO_COMPRESSION)
d << data.byteslice(0, 20_000)
d.params(Zlib::BEST_COMPRESSION, Zlib::DEFAULT_STRATEGY)
d << data.byteslice(20_000, 20_000)
d.params(Zlib::BEST_SPEED, Zlib::HUFFMAN_ONLY)
d << data.byteslice(40_000, 20_000)
out = d.finish
puts "params #{digest(out)} #{Zlib::Inflate.inflate(out) == data}"
d = Zlib::Deflate.new(1)
d.deflate("abc")
puts try { d.params(9, 0) }
d = Zlib::Deflate.new
d.set_dictionary("aaaaaaaaaa")
d << "abcdefghij"
p d.finish.bytes
puts try { d = Zlib::Deflate.new; d << "x"; d.set_dictionary("abc") }
d = Zlib::Deflate.new
e = d.dup
d << "hello " * 1000
e << "hello " * 1000
p [d.finish == e.finish, d.total_in, e.total_in]
i = Zlib::Inflate.new
puts try { i.dup.inflate(Zlib.deflate("x")).class }
chunks = []
r = Zlib::Deflate.new(0).deflate(INPUTS["random"], Zlib::FINISH) { |c| chunks << c.bytesize }
p [r, chunks]
chunks = []
r = Zlib::Deflate.deflate(INPUTS["random"], 0) { |c| chunks << c.bytesize }
p [r, chunks]
d = Zlib::Deflate.new
d.avail_out = 100
p d.avail_out
d << "x" * 1000
p [d.avail_out, d.avail_in, d.total_in]
p digest(d.flush_next_out), d.flush_next_in

puts "== inflate"
INPUTS.each do |name, data|
  z = Zlib.deflate(data)
  puts "#{name} #{digest(Zlib.inflate(z))}"
  inf = Zlib::Inflate.new
  out = +""
  slices(z, 1013).each { |c| out << inf.inflate(c) }
  out << inf.finish
  p [out == data, inf.total_in, inf.total_out, inf.adler, inf.finished?, inf.data_type]
  chunks = []
  inf = Zlib::Inflate.new
  r = inf.inflate(z) { |c| chunks << c.bytesize }
  p [r, chunks]
  buffer = +"keep me"
  inf = Zlib::Inflate.new
  r = inf.inflate(z, buffer: buffer)
  p [r.equal?(buffer), r == data, buffer.encoding]
end
inf = Zlib::Inflate.new
z = Zlib.deflate("abc" * 100)
z.each_char { |c| inf << c }
p [inf.finish, inf.finished?]
inf = Zlib::Inflate.new
p inf.inflate(z + "trailing garbage")
p [inf.finished?, inf.avail_in, inf.inflate("more"), inf.inflate(nil), inf.finish]
p Zlib.inflate(z + "trailing")
[-15, 31, 47, 0, 9, 15].each do |wbits|
  zz = wbits < 0 ? Zlib::Deflate.new(6, wbits).deflate("raw " * 50, Zlib::FINISH) :
    wbits == 31 || wbits == 47 ? Zlib.gzip("gz " * 50) : Zlib.deflate("zl " * 50)
  puts "#{wbits} #{try { Zlib::Inflate.new(wbits).inflate(zz) }}"
end
puts try { Zlib::Inflate.new(16).class }
puts try { Zlib::Inflate.new(-16) }

puts "== dictionaries"
dict = "abcdefghij" * 5
d = Zlib::Deflate.new
d.set_dictionary(dict)
zd = d.deflate("abcdefghijklmnop" * 10, Zlib::FINISH)
p hex(zd)
inf = Zlib::Inflate.new
puts try { inf.inflate(zd) }
p [inf.adler, Zlib.adler32(dict), inf.avail_in]
puts try { inf.set_dictionary("wrong") }
puts try { inf.set_dictionary(dict) }
puts try { inf.inflate("") }
p inf.finish
inf = Zlib::Inflate.new
inf.add_dictionary(dict)
inf.add_dictionary("other")
puts try { inf.inflate(zd) }
p inf.instance_variables
inf = Zlib::Inflate.new(-15)
puts try { inf.set_dictionary(dict) }
zz = Zlib::Deflate.new(6, -15)
zz.set_dictionary(dict)
raw = zz.deflate("abcdefghij" * 3, Zlib::FINISH)
puts try { inf.inflate(raw) }

puts "== sync"
d = Zlib::Deflate.new
part1 = d.deflate("first part " * 20, Zlib::FULL_FLUSH)
part2 = d.deflate("second part " * 20, Zlib::FINISH)
inf = Zlib::Inflate.new
corrupt = part1.dup
corrupt.setbyte(5, corrupt.getbyte(5) ^ 0xff)
puts try { inf.inflate(corrupt) }
p [inf.sync(part2), inf.sync_point?, inf.avail_in]
puts try { inf.inflate("") }
inf = Zlib::Inflate.new
p [inf.sync(""), inf.sync("xx"), inf.sync("\x00\x00\xff\xff".b + part2)]
puts try { inf.inflate("") }
inf = Zlib::Inflate.new(-15)
d = Zlib::Deflate.new(0, -15)
p inf.sync_point?
inf.inflate(d.deflate("abc", Zlib::SYNC_FLUSH).byteslice(0, 3))
p inf.sync_point?

puts "== corrupt, truncated and oversized"
good = Zlib.deflate(INPUTS["text"])
ggood = Zlib.gzip(INPUTS["text"])
CASES = {
  "empty" => "",
  "header" => "\x78\x9d".b,
  "method" => "\x77\x9c".b,
  "window" => "\x88\x1c".b + good.byteslice(2..),
  "dictid" => "\x78\xbb\x01\x02\x03\x04".b,
  "blocktype" => "\x78\x9c\x07".b,
  "stored" => "\x78\x9c\x01\x05\x00\x05\x00abcde".b,
  "storedlen" => "\x78\x9c\x01\x05\x00\x00\x00abcde".b,
  "toomany" => "\x78\x9c\xfd\xff".b,
  "codelens" => "\x78\x9c\x05\x00\x00\x00".b,
  "repeat" => "\x78\x9c\x05\xc0\x01\x00\x00\x00\x00\x00".b,
  "fixed" => "\x78\x9c\x03\x00".b,
  "dist" => "\x78\x9c\x63\x00\x00\x00\x00\x00".b,
  "toofar" => "\x78\x9c\x4b\x04\x02\x00".b,
  "check" => good.byteslice(0..-2) + (good.getbyte(-1) ^ 1).chr,
  "truncated" => good.byteslice(0, good.bytesize / 2),
  "oversized" => good + good,
  "garbage" => lcg_bytes(300, 1),
  "gzflags" => "\x1f\x8b\x08\xe0".b + ggood.byteslice(4..),
  "gzcrc" => ggood.byteslice(0..-9) + "\x00\x00\x00\x00".b + ggood.byteslice(-4..),
  "gzlen" => ggood.byteslice(0..-5) + "\x01\x00\x00\x00".b,
  "gzmethod" => "\x1f\x8b\x07".b + ggood.byteslice(3..),
  "gzhcrc" => "\x1f\x8b\x08\x02".b + ggood.byteslice(4, 6) + "\x00\x00".b + ggood.byteslice(10..),
}
CASES.each do |name, bytes|
  puts "#{name} #{try { Zlib.inflate(bytes) }} #{try { Zlib::Inflate.new(47).inflate(bytes) }} " \
       "#{try { Zlib::Inflate.new(-15).inflate(bytes) }} #{try { Zlib.gunzip(bytes) }}"
  inf = Zlib::Inflate.new(47)
  out = +""
  result = try { slices(bytes, 3).each { |c| out << inf.inflate(c) }; inf.finish }
  puts "  #{result} #{digest(out)} #{try { inf.total_in }} #{try { inf.avail_in }}"
end
16.times do |i|
  len = 300 + i * 41
  dyn = lcg_bytes(len, 100 + i, "ab")
  hdr = "\x78\x9c".b + lcg_bytes(len, 200 + i)
  puts "random #{i} #{try { Zlib.inflate(hdr) }} #{try { Zlib::Inflate.new(-15).inflate(dyn) }}"
end
seeds = [Zlib.deflate(INPUTS["text"], 9), Zlib.deflate(INPUTS["mixed"], 1),
         Zlib::Deflate.new(6, 31).deflate(INPUTS["short"] * 50, Zlib::FINISH),
         Zlib::Deflate.new(9, -15, 9, Zlib::FIXED).deflate(INPUTS["runs"], Zlib::FINISH)]
x = 12345
300.times do |i|
  bytes = seeds[i % seeds.size].dup
  (1 + i % 4).times do
    x = (x * 214013 + 2531011) & 0x7fffffff
    at = x % bytes.bytesize
    x = (x * 214013 + 2531011) & 0x7fffffff
    bytes.setbyte(at, (x >> 16) & 0xff)
  end
  wbits = (i % seeds.size) == 3 ? -15 : 47
  inf = Zlib::Inflate.new(wbits)
  puts "mut #{i} #{try { inf.inflate(bytes) }} #{try { inf.finish }} #{try { inf.total_in }}"
end

puts "== gzip writer"
def gz_bytes(level = nil, strategy = nil)
  io = StringIO.new("".b)
  gz = Zlib::GzipWriter.new(io, level, strategy)
  gz.mtime = 1_000_000_000
  yield gz
  gz.close
  io.string
end
[nil, 0, 1, 6, 9].each do |level|
  [nil, Zlib::FILTERED, Zlib::HUFFMAN_ONLY, Zlib::RLE, Zlib::FIXED].each do |strategy|
    s = gz_bytes(level, strategy) { |gz| gz.write(INPUTS["mixed"]) }
    puts "#{level.inspect} #{strategy.inspect} #{digest(s)} #{hex(s.byteslice(0, 10))}"
  end
end
s = gz_bytes do |gz|
  gz.orig_name = "name.txt\0ignored"
  gz.comment = "a comment"
  p [gz.pos, gz.sync, gz.level, gz.os_code, gz.crc, gz.mtime.to_i]
  gz.puts "line one", ["nested", ["deeper"]], 42, nil
  gz.print "a", "b", :c
  gz.printf("%05d|%s\n", 42, "x")
  gz.putc "Q"
  gz.putc 0x41
  gz << "chained" << " more\n"
  p gz.write("w1", "w2", 3)
  gz.flush
  gz.write("after flush")
  gz.flush(Zlib::FULL_FLUSH)
  gz.sync = true
  gz.write("synced")
  p [gz.pos, gz.tell, gz.crc, gz.sync]
  puts try { gz.mtime = 5 }
  puts try { gz.orig_name = "x" }
end
p hex(s)
io = StringIO.new("".b)
gz = Zlib::GzipWriter.new(io)
gz.mtime = 1
p [gz.close.equal?(io), gz.closed?, io.closed?]
puts try { gz.write("x") }
puts try { gz.mtime }
p gz.close
io = StringIO.new("".b)
gz = Zlib::GzipWriter.new(io, 9)
gz.mtime = 77
gz.write("finish keeps the io open")
p [gz.finish.equal?(io), io.closed?, hex(io.string)]
r = Zlib::GzipWriter.wrap(StringIO.new("".b)) { |w| w.mtime = 3; w.write("wrapped"); w.to_io }
p [r.class, r.closed?, hex(r.string)]

puts "== gzip reader"
lines = "first line\nsecond line\n\n\n\nfourth paragraph\nüñíçødé line\n" + "x" * 5000 + "\nlast"
gz = gz_bytes(6) { |w| w.orig_name = "orig"; w.comment = "comm"; w.write(lines) }
r = Zlib::GzipReader.new(StringIO.new(gz))
p [r.orig_name, r.comment, r.mtime.to_i, r.level, r.os_code, r.external_encoding, r.lineno, r.pos]
p r.gets, r.lineno, r.pos
p r.gets(""), r.gets(nil, 5), r.gets("line"), r.gets(3)
p r.read(4), r.getc, r.getbyte, r.readchar, r.readbyte
r.ungetc("ZZ")
r.ungetbyte(0x42)
p r.read(8), r.pos
p r.readpartial(10), digest(r.read)
p [r.eof?, r.read, r.read(5), r.gets, r.getc, r.unused, r.crc]
puts try { r.readpartial(1) }
puts try { r.readline }
r.rewind
p [r.lineno, r.pos, r.read(10), r.eof]
p r.each_line.first(2)
r.rewind
p r.readlines.size
r.rewind
p r.each_char.first(3), r.each_byte.first(3)
p r.close.class, r.closed?
puts try { r.read }
r = Zlib::GzipReader.new(StringIO.new(gz), external_encoding: Encoding::UTF_8)
r.gets
r.gets
r.gets("")
r.gets
p r.getc, r.getc, r.gets(nil, 2), r.gets(nil, 3)
r = Zlib::GzipReader.new(StringIO.new(gz), encoding: "ISO-8859-1:UTF-8")
p r.external_encoding, r.read(70)
r = Zlib::GzipReader.new(StringIO.new(gz))
p r.read(0), r.read(3, buf = +"zzzzz"), buf, r.readpartial(2, buf), buf
puts try { r.read(-1) }
multi = gz_bytes { |w| w.write("member one\n") } + gz_bytes(1) { |w| w.write("member two\n") } + "junk"
r = Zlib::GzipReader.new(StringIO.new(multi))
p r.read, r.unused
p Zlib::GzipReader.zcat(StringIO.new(gz_bytes { |w| w.write("a\nb\n") } + gz_bytes { |w| w.write("c\n") }))
got = []
Zlib::GzipReader.zcat(StringIO.new(gz_bytes { |w| w.write("a\nb\n") } + gz_bytes { |w| w.write("c\n") })) { |l| got << l }
p got
p Zlib::GzipReader.wrap(StringIO.new(gz)) { |rr| rr.read(5) }
[
  "", "\x1f".b, "not gzip at all", "\x1f\x8b\x09\x00".b + "\x00" * 6,
  "\x1f\x8b\x08\x02".b + "\x00" * 6, "\x1f\x8b\x08\x20".b + "\x00" * 6, "\x1f\x8b\x08\x80".b + "\x00" * 6,
  "\x1f\x8b\x08\x40".b + "\x00" * 6, "\x1f\x8b\x08\x04".b + "\x00" * 6 + "\x05".b,
  gz.byteslice(0, 30), gz.byteslice(0..-5),
  gz.byteslice(0..-9), gz.byteslice(0..-2) + "\x00".b,
  gz.byteslice(0..-9) + "\x00\x00\x00\x00".b + gz.byteslice(-4..)
].each_with_index do |bytes, i|
  out = try do
    r = Zlib::GzipReader.new(StringIO.new(bytes))
    [r.read, r.close].map { |v| v.is_a?(String) ? digest(v) : v.class }
  end
  e = begin
    Zlib::GzipReader.new(StringIO.new(bytes)).read
    nil
  rescue Zlib::GzipFile::Error => err
    err.input.nil? ? "no input" : digest(err.input)
  rescue StandardError
    "other"
  end
  puts "bad #{i} #{out} #{e} #{try { Zlib.gunzip(bytes) }}"
end
# A name with no NUL after it: from an IO, the reader asks for more and finds
# none. (Zlib.gunzip, which has no IO, is left out: CRuby calls `read` on the
# Qundef it keeps there.)
r = (Zlib::GzipReader.new(StringIO.new("\x1f\x8b\x08\x08".b + "\x00" * 6 + "abc".b)) rescue $!)
p r
err = (Zlib::GzipReader.new(StringIO.new("not gzip at all")) rescue $!)
p err.inspect, err.input

puts "== Zlib.gzip and gunzip"
[nil, 0, 1, 9].each do |level|
  [nil, Zlib::HUFFMAN_ONLY].each do |strategy|
    g = Zlib.gzip(INPUTS["text"], level: level, strategy: strategy)
    g.setbyte(4, 0); g.setbyte(5, 0); g.setbyte(6, 0); g.setbyte(7, 0)
    puts "#{level.inspect} #{strategy.inspect} #{digest(g)} #{hex(g.byteslice(0, 10))} #{digest(Zlib.gunzip(g))}"
  end
end
puts try { Zlib.gzip("x", level: 11) }
puts try { Zlib.gzip("x", nope: 1) }
p Zlib.gunzip(Zlib.gzip("")), Zlib.gunzip(Zlib.gzip("")).encoding

puts "== a block that leaves"
data = lcg_bytes(200_000, 21, "abcdefghijklmnopqrstuvwxyz")
z = Zlib.deflate(data)
inf = Zlib::Inflate.new
n = 0
inf.inflate(z) { |c| n += c.bytesize; break if n > 40_000 }
p [n, inf.avail_in, inf.total_in, inf.total_out, inf.finished?]
rest = inf.inflate("") 
p [rest&.bytesize, inf.finished?, inf.total_out]
begin
  inf2 = Zlib::Inflate.new
  inf2.inflate(z) { |c| raise "stop" }
rescue => e
  p [e.message, inf2.avail_in, inf2.total_out]
end
d = Zlib::Deflate.new(0)
sizes = []
d.deflate(data, Zlib::FINISH) { |c| sizes << c.bytesize; break if sizes.size == 3 }
p [sizes, d.avail_in, d.total_in]
p d.finish.bytesize
