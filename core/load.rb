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
# `$DEBUG`, and `$-d` its other name: `-d` sets it.
$DEBUG = false
alias $-d $DEBUG
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

  # What CRuby has loaded before a program starts, so `require` of one
  # answers false rather than looking for a file: here, the core classes
  # Spinel defines in `core/`.
  BUILTIN_FEATURES = %w[
    enumerator enumerator.so thread thread.rb fiber fiber.so rbconfig rbconfig.rb set set.rb io/wait io/wait.so
  ].freeze

  def self.__require__(name)
    name = File.__path__(name)
    return false if BUILTIN_FEATURES.include?(name)
    # ponytail: RubyGems is vendored with the rest of the stdlib (#47) but
    # cannot load yet — it wants `Module#autoload`, `Monitor` and a `Mutex`
    # (#319, #45) — and code that probes for it rescues the LoadError a Ruby
    # without it raises. Until it loads, requiring it is that LoadError.
    if name == "rubygems" || name == "rubygems.rb"
      raise LoadError.__cannot_load__(name)
    end
    # A built-in gem: its source is in the binary, and it is recorded the way
    # CRuby records the extension it stands in for.
    shipped = name.end_with?(".so") ? name[0, name.length - 3] : name
    source = __library__(shipped)
    unless source.nil?
      feature = shipped + ".so"
      return false if $LOADED_FEATURES.include?(feature)
      $LOADED_FEATURES.push(feature)
      begin
        __load_file__("<internal:" + shipped + ">", nil, nil, source)
      rescue Exception
        $LOADED_FEATURES.delete(feature)
        raise
      end
      return true
    end
    # A C extension in CRuby whose whole effect is one constant.
    if name == "rbconfig/sizeof"
      return false if RbConfig.const_defined?(:SIZEOF, false)
      RbConfig.__define_sizeof__
      $LOADED_FEATURES.push("rbconfig/sizeof.so")
      return true
    end
    path = __find_feature__(name)
    real = __fs_realpath__(path)
    real = path if Integer === real
    features = $LOADED_FEATURES
    loading = (@__loading__ ||= [])
    # Another thread is part-way through loading it: wait for it to finish,
    # as CRuby does, rather than answer as if it had (#45).
    loaders = (@__loaders__ ||= {})
    me = __thread_current__
    while (owner = loaders[path]) && owner != me && __thread_status__(owner) != :dead
      sleep(0.001)
    end
    if features.include?(path) || features.include?(real)
      # Still running: the file, or one it required, asked for it again.
      if loading.include?(path)
        __warning__("loading in progress, circular require considered harmful - #{path}", true)
      end
      return false
    end
    # Recorded before the file runs, so a file requiring itself while it
    # loads answers false rather than recursing; removed again if it raises.
    features.push(path)
    loading.push(path)
    loaders[path] = me
    autoloads = __autoload_feature__(path, me)
    begin
      __load_file__(path, nil)
      __autoload_feature_done__(autoloads, path)
    rescue Exception
      features.delete(path)
      raise
    ensure
      loading.delete(path)
      loaders.delete(path)
      autoloads&.each { |owner, name| owner.instance_variable_get(:@__autoloading__)&.delete(name) }
    end
    true
  end

  # A file an autoload names is that autoload running, however it came to be
  # required: while it loads, the loading thread sees the names it registered
  # as not yet defined. Answers the `[module, name]` pairs still waiting on it.
  def self.__autoload_feature__(path, me)
    registry = @__autoload_features__
    return nil if registry.nil?
    autoloads = registry[path]
    return nil if autoloads.nil?
    autoloads = autoloads.select { |owner, name| owner.__autoload_entry__(name) }
    autoloads.each { |owner, name| owner.instance_variable_get(:@__autoload_spent__)&.delete(name) }
    autoloads.each do |owner, name|
      loading = owner.instance_variable_get(:@__autoloading__) ||
                owner.instance_variable_set(:@__autoloading__, {})
      loading[name] = me
    end
    Module.instance_variable_set(:@__autoloaded__, true) unless autoloads.empty?
    autoloads
  end

  # Once the file has loaded, its autoloads are spent: a name it defined is a
  # constant, and one it did not stays listed in `constants` but does not
  # autoload while the file stays in `$LOADED_FEATURES` — CRuby's
  # `check_autoload_required`. A file that raised leaves them registered.
  def self.__autoload_feature_done__(autoloads, path)
    return if autoloads.nil?
    autoloads.each do |owner, name|
      entries = owner.instance_variable_get(:@__autoloads__)
      next if entries.nil? || !entries.key?(name)
      if __reflect_const_lookup__(owner, name, false).nil?
        spent = owner.instance_variable_get(:@__autoload_spent__) ||
                owner.instance_variable_set(:@__autoload_spent__, {})
        spent[name] = path
      else
        entries.delete(name)
      end
    end
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

  # The `self` of a wrapped `load`: a stand-in for `main`, with the wrapper
  # extended into it, so a top-level `include` lands in the wrapper rather
  # than in `Object`.
  def self.__wrapped_main__(wrapper)
    main = Object.new
    # What the real `main` was extended with comes along, behind the wrapper.
    real = __main__
    extended = real.singleton_class.ancestors.drop(1).take_while { |ancestor| !ancestor.equal?(Object) }
    extended.reverse_each { |mod| main.extend(mod) }
    main.extend(wrapper)
    main.define_singleton_method(:to_s) { "main" }
    main.define_singleton_method(:inspect) { "main" }
    main.define_singleton_method(:include) { |*modules| wrapper.include(*modules) }
    main.singleton_class.__send__(:alias_method, :using, :__main_using__)
    main.singleton_class.__send__(:private, :using)
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
