# Reads and writes a Windows-1252 file through IO's converters (#41).
src, out = ARGV

File.open(src, "r:Windows-1252:UTF-8") do |f|
  text = f.read
  p [text, text.encoding, text.valid_encoding?]
end

File.open(src, "r:Windows-1252:UTF-8") do |f|
  p [f.gets, f.getc, f.external_encoding, f.internal_encoding]
end

File.open(src, "r:Windows-1252") do |f|
  line = f.gets
  p [line, line.encoding, line.encode("UTF-8")]
end

File.open(out, "w:Windows-1252") { |f| f.write("naïve € “x” µ\n") }
p File.binread(out)

File.open(out, "w:Windows-1252") do |f|
  f.write("あ")
rescue Encoding::UndefinedConversionError => e
  p [e.class, e.message, e.source_encoding_name, e.destination_encoding_name]
end

File.binwrite(out, "a\x81b")
File.open(out, "r:Windows-1252:UTF-8") do |f|
  f.read
rescue Encoding::UndefinedConversionError => e
  p [e.class, e.message, e.error_char, e.source_encoding_name]
end

p File.read(src, encoding: "Windows-1252:UTF-8", newline: :universal)
p File.read(src, mode: "rb:Windows-1252:UTF-8", undef: :replace).bytesize
