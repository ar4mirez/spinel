# The default external encoding is the locale's (#41).
path = ARGV[0]
p [Encoding.default_external, Encoding.find("locale"), Encoding.find("filesystem"), Encoding.locale_charmap]
text = File.read(path)
p [text.encoding, text.valid_encoding?, ARGV[0].encoding, ENV.fetch("HOME").encoding, Dir.pwd.encoding]
p Encoding.default_external.names.last(3)
p File.open(path, "r:Windows-1252:UTF-8", &:read).encoding
