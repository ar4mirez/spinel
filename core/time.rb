# Time — an instant, and the calendar it is read on (#32).
#
# An instant is two Integers: seconds since 1970-01-01 UTC and nanoseconds
# into that second. Everything else a Time answers — the year, the weekday,
# the zone's name — is arithmetic on those two and a UTC offset, and all of it
# is Ruby. The one thing Ruby cannot do is read the clock, and that is the one
# primitive (`__sys_clock__`).
#
# A Time is read one of three ways, kept in `@mode`:
#
#   :utc    offset 0, zone "UTC"
#   :fixed  an offset given by the program ("+09:00"), and no zone name
#   :local  the offset the system's zone had at that instant
#
# The local zone comes from `TZ`, or from /etc/localtime, and is worked out by
# `Time::Zone` below from the same files the C library reads.
#
# ponytail: nanoseconds are the finest grain kept. Ruby keeps a Rational, so
# `Time.at(0.1)` there remembers the float's whole binary expansion; here it
# is cut at the ninth digit. `Rational` does not exist yet, and `subsec` and
# `to_r` wait for it.
class Time
  include Comparable

  MONTH_NAMES = %w[January February March April May June July August September October November December]
  DAY_NAMES = %w[Sunday Monday Tuesday Wednesday Thursday Friday Saturday]
  MONTH_ABBREVIATIONS = %w[jan feb mar apr may jun jul aug sep oct nov dec]
  private_constant :MONTH_NAMES, :DAY_NAMES, :MONTH_ABBREVIATIONS

  # -- making one ----------------------------------------------------------

  def self.now(in: nil)
    seconds, nanoseconds = __sys_clock__(Process::CLOCK_REALTIME)
    time = __at__(seconds, nanoseconds)
    zone = binding.local_variable_get(:in)
    zone.nil? ? time : time.__send__(:__in_zone__, zone)
  end

  class << self
    private

    # ponytail: `Marshal` does not exist, so there is nothing to load from.
    def _load(*)
      __needs_time__
    end
  end

  def self.__at__(seconds, nanoseconds)
    time = allocate
    time.__set__(seconds, nanoseconds)
    time
  end

  # `Time.at(seconds)`, `Time.at(seconds, fraction, unit)`, `Time.at(time)`.
  def self.at(seconds, fraction = nil, unit = :microsecond, in: nil)
    zone = binding.local_variable_get(:in)
    if seconds.is_a?(Time)
      time = __at__(seconds.to_i, seconds.nsec)
      time.__send__(:__copy_zone__, seconds)
    else
      whole, nanoseconds = __split__(seconds)
      unless fraction.nil?
        scale = case unit
                when :millisecond then 1_000_000
                when :usec, :microsecond then 1_000
                when :nsec, :nanosecond then 1
                else raise ArgumentError, "unexpected unit: " + unit.to_s
                end
        extra, sub = __split__(fraction)
        nanoseconds = nanoseconds + extra * scale + (sub * scale) / 1_000_000_000
        whole = whole + nanoseconds.div(1_000_000_000)
        nanoseconds = nanoseconds % 1_000_000_000
      end
      time = __at__(whole, nanoseconds)
    end
    zone.nil? ? time : time.__send__(:__in_zone__, zone)
  end

  # A number of seconds as whole seconds and nanoseconds, both Integers.
  def self.__split__(value)
    return [value, 0] if value.is_a?(Integer)
    if value.is_a?(Float)
      raise FloatDomainError, value.to_s if value.nan? || value.infinite?
      # Exactly: a Float is an integer times a power of two, and the
      # nanoseconds are that product floored. Float arithmetic would round
      # `-1.3 + 2` to 0.7 and lose the nanosecond Ruby keeps.
      mantissa, exponent = Math.frexp(value)
      scaled = (mantissa * 9_007_199_254_740_992).to_i * 1_000_000_000
      shift = 53 - exponent
      total = shift > 0 ? scaled.div(2**shift) : scaled * 2**-shift
      return [total.div(1_000_000_000), total % 1_000_000_000]
    end
    if value.is_a?(Rational)
      whole = value.floor
      return [whole, ((value - whole) * 1_000_000_000).floor]
    end
    if value.nil? || value.is_a?(String) || !value.respond_to?(:to_int)
      raise TypeError, "can't convert " + (value.nil? ? "nil" : value.class.to_s) + " into an exact number"
    end
    [value.to_int, 0]
  end

  def self.utc(*args)
    __from_civil__(args, :utc)
  end

  def self.local(*args)
    __from_civil__(args, :local)
  end

  class << self
    alias gm utc
    alias mktime local
  end

  # `Time.new` with no arguments is now. With a String it reads a timestamp;
  # otherwise the arguments are a civil time, and the seventh or `in:` says
  # where.
  def self.new(*args, in: nil, precision: 9)
    zone = binding.local_variable_get(:in)
    Integer.__index__(precision) unless precision.nil?
    if args.empty?
      seconds, nanoseconds = __sys_clock__(Process::CLOCK_REALTIME)
      time = allocate
      time.__set__(seconds, nanoseconds)
      time.__send__(:__in_zone__, zone) unless zone.nil?
      return time
    end
    if args.size > 7
      raise ArgumentError, "wrong number of arguments (given " + args.size.to_s + ", expected 0..7)"
    end
    if args.size == 1 && args[0].is_a?(String)
      return __from_string__(args[0], zone)
    end
    unless args[6].nil?
      raise ArgumentError, "timezone argument given as positional and keyword arguments" unless zone.nil?
      zone = args[6]
    end
    fields = __civil_fields__(args[0, 6])
    time = allocate
    zone = __zone_object__(zone)
    if zone.nil?
      time.__send__(:__set_civil__, fields, :local, 0)
    elsif __zone_object?(zone)
      time.__send__(:__set_civil_in__, fields, zone)
    else
      offset = __utc_offset__(zone)
      if offset.nil?
        time.__send__(:__set_civil__, fields, :utc, 0)
      else
        time.__send__(:__set_civil__, fields, :fixed, offset)
      end
    end
    time
  end

  # A zone that is an object rather than an offset: it converts between
  # local and UTC itself, with `local_to_utc` and `utc_to_local`.
  def self.__zone_object?(zone)
    return false if zone.nil? || zone.is_a?(String) || zone.is_a?(Integer)
    return false if zone.respond_to?(:to_str) || zone.respond_to?(:to_int)
    true
  end

  # What a zone object answered, as seconds: a Time is read by its clock
  # face, whatever offset it carries, and anything else by `to_i`.
  def self.__reading__(answer)
    return answer if answer.is_a?(Integer)
    return answer.to_i + answer.utc_offset if answer.is_a?(Time)
    answer.to_i
  end

  # A zone *name* that is not an offset is looked up by the class, if the
  # class knows how: `find_timezone`.
  def self.__zone_object__(zone)
    return zone unless zone.is_a?(String) && respond_to?(:find_timezone)
    return zone if zone == "UTC" || zone == "Z" || zone =~ /\A[A-IK-Z]\z/ || zone =~ /\A[+-]\d/
    find_timezone(zone)
  end

  def self.__from_string__(text, zone)
    match = /\A\s*(-?\d{4,})(?:-(\d\d)-(\d\d)(?:[ T](\d\d):(\d\d):(\d\d)(?:\.(\d+))?\s*(Z|UTC|[+-]\d\d(?::?\d\d(?::?\d\d)?)?)?)?)?\s*\z/.match(text)
    if match.nil?
      if text =~ /\A\s*-?\d{4,}-\d\d\s*\z/ || text =~ /\A\s*-?\d{4,}-\d\d-\d\d[ T]\d\d(:\d\d)?\s*\z/
        raise ArgumentError, "no time information"
      end
      raise ArgumentError, "can't parse: " + text.inspect
    end
    fields = [match[1].to_i, (match[2] || 1).to_i, (match[3] || 1).to_i,
              (match[4] || 0).to_i, (match[5] || 0).to_i, (match[6] || 0).to_i, 0]
    unless match[7].nil?
      digits = (match[7] + "000000000")[0, 9]
      fields[6] = digits.to_i
    end
    __check_civil__(fields)
    zone = match[8] unless match[8].nil?
    time = allocate
    if zone.nil?
      time.__send__(:__set_civil__, fields, :local, 0)
    else
      offset = __utc_offset__(zone)
      time.__send__(:__set_civil__, fields, offset.nil? ? :utc : :fixed, offset || 0)
    end
    time
  end

  # `Time.utc` and `Time.local` take a civil time in order, or the ten values
  # `to_a` answers — seconds first — of which the last four are ignored.
  def self.__from_civil__(args, mode)
    prefer = nil
    if args.size == 10
      # The ninth says which reading is meant when clocks going back make
      # one happen twice.
      prefer = args[8] if args[8] == true || args[8] == false
      args = [args[5], args[4], args[3], args[2], args[1], args[0]]
    elsif args.empty? || args.size > 7
      raise ArgumentError, "wrong number of arguments (given " + args.size.to_s + ", expected 1..8)"
    end
    fields = __civil_fields__(args[0, 6])
    unless args[6].nil?
      # Microseconds given on their own replace the second's fraction.
      micro, sub = __split__(args[6])
      fields[6] = micro * 1_000 + sub / 1_000_000
    end
    time = allocate
    time.__send__(:__set_civil__, fields, mode, 0, prefer)
    time
  end

  # Year, month, day, hour, minute, second and nanoseconds, from arguments
  # that may be Integers, Strings, nil, or — for the month — a name.
  def self.__civil_fields__(args)
    year = __civil_int__(args[0], "year")
    month = args[1]
    month = month.to_str if !month.is_a?(String) && !month.is_a?(Integer) && month.respond_to?(:to_str)
    if month.is_a?(String)
      named = MONTH_ABBREVIATIONS.index(month.downcase)
      month = named.nil? ? __civil_int__(month, "mon") : named + 1
    else
      month = month.nil? ? 1 : __civil_int__(month, "mon")
    end
    day = args[2].nil? ? 1 : __civil_int__(args[2], "mday")
    hour = args[3].nil? ? 0 : __civil_int__(args[3], "hour")
    minute = args[4].nil? ? 0 : __civil_int__(args[4], "min")
    second = 0
    nanoseconds = 0
    unless args[5].nil?
      if args[5].is_a?(String)
        second = __civil_int__(args[5], "sec")
      else
        second, nanoseconds = __split__(args[5])
      end
    end
    fields = [year, month, day, hour, minute, second, nanoseconds]
    __check_civil__(fields)
    fields
  end

  def self.__civil_int__(value, what)
    return value if value.is_a?(Integer)
    return value.floor if value.is_a?(Float)
    if value.is_a?(String)
      unless value =~ /\A\s*[+-]?\d+\s*\z/
        raise ArgumentError, "invalid value for Integer(): " + value.inspect
      end
      return value.to_i
    end
    if value.nil? || !value.respond_to?(:to_int)
      raise TypeError, "can't convert " + (value.nil? ? "nil" : value.class.to_s) + " into an exact number"
    end
    value.to_int
  end

  def self.__check_civil__(fields)
    raise ArgumentError, "mon out of range" if fields[1] < 1 || fields[1] > 12
    raise ArgumentError, "argument out of range" if fields[2] < 1 || fields[2] > 31
    hour = fields[3]
    if hour < 0 || hour > 24 || (hour == 24 && (fields[4] > 0 || fields[5] > 0))
      raise ArgumentError, hour == 24 ? "argument out of range" : "hour out of range"
    end
    raise ArgumentError, "min out of range" if fields[4] < 0 || fields[4] > 59
    raise ArgumentError, "argument out of range" if fields[5] < 0
    raise ArgumentError, "sec out of range" if fields[5] > 60
  end

  # A zone argument as seconds east of UTC, or nil for UTC itself.
  def self.__utc_offset__(zone)
    if zone.is_a?(Integer)
      raise ArgumentError, "utc_offset out of range" if zone <= -86400 || zone >= 86400
      return zone
    end
    unless zone.is_a?(String)
      if zone.respond_to?(:to_str)
        zone = zone.to_str
      elsif zone.respond_to?(:to_int)
        return __utc_offset__(zone.to_int)
      else
        raise TypeError, "can't convert " + zone.class.to_s + " into an exact number"
      end
    end
    return nil if zone == "UTC" || zone == "Z" || zone == "-00:00"
    if zone.size == 1 && zone =~ /\A[A-IK-Z]\z/
      letter = zone.getbyte(0)
      return (letter - 64) * 3600 if letter <= 73
      return (letter - 65) * 3600 if letter <= 77
      return (77 - letter) * 3600
    end
    match = /\A([+-])(\d\d)(?::?(\d\d)(?::?(\d\d))?)?\z/.match(zone)
    if !match.nil? && match[2].to_i > 23
      raise ArgumentError, "utc_offset out of range"
    end
    if match.nil? || (match[3] || 0).to_i > 59 || (match[4] || 0).to_i > 59
      raise ArgumentError, "\"+HH:MM\", \"-HH:MM\", \"UTC\" or \"A\"..\"I\",\"K\"..\"Z\" expected for utc_offset: " + zone
    end
    seconds = match[2].to_i * 3600 + (match[3] || 0).to_i * 60 + (match[4] || 0).to_i
    match[1] == "-" ? -seconds : seconds
  end

  # Days since 1970-01-01 for a civil date, and back. The proleptic Gregorian
  # calendar, by the era arithmetic that needs no table.
  def self.__days__(year, month, day)
    year = year - 1 if month <= 2
    era = year.div(400)
    year_of_era = year - era * 400
    shifted = month > 2 ? month - 3 : month + 9
    day_of_year = (153 * shifted + 2) / 5 + day - 1
    day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year
    era * 146_097 + day_of_era - 719_468
  end

  def self.__civil__(days)
    days = days + 719_468
    era = days.div(146_097)
    day_of_era = days - era * 146_097
    year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365
    year = year_of_era + era * 400
    day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100)
    shifted = (5 * day_of_year + 2) / 153
    day = day_of_year - (153 * shifted + 2) / 5 + 1
    month = shifted < 10 ? shifted + 3 : shifted - 9
    year = year + 1 if month <= 2
    [year, month, day]
  end

  def __set__(seconds, nanoseconds)
    @seconds = seconds
    @nanoseconds = nanoseconds
    @mode = :local
    @offset = nil
    self
  end

  # A civil time in a zone object's care: it is shown the reading as a UTC
  # Time and answers the instant, as a Time or as anything with `to_i`.
  def __set_civil_in__(fields, zone)
    __set_civil__(fields, :utc, 0)
    wall = @seconds
    if zone.respond_to?(:local_to_utc)
      answer = zone.local_to_utc(Time.__at__(wall, @nanoseconds).utc)
      # The instant it names: a Time's own, offset and all.
      @seconds = answer.is_a?(Integer) ? answer : answer.to_i
    end
    offset = wall - @seconds
    raise ArgumentError, "utc_offset out of range" if offset <= -86_400 || offset >= 86_400
    @mode = :fixed
    @offset = offset
    @tz = zone
    self
  end
  private :__set_civil_in__

  # The instant a civil time names, read in `mode`. A day past the month's
  # end, an hour of 24 and a second of 60 all run on into what follows.
  def __set_civil__(fields, mode, offset, prefer = nil)
    days = Time.__days__(fields[0], fields[1], 1) + fields[2] - 1
    wall = days * 86_400 + fields[3] * 3_600 + fields[4] * 60 + fields[5]
    @nanoseconds = fields[6]
    @mode = mode
    @offset = nil
    @zone = nil
    @dst = nil
    if mode == :utc
      @seconds = wall
    elsif mode == :fixed
      @seconds = wall - offset
      @offset = offset
    else
      @seconds = Zone.local.instant_of(wall, prefer)
      # Read now: the zone is the one in force when the Time was made, not
      # whatever `TZ` says when it is next asked.
      __local__
    end
    self
  end
  private :__set_civil__

  def __copy_zone__(other)
    @zone = nil
    @dst = nil
    tz = other.__tz__
    return __in_zone__(tz) unless tz.nil?
    @mode = other.__mode__
    @offset = other.__mode__ == :fixed ? other.utc_offset : nil
    self
  end
  private :__copy_zone__

  def __mode__
    @mode
  end

  def __tz__
    @tz
  end

  def __in_zone__(zone)
    @zone = nil
    @dst = nil
    @tz = nil
    zone = self.class.__zone_object__(zone)
    if Time.__zone_object?(zone)
      offset = 0
      if zone.respond_to?(:utc_to_local)
        answer = zone.utc_to_local(Time.__at__(@seconds, @nanoseconds).utc)
        offset = Time.__reading__(answer) - @seconds
        raise ArgumentError, "utc_offset out of range" if offset <= -86_400 || offset >= 86_400
      end
      @mode = :fixed
      @offset = offset
      @tz = zone
      return self
    end
    offset = Time.__utc_offset__(zone)
    if offset.nil?
      @mode = :utc
      @offset = nil
    else
      @mode = :fixed
      @offset = offset
    end
    self
  end
  private :__in_zone__

  # -- where it is read -----------------------------------------------------

  # The local zone's answer for this instant: offset, whether it is summer
  # time, and the abbreviation. Asked once and kept, since every field needs
  # the offset.
  def __local__
    return [@offset, @dst, @zone] unless @zone.nil?
    found = Zone.local.at(@seconds)
    found = [found[0], found[1], found[2].dup.force_encoding(Encoding::US_ASCII)]
    # A frozen Time is asked afresh each time rather than written to.
    unless frozen?
      @offset = found[0]
      @dst = found[1]
      @zone = found[2]
    end
    found
  end
  private :__local__

  def utc_offset
    return 0 if @mode == :utc
    return __local__[0] if @mode == :local
    @offset
  end
  alias gmt_offset utc_offset
  alias gmtoff utc_offset

  def utc?
    @mode == :utc
  end
  alias gmt? utc?

  # "UTC", the local zone's abbreviation, the zone object a Time was given,
  # or nil for a bare offset.
  def zone
    return "UTC" if @mode == :utc
    return @tz if @mode == :fixed
    __local__[2]
  end

  def isdst
    if @mode == :fixed
      return false if @tz.nil? || !@tz.respond_to?(:dst?)
      return @tz.dst?(self) ? true : false
    end
    return false unless @mode == :local
    __local__[1]
  end
  alias dst? isdst

  def utc
    return self if @mode == :utc
    @mode = :utc
    @offset = nil
    @zone = nil
    @dst = nil
    @tz = nil
    self
  end
  alias gmtime utc

  def localtime(offset = nil)
    # Already local is already done: the zone it was read in stays.
    return self if offset.nil? && @mode == :local
    @zone = nil
    @dst = nil
    if offset.nil?
      @mode = :local
      @offset = nil
      @tz = nil
      __local__
    else
      __in_zone__(offset)
    end
    self
  end

  def getutc
    Time.__at__(@seconds, @nanoseconds).utc
  end
  alias getgm getutc

  def getlocal(offset = nil)
    Time.__at__(@seconds, @nanoseconds).localtime(offset)
  end

  # -- the calendar ---------------------------------------------------------

  # Seconds since the epoch on this Time's own wall clock.
  def __wall__
    @seconds + utc_offset
  end
  private :__wall__

  def __date__
    Time.__civil__(__wall__.div(86_400))
  end
  private :__date__

  def year = __date__[0]
  def month = __date__[1]
  def day = __date__[2]
  alias mon month
  alias mday day

  def hour = (__wall__ % 86_400) / 3_600
  def min = (__wall__ % 3_600) / 60
  def sec = __wall__ % 60

  def nsec = @nanoseconds
  alias tv_nsec nsec

  def usec = @nanoseconds / 1000
  alias tv_usec usec

  def to_i = @seconds
  alias tv_sec to_i

  def to_f
    @seconds + @nanoseconds / 1_000_000_000.0
  end

  def to_r
    Rational(@seconds * 1_000_000_000 + @nanoseconds, 1_000_000_000)
  end

  def subsec
    @nanoseconds == 0 ? 0 : Rational(@nanoseconds, 1_000_000_000)
  end

  def wday
    (__wall__.div(86_400) + 4) % 7
  end

  def yday
    days = __wall__.div(86_400)
    days - Time.__days__(Time.__civil__(days)[0], 1, 1) + 1
  end

  def sunday? = wday == 0
  def monday? = wday == 1
  def tuesday? = wday == 2
  def wednesday? = wday == 3
  def thursday? = wday == 4
  def friday? = wday == 5
  def saturday? = wday == 6

  def to_a
    [sec, min, hour, day, month, year, wday, yday, isdst, zone]
  end

  def deconstruct_keys(keys)
    all = { year: year, month: month, day: day, yday: yday, wday: wday, hour: hour,
            min: min, sec: sec, subsec: nil, dst: isdst, zone: zone }
    if keys.nil?
      all[:subsec] = subsec
      return all
    end
    unless keys.is_a?(Array)
      raise TypeError, "wrong argument type " + keys.class.to_s + " (expected Array or nil)"
    end
    out = {}
    keys.each do |key|
      next unless all.key?(key)
      out[key] = key == :subsec ? subsec : all[key]
    end
    out
  end

  # -- arithmetic -----------------------------------------------------------

  def __shifted__(seconds, nanoseconds)
    seconds = seconds + nanoseconds.div(1_000_000_000)
    time = Time.__at__(seconds, nanoseconds % 1_000_000_000)
    time.__send__(:__copy_zone__, self)
  end
  private :__shifted__

  def +(other)
    raise TypeError, "time + time?" if other.is_a?(Time)
    whole, nanoseconds = Time.__split__(other)
    __shifted__(@seconds + whole, @nanoseconds + nanoseconds)
  end

  def -(other)
    if other.is_a?(Time)
      return (@seconds - other.to_i) + (@nanoseconds - other.nsec) / 1_000_000_000.0
    end
    whole, nanoseconds = Time.__split__(other)
    __shifted__(@seconds - whole, @nanoseconds - nanoseconds)
  end

  # Against what is not a Time, the other side is asked and its answer is
  # turned round: CRuby's `rb_invcmp`. Measured.
  def <=>(other)
    unless Time === other
      order = other <=> self
      return nil if order.nil?
      return order > 0 ? -1 : (order < 0 ? 1 : 0)
    end
    [@seconds, @nanoseconds] <=> [other.to_i, other.nsec]
  end

  def ==(other)
    Time === other && @seconds == other.to_i && @nanoseconds == other.nsec
  end

  def eql?(other)
    Time === other && @seconds == other.to_i && @nanoseconds == other.nsec
  end

  def hash
    [@seconds, @nanoseconds].hash
  end

  # To `digits` places of a second: half away from zero, down, or up.
  def round(digits = 0)
    __rounded__(digits, 1)
  end

  def floor(digits = 0)
    __rounded__(digits, 0)
  end

  def ceil(digits = 0)
    __rounded__(digits, 2)
  end

  def __rounded__(digits, how)
    digits = Integer.__index__(digits)
    raise ArgumentError, "negative ndigits given" if digits < 0
    return __shifted__(@seconds, @nanoseconds) if digits >= 9
    grain = 10**(9 - digits)
    rest = @nanoseconds % grain
    down = @nanoseconds - rest
    up = how == 2 ? rest > 0 : (how == 1 && rest * 2 >= grain)
    __shifted__(@seconds, up ? down + grain : down)
  end
  private :__rounded__

  # -- as text ----------------------------------------------------------------

  def to_s
    strftime(utc? ? "%Y-%m-%d %H:%M:%S UTC" : "%Y-%m-%d %H:%M:%S %z")
  end

  # `to_s`, with the fraction of a second when there is one.
  def inspect
    text = strftime("%Y-%m-%d %H:%M:%S")
    if @nanoseconds > 0
      digits = @nanoseconds.to_s.rjust(9, "0")
      digits = digits[0, digits.size - 1] while digits.end_with?("0")
      text = text + "." + digits
    end
    text + (utc? ? " UTC" : strftime(" %z"))
  end

  def ctime
    strftime("%a %b %e %H:%M:%S %Y")
  end
  alias asctime ctime

  def xmlschema(digits = 0)
    digits = Integer.__index__(digits)
    text = strftime("%Y-%m-%dT%H:%M:%S")
    if digits > 0
      text = text + "." + (@nanoseconds.to_s.rjust(9, "0") + "0" * digits)[0, digits]
    end
    text + (utc? ? "Z" : strftime("%:z"))
  end
  alias iso8601 xmlschema

  # `printf` for a Time. A directive is `%`, flags, a width, an optional `:`
  # for `%z`, and a letter; anything that is not one is copied as it stands.
  def strftime(format)
    format = format.to_str unless format.is_a?(String)
    out = +""
    index = 0
    size = format.size
    while index < size
      char = format[index]
      if char != "%"
        out << char
        index = index + 1
        next
      end
      start = index
      index = index + 1
      flags = +""
      while index < size && "-_0^#".include?(format[index])
        # Of `_` and `0` the later one decides the padding.
        flags.delete!("_0") if format[index] == "_" || format[index] == "0"
        flags << format[index]
        index = index + 1
      end
      width = nil
      while index < size && format[index] >= "0" && format[index] <= "9"
        width = (width || 0) * 10 + format[index].to_i
        index = index + 1
      end
      colons = 0
      while index < size && format[index] == ":"
        colons = colons + 1
        index = index + 1
      end
      letter = index < size ? format[index] : nil
      index = index + 1
      piece = letter.nil? ? nil : __directive__(letter, flags, width, colons)
      if piece.nil?
        out << format[start, index - start]
      else
        out << piece
      end
    end
    out
  end

  # A number, padded: zeros by default or spaces, to the directive's own
  # width unless one was given, and not at all under `-`.
  def __number__(value, default_width, pad, flags, width)
    pad = " " if flags.include?("_")
    pad = "0" if flags.include?("0")
    text = value.abs.to_s
    return (value < 0 ? "-" : "") + text if flags.include?("-")
    width = default_width if width.nil?
    sign = value < 0 ? "-" : ""
    if pad == "0"
      sign + text.rjust(width - sign.size, "0")
    else
      (sign + text).rjust(width, " ")
    end
  end
  private :__number__

  def __text__(value, flags, width)
    value = value.upcase if flags.include?("^")
    # `#` changes the case, and for a name that means capitals; `%p` and
    # `%Z`, which are capitals already, go the other way before they get here.
    value = value.upcase if flags.include?("#")
    return value if width.nil? || flags.include?("-")
    value.rjust(width, flags.include?("0") ? "0" : " ")
  end
  private :__text__

  def __directive__(letter, flags, width, colons)
    return __zone_offset__(flags, width, colons) if letter == "z"
    return nil if colons > 0
    case letter
    when "Y"
      value = year
      return __number__(value, value < 0 ? 5 : 4, "0", flags, width) if width.nil? && !flags.include?("_")
      __number__(value, 4, "0", flags, width)
    when "C" then __number__(year.div(100), 2, "0", flags, width)
    when "y" then __number__(year % 100, 2, "0", flags, width)
    when "m" then __number__(month, 2, "0", flags, width)
    when "d" then __number__(day, 2, "0", flags, width)
    when "e" then __number__(day, 2, " ", flags, width)
    when "j" then __number__(yday, 3, "0", flags, width)
    when "H" then __number__(hour, 2, "0", flags, width)
    when "k" then __number__(hour, 2, " ", flags, width)
    when "I" then __number__((hour + 11) % 12 + 1, 2, "0", flags, width)
    when "l" then __number__((hour + 11) % 12 + 1, 2, " ", flags, width)
    when "M" then __number__(min, 2, "0", flags, width)
    when "S" then __number__(sec, 2, "0", flags, width)
    when "L" then __fraction__(width || 3)
    when "N" then __fraction__(width || 9)
    when "s" then __number__(@seconds, 1, "0", flags, width)
    when "u" then __number__(wday == 0 ? 7 : wday, 1, "0", flags, width)
    when "w" then __number__(wday, 1, "0", flags, width)
    when "U" then __number__((yday + 6 - wday) / 7, 2, "0", flags, width)
    when "W" then __number__((yday + 6 - (wday + 6) % 7) / 7, 2, "0", flags, width)
    when "V" then __number__(__iso_week__[1], 2, "0", flags, width)
    when "G" then __number__(__iso_week__[0], 4, "0", flags, width)
    when "g" then __number__(__iso_week__[0] % 100, 2, "0", flags, width)
    when "B" then __text__(MONTH_NAMES[month - 1], flags, width)
    when "b", "h" then __text__(MONTH_NAMES[month - 1][0, 3], flags, width)
    when "A" then __text__(DAY_NAMES[wday], flags, width)
    when "a" then __text__(DAY_NAMES[wday][0, 3], flags, width)
    when "p"
      text = hour < 12 ? "AM" : "PM"
      text = text.downcase if flags.include?("#") && !flags.include?("^")
      __text__(text, flags.delete("#"), width)
    when "P" then __text__(hour < 12 ? "am" : "pm", flags, width)
    when "Z"
      text = if @tz.nil?
               zone.to_s
             else
               @tz.respond_to?(:abbr) ? @tz.abbr(self).to_s : ""
             end
      text = text.downcase if flags.include?("#") && !flags.include?("^")
      __text__(text, flags.delete("#"), width)
    when "n" then "\n"
    when "t" then "\t"
    when "%" then "%"
    when "c" then __text__(strftime("%a %b %e %H:%M:%S %Y"), flags, width)
    when "D", "x" then __text__(strftime("%m/%d/%y"), flags, width)
    when "F" then __text__(strftime("%Y-%m-%d"), flags, width)
    when "T", "X" then __text__(strftime("%H:%M:%S"), flags, width)
    when "R" then __text__(strftime("%H:%M"), flags, width)
    when "r" then __text__(strftime("%I:%M:%S %p"), flags, width)
    when "v" then __text__(strftime("%e-%^b-%Y"), flags, width)
    end
  end
  private :__directive__

  # The first `digits` digits of the fraction of a second, with zeros past
  # the ninth: the instant has no more to give.
  def __fraction__(digits)
    text = @nanoseconds.to_s.rjust(9, "0")
    digits <= 9 ? text[0, digits] : text + "0" * (digits - 9)
  end
  private :__fraction__

  # ISO 8601's year and week: weeks start on Monday, and week 1 is the one
  # with the year's first Thursday.
  def __iso_week__
    weekday = (wday + 6) % 7
    week = (yday - weekday + 9) / 7
    iso_year = year
    if week < 1
      iso_year = iso_year - 1
      last = Time.__days__(iso_year, 12, 31) - Time.__days__(iso_year, 1, 1) + 1
      week = (yday + last - weekday + 9) / 7
    elsif week == 53
      length = Time.__days__(iso_year + 1, 1, 1) - Time.__days__(iso_year, 1, 1)
      if yday - weekday + 3 > length
        week = 1
        iso_year = iso_year + 1
      end
    end
    [iso_year, week]
  end
  private :__iso_week__

  # `%z`: `+hhmm`, `+hh:mm` with one colon, `+hh:mm:ss` with two. The sign
  # counts toward the width, and under `_` the padding is spaces before it.
  def __zone_offset__(flags, width, colons)
    offset = utc_offset
    # RFC 3339's `-0000`, "UTC, and the local offset is not known": what
    # `-` asks for on a UTC time.
    sign = offset < 0 || (utc? && flags.include?("-")) ? "-" : "+"
    offset = offset.abs
    hours = offset / 3_600
    minutes = (offset % 3_600) / 60
    seconds = offset % 60
    rest = case colons
           when 0 then minutes.to_s.rjust(2, "0")
           when 1 then ":" + minutes.to_s.rjust(2, "0")
           when 2 then ":" + minutes.to_s.rjust(2, "0") + ":" + seconds.to_s.rjust(2, "0")
           else return nil
           end
    if flags.include?("_")
      return (sign + hours.to_s + rest).rjust(width || (3 + rest.size), " ")
    end
    width = 3 + rest.size if width.nil?
    sign + (hours.to_s + rest).rjust(width - 1, "0")
  end
  private :__zone_offset__

  # The system's time zone, read the way the C library reads it.
  #
  # `TZ` names a file under the zone directory ("America/New_York") or is a
  # POSIX rule ("CET-1", "EST5EDT,M3.2.0,M11.1.0"). Unset, the zone is
  # /etc/localtime. A zone file is TZif: a table of instants at which the
  # offset changed, and a POSIX rule for everything after the table ends —
  # which, for the files systems ship now, is every year since 2007.
  class Zone
    DIRECTORIES = ["/usr/share/zoneinfo", "/usr/lib/zoneinfo", "/var/db/timezone/zoneinfo", "/usr/share/lib/zoneinfo"]

    def self.local
      name = ENV["TZ"]
      @zones ||= {}
      key = name.nil? ? :system : name
      @zones[key] ||= new(name)
    end

    def initialize(name)
      @transitions = []
      @types = []
      @rule = nil
      @fallback = [0, false, "UTC"]
      if name.nil? || name.empty?
        __load__("/etc/localtime") if File.exist?("/etc/localtime")
        return
      end
      name = name[1, name.size - 1] if name.start_with?(":")
      unless name.include?("..") || name.empty?
        DIRECTORIES.each do |directory|
          path = name.start_with?("/") ? name : directory + "/" + name
          if File.file?(path)
            __load__(path)
            return
          end
        end
      end
      @rule = Zone.__rule__(name)
    end

    # Offset, summer time and abbreviation at an instant.
    def at(seconds)
      unless @transitions.empty?
        if seconds < @transitions[0][0]
          return @first unless @first.nil?
        elsif @rule.nil? || seconds < @transitions[@transitions.size - 1][0]
          low = 0
          high = @transitions.size - 1
          while low < high
            middle = (low + high + 1) / 2
            if @transitions[middle][0] <= seconds
              low = middle
            else
              high = middle - 1
            end
          end
          return @types[@transitions[low][1]]
        end
      end
      return @fallback if @rule.nil?
      Zone.__apply__(@rule, seconds)
    end

    # The instant a wall-clock reading names. A reading that happens twice,
    # when clocks go back, is the later one; one that never happens, when
    # they go forward, is taken as if they had not.
    def instant_of(wall, summer = nil)
      before = at(wall - 86_400)[0]
      after = at(wall + 86_400)[0]
      return wall - before if before == after && at(wall - before)[0] == before
      late = wall - (before < after ? before : after)
      early = wall - (before < after ? after : before)
      late_fits = at(late)[0] == wall - late
      early_fits = at(early)[0] == wall - early
      if late_fits && early_fits && !summer.nil?
        return at(late)[1] == summer ? late : early
      end
      return late if late_fits
      return early if early_fits
      wall - before
    end

    def __load__(path)
      data = File.open(path, "rb") { |file| file.read }
      return unless data.bytesize >= 44 && data[0, 4] == "TZif"
      version = data.getbyte(4)
      counts = (0...6).map { |i| Zone.__u32__(data, 20 + i * 4) }
      wide = false
      start = 44
      if version >= 50
        # Past the 32-bit block to the 64-bit one that repeats it.
        start = 44 + counts[3] * 5 + counts[4] * 6 + counts[5] + counts[2] * 8 + counts[0] + counts[1]
        return unless data[start, 4] == "TZif"
        counts = (0...6).map { |i| Zone.__u32__(data, start + 20 + i * 4) }
        start = start + 44
        wide = true
      end
      isut, isstd, leaps, times, types, chars = counts
      step = wide ? 8 : 4
      at = start
      instants = (0...times).map { |i| wide ? Zone.__s64__(data, at + i * 8) : Zone.__s32__(data, at + i * 4) }
      at = at + times * step
      indexes = (0...times).map { |i| data.getbyte(at + i) }
      at = at + times
      raw = (0...types).map do |i|
        [Zone.__s32__(data, at + i * 6), data.getbyte(at + i * 6 + 4) != 0, data.getbyte(at + i * 6 + 5)]
      end
      at = at + types * 6
      names = data[at, chars]
      at = at + chars + leaps * (step + 4) + isstd + isut
      @types = raw.map do |offset, dst, name_at|
        stop = name_at
        stop = stop + 1 while stop < names.bytesize && names.getbyte(stop) != 0
        [offset, dst, names[name_at, stop - name_at]]
      end
      @transitions = (0...times).map { |i| [instants[i], indexes[i]] }
      @first = @types.find { |type| !type[1] } || @types[0]
      @fallback = @transitions.empty? ? (@types[0] || @fallback) : @types[@transitions[times - 1][1]]
      if wide && data.getbyte(at) == 10
        stop = at + 1
        stop = stop + 1 while stop < data.bytesize && data.getbyte(stop) != 10
        footer = data[at + 1, stop - at - 1]
        @rule = Zone.__rule__(footer) unless footer.empty?
      end
    end

    def self.__u32__(data, at)
      data.getbyte(at) * 16_777_216 + data.getbyte(at + 1) * 65_536 + data.getbyte(at + 2) * 256 + data.getbyte(at + 3)
    end

    def self.__s32__(data, at)
      value = __u32__(data, at)
      value >= 2_147_483_648 ? value - 4_294_967_296 : value
    end

    def self.__s64__(data, at)
      value = __u32__(data, at) * 4_294_967_296 + __u32__(data, at + 4)
      value >= 9_223_372_036_854_775_808 ? value - 18_446_744_073_709_551_616 : value
    end

    # A POSIX zone: `STD offset [DST [offset] [,start[/time],end[/time]]]`.
    # The offset is hours *west*, so it is negated. nil when it is not one,
    # and then the zone is UTC under the name it was given.
    def self.__rule__(text)
      name = /\A(?:<([^>]+)>|([A-Za-z]{3,}))/
      clock = /\A([+-]?)(\d{1,3})(?::(\d{1,2})(?::(\d{1,2}))?)?/
      match = name.match(text)
      return nil if match.nil?
      standard = match[1] || match[2]
      rest = match.post_match
      match = clock.match(rest)
      return [standard, 0, nil, 0, nil, nil] if match.nil?
      offset = -__clock__(match)
      rest = match.post_match
      match = name.match(rest)
      return [standard, offset, nil, 0, nil, nil] if match.nil?
      summer = match[1] || match[2]
      rest = match.post_match
      summer_offset = offset + 3_600
      match = clock.match(rest)
      unless match.nil?
        summer_offset = -__clock__(match)
        rest = match.post_match
      end
      # The United States' rule, which is what a zone with no rule means.
      rest = ",M3.2.0,M11.1.0" if rest.empty?
      parts = rest.split(",")
      return [standard, offset, nil, 0, nil, nil] unless parts.size == 3
      first = __change__(parts[1])
      last = __change__(parts[2])
      return [standard, offset, nil, 0, nil, nil] if first.nil? || last.nil?
      [standard, offset, summer, summer_offset, first, last]
    end

    def self.__clock__(match)
      seconds = match[2].to_i * 3_600 + (match[3] || 0).to_i * 60 + (match[4] || 0).to_i
      match[1] == "-" ? -seconds : seconds
    end

    # One end of summer time: `Mm.w.d`, `Jn` or `n`, and the local time of
    # day it happens, 02:00 unless said.
    def self.__change__(text)
      date, time = text.split("/")
      at = 7_200
      unless time.nil?
        match = /\A([+-]?)(\d{1,3})(?::(\d{1,2})(?::(\d{1,2}))?)?\z/.match(time)
        return nil if match.nil?
        at = __clock__(match)
      end
      if (match = /\AM(\d{1,2})\.(\d)\.(\d)\z/.match(date))
        [:month, match[1].to_i, match[2].to_i, match[3].to_i, at]
      elsif (match = /\AJ(\d{1,3})\z/.match(date))
        [:julian, match[1].to_i, 0, 0, at]
      elsif (match = /\A(\d{1,3})\z/.match(date))
        [:day, match[1].to_i, 0, 0, at]
      end
    end

    # The local-clock second of the year's change, as seconds since the epoch
    # on a clock that never changes.
    def self.__change_at__(change, year)
      kind, a, b, c, at = change
      days = if kind == :month
               first = Time.__days__(year, a, 1)
               weekday = (first + 4) % 7
               day = first + (c - weekday) % 7 + (b - 1) * 7
               next_month = a == 12 ? Time.__days__(year + 1, 1, 1) : Time.__days__(year, a + 1, 1)
               day = day - 7 while day >= next_month
               day
             elsif kind == :julian
               leap = Time.__days__(year, 3, 1) - Time.__days__(year, 2, 1) == 29
               Time.__days__(year, 1, 1) + a - 1 + (leap && a >= 60 ? 1 : 0)
             else
               Time.__days__(year, 1, 1) + a
             end
      days * 86_400 + at
    end

    def self.__apply__(rule, seconds)
      standard, offset, summer, summer_offset, first, last = rule
      return [offset, false, standard] if summer.nil?
      year = Time.__civil__((seconds + offset).div(86_400))[0]
      begins = __change_at__(first, year) - offset
      ends = __change_at__(last, year) - summer_offset
      inside = begins < ends ? (seconds >= begins && seconds < ends) : (seconds >= begins || seconds < ends)
      inside ? [summer_offset, true, summer] : [offset, false, standard]
    end
  end
  private_constant :Zone
end
