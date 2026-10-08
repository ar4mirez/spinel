# `File` and `Dir`: paths (#39), and since the first slice of #41 a `File`
# that is a real descriptor — opened, read, written, sought and closed — with
# `File::Stat` and the directory and link calls beside it. All Ruby, over
# primitives that each make one system call and answer an errno on failure.

class File < IO
  # The open and lock flags are the platform's; the `fnmatch` flags are
  # Ruby's own numbering.
  module Constants
    __fs_constants__.each { |name, value| const_set(name, value) }
    BINARY = 0
    SHARE_DELETE = 0
    FNM_SYSCASE = 0
    FNM_SHORTNAME = 0
    FNM_NOESCAPE = 1
    FNM_PATHNAME = 2
    FNM_DOTMATCH = 4
    FNM_CASEFOLD = 8
    FNM_EXTGLOB = 16
    NULL = "/dev/null"
  end

  SEPARATOR = "/"
  Separator = SEPARATOR
  ALT_SEPARATOR = nil
  PATH_SEPARATOR = ":"

  # A path argument: a String, or what `to_path` or `to_str` makes of one.
  def self.__path__(path)
    return path if String === path
    return path.to_path if path.respond_to?(:to_path)
    return path.to_str if path.respond_to?(:to_str)
    raise TypeError, "no implicit conversion of #{path.nil? ? "nil" : path.class} into String"
  end

  # Raise the `SystemCallError` a primitive answered with an errno.
  def self.__check__(result, path, where = nil)
    return result unless Integer === result
    # `strerror @ where - path`, which is how CRuby's `rb_syserr_fail_path`
    # names the C function that made the call.
    raise SystemCallError.new(path, result, where)
  end

  def self.join(*parts)
    out = "".dup
    __join_into__(out, parts, [], true)
    out
  end

  # Exactly one separator between parts, keeping the ones a part already
  # has at its own ends: `File.join("q//", "b")` is "q//b". Measured.
  def self.__join_into__(out, parts, seen, first)
    parts.each do |part|
      # An empty Array is an empty part: `File.join([], "a")` is "/a".
      part = "" if Array === part && part.empty?
      if Array === part
        raise ArgumentError, "recursive array" if seen.any? { |s| s.equal?(part) }
        seen.push(part)
        first = __join_into__(out, part, seen, first)
        seen.pop
        next
      end
      part = __path__(part)
      if first
        out << part
      elsif out.end_with?("/") && part.start_with?("/")
        # Both sides have separators: the right part's win. Measured.
        out.sub!(%r{/+\z}, "")
        out << part
      elsif out.end_with?("/") || part.start_with?("/")
        out << part
      else
        out << "/" << part
      end
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
      home = Dir.home
      path = path == "~" ? home : (path.start_with?("~/") ? home + path[1..] : (raise ArgumentError, "user #{path[1..]} doesn't exist"))
    end
    __normalize__(path.start_with?("/") ? path : join(dir.nil? ? Dir.pwd : expand_path(dir), path))
  end

  def self.absolute_path(path, dir = nil)
    path = __path__(path)
    __normalize__(path.start_with?("/") ? path : join(dir.nil? ? Dir.pwd : absolute_path(dir), path))
  end

  # `.` and `..` resolved lexically, separators collapsed.
  def self.__normalize__(path)
    parts = []
    path.split("/").each do |part|
      next if part.empty? || part == "."
      if part == ".."
        parts.pop
      else
        parts.push(part)
      end
    end
    "/" + parts.join("/")
  end

  def self.realpath(path, dir = nil)
    path = __path__(path)
    path = join(__path__(dir), path) unless dir.nil? || path.start_with?("/")
    __check__(__fs_realpath__(path), path, "rb_check_realpath_internal")
  end

  # `realpath`, but the last component need not exist.
  def self.realdirpath(path, dir = nil)
    path = expand_path(path, dir)
    return realpath(path) if exist?(path) || symlink?(path)
    join(realpath(dirname(path)), basename(path))
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
  def self.readable?(path) = __fs_access__(__path__(path), 4)
  def self.writable?(path) = __fs_access__(__path__(path), 2)
  def self.executable?(path) = __fs_access__(__path__(path), 1)

  def self.symlink?(path)
    __fs_kind__(__path__(path), false) == :link
  end

  def self.read(path)
    path = __path__(path)
    bytes = __check__(__fs_read__(path), path, "rb_sysopen")
    bytes.force_encoding(Encoding.default_external)
  end

  def self.readlines(path, separator = $/, chomp: false)
    open(path) { |file| file.readlines(separator, chomp: chomp) }
  end

  def self.foreach(path, separator = $/, chomp: false, &block)
    return enum_for(:foreach, path, separator, chomp: chomp) unless block
    open(path) { |file| file.each_line(separator, chomp: chomp, &block) }
    nil
  end

  # --- the descriptor half (#41, first slice) --------------------------------
  #
  # A `File` is a descriptor from `open(2)` and a read buffer. Writes go
  # straight out; reads fill the buffer a block at a time, which is what lets
  # `gets` look for a separator without reading the file a byte at a time.
  #
  # ponytail: no write buffer, no transcoding on the way in or out, and no
  # `flock`, `truncate` or times. Those are the rest of #41.

  def self.open(path, mode = "r", perm = nil, **options)
    file = new(path, mode, perm, **options)
    return file unless block_given?
    begin
      yield file
    ensure
      file.close unless file.closed?
    end
  end

  # `open(2)`'s flags for a mode: an Integer is already that, and a String is
  # one of six letters with `b`, `t` and an `:encoding` to set aside.
  def self.__mode_flags__(mode)
    return mode if Integer === mode
    letters = mode.to_s.split(":")[0].to_s.delete("bt")
    case letters
    when "r" then RDONLY
    when "r+" then RDWR
    when "w" then WRONLY | CREAT | TRUNC
    when "w+" then RDWR | CREAT | TRUNC
    when "a" then WRONLY | CREAT | APPEND
    when "a+" then RDWR | CREAT | APPEND
    else raise ArgumentError, "invalid access mode #{mode}"
    end
  end

  def self.umask(*mask)
    if mask.size > 1
      raise ArgumentError, "wrong number of arguments (given #{mask.size}, expected 0..1)"
    end
    __fs_umask__(mask.empty? ? nil : Integer.__index__(mask[0]))
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

  def self.rename(from, to)
    from = __path__(from)
    to = __path__(to)
    result = __fs_rename__(from, to)
    raise SystemCallError.new("(#{from}, #{to})", result, "rb_file_s_rename") if Integer === result
    0
  end

  def self.symlink(target, link)
    target = __path__(target)
    link = __path__(link)
    result = __fs_symlink__(target, link)
    raise SystemCallError.new("(#{target}, #{link})", result, "rb_file_s_symlink") if Integer === result
    0
  end

  def self.readlink(path)
    path = __path__(path)
    __check__(__fs_readlink__(path), path, "rb_readlink")
  end

  def self.chmod(mode, *paths)
    mode = Integer.__index__(mode)
    paths.each do |path|
      path = __path__(path)
      __check__(__fs_chmod__(path, mode), path, "apply2files")
    end
    paths.size
  end

  def self.stat(path)
    Stat.new(path)
  end

  def self.lstat(path)
    Stat.__lstat__(path)
  end

  def self.size(path)
    path = path.path if File === path
    stat(path).size
  end

  def self.size?(path)
    return nil unless exist?(path)
    size = size(path)
    size == 0 ? nil : size
  end

  def self.zero?(path)
    exist?(path) && size(path) == 0
  end

  def self.empty?(path)
    zero?(path)
  end

  def self.write(path, data, mode: "w", perm: nil)
    open(path, mode, perm) { |file| file.write(data) }
  end

  def initialize(path, mode = "r", perm = nil, **options)
    __warning__("File::new() does not take block; use File::open() instead") if block_given?
    mode = options[:mode] if options.key?(:mode)
    if Integer === path
      # A descriptor something else opened.
      @path = nil
      @fileno = path
      flags = File.__mode_flags__(mode)
    else
      @path = File.__path__(path)
      flags = File.__mode_flags__(mode)
      flags = flags | options[:flags] if options[:flags]
      perm = Integer.__index__(perm) unless perm.nil?
      @fileno = File.__check__(__fd_open__(@path, flags, perm || 0666), @path, "rb_sysopen")[0]
    end
    # 0 reads, 1 writes, 2 does both: `O_ACCMODE`, the same on every target.
    @access = flags & 3
    @external = nil
    unless Integer === mode
      text = mode.to_s
      named = text.split(":")[1]
      if named
        @external = Encoding.find(named.split("|")[0].sub(/\Abom\|/i, ""))
      elsif text.include?("b")
        @external = Encoding::BINARY
      end
    end
    named = options[:external_encoding] || options[:encoding]
    @external = Encoding === named ? named : Encoding.find(named.to_s.split(":")[0]) if named
    @external = Encoding::BINARY if options[:binmode]
    # With an internal encoding set, a file opened without an encoding takes
    # the external one as it is now, and keeps it.
    @external_default = Encoding.default_internal.nil? ? nil : Encoding.default_external
    @rbuf = "".b
    @lineno = 0
    @closed = false
    @sync = false
  end

  # A new String each time, measured: the caller may change what it is given.
  def path
    @path.nil? ? nil : @path.dup
  end
  alias to_path path

  def closed? = @closed

  def close
    return nil if @closed
    @closed = true
    File.__check__(__fd_close__(@fileno), @path)
    nil
  end

  def fileno
    __check_open__
    @fileno
  end
  alias to_i fileno

  def __check_open__
    raise IOError, "closed stream" if @closed
  end

  def __readable__
    __check_open__
    raise IOError, "not opened for reading" if @access == 1
  end

  def __writable__
    __check_open__
    raise IOError, "not opened for writing" if @access == 0
  end

  # Pull up to `want` more bytes into the buffer; false at the end of the file.
  def __fill__(want = 65536)
    chunk = File.__check__(__fd_read__(@fileno, want), @path)
    return false if chunk.empty?
    @rbuf << chunk
    true
  end

  def __take__(count)
    taken = @rbuf.byteslice(0, count)
    @rbuf = @rbuf.byteslice(count, @rbuf.bytesize - count)
    taken
  end

  def __encoding__
    @external || Encoding.default_external
  end

  # Whether the first `cut` buffered bytes end part way through a UTF-8
  # character: a lead byte with fewer bytes after it than it announces.
  def __partial_character__(cut)
    back = 1
    while back <= 4 && back <= cut
      byte = @rbuf.getbyte(cut - back)
      if byte & 0xC0 != 0x80
        return false if byte < 0xC0
        needed = byte >= 0xF0 ? 4 : (byte >= 0xE0 ? 3 : 2)
        return back < needed
      end
      back += 1
    end
    false
  end

  def read(length = nil, buffer = nil)
    __readable__
    if length.nil?
      nil while __fill__
      out = __take__(@rbuf.bytesize).force_encoding(__encoding__)
    else
      length = Integer.__index__(length)
      raise ArgumentError, "negative length #{length} given" if length < 0
      while @rbuf.bytesize < length
        break unless __fill__(length - @rbuf.bytesize)
      end
      if @rbuf.empty? && length > 0
        buffer.clear unless buffer.nil?
        return nil
      end
      out = __take__(length)
    end
    return out if buffer.nil?
    # A sized read fills the caller's buffer and leaves its encoding alone.
    encoding = buffer.encoding
    buffer.replace(out)
    buffer.force_encoding(encoding) unless length.nil?
    buffer
  end

  def gets(separator = $/, limit = nil, chomp: false)
    __readable__
    if Integer === separator && limit.nil?
      limit = separator
      separator = $/
    end
    return "" if limit == 0
    if separator.nil? && limit.nil?
      line = read
      return $_ = nil if line.empty?
      @lineno += 1
      return $_ = line
    end
    # An empty separator is paragraph mode: a blank line ends the record.
    paragraph = !separator.nil? && separator.empty?
    # Blank lines before a record are not part of it.
    __take__(1) while paragraph && (@rbuf.empty? ? __fill__ : true) && @rbuf.getbyte(0) == 10
    wanted = separator.nil? ? nil : (paragraph ? "\n\n" : separator).b
    from = 0
    cut = nil
    while cut.nil?
      at = wanted.nil? ? nil : @rbuf.index(wanted, from)
      if at
        cut = at + wanted.bytesize
      elsif !limit.nil? && limit > 0 && @rbuf.bytesize >= limit
        cut = limit
      else
        from = @rbuf.bytesize - (wanted.nil? ? 0 : wanted.bytesize) + 1
        from = 0 if from < 0
        cut = @rbuf.bytesize unless __fill__
      end
    end
    if !limit.nil? && limit > 0 && limit <= cut
      cut = limit
      # A limit that lands inside a character takes the rest of it: Ruby
      # does not hand back half a character. UTF-8 only, where the bytes
      # that continue one are recognisable.
      if __encoding__ == Encoding::UTF_8
        extra = 0
        while extra < 16 && __partial_character__(cut) && (cut < @rbuf.bytesize || __fill__)
          cut += 1
          extra += 1
        end
      end
    end
    return $_ = nil if cut == 0
    line = __take__(cut).force_encoding(__encoding__)
    if paragraph
      # The blank lines between records belong to neither.
      __take__(1) while (@rbuf.empty? ? __fill__ : true) && @rbuf.getbyte(0) == 10
    end
    # In paragraph mode `chomp` takes the blank line that ended the record and
    # nothing else: the last record keeps its own newline. Measured.
    line = line.chomp(paragraph ? "\n\n" : separator) if chomp && !separator.nil?
    @lineno += 1
    $. = @lineno
    $_ = line
  end

  def readline(separator = $/, limit = nil, chomp: false)
    line = gets(separator, limit, chomp: chomp)
    raise EOFError, "end of file reached" if line.nil?
    line
  end

  def each_line(separator = $/, limit = nil, chomp: false)
    # Without `chomp:` unless it was asked for: the enumerator hands keywords
    # back as a trailing Hash, which is one argument too many here.
    return enum_for(:each_line, separator, limit) unless block_given? || chomp
    return enum_for(:each_line, separator, limit, chomp: chomp) unless block_given?
    while (line = gets(separator, limit, chomp: chomp))
      yield line
    end
    self
  end
  alias each each_line

  def readlines(separator = $/, limit = nil, chomp: false)
    lines = []
    each_line(separator, limit, chomp: chomp) { |line| lines.push(line) }
    lines
  end

  def getbyte
    __readable__
    return nil if @rbuf.empty? && !__fill__
    __take__(1).getbyte(0)
  end

  def readbyte
    byte = getbyte
    raise EOFError, "end of file reached" if byte.nil?
    byte
  end

  def each_byte
    return enum_for(:each_byte) unless block_given?
    while (byte = getbyte)
      yield byte
    end
    self
  end

  def eof?
    __readable__
    @rbuf.empty? && !__fill__
  end
  alias eof eof?

  def lineno
    __readable__
    @lineno
  end

  def lineno=(number)
    __readable__
    @lineno = Integer.__index__(number)
  end

  def seek(offset, whence = IO::SEEK_SET)
    __check_open__
    whence = { SET: IO::SEEK_SET, CUR: IO::SEEK_CUR, END: IO::SEEK_END }.fetch(whence, whence) if Symbol === whence
    offset = Integer.__index__(offset)
    # The descriptor is ahead of the reader by what is buffered.
    offset -= @rbuf.bytesize if whence == IO::SEEK_CUR
    File.__check__(__fd_seek__(@fileno, offset, whence), @path)
    @rbuf = "".b
    0
  end

  def rewind
    seek(0)
    @lineno = 0
    0
  end

  def pos
    __check_open__
    File.__check__(__fd_seek__(@fileno, 0, IO::SEEK_CUR), @path)[0] - @rbuf.bytesize
  end
  alias tell pos

  def pos=(position)
    seek(position)
    position
  end

  def write(*objects)
    __writable__
    # Bytes read ahead are bytes the descriptor has moved past; a write
    # belongs where the reader is.
    seek(0, IO::SEEK_CUR) unless @rbuf.empty?
    written = 0
    objects.each do |object|
      text = String === object ? object : object.to_s
      written += File.__check__(__fd_write__(@fileno, text), @path)[0]
    end
    written
  end

  def flush
    __check_open__
    self
  end

  def binmode
    __check_open__
    @external = Encoding::BINARY
    self
  end

  # The encoding the file was opened with. A file that can be written has
  # one only when it was given one; a file that can only be read falls back
  # to the default. CRuby's `io_encoding_get`.
  def external_encoding
    __check_open__
    return @external unless @external.nil?
    return @external_default unless @external_default.nil?
    @access == 0 ? Encoding.default_external : nil
  end

  def size
    __check_open__
    File.size(@path)
  end

  def stat
    __check_open__
    File.stat(@path)
  end

  def lstat
    __check_open__
    File.lstat(@path)
  end

  def chmod(mode)
    __check_open__
    File.chmod(mode, @path)
    0
  end

  def tty?
    false
  end
  alias isatty tty?

  def inspect
    @closed ? "#<File:#{@path} (closed)>" : "#<File:#{@path}>"
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
      stat = allocate
      stat.__fields__(File.__check__(__fs_stat__(path, false), path, "rb_file_s_lstat"))
      stat
    end

    def __fields__(fields)
      @fields = fields
    end

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
    def grpowned? = gid == Process.egid

    def inspect
      "#<File::Stat dev=0x#{dev.to_s(16)}, ino=#{ino}, mode=0#{mode.to_s(8)}, nlink=#{nlink}, " \
        "uid=#{uid}, gid=#{gid}, rdev=0x#{rdev.to_s(16)}, size=#{size}, blksize=#{blksize}, blocks=#{blocks}>"
    end
  end
end

class IO
  include File::Constants
end

class Dir
  include Enumerable

  def self.pwd
    File.__check__(__fs_getcwd__, ".")
  end

  def self.getwd
    pwd
  end

  # ponytail: `HOME` is the environment, which is `ENV` and `Process` (#43).
  def self.home
    __needs_process__
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

  def self.children(path)
    path = File.__path__(path)
    File.__check__(__fs_children__(path), path, "dir_initialize")
  end

  def self.entries(path)
    [".", ".."] + children(path)
  end

  def self.each_child(path, &block)
    return enum_for(:each_child, path) unless block
    children(path).each(&block)
    nil
  end

  def self.[](*patterns, base: nil, sort: true)
    glob(patterns, base: base)
  end

  # `*`, `?`, `[...]`, `{a,b}` and `**/`, matched a segment at a time; a name
  # starting with `.` only by a pattern that does too. Results are sorted
  # within each directory, and a brace's alternatives keep their order,
  # measured. Flags are not supported yet.
  def self.glob(patterns, flags = 0, base: nil, sort: true, &block)
    raise NotImplementedError, "Dir.glob flags" unless flags == 0
    results = []
    Array(patterns).each do |pattern|
      __expand_braces__(File.__path__(pattern)).each do |expanded|
        results.concat(__glob__(expanded, base.nil? ? nil : File.__path__(base)))
      end
    end
    return results unless block
    results.each(&block)
    nil
  end

  def self.__expand_braces__(pattern)
    open = pattern.index("{")
    return [pattern] if open.nil?
    depth = 0
    i = open
    while i < pattern.size
      depth += 1 if pattern[i] == "{"
      depth -= 1 if pattern[i] == "}"
      break if depth == 0
      i += 1
    end
    return [pattern] if depth != 0
    inner = pattern[open + 1...i]
    alternatives = []
    current = +""
    level = 0
    inner.each_char do |c|
      if c == "," && level == 0
        alternatives << current
        current = +""
      else
        level += 1 if c == "{"
        level -= 1 if c == "}"
        current << c
      end
    end
    alternatives << current
    head = pattern[0...open]
    tail = pattern[i + 1..]
    alternatives.flat_map { |alt| __expand_braces__(head + alt + tail) }
  end

  def self.__glob__(pattern, base)
    absolute = pattern.start_with?("/")
    segments = pattern.split("/", -1)
    segments.shift if absolute
    paths = [absolute ? "/" : ""]
    root = base || "."
    segments.each_with_index do |segment, index|
      last = index == segments.size - 1
      if segment.empty?
        # A trailing `/`: directories only.
        paths = paths.select { |path| File.directory?(__on_disk__(path, root)) }.map { |path| path + "/" } if last
        next
      end
      if segment == "**"
        paths = paths.flat_map { |path| [path] + __descendants__(path, root) }
        next
      end
      if segment.match?(/[*?\[]/)
        matcher = __segment_regexp__(segment)
        paths = paths.flat_map do |path|
          dir = __on_disk__(path, root)
          next [] unless File.directory?(dir)
          children = __fs_children__(dir)
          next [] if Integer === children
          children.select { |name| matcher.match?(name) && (!name.start_with?(".") || segment.start_with?(".")) }
                  .map { |name| __join__(path, name) }
        end
      else
        paths = paths.map { |path| __join__(path, segment) }
                     .select { |path| File.exist?(__on_disk__(path, root)) || File.symlink?(__on_disk__(path, root)) }
      end
    end
    paths
  end

  def self.__join__(path, name)
    path.empty? ? name : (path.end_with?("/") ? path + name : path + "/" + name)
  end

  def self.__on_disk__(path, root)
    return path if path.start_with?("/")
    path.empty? ? root : File.join(root, path)
  end

  # Every directory below `path`, depth first in sorted order, skipping
  # names that start with `.`, as `**/` does.
  def self.__descendants__(path, root)
    dir = __on_disk__(path, root)
    children = __fs_children__(dir)
    return [] if Integer === children
    children.flat_map do |name|
      next [] if name.start_with?(".")
      child = __join__(path, name)
      next [] unless File.directory?(__on_disk__(child, root)) && !File.symlink?(__on_disk__(child, root))
      [child] + __descendants__(child, root)
    end
  end

  def self.__segment_regexp__(segment)
    source = +"\\A"
    i = 0
    while i < segment.size
      c = segment[i]
      case c
      when "*" then source << "[^/]*"
      when "?" then source << "[^/]"
      when "["
        close = segment.index("]", i + 1)
        if close
          body = segment[i + 1...close]
          body = "^" + body[1..] if body.start_with?("!")
          source << "[" << body << "]"
          i = close
        else
          source << "\\["
        end
      when "\\"
        i += 1
        source << Regexp.escape(segment[i].to_s)
      else source << Regexp.escape(c)
      end
      i += 1
    end
    Regexp.new(source + "\\z")
  end
  
  def self.chdir(path = nil)
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
      __fs_chdir__(before)
    end
  end
end
