# Files left open at exit, each with a converter to finish, in the directory
# given. CRuby finishes each one when it finalizes the File, which writes the
# attribute's closing quote (#456) and ISO-2022-JP's escape back to ASCII
# (#420). Measured on ruby 4.0.9.
dir = ARGV[0]
at_exit { $stdout.write("at_exit\n") }
a = File.open(File.join(dir, "a"), "w:UTF-8", xml: :attr)
a.write("<")
a.write("&")
empty = File.open(File.join(dir, "empty"), "w:UTF-8", xml: :attr)
empty.write("")
utf16 = File.open(File.join(dir, "utf16"), "w:UTF-16LE", xml: :attr)
utf16.write("a")
closed = File.open(File.join(dir, "closed"), "w:UTF-8", xml: :attr)
closed.write("a")
closed.close
bin = File.open(File.join(dir, "binmode"), "w:UTF-8", xml: :attr)
bin.write("a")
bin.binmode
both = File.open(File.join(dir, "both"), "w:UTF-8", xml: :attr)
both.set_encoding("UTF-8", xml: :text)
both.write("<") rescue nil
stateful = File.open(File.join(dir, "stateful"), "w:ISO-2022-JP")
stateful.write("あ")
at_exit { stateful.write("い") }
utf16_mark = File.open(File.join(dir, "utf16_mark"), "wb:UTF-16")
utf16_mark.write("a")
utf16_mark.write("b")
# Part of a character is the converter's error, which is dropped at the end.
begun = File.open(File.join(dir, "begun"), "wb:UTF-16LE")
begun.write("a\xE3")
$stdout.set_encoding("UTF-8", xml: :attr)
$stdout.write("a")
