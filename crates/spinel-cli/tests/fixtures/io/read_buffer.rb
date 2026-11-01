# How far one read of a stateful converter goes on a long file, where nothing
# stops the call before CRuby's 128KB of bytes or of what it makes (#513).
n = File.join(ARGV[0], "long")
g = ->(s, **o) do
  File.binwrite(n, s)
  File.open(n, "r:UTF-8:ISO-2022-JP", **o) do |f|
    begin
      f.getc
    rescue EncodingError => e
      print e.class, " "
    end
    puts f.pos
  end
end
e = "é".b
g.(e * 33000, undef: :replace)
g.(e * 22000, xml: :attr)
