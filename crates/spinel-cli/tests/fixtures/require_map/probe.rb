# What the require map says for each name given, over the expanded load
# path: each entry a search for it stops at, in order — "map <entry>" where
# the map says the entry has it (no look on disk), "disk <entry>" where it
# must be looked at on disk — and "none" where the map says no entry after
# the last stop has it. A search stops at the first entry that has the file;
# this one goes on, to show every stop. Not something a program calls: it is
# how the test sees the map in use. Entries are named relative to this
# directory, the standard library as `rubylibdir`.
$LOAD_PATH.unshift(File.join(__dir__, "beta/lib"), File.join(__dir__, "alpha/lib"))
dirs = Kernel.__expanded_load_path__
stdlib = File.realpath(RbConfig::CONFIG["rubylibdir"])
name = lambda do |dir|
  next "rubylibdir" if dir == stdlib
  dir.start_with?(__dir__ + "/") ? dir[(__dir__.length + 1)..] : "elsewhere"
end
ARGV.each do |file|
  stops = []
  at = Kernel.__require_map_scan__(dirs, file, 0)
  until at.nil?
    if at < 0
      stops << "map #{name.(dirs[-at - 1])}"
      at = -at - 1
    else
      stops << "disk #{name.(dirs[at])}"
    end
    at = Kernel.__require_map_scan__(dirs, file, at + 1)
  end
  stops << "none" unless stops.last&.start_with?("map")
  puts "#{file}: #{stops.join(", ")}"
end
