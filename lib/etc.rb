# `etc` (#111), as CRuby 4.0 ships it. CRuby's is `ext/etc/etc.c`, a C
# extension with no Ruby half; here it is Ruby over the C library calls in
# `spinel-vm/src/etc.rs`, the `__etc_*__` primitives. What each method
# answers, raises and how it converts its argument follows `etc.c`, measured
# against CRuby 4.0.9 on Linux.
#
# The user and group databases' cursors (`getpwent` and the rest) are the C
# library's, one per process, as they are for CRuby.

module Etc
  # `rb_str_new_cstr`, so BINARY, and not frozen.
  VERSION = "1.4.6".b

  __etc_constants__.each { |name, value| const_set(name, value) }

  Passwd = Struct.new(:name, :passwd, :uid, :gid, :gecos, :dir, :shell)
  Group = Struct.new(:name, :passwd, :gid, :mem)

  # --- conversions, as etc.c's macros make them ------------------------------

  # A Float as `rb_num2long` takes it: finite, at least LONG_MIN and below
  # `upper`, which `rb_num2ulong` raises to ULONG_MAX + 1.
  def self.__float_long__(value, upper = 9_223_372_036_854_775_808.0)
    unless value.finite? && value >= -9_223_372_036_854_775_808.0 && value < upper
      raise RangeError, "float #{format("%-.10g", value)} out of range of integer"
    end
    value.to_i
  end
  private_class_method :__float_long__

  # `NUM2UINT`, which `uid_t` and `gid_t` go through: a C long, then the
  # range of an unsigned int, where -1 stands for the largest.
  def self.__uid_value__(value)
    value =
      case value
      when Integer then value
      when Float then __float_long__(value, 18_446_744_073_709_551_616.0)
      when nil then raise TypeError, "no implicit conversion of nil into Integer"
      else Integer.__index__(value)
      end
    if value > 0xFFFF_FFFF_FFFF_FFFF || value < -0xFFFF_FFFF_FFFF_FFFF
      raise RangeError, "bignum too big to convert into 'unsigned long'"
    end
    raise RangeError, "integer #{value} too big to convert to 'unsigned int'" if value > 0xFFFF_FFFF
    raise RangeError, "integer #{value} too small to convert to 'unsigned int'" if value < -0x8000_0000
    value & 0xFFFF_FFFF
  end
  private_class_method :__uid_value__

  # An id as etc.c prints it in "can't find", a C int.
  def self.__id_text__(id) = (id >= 0x8000_0000 ? id - 0x1_0000_0000 : id).to_s
  private_class_method :__id_text__

  # `NUM2INT`, for a `sysconf`, `confstr` or `pathconf` name.
  def self.__c_int__(value)
    value = Float === value ? __float_long__(value) : Integer.__index__(value)
    if value > 0x7FFF_FFFF_FFFF_FFFF || value < -0x8000_0000_0000_0000
      raise RangeError, "bignum too big to convert into 'long'"
    end
    raise RangeError, "integer #{value} too big to convert to 'int'" if value > 0x7FFF_FFFF
    raise RangeError, "integer #{value} too small to convert to 'int'" if value < -0x8000_0000
    value
  end

  # `StringValueCStr`: a String, without a NUL in it.
  def self.__cstr__(value)
    value = IO.__str__(value)
    raise ArgumentError, "string contains null byte" if value.include?("\0")
    value
  end
  private_class_method :__cstr__

  # A string the C library answered, in the locale's encoding, as etc.c's
  # `safe_setup_locale_str` makes it. The password fields are
  # `safe_setup_str`'s, and stay BINARY.
  def self.__locale__(bytes) = bytes.force_encoding(Encoding.find("locale"))
  private_class_method :__locale__

  def self.__passwd__(fields)
    return nil if fields.nil?
    name, passwd, uid, gid, gecos, dir, shell = fields
    Passwd.new(__locale__(name), passwd, uid, gid, __locale__(gecos), __locale__(dir), __locale__(shell))
  end
  private_class_method :__passwd__

  def self.__group__(fields)
    return nil if fields.nil?
    name, passwd, gid, mem = fields
    Group.new(__locale__(name), passwd, gid, mem.map { |member| __locale__(member) })
  end
  private_class_method :__group__

  # --- the user database -----------------------------------------------------

  def self.getlogin
    name = __etc_login__
    return __locale__(name) unless name.nil?
    user = ENV["USER"]
    user.nil? ? nil : __locale__(user.dup)
  end

  def self.getpwuid(*args)
    raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..1)" if args.size > 1
    uid = args.empty? ? Process.uid : __uid_value__(args[0])
    __passwd__(__etc_passwd__(uid)) || raise(ArgumentError, "can't find user for #{__id_text__(uid)}")
  end

  def self.getpwnam(name)
    name = __cstr__(name)
    __passwd__(__etc_passwd__(name.b)) || raise(ArgumentError, "can't find user for #{name}")
  end

  def self.setpwent
    __etc_pwent__(0)
    nil
  end

  def self.getpwent = __passwd__(__etc_pwent__(1))

  def self.endpwent
    __etc_pwent__(2)
    nil
  end

  # Every entry, from the first, with the database closed after; one walk at
  # a time, as etc.c's `passwd_blocking` allows.
  def self.__each_passwd__
    raise RuntimeError, "parallel passwd iteration" if @__passwd_blocking
    @__passwd_blocking = true
    begin
      __etc_pwent__(0)
      while (entry = __etc_pwent__(1))
        yield __passwd__(entry)
      end
    ensure
      __etc_pwent__(2)
      @__passwd_blocking = false
    end
    nil
  end
  private_class_method :__each_passwd__

  def self.passwd(&block)
    return __each_passwd__(&block) if block
    getpwent
  end

  # --- the group database ----------------------------------------------------

  def self.getgrgid(*args)
    raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..1)" if args.size > 1
    gid = args.empty? ? Process.gid : __uid_value__(args[0])
    __group__(__etc_group__(gid)) || raise(ArgumentError, "can't find group for #{__id_text__(gid)}")
  end

  def self.getgrnam(name)
    name = __cstr__(name)
    __group__(__etc_group__(name.b)) || raise(ArgumentError, "can't find group for #{name}")
  end

  def self.setgrent
    __etc_grent__(0)
    nil
  end

  def self.getgrent = __group__(__etc_grent__(1))

  def self.endgrent
    __etc_grent__(2)
    nil
  end

  def self.__each_group__
    raise RuntimeError, "parallel group iteration" if @__group_blocking
    @__group_blocking = true
    begin
      __etc_grent__(0)
      while (entry = __etc_grent__(1))
        yield __group__(entry)
      end
    ensure
      __etc_grent__(2)
      @__group_blocking = false
    end
    nil
  end
  private_class_method :__each_group__

  def self.group(&block)
    return __each_group__(&block) if block
    getgrent
  end

  class << Passwd
    include Enumerable

    def each(&block)
      return to_enum(:each) unless block
      Etc.send(:__each_passwd__, &block)
      self
    end
  end

  class << Group
    include Enumerable

    def each(&block)
      return to_enum(:each) unless block
      Etc.send(:__each_group__, &block)
      self
    end
  end

  # --- the system ------------------------------------------------------------

  # CRuby answers the directory it was configured with; Spinel has no install
  # prefix of its own, so it answers the system's, which is what a CRuby built
  # for `/usr` answers.
  def self.sysconfdir = "/etc".b

  def self.systmpdir = "/tmp".dup.force_encoding(Encoding.find("filesystem"))

  def self.uname
    fields = __etc_uname__
    raise SystemCallError.new("uname", fields) if Integer === fields
    sysname, nodename, release, version, machine = fields
    { sysname: sysname, nodename: nodename, release: release, version: version, machine: machine }
  end

  def self.sysconf(name)
    result = __etc_sysconf__(__c_int__(name))
    raise SystemCallError.new("sysconf", result) if Integer === result
    result&.first
  end

  def self.confstr(name)
    result = __etc_confstr__(__c_int__(name))
    raise SystemCallError.new("confstr", result) if Integer === result
    result
  end

  def self.nprocessors
    result = __etc_nprocessors__
    raise SystemCallError.new("sched_getaffinity", result) if Integer === result
    result.first
  end
end

class IO
  # `fpathconf(3)` on this stream's descriptor; nil for no limit.
  def pathconf(name)
    name = Etc.__c_int__(name)
    result = __etc_fpathconf__(fileno, name)
    raise SystemCallError.new("fpathconf", result) if Integer === result
    result&.first
  end
end
