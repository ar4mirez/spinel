# An IO the collector frees is finalized, as CRuby's `rb_io_fptr_finalize`
# does (#460): its write converter is finished, which closes an
# `xml: :attr` quote and writes ISO-2022-JP's escape back to ASCII (#420),
# and its descriptor is closed, unless it is not to `autoclose` it. Measured
# on ruby 4.0.9, which prints the same.
#
# Every IO meant to die is made inside a method, so that once the method
# returns nothing refers to it.
dir = ARGV[0]

# The files under `dir` some descriptor is open on, found by matching each
# descriptor's device and inode, since not every platform has /proc/self/fd.
def open_fds(dir)
  files = Dir.children(dir).to_h { |name| s = File.stat(File.join(dir, name)); [[s.dev, s.ino], name] }
  (3...256).filter_map { |fd| s = (IO.for_fd(fd, autoclose: false).stat rescue nil) and files[[s.dev, s.ino]] }.sort
end

def quoted(path) = (io = File.open(path, "w:UTF-8", xml: :attr); io.write("<a"); nil)
def utf16(path) = (io = File.open(path, "w:UTF-16LE", xml: :attr); io.write("a"); nil)
def unquoted(path) = (io = File.open(path, "w:UTF-8", xml: :attr); io.write("a"); io.binmode; nil)
# Reopened by path with its quote open: the new file is the one it closes
# in (#488).
def reopened(path) = (io = File.open("#{path}-a", "w:UTF-8", xml: :attr); io.write("x"); io.reopen("#{path}-b", "w"); io.write("y"); nil)
def stateful(path) = (io = File.open(path, "w:ISO-2022-JP"); io.write("あ"); io.write("い"); nil)
def quoted_stateful(path) = (io = File.open(path, "w:ISO-2022-JP", xml: :attr); io.write("あ"); nil)
# Part of a character, which no error can be raised for: it is dropped.
def begun(path) = (io = File.open(path, "w:UTF-16LE"); io.write("a\xE3"); nil)
def left(path) = (File.open(path, "w"); nil)
def kept(path) = (IO.new(IO.sysopen(path, "w"), autoclose: false); nil)
def copied(path) = (io = File.open(path, "w"); io.dup; io.close; nil)
def undefined(path) = (ObjectSpace.undefine_finalizer(File.open(path, "w")); nil)
# Closed before it is collected: the number it had is another file's by
# then, and that one stays open.
def closed(path) = (File.open(path, "w").close; nil)

quoted(File.join(dir, "quoted"))
utf16(File.join(dir, "utf16"))
unquoted(File.join(dir, "unquoted"))
reopened(File.join(dir, "reopened"))
stateful(File.join(dir, "stateful"))
quoted_stateful(File.join(dir, "quoted_stateful"))
begun(File.join(dir, "begun"))
left(File.join(dir, "left"))
kept(File.join(dir, "kept"))
copied(File.join(dir, "copied"))
undefined(File.join(dir, "undefined"))
closed(File.join(dir, "closed"))
reused = File.open(File.join(dir, "reused"), "w")
GC.start
p File.binread(File.join(dir, "quoted"))
p File.binread(File.join(dir, "utf16"))
p File.binread(File.join(dir, "unquoted"))
p File.binread(File.join(dir, "reopened-a")), File.binread(File.join(dir, "reopened-b"))
p File.binread(File.join(dir, "stateful")), File.binread(File.join(dir, "quoted_stateful")), File.binread(File.join(dir, "begun"))
p open_fds(dir)
reused.write("ok")
reused.close
p File.read(File.join(dir, "reused"))
