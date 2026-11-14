# `require`, `require_relative` and `load` (#39), in Ruby over one primitive:
# `__load_file__` parses, compiles and runs a file's top level in a frame of
# its own. Finding the file, `$LOAD_PATH`, and `$LOADED_FEATURES` are here.
#
# The runtime owns these globals, so they are read-only once set.

$LOAD_PATH = []
alias $: $LOAD_PATH
alias $-I $LOAD_PATH
# What CRuby 4.0 has provided before a program starts, recorded as it
# records them: here, the classes Spinel defines in `core/`. `pathname.so`
# is only `Pathname`'s core half, so `require "pathname"` still loads
# `pathname.rb` from the standard library.
$LOADED_FEATURES = %w[
  enumerator.so thread.rb fiber.so rational.so complex.so pathname.so ruby2_keywords.rb set.rb
]
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

  # Relative to the file's real path, as CRuby's `rb_current_realfilepath`
  # has it: a file reached through a symlink requires beside its target.
  def require_relative(name)
    name = File.__path__(name)
    file, = __caller_binding__.source_location
    if file.nil? || file.start_with?("(eval at ")
      raise LoadError, "cannot infer basepath"
    end
    real = __fs_realpath__(file)
    file = real if String === real
    Kernel.__require__(File.absolute_path(name, File.dirname(file)))
  end

  def load(name, wrap = false)
    name = File.__path__(name)
    path = Kernel.__find_load__(name)
    module_ = Module === wrap ? wrap : (wrap ? Module.new : nil)
    __load_file__(path, module_, module_ ? Kernel.__wrapped_main__(module_) : nil)
    true
  end

  module_function :require, :require_relative, :load

  # Features CRuby loads from a file of its architecture directory when
  # asked, and Spinel defines in `core/` before a program starts: the names
  # `require` takes for each, and the file CRuby records for it.
  def self.__core_features__
    @__core_features__ ||= {
      "rbconfig" => "rbconfig.rb", "rbconfig.rb" => "rbconfig.rb",
      "io/wait" => "io/wait.so", "io/wait.so" => "io/wait.so",
    }.freeze
  end

  # The first `require` of one records it, under the architecture directory
  # as CRuby does, and answers true; the class is already there.
  def self.__require_core_feature__(file)
    archdir = RbConfig::CONFIG["rubyarchdir"]
    feature = archdir ? File.join(archdir, file) : file
    return false if $LOADED_FEATURES.include?(feature)
    $LOADED_FEATURES.push(feature)
    true
  end

  def self.__require__(name)
    name = File.__path__(name)
    core = __core_features__[name]
    return __require_core_feature__(core) if core
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
    features = $LOADED_FEATURES
    loading = (@__loading__ ||= [])
    loaders = (@__loaders__ ||= {})
    me = __thread_current__
    path = nil
    while true
      found, path = __search_required__(name)
      raise LoadError.__cannot_load__(name) if found.nil?
      # Provided already, by a file that finished loading.
      return false if path.nil?
      __native_extension__(path) if found == :s
      # Another thread is part-way through loading it: wait for it to finish,
      # as CRuby does, rather than answer as if it had (#45). Then look again,
      # since a load that raised leaves the file to load here.
      owner = loaders[path]
      break unless owner && owner != me && __thread_status__(owner) != :dead
      sleep(0.001) while loaders[path] == owner && __thread_status__(owner) != :dead
    end
    if loaders.key?(path)
      # Still running: the file, or one it required, asked for it again.
      if loading.include?(path)
        __warning__("loading in progress, circular require considered harmful - #{path}", true)
      end
      return false
    end
    # The same file by another name, through a symlink: loaded once.
    __sync_features__
    realpaths = @__realpaths__
    real = @__realpath_map__[path] || __realpath_of__(path)
    return false if realpaths.key?(real)
    # Recorded before the file runs, so a file requiring itself while it
    # loads answers false rather than recursing; removed again if it raises.
    features.push(path)
    __provided__(path, real)
    loading.push(path)
    loaders[path] = me
    autoloads = __autoload_feature__(path, me)
    loaded = false
    begin
      __load_file__(path, nil)
      __autoload_feature_done__(autoloads, path)
      loaded = true
    ensure
      # A load that did not finish — an exception, or a `throw` out of the
      # file — is not recorded, so the next `require` runs it again.
      features.delete(path) unless loaded
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
        owner.__autoload_forget__(name)
      end
    end
  end

  # --- Feature lookup: CRuby's `load.c`, in Ruby ------------------------------
  #
  # `require` asks `$LOADED_FEATURES` before it looks on disk, matching the
  # name as given and also as "load path entry/name.ext" for the load path as
  # it is now (`__feature_p__`). Only a name nothing provides is searched for,
  # each extension across the whole load path before the next (`__search__`).
  # ponytail: a linear scan of `$LOADED_FEATURES` per `require` (#39's D1),
  # where CRuby keeps a `features_index` by short name; the index goes in with
  # #66's require map, which is where lookup speed is the goal.

  # The extensions a name without one is tried with, in order: Ruby, then a
  # Spinel extension, then a CRuby one, which is found only to say so.
  def self.__loadable_ext__
    @__loadable_ext__ ||= [".rb", *__native_ext__].freeze
  end

  # A shared object's extensions, in the order they are looked for: a name
  # that asks for `foo.so` is answered by `foo.spinel` first.
  def self.__native_ext__
    @__native_ext__ ||= [".spinel", __dlext__].freeze
  end

  # A shared object `require` found and cannot load: a CRuby C extension, or
  # a Spinel one before its loader (#104). Like CRuby's failed `dlopen`, the
  # `LoadError` has no `path`, since the file is there.
  # ponytail: `.spinel` extensions are found but not loaded; #104 loads them.
  def self.__native_extension__(path)
    if path.end_with?(".spinel")
      raise LoadError, "Spinel extensions cannot be loaded yet (#104) - #{path}"
    end
    raise LoadError, "#{path} is a CRuby C extension, which Spinel cannot load; " \
                     "the gem needs a Spinel version - #{path}"
  end

  # A name a shared object provides is searched for as Ruby only.
  def self.__ruby_ext__
    @__ruby_ext__ ||= [".rb"].freeze
  end

  # CRuby's `IS_DLEXT`, and `IS_SOEXT` or `IS_DLEXT`, for matching recorded
  # features. Worked out on first use: `RUBY_PLATFORM` comes after this file.
  def self.__dlext__
    @__dlext__ ||= RUBY_PLATFORM.include?("darwin") ? ".bundle" : ".so"
  end

  # `.spinel` is one too, so `foo.so` and `foo.spinel` each answer for the
  # other when `require` asks whether a feature is loaded.
  def self.__so_exts__
    @__so_exts__ ||= [".so", ".o", __dlext__, ".spinel"].uniq.freeze
  end

  # CRuby's `search_required`: what kind of file `name` is (`:r` for Ruby,
  # `:s` for a shared object, `:u` for a feature recorded without an
  # extension) and the path to load, or `[kind, nil]` when a feature already
  # provides it, or nil when nothing does. The path is also answered when
  # the feature providing it is still loading, so `require` waits for it.
  def self.__search_required__(name)
    ext = __ext_of__(name)
    ft = loading = nil
    if ext
      if ext == ".rb"
        ft, loading = __feature_p__(name, ext, true, false)
        return [:r, loading] if ft
        found = __search__(name)
        return nil if found.nil?
        path = found[1]
        ft, loading = __feature_p__(path, ".rb", true, true)
        return [:r, ft.nil? || loading ? path : nil]
      elsif __so_exts__.include?(ext)
        ft, loading = __feature_p__(name, ext, false, false)
        return [:s, loading] if ft
        found = __search__(name[0, name.length - ext.length], __native_ext__)
        return nil if found.nil?
        path = found[1]
        ft, loading = __feature_p__(path, found[0], false, true)
        return [:s, ft.nil? || loading ? path : nil]
      elsif ext == ".bundle" || ext == ".dll"
        # Another platform's shared object: never one Spinel loads.
        return nil
      end
    else
      ft, loading = __feature_p__(name, nil, false, false)
      return [:r, loading] if ft == :r
    end
    found = __search__(name, ft == :s ? __ruby_ext__ : __loadable_ext__)
    if found.nil?
      return [ft, loading] if ft
      ft, = __feature_p__(name, nil, false, true)
      return ft ? [ft, nil] : nil
    end
    found_ext, path = found
    rb = found_ext == ".rb"
    return [ft, loading] if ft && !rb
    provided, loading = __feature_p__(path, found_ext, rb, true)
    [rb ? :r : :s, provided && !loading ? nil : path]
  end

  # CRuby's `rb_feature_p`: whether `$LOADED_FEATURES` provides `feature`, as
  # `[kind, loading]`, where `loading` is the entry when its file is still
  # loading, or nil. `ext` is the extension `feature` ends in, if it is one
  # `require` knows, and `rb` whether that is `.rb`. Unless `expanded`, an
  # entry also matches as "load path entry/feature.ext", for the load path
  # as it is now, so `require "foo"` finds the `foo.rb` an entry gave.
  def self.__feature_p__(feature, ext, rb, expanded)
    if ext
      len = feature.length - ext.length
      type = rb ? :r : :s
    else
      len = feature.length
      type = nil
    end
    # A name with its extension, matched only as given, is provided exactly
    # when an entry is that name: what the scan below comes to, in one look.
    if expanded && ext
      return nil unless $LOADED_FEATURES.include?(feature)
      return [type, __loading_entry__(feature)]
    end
    base = feature[0, len]
    # The entries CRuby's index lists under `feature`, in its order: for a
    # name without an extension, those ending in `.rb` come first.
    candidates = __features_containing__(feature).select { |entry| __feature_candidate__(entry, feature) }
    candidates = candidates.partition { |entry| entry.end_with?(".rb") }.flatten(1) if ext.nil?
    load_path = nil
    candidates.each do |entry|
      next if entry.length < len
      if entry.start_with?(base)
        rest = entry[len..]
      else
        next if expanded
        load_path ||= __expanded_load_path__
        prefix = __loaded_feature_path__(entry, feature, len, type, load_path)
        next if prefix.nil?
        # After one entry matches through the load path, only the name as
        # given does: CRuby's "distractor" rule.
        expanded = true
        rest = entry[(prefix.length + 1 + len)..]
      end
      if rest.empty?
        next if ext
        return [:u, __loading_entry__(entry)]
      end
      next unless rest.start_with?(".")
      return [:s, __loading_entry__(entry)] if (!rb || !ext) && __so_exts__.include?(rest)
      return [:r, __loading_entry__(entry)] if (rb || !ext) && rest == ".rb"
    end
    nil
  end

  # Whether CRuby's `features_index` lists `entry` under `key`: `key` is
  # `entry`, or what follows one of its separators, with or without the
  # extension `entry` ends in.
  def self.__feature_candidate__(entry, key)
    return true if entry == key || entry.end_with?("/" + key)
    dot = entry.rindex(".")
    return false if dot.nil? || dot == 0 || entry.index("/", dot)
    bare = entry[0, dot]
    bare == key || bare.end_with?("/" + key)
  end

  # CRuby's `loaded_feature_path`: the load path entry `name` is
  # "entry/feature.ext" under, with an extension `type` accepts, or nil.
  def self.__loaded_feature_path__(name, feature, len, type, load_path)
    vlen = name.length
    return nil if vlen < len + 1
    base = feature[0, len]
    if feature.include?(".") && name.end_with?(base)
      plen = vlen - len
    else
      dot = name.rindex(".")
      slash = name.rindex("/")
      return nil if dot.nil? || dot == 0 || (slash && slash > dot)
      return nil if dot < len || name[dot - len, len] != base
      plen = dot - len
    end
    return nil if plen > 0 && name[plen - 1] != "/"
    rest = name[(plen + len)..]
    return nil if type == :s && rest != __dlext__ && rest != ".spinel"
    return nil if type == :r && rest != ".rb"
    plen -= 1 if plen > 0
    prefix = name[0, plen]
    load_path.include?(prefix) ? prefix : nil
  end

  def self.__loading_entry__(entry)
    loaders = @__loaders__
    loaders && loaders.key?(entry) ? entry : nil
  end

  # The extension `name` ends in: from its last `.`, unless a separator
  # follows it.
  def self.__ext_of__(name)
    dot = name.rindex(".")
    return nil if dot.nil? || name.index("/", dot)
    name[dot..]
  end

  # CRuby's expanded load path: each `$LOAD_PATH` entry made absolute and
  # resolved through its symlinks, kept until `$LOAD_PATH` changes. Relative
  # entries are expanded again when the working directory changes, `~` ones
  # every time, and entries that are not Strings (a `to_path`) every time too;
  # String entries are frozen, as CRuby freezes them.
  def self.__expanded_load_path__
    load_path = $LOAD_PATH
    expanded = @__expanded_load_path__
    if expanded.nil? || !(@__load_path_snapshot__ == load_path)
      relative = non_cache = false
      expanded = load_path.map do |entry|
        if String === entry
          entry.freeze
        else
          non_cache = true
        end
        relative = true unless File.__path__(entry).start_with?("/")
        __expand_load_path_entry__(entry)
      end
      @__load_path_check__ = relative ? Dir.pwd : (non_cache ? :non_cache : nil)
      @__load_path_snapshot__ = load_path.dup
    elsif (check = @__load_path_check__) == :non_cache
      expanded = __reexpand_load_path__(load_path, expanded) { false }
    elsif check
      cwd = Dir.pwd
      if check == cwd
        expanded = __reexpand_load_path__(load_path, expanded) { |path| path.start_with?("~") }
      else
        @__load_path_check__ = cwd
        expanded = __reexpand_load_path__(load_path, expanded) { |path| !path.start_with?("/") }
      end
    end
    @__expanded_load_path__ = expanded.freeze
  end

  # The cached expansion, with the String entries the block picks and every
  # other entry expanded again.
  def self.__reexpand_load_path__(load_path, expanded)
    changed = false
    fresh = load_path.each_with_index.map do |entry, i|
      next expanded[i] if String === entry && !yield(entry)
      changed = true
      __expand_load_path_entry__(entry)
    end
    changed ? fresh : expanded
  end

  def self.__expand_load_path_entry__(entry)
    path = File.expand_path(File.__path__(entry))
    real = __fs_realpath__(path)
    return path unless String === real
    File.__in_path_encoding__(File.__fs_string__(real), path.encoding)
  end

  # The `$LOADED_FEATURES` entries `feature` is part of, in order. The scan
  # is a search of the entries joined into one string, so an entry costs a
  # look at its bytes rather than a block call; only one that contains
  # `feature` is handed back to be matched. The joined copy is there for
  # speed: with the plain `select` scan, a Ruby block per entry, the
  # `bench/bytecode_cache.rb` warm run took about 970 ms, against about 525 ms
  # with the copy and about 500 ms before this lookup. It is still the linear
  # scan above, with no index; the `features_index` is #66's.
  def self.__features_containing__(feature)
    __sync_features__
    text = @__features_text__
    return $LOADED_FEATURES.select { |entry| String === entry && entry.include?(feature) } if text.nil?
    key = feature.b
    at = text.index(key)
    return [] if at.nil?
    features = @__features_snapshot__
    found = []
    index = 0
    counted = 0
    until at.nil?
      index += text.byteslice(counted, at - counted).count("\0")
      entry = features[index]
      found << entry if String === entry
      counted = text.index("\0", at)
      break if counted.nil?
      at = text.index(key, counted + 1)
    end
    found
  end

  # What `require` keeps beside `$LOADED_FEATURES`, worked out again when it
  # changes other than by `require`: a copy to tell that by, the entries
  # joined with NULs for `__features_containing__` (nil when one holds a NUL
  # itself), and CRuby's realpath set, so one file required under two names
  # through symlinks loads once. Each entry's realpath is kept from the last
  # build, as CRuby keeps it.
  def self.__sync_features__
    features = $LOADED_FEATURES
    return if @__realpaths__ && @__features_snapshot__ == features
    previous = @__realpath_map__ || {}
    map = {}
    realpaths = {}
    text = "".b
    features.each_with_index do |entry, i|
      text << "\0" unless i == 0
      next unless String === entry
      text << entry.b
      real = previous[entry] || __realpath_of__(entry)
      map[entry] = real
      realpaths[real] = true
    end
    text = nil if text.count("\0") != [features.length - 1, 0].max
    @__features_snapshot__ = features.dup
    @__features_text__ = text
    @__realpath_map__ = map
    @__realpaths__ = realpaths
  end

  # `path`, just pushed onto `$LOADED_FEATURES` by `require`, recorded in
  # what `__sync_features__` keeps.
  def self.__provided__(path, real)
    text = @__features_text__
    unless text.nil?
      text << "\0" unless @__features_snapshot__.empty?
      text << path.b
    end
    @__features_snapshot__.push(path)
    @__realpath_map__[path] = real
    @__realpaths__[real] = true
  end

  # A feature recorded without a directory, as the provided ones are, is not
  # a file in the working directory.
  def self.__realpath_of__(path)
    return path unless path.start_with?("/")
    real = __fs_realpath__(path)
    String === real ? real : path
  end

  # The file `require` would load for `name`, or a `LoadError`: what an
  # autoload is keyed by.
  def self.__find_feature__(name)
    ext = __ext_of__(name)
    raise LoadError.__cannot_load__(name) if __so_exts__.include?(ext) || ext == ".bundle" || ext == ".dll"
    found = ext == ".rb" ? __search__(name) : __search__(name, __loadable_ext__)
    raise LoadError.__cannot_load__(name) if found.nil?
    found[1]
  end

  # `load` takes the name as written, then tries it against the working
  # directory when no load path entry has it.
  def self.__find_load__(name)
    found = __search__(name)&.last
    found ||= File.expand_path(name) if File.file?(name)
    raise LoadError.__cannot_load__(name) if found.nil?
    found
  end

  # CRuby's `rb_find_file_ext`: `name` with each of `exts` in turn, across
  # the whole expanded load path before the next, as `[ext, path]`, or nil.
  # A path relative to the working directory on purpose is looked for there.
  def self.__search__(name, exts = [""])
    if __explicit__(name)
      path = File.expand_path(name)
      exts.each do |ext|
        return [ext, path + ext] if File.file?(path + ext)
      end
      return nil
    end
    dirs = __expanded_load_path__
    exts.each do |ext|
      file = name + ext
      dirs.each do |dir|
        path = __search_entry__(dir, file)
        return [ext, path] unless path.nil?
      end
    end
    nil
  end

  # One expanded load path entry's answer for `file`: where a per-directory
  # require map (#66) answers without a `stat`.
  def self.__search_entry__(dir, file)
    path = File.expand_path(file, dir)
    File.file?(path) ? path : nil
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
