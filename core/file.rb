# `File` and `Dir`: paths (#39), and since #41 a `File` that is an `IO` whose
# descriptor came from opening a path, with `File::Stat`, `fnmatch` and the
# directory and link calls beside it. All Ruby, over primitives that each make
# one system call and answer an errno on failure.

class File < IO
  SEPARATOR = "/"
  Separator = SEPARATOR
  ALT_SEPARATOR = nil
  PATH_SEPARATOR = ":"

  # A path argument: a String, or what `to_path` or `to_str` makes of one.
  def self.__path__(path)
    return __path_check__(path) if String === path
    __path_check__(__path_of__(path))
  end

  def self.__path_check__(path)
    unless path.encoding.ascii_compatible?
      raise Encoding::CompatibilityError, "path name must be ASCII-compatible (#{path.encoding}): #{path.inspect}"
    end
    raise ArgumentError, "path name contains null byte" if path.include?("\0")
    path
  end

  def self.__path_of__(path)
    # Measured: what `to_path` answers is held to being a String, so a
    # `to_path` of 1 cannot open descriptor 1.
    path = Kernel.__check_funcall__(path, :to_path) { path }
    return path if String === path
    converted = Kernel.__check_funcall__(path, :to_str) do
      raise TypeError, "no implicit conversion of #{nil.equal?(path) ? "nil" : Kernel.__class_of__(path)} into String"
    end
    return converted if String === converted
    klass = Kernel.__class_of__(path)
    raise TypeError, "can't convert #{klass} to String (#{klass}#to_str gives #{Kernel.__class_of__(converted)})"
  end

  # Raise the `SystemCallError` a primitive answered with an errno.
  def self.__check__(result, path, where = nil)
    return result unless Integer === result
    # `strerror @ where - path`, which is how CRuby's `rb_syserr_fail_path`
    # names the C function that made the call.
    raise SystemCallError.new(path, result, where)
  end

  # In the encoding CRuby's `rb_file_join` gives the result: it starts as an
  # empty US-ASCII string and each part goes in by `fs_enc_check`, so the
  # paths' own encoding is kept. Nothing to join is an empty binary string.
  def self.join(*parts)
    return "".b if parts.empty?
    out = "".dup.force_encoding(Encoding::US_ASCII)
    __join_into__(out, parts, [], true)
    out
  end

  # Exactly one separator between parts, keeping the ones a part already
  # has at its own ends: `File.join("q//", "b")` is "q//b". Measured.
  def self.__join_into__(out, parts, seen, first)
    parts.each do |part|
      # An empty Array is an empty part, joined as nothing to join is:
      # `File.join([], "a")` is a binary "/a".
      part = "".b if Array === part && part.empty?
      if Array === part
        raise ArgumentError, "recursive array" if seen.any? { |s| s.equal?(part) }
        seen.push(part)
        first = __join_into__(out, part, seen, first)
        seen.pop
        next
      end
      part = __path_of__(part) unless String === part
      raise ArgumentError, "string contains null byte" if part.include?("\0")
      if first
      elsif out.end_with?("/") && part.start_with?("/")
        # Both sides have separators: the right part's win. Measured.
        out.sub!(%r{/+\z}, "")
      elsif !out.end_with?("/") && !part.start_with?("/")
        out << "/"
      end
      encoding = __fs_enc_check__(out, part)
      out << part
      out.force_encoding(encoding)
      first = false
    end
    first
  end

  def self.basename(path, suffix = nil)
    path = __path__(path)
    return "" if path.empty?
    stripped = path.sub(%r{/+\z}, "")
    return "/" if stripped.empty?
    base = stripped.sub(%r{\A.*/}, "")
    unless suffix.nil?
      suffix = __path__(suffix)
      if suffix == ".*"
        dot = base.rindex(".")
        base = base[0, dot] if dot && dot > 0
      elsif base.end_with?(suffix) && base != suffix
        base = base[0, base.size - suffix.size]
      end
    end
    base
  end

  def self.dirname(path, level = 1)
    path = __path__(path)
    raise ArgumentError, "negative level: #{level}" if level < 0
    level.times { path = __dirname__(path) }
    path
  end

  def self.__dirname__(path)
    stripped = path.sub(%r{/+\z}, "")
    return path.start_with?("/") ? "/" : "." if stripped.empty?
    slash = stripped.rindex("/")
    return "." if slash.nil?
    dir = stripped[0, slash].sub(%r{/+\z}, "")
    # Repeated leading separators are one. Measured.
    dir.empty? ? "/" : dir.sub(%r{\A/+}, "/")
  end

  def self.extname(path)
    base = basename(path)
    dot = base.rindex(".")
    return "" if dot.nil? || dot == 0 || base.sub(/\A\.+/, "").index(".").nil?
    base[dot..]
  end

  def self.absolute_path?(path)
    __path__(path).start_with?("/")
  end

  def self.expand_path(path, dir = nil)
    path = __path__(path)
    if path.start_with?("~")
      slash = path.index("/")
      user = slash.nil? ? path[1..] : path[1...slash]
      home = __fs_string__(user.empty? ? Dir.home : Dir.home(user))
      raise ArgumentError, "non-absolute home" unless home.start_with?("/")
      encoding = __fs_enc_check__(home, path)
      path = home + (slash.nil? ? "" : path[slash..])
    end
    __expand__(path, encoding, dir) { |base| expand_path(base) }
  end

  def self.absolute_path(path, dir = nil)
    __expand__(__path__(path), nil, dir) { |base| absolute_path(base) }
  end

  # `path` made absolute against `dir`, expanded by the block, or the working
  # directory, in the encoding CRuby's `rb_file_expand_path_internal` gives
  # it: an absolute path's own, else what `fs_enc_check` makes of it and the
  # directory, which is in the filesystem encoding when it is the working one.
  def self.__expand__(path, encoding, dir)
    if path.start_with?("/")
      expanded = path
    elsif dir.nil?
      cwd = __fs_string__(Dir.pwd)
      encoding = __fs_enc_check__(path, cwd)
      expanded = join(cwd, path)
    else
      base = yield(dir)
      encoding = __fs_enc_check__(base, path)
      expanded = join(base, path)
    end
    __normalize__(expanded).force_encoding(encoding || path.encoding)
  end

  # A path the system handed back, in the filesystem encoding as CRuby tags
  # one.
  def self.__fs_string__(path)
    path.dup.force_encoding(Encoding.find("filesystem"))
  end

  # CRuby's `fs_enc_check`: the encoding two paths combine in, except that
  # where that is US-ASCII it is the first's own, or failing that the second's.
  def self.__fs_enc_check__(first, second)
    encoding = first.__combined_encoding__(second)
    return encoding unless encoding == Encoding::US_ASCII
    first.encoding == Encoding::US_ASCII ? second.encoding : first.encoding
  end

  # `.` and `..` resolved lexically, separators collapsed.
  # Measured: the run of separators a path starts with is kept as written,
  # `//a` and `////a` included, until a `..` climbs back past it.
  def self.__normalize__(path)
    # Already normal, with no repeated separator, no `.` or `..` and no
    # trailing separator, a copy of it is the result: one regexp search, not
    # a step per component, which made `expand_path` of a long path cost its
    # length many times over (#49). Searched as bytes, so a broken path is
    # looked at as the split below looks at it.
    return path.dup if path.start_with?("/") && !path.b.match?(%r{//|/\.\.?(?:/|\z)|./\z})
    root = path[/\A\/+/] || "/"
    parts = []
    path.split("/").each do |part|
      next if part.empty? || part == "."
      if part == ".."
        root = "/" unless parts.empty?
        parts.pop
      else
        parts.push(part)
      end
    end
    root + parts.join("/")
  end

  def self.realpath(path, dir = nil)
    path = __path__(path)
    # The result is in the encoding of `path` as given, not of its join with
    # `dir`. Measured.
    encoding = path.encoding
    path = join(__path__(dir), path) unless dir.nil? || path.start_with?("/")
    resolved = __check__(__fs_realpath__(path), path, "rb_check_realpath_internal")
    __in_path_encoding__(__fs_string__(resolved), encoding)
  end

  # A path the system resolved, in the encoding of the one it was asked
  # about, as CRuby's `rb_check_realpath_internal` gives it: transcoded where
  # it can be, else tagged with that encoding as it is. Measured.
  def self.__in_path_encoding__(resolved, encoding)
    return resolved if resolved.encoding == encoding
    return resolved.force_encoding(encoding) if resolved.ascii_only?
    begin
      resolved.encode(encoding)
    rescue EncodingError
      resolved.force_encoding(encoding)
    end
  end

  # `realpath`, but the last component need not exist. The path is worked
  # out in raw bytes, as CRuby's `rb_check_realpath_emulate` does it.
  def self.realdirpath(path, dir = nil)
    path = __path__(path)
    dir = __path__(dir) unless dir.nil?
    cwd = Dir.pwd.b
    # `absolute_path`, not `expand_path`: a leading "~" in `path` or `dir` is
    # a file name here, as it is to `rb_check_realpath_emulate` (#410).
    base = dir.nil? ? cwd : absolute_path(dir.b, cwd).b
    expanded = absolute_path(path.b, base).b
    resolved = if exist?(expanded) || symlink?(expanded)
      realpath(expanded)
    else
      join(realpath(dirname(expanded)), basename(expanded))
    end
    __realpath_rec_encoding__(resolved.b, path, dir)
  end

  # The encoding `rb_check_realpath_emulate` gives the path it resolved.
  # It starts from the root of `path` when that is absolute, else of `dir`
  # when that is, else of the working directory, which it reads as binary;
  # a binary or US-ASCII `path` starts it in the filesystem encoding
  # instead. Whatever that is, the bytes go on in it, and are then tagged
  # with `path`'s encoding where they are ASCII or that is binary, else
  # transcoded to it where they can be, and kept as they are where they
  # cannot. Measured.
  def self.__realpath_rec_encoding__(resolved, path, dir)
    encoding = path.encoding
    root = if path.b.start_with?("/")
      encoding
    elsif !dir.nil? && dir.b.start_with?("/")
      dir.encoding
    else
      Encoding::BINARY
    end
    root = Encoding.find("filesystem") if encoding == Encoding::BINARY || encoding == Encoding::US_ASCII
    resolved = resolved.force_encoding(root)
    return resolved if root == encoding
    return resolved.force_encoding(encoding) if resolved.ascii_only? || encoding == Encoding::BINARY
    resolved.encode(encoding)
  rescue EncodingError
    resolved
  end

  def self.exist?(path)
    !__fs_kind__(__path__(path), true).nil?
  end

  def self.file?(path)
    __fs_kind__(__path__(path), true) == :file
  end

  def self.directory?(path)
    __fs_kind__(__path__(path), true) == :directory
  end

  # `access(2)` with the real ids, as CRuby's `rb_eaccess` falls back to.
  def self.readable?(path) = __fs_access__(__path__(path), 4, true)
  def self.writable?(path) = __fs_access__(__path__(path), 2, true)
  def self.executable?(path) = __fs_access__(__path__(path), 1, true)
  def self.readable_real?(path) = __fs_access__(__path__(path), 4, false)
  def self.writable_real?(path) = __fs_access__(__path__(path), 2, false)
  def self.executable_real?(path) = __fs_access__(__path__(path), 1, false)

  def self.symlink?(path)
    __fs_kind__(__path__(path), false) == :link
  end

  # --- the descriptor half (#41) ---------------------------------------------
  #
  # A `File` is an `IO` whose descriptor came from opening a path; reading,
  # writing and positions are all `IO`'s. What is here is what a path adds.

  def initialize(path, mode = nil, perm = nil, **options)
    # A descriptor something else opened.
    return super(path, mode, **options) if Integer === path
    raise ArgumentError, "mode specified twice" if !mode.nil? && options.key?(:mode)
    path = File.__path__(path)
    given = mode.nil? ? options.fetch(:mode, "r") : mode
    flags = IO.__mode_flags__(given)[0]
    flags |= Integer.__index__(options[:flags]) if options[:flags]
    perm = options[:perm] if perm.nil?
    options = options.reject { |key, _| key == :perm }
    bom = __modeenc__(mode, options)
    perm = perm.nil? ? 0666 : Integer.__index__(perm)
    fd = IO.__check__(__fd_open__(path, flags, perm), path, "rb_sysopen")[0]
    @path = path.dup
    begin
      __open_fd__(fd, options, bom)
    rescue Exception
      __fd_close__(fd)
      raise
    end
  end

  # What writes in progress have gathered goes first, as `rb_file_size`'s
  # `flush_before_seek` on an IO that writes.
  def size
    __check_open__
    __flush_gathered__ if __writes__?
    stat.size
  end

  def lstat
    __check_open__
    File.lstat(@path)
  end

  def chmod(mode)
    __check_open__
    IO.__check__(__fs_chmod__(@path, IO.__c_uint__(mode)), @path)
    0
  end

  def chown(owner, group)
    __check_open__
    File.chown(owner, group, @path)
    0
  end

  # CRuby's `rb_file_truncate`: the length first, then the IO, whose bytes
  # buffered are given back as `flush` gives them, and what writes in progress
  # have gathered written.
  def truncate(length)
    length = Integer.__index__(length)
    __writable__
    __flush_gathered__
    IO.__check__(__fs_truncate__(@fileno, length), @path)
    0
  end

  # `flock(2)`. With `LOCK_NB`, a lock someone else holds is `false`. What
  # writes in progress have gathered goes first, as `rb_file_flock` flushes an
  # IO that writes.
  def flock(operation)
    __check_open__
    __flush_gathered__ if __writes__?
    result = __fd_flock__(@fileno, Integer.__index__(operation))
    return false if result == Errno::EWOULDBLOCK::Errno && Integer.__index__(operation) & LOCK_NB != 0
    IO.__check__(result, @path)
    0
  end

  def atime = stat.atime
  def mtime = stat.mtime
  def ctime = stat.ctime

  def birthtime
    __check_open__
    raise NotImplementedError, "birthtime() function is unimplemented on this machine"
  end

  # --- what a path names -------------------------------------------------------

  def self.path(path) = __path__(path).dup

  def self.split(path)
    path = __path__(path)
    [dirname(path), basename(path)]
  end

  def self.umask(*mask)
    if mask.size > 1
      raise ArgumentError, "wrong number of arguments (given #{mask.size}, expected 0..1)"
    end
    __fs_umask__(mask.empty? ? nil : IO.__c_uint__(mask[0]))
  end

  def self.delete(*paths)
    paths.each do |path|
      path = __path__(path)
      __check__(__fs_unlink__(path), path, "apply2files")
    end
    paths.size
  end

  def self.unlink(*paths)
    delete(*paths)
  end

  # The calls that name two paths name both in their error, in parentheses.
  def self.__check_two__(result, from, to, where)
    raise SystemCallError.new("(#{from}, #{to})", result, where) if Integer === result
    0
  end

  def self.rename(from, to)
    from = __path__(from)
    to = __path__(to)
    __check_two__(__fs_rename__(from, to), from, to, "rb_file_s_rename")
  end

  def self.symlink(target, link)
    target = __path__(target)
    link = __path__(link)
    __check_two__(__fs_symlink__(target, link), target, link, "rb_file_s_symlink")
  end

  def self.link(target, link)
    target = __path__(target)
    link = __path__(link)
    __check_two__(__fs_link__(target, link), target, link, "rb_file_s_link")
  end

  def self.mkfifo(path, mode = 0666)
    path = __path__(path)
    __check__(__fs_mkfifo__(path, Integer.__index__(mode)), path, "rb_file_s_mkfifo")
    0
  end

  def self.readlink(path)
    path = __path__(path)
    __check__(__fs_readlink__(path), path, "rb_readlink")
  end

  def self.chmod(mode, *paths)
    mode = IO.__c_uint__(mode)
    paths.each do |path|
      path = __path__(path)
      __check__(__fs_chmod__(path, mode), path, "apply2files")
    end
    paths.size
  end

  # A nil owner or group, or -1, leaves that one as it is.
  def self.__chown__(owner, group, paths, follow)
    owner = owner.nil? ? -1 : Integer.__index__(owner)
    group = group.nil? ? -1 : Integer.__index__(group)
    paths.each do |path|
      path = __path__(path)
      __check__(__fs_chown__(path, owner, group, follow), path, "apply2files")
    end
    paths.size
  end

  def self.chown(owner, group, *paths) = __chown__(owner, group, paths, true)
  def self.lchown(owner, group, *paths) = __chown__(owner, group, paths, false)

  # A time for `utime`: nil is now, a Time its seconds and nanoseconds, a
  # number seconds since the epoch.
  def self.__utime_parts__(time)
    return [nil, nil] if time.nil?
    return [time.to_i, time.nsec] if Time === time
    time = Time.at(time)
    [time.to_i, time.nsec]
  end

  def self.__utime__(atime, mtime, paths, follow)
    # Measured: one nil is now for both.
    atime, mtime = Time.now, Time.now if atime.nil? && mtime.nil?
    a = __utime_parts__(atime)
    m = __utime_parts__(mtime)
    paths.each do |path|
      path = __path__(path)
      __check__(__fs_utime__(path, a[0], a[1], m[0], m[1], follow), path, "rb_file_s_utime")
    end
    paths.size
  end

  def self.utime(atime, mtime, *paths) = __utime__(atime, mtime, paths, true)
  def self.lutime(atime, mtime, *paths) = __utime__(atime, mtime, paths, false)

  def self.truncate(path, length)
    path = __path__(path)
    __check__(__fs_truncate__(path, Integer.__index__(length)), path, "rb_file_s_truncate")
    0
  end

  def self.stat(path)
    Stat.new(path)
  end

  def self.lstat(path)
    Stat.__lstat__(path)
  end

  # A path, or an IO, which is asked about its descriptor.
  def self.__stat_of__(path)
    return stat(path) if String === path
    io = IO.try_convert(path)
    io.nil? ? stat(path) : io.stat
  end

  def self.__stat_or_nil__(path)
    __stat_of__(path)
  rescue SystemCallError
    nil
  end

  def self.size(path)
    __stat_of__(path).size
  end

  def self.size?(path)
    stat = __stat_or_nil__(path)
    stat.nil? || stat.size == 0 ? nil : stat.size
  end

  def self.zero?(path)
    stat = __stat_or_nil__(path)
    !stat.nil? && stat.size == 0
  end

  def self.empty?(path)
    zero?(path)
  end

  def self.atime(path) = __stat_of__(path).atime
  def self.mtime(path) = __stat_of__(path).mtime
  def self.ctime(path) = __stat_of__(path).ctime

  def self.birthtime(path)
    __stat_of__(path)
    raise NotImplementedError, "birthtime() function is unimplemented on this machine"
  end

  def self.ftype(path)
    lstat(path).ftype
  end

  def self.identical?(first, second)
    a = __stat_or_nil__(first)
    b = __stat_or_nil__(second)
    !a.nil? && !b.nil? && a.dev == b.dev && a.ino == b.ino
  end

  # The questions `File::Stat` answers, asked of a path that may not exist.
  %i[pipe? socket? blockdev? chardev? setuid? setgid? sticky? owned? grpowned?
     world_readable? world_writable?].each do |name|
    define_singleton_method(name) do |path|
      stat = __stat_or_nil__(path)
      stat.nil? ? (name.to_s.start_with?("world") ? nil : false) : stat.__send__(name)
    end
  end

  # --- `fnmatch` ---------------------------------------------------------------
  #
  # CRuby's `fnmatch_helper`, `fnmatch` and `bracket` from `dir.c`, ported
  # over the pattern's and the path's characters, with nil past the end
  # where C has the terminating NUL.

  def self.fnmatch(pattern, path, flags = 0)
    pattern = __path__(pattern)
    path = __path__(path)
    flags = Integer.__index__(flags)
    if flags & FNM_EXTGLOB != 0
      return __brace_expand__(pattern.chars, flags).any? { |one| __fnmatch__(one, path.chars, flags) }
    end
    __fnmatch__(pattern.chars, path.chars, flags)
  end

  singleton_class.alias_method :fnmatch?, :fnmatch

  # CRuby's `ruby_brace_expand`: every pattern a `{a,b}` stands for, nested
  # ones included, with a backslash keeping the next character literal. A
  # `{` without its `}` stands for nothing.
  def self.__brace_expand__(chars, flags)
    escape = flags & FNM_NOESCAPE == 0
    lbrace = rbrace = nil
    nest = 0
    p = 0
    while p < chars.size
      if chars[p] == "{"
        lbrace = p if nest == 0
        nest += 1
      end
      if chars[p] == "}" && !lbrace.nil?
        nest -= 1
        if nest == 0
          rbrace = p
          break
        end
      end
      if chars[p] == "\\" && escape
        p += 1
        break if p >= chars.size
      end
      p += 1
    end
    return [chars] if lbrace.nil?
    return [] if rbrace.nil?
    head = chars[0, lbrace]
    tail = chars[rbrace + 1..]
    out = []
    p = lbrace
    while p < rbrace
      p += 1
      start = p
      nest = 0
      while p < rbrace && !(chars[p] == "," && nest == 0)
        nest += 1 if chars[p] == "{"
        nest -= 1 if chars[p] == "}"
        if chars[p] == "\\" && escape
          p += 1
          break if p == rbrace
        end
        p += 1
      end
      out.concat(__brace_expand__(head + chars[start...p] + tail, flags))
    end
    out
  end

  def self.__fnmatch__(pc, sc, flags)
    return __fnmatch_helper__(pc, 0, sc, 0, flags)[0] if flags & FNM_PATHNAME == 0
    period = flags & FNM_DOTMATCH == 0
    p = s = 0
    ptmp = stmp = nil
    loop do
      if pc[p] == "*" && pc[p + 1] == "*" && pc[p + 2] == "/"
        p += 3 while pc[p] == "*" && pc[p + 1] == "*" && pc[p + 2] == "/"
        ptmp = p
        stmp = s
      end
      matched, p, s = __fnmatch_helper__(pc, p, sc, s, flags)
      if matched
        s += 1 while !sc[s].nil? && sc[s] != "/"
        if !pc[p].nil? && !sc[s].nil?
          p += 1
          s += 1
          next
        end
        return true if pc[p].nil? && sc[s].nil?
      end
      # Failed: let `**/` swallow one more directory.
      if !ptmp.nil? && !(period && sc[stmp] == ".")
        stmp += 1 while !sc[stmp].nil? && sc[stmp] != "/"
        unless sc[stmp].nil?
          p = ptmp
          stmp += 1
          s = stmp
          next
        end
      end
      return false
    end
  end

  # One segment of the path, or all of it without `FNM_PATHNAME`. Answers
  # whether it matched and where each side stopped.
  def self.__fnmatch_helper__(pc, p, sc, s, flags)
    period = flags & FNM_DOTMATCH == 0
    pathname = flags & FNM_PATHNAME != 0
    escape = flags & FNM_NOESCAPE == 0
    nocase = flags & FNM_CASEFOLD != 0
    unescape = ->(i) { escape && pc[i] == "\\" ? i + 1 : i }
    at_end = ->(chars, i) { chars[i].nil? || (pathname && chars[i] == "/") }
    # A leading period is matched only by one.
    return [false, p, s] if period && sc[s] == "." && pc[unescape.(p)] != "."
    ptmp = stmp = nil
    loop do
      failed = false
      case pc[p]
      when "*"
        p += 1 while pc[p] == "*"
        return [true, unescape.(p), s] if at_end.(pc, unescape.(p))
        return [false, p, s] if at_end.(sc, s)
        ptmp = p
        stmp = s
        next
      when "?"
        return [false, p, s] if at_end.(sc, s)
        p += 1
        s += 1
        next
      when "["
        return [false, p, s] if at_end.(sc, s)
        after = __fnmatch_bracket__(pc, p + 1, sc[s], flags)
        if after
          p = after
          s += 1
          next
        end
        failed = true
      end
      unless failed
        p = unescape.(p)
        return [at_end.(pc, p), p, s] if at_end.(sc, s)
        if !at_end.(pc, p) && (pc[p] == sc[s] || (nocase && pc[p].upcase == sc[s].upcase))
          p += 1
          s += 1
          next
        end
      end
      # Try the last `*` one character further on.
      return [false, p, s] if ptmp.nil?
      p = ptmp
      stmp += 1
      s = stmp
    end
  end

  # A `[...]` at `p`, which is just past the `[`, against one character:
  # where the pattern goes on after the `]`, or nil when it does not match.
  def self.__fnmatch_bracket__(pc, p, char, flags)
    nocase = flags & FNM_CASEFOLD != 0
    escape = flags & FNM_NOESCAPE == 0
    return nil if pc[p].nil?
    ok = false
    negated = false
    if pc[p] == "!" || pc[p] == "^"
      negated = true
      p += 1
    end
    fold = ->(c) { nocase ? c.upcase.ord : c.ord }
    while pc[p] != "]"
      t1 = p
      t1 += 1 if escape && pc[t1] == "\\"
      return nil if pc[t1].nil?
      p = t1 + 1
      return nil if pc[p].nil?
      if pc[p] == "-" && pc[p + 1] != "]"
        t2 = p + 1
        t2 += 1 if escape && pc[t2] == "\\"
        return nil if pc[t2].nil?
        p = t2 + 1
        next if ok
        if pc[t1] == char || pc[t2] == char
          ok = true
          next
        end
        c = fold.(char)
        next if c < fold.(pc[t1]) || c > fold.(pc[t2])
      else
        next if ok
        if pc[t1] == char
          ok = true
          next
        end
        next unless nocase && pc[t1].upcase == char.upcase
      end
      ok = true
    end
    ok == negated ? nil : p + 1
  end

  # What `stat(2)` says about a path, as it was when asked.
  class Stat
    include Comparable

    def initialize(path)
      path = File.__path__(path)
      @fields = File.__check__(__fs_stat__(path, true), path, "rb_file_s_stat")
    end

    def self.__lstat__(path)
      path = File.__path__(path)
      __from__(File.__check__(__fs_stat__(path, false), path, "rb_file_s_lstat"))
    end

    def self.__from__(fields)
      stat = allocate
      stat.__fields__(fields)
      stat
    end

    def __fields__(fields)
      @fields = fields
    end

    def initialize_copy(other)
      @fields = other.__fields_of__.dup
    end

    def __fields_of__ = @fields

    def dev = @fields[0]
    def ino = @fields[1]
    def mode = @fields[2]
    def nlink = @fields[3]
    def uid = @fields[4]
    def gid = @fields[5]
    def rdev = @fields[6]
    def size = @fields[7]
    def blksize = @fields[8]
    def blocks = @fields[9]

    def atime = Time.at(@fields[10], @fields[11], :nanosecond)
    def mtime = Time.at(@fields[12], @fields[13], :nanosecond)
    def ctime = Time.at(@fields[14], @fields[15], :nanosecond)

    def birthtime
      raise NotImplementedError, "birthtime() function is unimplemented on this machine"
    end

    def <=>(other)
      return nil unless Stat === other
      mtime <=> other.mtime
    end

    # How the platform packs a device number: glibc's `major`/`minor` on
    # Linux, the BSDs' everywhere else.
    def self.__major__(dev)
      return ((dev >> 8) & 0xfff) | ((dev >> 32) & ~0xfff) if RUBY_PLATFORM.include?("linux")
      (dev >> 24) & 0xff
    end

    def self.__minor__(dev)
      return (dev & 0xff) | ((dev >> 12) & ~0xff) if RUBY_PLATFORM.include?("linux")
      dev & 0xffffff
    end

    def dev_major = Stat.__major__(dev)
    def dev_minor = Stat.__minor__(dev)
    def rdev_major = Stat.__major__(rdev)
    def rdev_minor = Stat.__minor__(rdev)

    # The file type bits of `st_mode`, which POSIX numbers the same everywhere.
    def __type__ = mode & 0170000

    def file? = __type__ == 0100000
    def directory? = __type__ == 0040000
    def symlink? = __type__ == 0120000
    def pipe? = __type__ == 0010000
    def socket? = __type__ == 0140000
    def blockdev? = __type__ == 0060000
    def chardev? = __type__ == 0020000

    def ftype
      case __type__
      when 0100000 then "file"
      when 0040000 then "directory"
      when 0120000 then "link"
      when 0010000 then "fifo"
      when 0140000 then "socket"
      when 0060000 then "blockSpecial"
      when 0020000 then "characterSpecial"
      else "unknown"
      end
    end

    def zero? = size == 0
    def size? = size == 0 ? nil : size

    def setuid? = mode & 04000 != 0
    def setgid? = mode & 02000 != 0
    def sticky? = mode & 01000 != 0

    # The permission bits when anyone may, nil when not.
    def world_readable? = mode & 0004 == 0 ? nil : mode & 0777
    def world_writable? = mode & 0002 == 0 ? nil : mode & 0777

    def owned? = uid == Process.euid
    # The effective group, or any of the supplementary ones, as CRuby's
    # `rb_group_member` asks.
    def grpowned? = gid == Process.egid || Process.groups.include?(gid)

    # CRuby's `rb_stat_r` and friends: the owner's bits for the owner, the
    # group's for the group, everyone's otherwise, and root may read and
    # write anything and execute what anyone may.
    def __may__(bit, user, group)
      if user == 0
        return true unless bit == 0100
        return mode & 0111 != 0 || directory?
      end
      return mode & bit != 0 if uid == user
      return mode & (bit >> 3) != 0 if gid == group
      mode & (bit >> 6) != 0
    end

    def readable? = __may__(0400, Process.euid, Process.egid)
    def writable? = __may__(0200, Process.euid, Process.egid)
    def executable? = __may__(0100, Process.euid, Process.egid)
    def readable_real? = __may__(0400, Process.uid, Process.gid)
    def writable_real? = __may__(0200, Process.uid, Process.gid)
    def executable_real? = __may__(0100, Process.uid, Process.gid)

    def inspect
      "#<File::Stat dev=0x#{dev.to_s(16)}, ino=#{ino}, mode=0#{mode.to_s(8)}, nlink=#{nlink}, " \
        "uid=#{uid}, gid=#{gid}, rdev=0x#{rdev.to_s(16)}, size=#{size}, blksize=#{blksize}, blocks=#{blocks}, " \
        "atime=#{atime.inspect}, mtime=#{mtime.inspect}, ctime=#{ctime.inspect}>"
    end
  end
end

# The predicates `File` answers as singleton methods, as a module of their
# own. CRuby defines each C function twice, once here and once on `File`;
# here the second definition is a call to the first.
module FileTest
  def blockdev?(path) = File.blockdev?(path)
  def chardev?(path) = File.chardev?(path)
  def directory?(path) = File.directory?(path)
  def empty?(path) = File.empty?(path)
  def executable?(path) = File.executable?(path)
  def executable_real?(path) = File.executable_real?(path)
  def exist?(path) = File.exist?(path)
  def file?(path) = File.file?(path)
  def grpowned?(path) = File.grpowned?(path)
  def identical?(first, second) = File.identical?(first, second)
  def owned?(path) = File.owned?(path)
  def pipe?(path) = File.pipe?(path)
  def readable?(path) = File.readable?(path)
  def readable_real?(path) = File.readable_real?(path)
  def setgid?(path) = File.setgid?(path)
  def setuid?(path) = File.setuid?(path)
  def size(path) = File.size(path)
  def size?(path) = File.size?(path)
  def socket?(path) = File.socket?(path)
  def sticky?(path) = File.sticky?(path)
  def symlink?(path) = File.symlink?(path)
  def world_readable?(path) = File.world_readable?(path)
  def world_writable?(path) = File.world_writable?(path)
  def writable?(path) = File.writable?(path)
  def writable_real?(path) = File.writable_real?(path)
  alias zero? empty?
  module_function(*instance_methods(false))
end

class Dir
  include Enumerable

  def self.pwd
    File.__check__(__fs_getcwd__, ".")
  end

  def self.getwd
    pwd
  end

# `HOME`, or the user's entry in the password database; with a name, that
# user's entry. Measured: a relative `HOME` is answered as it is.
def self.home(user = nil)
  if user.nil?
    home = ENV["HOME"]
    home = __passwd_home__(nil, Process.uid) if home.nil?
    raise ArgumentError, "couldn't find login name -- expanding '~'" if home.nil?
    return home.dup
  end
  user = IO.__str__(user)
  home = __passwd_home__(user, nil)
  raise ArgumentError, "user #{user} doesn't exist" if home.nil?
  home
end

# `getpwnam(3)` and `getpwuid(3)`, read from `/etc/passwd`.
# ponytail: only the users that file lists. One that NSS finds elsewhere
# (LDAP, systemd-homed) needs the libc call, which is `etc`'s (#111).
def self.__passwd_home__(user, uid)
  File.foreach("/etc/passwd") do |line|
    fields = line.chomp.split(":")
    next if fields.size < 6
    return fields[5] if user.nil? ? fields[2].to_i == uid : fields[0] == user
  end
  nil
rescue SystemCallError
  nil
end

  def self.exist?(path)
    File.directory?(path)
  end

  def self.mkdir(path, mode = 0777)
    path = File.__path__(path)
    File.__check__(__fs_mkdir__(path, Integer.__index__(mode)), path, "dir_s_mkdir")
    0
  end

  def self.rmdir(path)
    path = File.__path__(path)
    File.__check__(__fs_rmdir__(path), path, "dir_s_rmdir")
    0
  end

  def self.delete(path) = rmdir(path)
  def self.unlink(path) = rmdir(path)

  # `encoding:` tags each name with that encoding, as `dir.c` does; the
  # bytes are the file system's either way.
  def self.children(path, encoding: nil)
    __encode_names__(__children__(path), encoding)
  end

  def self.__children__(path)
    path = File.__path__(path)
    File.__check__(__fs_children__(path), path, "dir_initialize")
  end

  # Each name as `rb_external_str_new_with_enc` makes it: tagged with the
  # directory's encoding, which with no `encoding:` is the file system's,
  # the default external encoding as `rb_filesystem_encoding` answers it.
  def self.__encode_names__(names, encoding)
    encoding = encoding.nil? ? Encoding.find("filesystem") : Encoding.find(encoding)
    names.each { |name| name.force_encoding(encoding) }
    __external_names__(names)
  end

  # A binary name, or a US-ASCII one with non-ASCII bytes, is binary.
  # Otherwise it is converted to `Encoding.default_internal` when one is
  # set, as `rb_str_conv_enc` converts: an ASCII-only name is only retagged,
  # and one that will not convert is kept as it was.
  def self.__external_names__(names)
    internal = Encoding.default_internal
    names.map! do |name|
      from = name.encoding
      if from == Encoding::BINARY || (from == Encoding::US_ASCII && !name.ascii_only?)
        name.force_encoding(Encoding::BINARY)
      elsif internal.nil? || from == internal
        name
      elsif internal == Encoding::BINARY || (internal.ascii_compatible? && from.ascii_compatible? && name.ascii_only?)
        name.force_encoding(internal)
      else
        begin
          name.encode(internal)
        rescue EncodingError
          name
        end
      end
    end
  end

  # A path that is not a directory is not an empty one; a missing one raises.
  def self.empty?(path)
    path = File.__path__(path)
    children = __fs_children__(path)
    return false if children == Errno::ENOTDIR::Errno
    File.__check__(children, path, "rb_dir_s_empty_p").empty?
  end

  def self.entries(path, encoding: nil)
    __encode_names__([+".", +".."] + __children__(path), encoding)
  end

  def self.each_child(path, encoding: nil, &block)
    return enum_for(:each_child, path, encoding: encoding) unless block
    children(path, encoding: encoding).each(&block)
    nil
  end

  def self.foreach(path, encoding: nil, &block)
    return enum_for(:foreach, path, encoding: encoding) unless block
    entries(path, encoding: encoding).each(&block)
    nil
  end

  # With a block, the directory is closed when it returns, and its value is
  # the block's.
  def self.open(path, encoding: nil)
    dir = new(path, encoding: encoding)
    return dir unless block_given?
    begin
      yield dir
    ensure
      dir.close
    end
  end

  # A Dir over a descriptor it does not open, with no path, as `dir_s_for_fd`
  # makes one: `fdopendir(3)` checks it is a directory.
  def self.for_fd(fd)
    fd = Integer.__index__(fd)
    dir = allocate
    dir.__attach__(fd, nil, nil, "fdopendir")
    dir
  end

  # `fchdir(2)`; with a block, back to the directory it started in, held
  # open as a Dir for the while, as `dir_s_fchdir` does, so a working
  # directory removed meanwhile is still where it returns.
  def self.fchdir(fd)
    fd = Integer.__index__(fd)
    unless block_given?
      File.__check__(__fs_fchdir__(fd), "fchdir")
      return 0
    end
    before = new(".")
    begin
      File.__check__(__fs_fchdir__(fd), "fchdir")
      begin
        yield
      ensure
        File.__check__(__fs_fchdir__(before.fileno), "fchdir")
      end
    ensure
      before.close
    end
  end

  def self.chroot(path)
    path = File.__path__(path)
    File.__check__(__fs_chroot__(path), path, "dir_s_chroot")
    0
  end

  # ponytail: a Dir holds a descriptor, as CRuby's holds a `DIR *`, but reads
  # every entry when it is opened or rewound, so `pos` counts entries rather
  # than being `telldir(3)`'s cookie, and a name made after that is not seen.
  def initialize(path, encoding: nil)
    path = File.__path__(path)
    fd = File.__check__(__fd_opendir__(path), path, "dir_initialize")[0]
    __attach__(fd, path, encoding, "dir_initialize")
  end

  def __attach__(fd, path, encoding, where)
    @path = path
    @encoding = encoding
    @fileno = fd
    @closed = false
    @finalizer = Finalizer.new(fd)
    __finalizer_add__(self, @finalizer)
    begin
      __load__(where)
    rescue SystemCallError
      # Not this Dir's to close when it was given the descriptor.
      __fd_close__(fd) unless path.nil?
      @finalizer.fd = nil
      @closed = true
      raise
    end
  end

  def __load__(where = "rewinddir")
    names = __fd_dirents__(@fileno)
    raise SystemCallError.new(@path.nil? ? where : @path, names, @path.nil? ? nil : where) if Integer === names
    @entries = Dir.__encode_names__([+".", +".."] + names, @encoding)
    @pos = 0
  end

  # Closes the descriptor when a Dir nobody closed is collected.
  class Finalizer
    attr_writer :fd

    def initialize(fd)
      @fd = fd
    end

    def call(_id)
      fd = @fd
      @fd = nil
      __fd_close__(fd) unless fd.nil?
      nil
    rescue Exception
      nil
    end
  end
  private_constant :Finalizer

  def to_path = @path
  def inspect = "#<Dir:#{@path}>"

  def fileno
    __check_open__
    @fileno
  end

  def read
    __check_open__
    name = @entries[@pos]
    @pos += 1 unless name.nil?
    name
  end

  def each
    return enum_for(:each) unless block_given?
    __check_open__
    # From the start each time, as `dir_each_entry`'s `rewinddir`.
    @pos = 0
    while (name = read)
      yield name
    end
    self
  end

  def children
    __check_open__
    @entries.reject { |name| name == "." || name == ".." }
  end

  def each_child(&block)
    return enum_for(:each_child) unless block
    children.each(&block)
    self
  end

  def pos
    __check_open__
    @pos
  end

  def seek(pos)
    __check_open__
    @pos = Integer.__index__(pos)
    self
  end

  def pos=(pos)
    seek(pos)
    pos
  end

  def rewind
    __check_open__
    __load__
    self
  end

  # `fchdir(2)` to this directory, as `dir_chdir` does where there is one.
  def chdir(&block)
    Dir.fchdir(fileno, &block)
  end

  # A descriptor another Dir closed is `EBADF` here, as `closedir` finds it.
  def close
    return nil if @closed
    @closed = true
    @finalizer.fd = nil
    result = __fd_close__(@fileno)
    raise SystemCallError.new("closedir", result) if Integer === result
    nil
  end

  def __check_open__
    raise IOError, "closed directory" if @closed
  end

  # One pattern is `rb_push_glob`'s, several `dir_globs`'.
  def self.[](*patterns, base: nil, sort: true)
    glob(patterns.size == 1 ? patterns[0] : patterns, base: base, sort: sort)
  end

  # CRuby's `dir.c` glob, ported: `glob_make_pattern` cuts a pattern into
  # segments, and `glob_helper` walks them, reading a directory only where a
  # segment is magical or `**/` is recursing, and matching each name with
  # `File.fnmatch`'s own `fnmatch`. A brace is expanded where it is reached,
  # so `A*/{y,x}` answers `A1/y A1/x A2/y A2/x`; under `**/` one with no `/`
  # in it is matched name by name instead. Names are read sorted, whatever
  # `sort:` says. Measured on ruby 4.0.9: FNM_CASEFOLD changes nothing on
  # Linux, and FNM_EXTGLOB and FNM_PATHNAME nothing either.
  def self.glob(pattern, _flags = 0, flags: _flags, base: nil, sort: true, &block)
    flags = Integer.__index__(flags)
    unless sort == true || sort == false
      raise ArgumentError, "expected true or false as sort: #{sort.inspect}"
    end
    unless base.nil?
      base = File.__path__(base)
      base = nil if base.empty?
    end
    # A NUL once separated patterns in one String; in an Array it is in a
    # path, as any other.
    if String === pattern && pattern.include?("\0")
      raise ArgumentError, "nul-separated glob pattern is deprecated"
    end
    patterns = (Array === pattern ? pattern : [pattern]).map { |one| File.__path__(one) }
    results = []
    patterns.each { |one| __glob0__(one, flags & (File::FNM_NOESCAPE | File::FNM_DOTMATCH), base, results) }
    return results unless block
    results.each(&block)
    nil
  end

  # `ruby_glob0`: a leading `/` starts at the root, and otherwise `base:` is
  # where the walk starts, left out of what it answers.
  def self.__glob0__(pattern, flags, base, results)
    encoding = pattern.encoding
    encoding = Encoding.find("filesystem") if encoding == Encoding::US_ASCII
    encoding = Encoding::BINARY if encoding == Encoding::US_ASCII
    chars = pattern.dup.force_encoding(encoding).chars
    root = chars[0] == "/" ? 1 : 0
    if root == 0 && !base.nil?
      path = base.dup.force_encoding(encoding)
      baselen = path.bytesize
      dirsep = true
    else
      path = chars[0, root].join.force_encoding(encoding)
      baselen = 0
      dirsep = false
    end
    list = __glob_make_pattern__(chars[root..], flags)
    __glob_helper__(path, baselen, dirsep, :unknown, [list], flags, false, encoding, results)
  end

  # A segment as `glob_make_pattern` makes it: `[type, chars, next]`. A run
  # of plain segments before any `**/` is one, so the `/`s between them,
  # however many, are kept as written.
  def self.__glob_make_pattern__(c, flags)
    nodes = []
    p = 0
    e = c.size
    dirsep = false
    recursive = false
    while p < e
      if p + 2 < e && c[p] == "*" && c[p + 1] == "*" && c[p + 2] == "/"
        loop do
          p += 3
          p += 1 while c[p] == "/"
          break unless c[p] == "*" && c[p + 1] == "*" && c[p + 2] == "/"
        end
        nodes << [:recursive, nil, nil]
        dirsep = true
        recursive = true
      else
        m = __find_dirsep__(c, p, flags)
        magic = __has_magic__(c, p, m, flags)
        if magic == :plain && !recursive && m < e
          loop do
            m2 = __find_dirsep__(c, m + 1, flags)
            break unless m2 < e && __has_magic__(c, m + 1, m2, flags) == :plain
            m = m2
          end
        end
        nodes << [magic, c[p...m], nil]
        if m < e
          dirsep = true
          p = m + 1
        else
          dirsep = false
          p = m
        end
      end
    end
    nodes << [dirsep ? :match_dir : :match_all, nil, nil]
    (nodes.size - 1).times { |i| nodes[i][2] = nodes[i + 1] }
    nodes[0]
  end

  # The next `/` that ends a segment: not one in `[...]` or `{...}`, nor an
  # escaped one.
  def self.__find_dirsep__(c, p, flags)
    escape = flags & File::FNM_NOESCAPE == 0
    open = false
    while p < c.size
      ch = c[p]
      p += 1
      case ch
      when "[", "{" then open = true
      when "]", "}" then open = false
      when "/" then return p - 1 unless open
      when "\\"
        if escape
          return c.size if p >= c.size
          p += 1
        end
      end
    end
    c.size
  end

  def self.__has_magic__(c, p, pend, flags)
    escape = flags & File::FNM_NOESCAPE == 0
    magical = false
    while p < pend
      ch = c[p]
      p += 1
      case ch
      when "{" then return :brace
      when "*", "?", "[" then magical = true
      when "\\" then p += 1 if escape
      end
    end
    magical ? :magical : :plain
  end

  # A segment list written back as a pattern, from `node` on, for a brace to
  # be expanded over: `join_path_from_pattern`.
  def self.__join_pattern__(node)
    out = nil
    until node.nil?
      part = case node[0]
             when :recursive then ["*", "*"]
             when :match_dir then []
             else node[1]
             end
      unless part.nil?
        out = out.nil? ? part.dup : out + ["/"] + part
      end
      node = node[2]
    end
    out || []
  end

  def self.__glob_join__(path, dirsep, name)
    dirsep ? path + "/" + name : path + name
  end

  # `glob_helper`: what is at `path` against every segment list that has
  # reached it. `pathtype` is what is known of it, `:unknown` until it is
  # looked at; `skipdot` is set once a directory has been read, so `.` is
  # offered only in the first.
  def self.__glob_helper__(path, baselen, dirsep, pathtype, lists, flags, skipdot, encoding, results)
    plain = magical = recursive = match_all = match_dir = brace = false
    lists.each do |node|
      if node[0] == :recursive
        recursive = true
        node = node[2]
      end
      case node[0]
      when :plain then plain = true
      when :brace then brace = true if !recursive || node[1].include?("/")
      when :magical then magical = true
      when :match_all then match_all = true
      when :match_dir then match_dir = true
      end
    end

    if brace
      File.__brace_expand__(__join_pattern__(lists[0]), flags).each do |expanded|
        list = __glob_make_pattern__(expanded, flags)
        __glob_helper__(path, baselen, dirsep, pathtype, [list], flags, skipdot, encoding, results)
      end
      return
    end

    unless path.empty?
      pathtype = __fs_kind__(path, false) || :noent if match_all && pathtype == :unknown
      if match_dir && (pathtype == :unknown || pathtype == :link)
        pathtype = __fs_kind__(path, true) || :noent
      end
      results << __glob_result__(path, baselen, "") if match_all && pathtype != :noent
      if match_dir && pathtype == :directory
        results << __glob_result__(path, baselen, dirsep ? "/" : "")
      end
    end
    return if pathtype == :noent

    dotmatch = flags & File::FNM_DOTMATCH != 0
    if magical || recursive
      names = __fs_children__(path.empty? ? "." : path)
      # A directory that cannot be read has nothing in it to match.
      return if Integer === names
      names.unshift(+".")
      names.sort!
      names.each do |name|
        name.force_encoding(encoding)
        kind = :unknown
        dotfile = 0
        if name.start_with?(".")
          dotfile = 1
          if name == "."
            # Not `.` while recursing, so as not to recurse forever.
            next if recursive && !dotmatch
            next if skipdot
            dotfile = 2
            kind = :directory
          elsif name == ".."
            next
          end
        end
        buf = __glob_join__(path, dirsep, name)
        # `**/` never descends into a dot directory unless FNM_DOTMATCH,
        # nor into `.`, nor through a link.
        descend = dotfile < (dotmatch ? 2 : 1)
        kind = __fs_kind__(buf, false) || :noent if recursive && descend && kind == :unknown
        chars = nil
        next_lists = []
        lists.each do |node|
          if node[0] == :recursive
            next_lists << node if descend && kind == :directory
            node = node[2]
          end
          case node[0]
          when :brace
            chars ||= name.chars
            if File.__brace_expand__(node[1], flags).any? { |one| File.__fnmatch__(one, chars, flags) }
              next_lists << node[2]
            end
          when :plain, :magical
            chars ||= name.chars
            next_lists << node[2] if File.__fnmatch__(node[1], chars, flags)
          end
        end
        next if next_lists.empty?
        __glob_helper__(buf, baselen, true, kind, next_lists, flags, true, encoding, results)
      end
    elsif plain
      escape = flags & File::FNM_NOESCAPE == 0
      copy = lists.map { |node| node[0] == :plain ? node : nil }
      copy.each_with_index do |node, i|
        next if node.nil?
        name = escape ? __remove_backslashes__(node[1]) : node[1]
        next_lists = [node[2]]
        (i + 1...copy.size).each do |j|
          other = copy[j]
          if !other.nil? && File.__fnmatch__(other[1], name, flags)
            next_lists << other[2]
            copy[j] = nil
          end
        end
        buf = __glob_join__(path, dirsep, name.join.force_encoding(encoding))
        __glob_helper__(buf, baselen, true, :unknown, next_lists, flags, skipdot, encoding, results)
      end
    end
  end

  # A backslash keeps the character after it, and goes.
  def self.__remove_backslashes__(chars)
    out = []
    i = 0
    while i < chars.size
      if chars[i] == "\\"
        i += 1
        break if i >= chars.size
      end
      out << chars[i]
      i += 1
    end
    out
  end

  # What a match answers: the path without `base:`, in the pattern's
  # encoding, as `push_pattern` makes it.
  # A `/` after the base is left out, but not the one a `**/` adds to the
  # base itself.
  def self.__glob_result__(path, baselen, suffix)
    sub = path.byteslice(baselen, path.bytesize - baselen)
    sub = sub.byteslice(1, sub.bytesize - 1) if baselen > 0 && sub.start_with?("/")
    __external_names__([sub + suffix])[0]
  end

  # With no path, `HOME`, else `LOGDIR`, as `dir_s_chdir` reads them.
  def self.chdir(path = (home = true))
    if home
      path = ENV["HOME"] || ENV["LOGDIR"]
      raise ArgumentError, "HOME/LOGDIR not set" if path.nil?
    end
    path = File.__path__(path)
    unless block_given?
      File.__check__(__fs_chdir__(path), path, "rb_dir_chdir")
      return 0
    end
    before = pwd
    File.__check__(__fs_chdir__(path), path, "rb_dir_chdir")
    begin
      yield path
    ensure
      # Measured: an original directory that is gone by now is an error.
      File.__check__(__fs_chdir__(before), before, "rb_dir_chdir")
    end
  end
end
