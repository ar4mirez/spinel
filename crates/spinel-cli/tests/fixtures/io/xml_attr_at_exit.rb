# Files left open at exit, each with `xml: :attr`, in the directory given.
# CRuby finishes each one's converter when it finalizes the File, which
# writes the attribute's closing quote (#456). Measured on ruby 4.0.9.
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
$stdout.set_encoding("UTF-8", xml: :attr)
$stdout.write("a")
