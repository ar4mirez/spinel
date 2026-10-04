# The path half of `File` and `Dir` (#39): what `require` needs to find a
# file, in Ruby over five file system primitives. Reading and writing through
# an `IO` is #41's, so `IO` is only the superclass here.

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
    message = where ? "#{where} - #{path}" : path
    raise SystemCallError.new(message, result)
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

  # ponytail: reading only, and the whole file at once. A `File` here is the
  # contents and a position; descriptors, writing, and the rest of `IO` are
  # #41's, and a write mode refuses rather than pretending.
  def self.open(path, mode = "r", **options)
    file = new(path, mode, **options)
    return file unless block_given?
    begin
      yield file
    ensure
      file.close
    end
  end

  def initialize(path, mode = "r", **options)
    @path = File.__path__(path)
    mode = mode.to_s
    __needs_io__ unless mode.start_with?("r") && !mode.include?("+")
    encoding = mode.split(":")[1]
    @contents = File.read(@path)
    @contents.force_encoding(encoding) if encoding
    @position = 0
    @closed = false
    @fileno = nil
    @sync = false
  end

  def path = @path
  alias to_path path

  def closed? = @closed

  def close
    @closed = true
    nil
  end

  def __check_open__
    raise IOError, "closed stream" if @closed
  end

  def read(length = nil)
    __check_open__
    rest = @contents.byteslice(@position, @contents.bytesize - @position)
    if length.nil?
      @position = @contents.bytesize
      return rest
    end
    return nil if rest.empty? && length > 0
    chunk = rest.byteslice(0, length)
    @position += chunk.bytesize
    chunk.force_encoding(Encoding::BINARY)
  end

  def gets(separator = $/, chomp: false)
    __check_open__
    return nil if @position >= @contents.bytesize
    rest = @contents.byteslice(@position, @contents.bytesize - @position)
    at = separator.nil? ? nil : rest.index(separator)
    line = at.nil? ? rest : rest[0, at + separator.size]
    @position += line.bytesize
    line = line.chomp(separator) if chomp && separator
    $_ = line
  end

  def each_line(separator = $/, chomp: false)
    return enum_for(:each_line, separator) unless block_given? || chomp
    return enum_for(:each_line, separator, chomp: chomp) unless block_given?
    while (line = gets(separator, chomp: chomp))
      yield line
    end
    self
  end

  def readlines(separator = $/, chomp: false)
    each_line(separator, chomp: chomp).to_a
  end

  def eof?
    __check_open__
    @position >= @contents.bytesize
  end

  alias eof eof?

  def rewind
    @position = 0
    0
  end

  def write(*)
    __needs_io__
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
