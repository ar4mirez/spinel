# What CRuby 4.0 provides before a program starts answers false, and a CRuby
# C extension on the load path is found and refused, with no `path` (#39 S2).
p $LOADED_FEATURES.first(8)
p require("set")
p require("complex")
p require("pathname.so")
# `pathname.so` is Pathname's core half: the library's Ruby half still loads.
p require("pathname")
p Pathname.method_defined?(:find)
$LOAD_PATH.unshift File.join(__dir__, "native")
# The platform's shared-object extension, as CRuby looks for: `.so`, or
# `.bundle` on macOS.
begin
  require "cext"
rescue LoadError => e
  p [e.message.include?("cext.#{RbConfig::CONFIG["DLEXT"]}"), e.path]
end
