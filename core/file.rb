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
