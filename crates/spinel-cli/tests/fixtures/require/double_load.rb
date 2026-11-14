# A file required, then a directory with a file of the same name put ahead
# of it on the load path, as `Bundler.setup` and gem activation do: the
# feature is already provided, so the second `require` loads nothing.
$LOAD_PATH.unshift File.join(__dir__, "stdlib")
p require("double_load")
$LOAD_PATH.unshift File.join(__dir__, "gemlib")
p require("double_load")
p require("double_load.rb")
p DOUBLE_LOAD
p $LOADED_FEATURES.grep(/double_load/).map { |path| path.split("/").last(2).join("/") }
