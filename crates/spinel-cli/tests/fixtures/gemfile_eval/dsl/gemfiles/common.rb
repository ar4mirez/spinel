# Evaluated by eval_gemfile, relative to the Gemfile; this file's own
# eval_gemfile is relative to this file.
gem "common", __dir__ == File.dirname(__FILE__) ? "1.0" : "0"
eval_gemfile "nested.rb"
