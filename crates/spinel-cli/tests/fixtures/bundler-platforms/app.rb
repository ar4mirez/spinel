# The app `crates/spinel-cli/tests/bundler_setup.rs` boots once `spinel
# install` has installed this directory's Gemfile.lock: it boots through
# `bundler/setup`, says how Bundler sees the Ruby running (`RUBY_ENGINE`,
# `CurrentRuby#ruby?`, `#mri?`), and prints the gems the bundle put on the
# load path.
require "bundler/setup"

puts [RUBY_ENGINE, Bundler.current_ruby.ruby?, Bundler.current_ruby.mri?].join(" ")
bundle = Bundler.bundle_path.to_s
puts $LOAD_PATH.select { |path| path.start_with?(bundle) }.map { |path| path[%r{/gems/([^/]+)/}, 1] }.sort.join(" ")
