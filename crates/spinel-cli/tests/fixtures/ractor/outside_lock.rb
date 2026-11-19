# A Ractor blocked in a call made outside its lock — `flock` waiting for a
# lock another descriptor holds — lets the class lock go while it waits
# (#119), so the main Ractor defines classes and methods meanwhile rather
# than waiting for a call that only the main Ractor can end. First a Ractor
# with one thread, then one whose two threads both wait there, so that its
# heap is left with no thread running it.
Warning[:experimental] = false
require "tmpdir"

Dir.mktmpdir do |dir|
  path = File.join(dir, "lock")
  File.write(path, "")
  held = File.open(path)
  held.flock(File::LOCK_EX)

  alone = Ractor.new(path) do |path|
    File.open(path) { |f| f.flock(File::LOCK_EX) }
    :alone
  end
  two = Ractor.new(path) do |path|
    thread = Thread.new { File.open(path) { |f| f.flock(File::LOCK_SH) }; :thread }
    File.open(path) { |f| f.flock(File::LOCK_SH) }
    [:main, thread.value]
  end
  sleep 0.2

  class Later
    def hello = :hello
  end
  Later.define_method(:world) { :world }
  p [Later.new.hello, Later.new.world]

  held.flock(File::LOCK_UN)
  p alone.value
  p two.value
  held.close
end
