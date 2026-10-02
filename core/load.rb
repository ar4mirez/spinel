# `require`, `require_relative` and `load` (#39), in Ruby over one primitive:
# `__load_file__` parses, compiles and runs a file's top level in a frame of
# its own. Finding the file, `$LOAD_PATH`, and `$LOADED_FEATURES` are here.
#
# The runtime owns these globals, so they are read-only once set.

$LOAD_PATH = []
alias $: $LOAD_PATH
alias $-I $LOAD_PATH
$LOADED_FEATURES = []
alias $" $LOADED_FEATURES
# The command line's `-a`, `-l` and `-p`, which Spinel does not take.
$-a = false
$-l = false
$-p = false
# `$?` is the last child's status, nil until `Process` (#43) starts one.
__freeze_global__(:$LOAD_PATH, :$:, :$-I, :$LOADED_FEATURES, :$", :$-a, :$-l, :$-p, :$?)

class LoadError
  def path
    @path
  end
end

module Kernel
  def require(name)
    Kernel.__require__(name)
  end

  def require_relative(name)
    name = File.__path__(name)
    file, = __caller_binding__.source_location
    if file.nil? || file.start_with?("(eval at ")
      raise LoadError, "cannot infer basepath"
    end
    Kernel.__require__(File.expand_path(name, File.dirname(file)))
  end

  def load(name, wrap = false)
    name = File.__path__(name)
    path = Kernel.__find_load__(name)
    module_ = Module === wrap ? wrap : (wrap ? Module.new : nil)
    __load_file__(path, module_, module_ ? Kernel.__wrapped_main__(module_) : nil)
    true
  end

  module_function :require, :require_relative, :load

  def self.__require__(name)
    name = File.__path__(name)
    path = __find_feature__(name)
    real = __fs_realpath__(path)
    real = path if Integer === real
    features = $LOADED_FEATURES
    return false if features.include?(path) || features.include?(real)
    # Recorded before the file runs, so a file requiring itself while it
    # loads answers false rather than recursing; removed again if it raises.
    features.push(path)
    begin
      __load_file__(path, nil)
    rescue Exception
      features.delete(path)
      raise
    end
    true
  end

  # The file `require` would load for `name`, or a `LoadError`.
  def self.__find_feature__(name)
    raise LoadError.__cannot_load__(name) if name.end_with?(".so", ".o", ".bundle", ".dll")
    candidate = name.end_with?(".rb") ? name : name + ".rb"
    found = __search__(candidate)
    raise LoadError.__cannot_load__(name) if found.nil?
    found
  end

  # `load` takes the name as written, then tries it against the working
  # directory when no load path entry has it.
  def self.__find_load__(name)
    found = __search__(name)
    found ||= File.expand_path(name) if File.file?(name)
    raise LoadError.__cannot_load__(name) if found.nil?
    found
  end

  def self.__search__(name)
    if __explicit__(name)
      path = File.expand_path(name)
      return File.file?(path) ? path : nil
    end
    $LOAD_PATH.each do |dir|
      path = File.expand_path(name, File.__path__(dir))
      return path if File.file?(path)
    end
    nil
  end

  # The `self` of a wrapped `load`: `main` again, but with the wrapper
  # extended into it, so a top-level `include` lands in the wrapper rather
  # than in `Object`.
  def self.__wrapped_main__(wrapper)
    main = Object.new
    main.extend(wrapper)
    main.define_singleton_method(:to_s) { "main" }
    main.define_singleton_method(:inspect) { "main" }
    main.define_singleton_method(:include) { |*modules| wrapper.include(*modules) }
    main
  end

  # An absolute path, or one relative to the working directory on purpose.
  def self.__explicit__(name)
    name.start_with?("/", "./", "../", "~") || name == "." || name == ".."
  end
end

class LoadError
  def self.__cannot_load__(name)
    error = new("cannot load such file -- #{name}")
    error.instance_variable_set(:@path, name)
    error
  end
end
