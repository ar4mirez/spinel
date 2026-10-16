# `date_core` (#60): the C half of CRuby's `date` gem — `date_core.c`,
# `date_parse.c`, `date_strftime.c`, `date_strptime.c` — in Ruby. `date.rb`,
# the gem's own Ruby half, requires it.
#
# A date is kept as CRuby keeps one: the chronological Julian day, the
# seconds into it and the nanoseconds past those, all in UTC, beside the
# offset from UTC and the day of calendar reform. The civil fields are worked
# out from those, in the local time the offset says, when asked for. The
# calendar arithmetic is exact: the floating-point formulas of `date_core.c`
# turned into integer division, so a year far from now is as exact as one
# near it.

class Date
  include Comparable

  class Error < ArgumentError
  end

  ITALY = 2299161
  ENGLAND = 2361222
  JULIAN = Float::INFINITY
  GREGORIAN = -Float::INFINITY

  MONTHNAMES = [nil, "January", "February", "March", "April", "May", "June", "July",
                "August", "September", "October", "November", "December"].map { |s| s&.encode(Encoding::US_ASCII)&.freeze }.freeze
  ABBR_MONTHNAMES = [nil, "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
                    .map { |s| s&.encode(Encoding::US_ASCII)&.freeze }.freeze
  DAYNAMES = %w[Sunday Monday Tuesday Wednesday Thursday Friday Saturday]
             .map { |s| s.encode(Encoding::US_ASCII).freeze }.freeze
  ABBR_DAYNAMES = %w[Sun Mon Tue Wed Thu Fri Sat].map { |s| s.encode(Encoding::US_ASCII).freeze }.freeze

  DAY_IN_SECONDS = 86400
  private_constant :DAY_IN_SECONDS

  # --- the calendar, exactly --------------------------------------------------

  # The day of reform as a number to compare with: a Float for the two
  # proleptic calendars, an Integer otherwise.
  def self.__sg__(start)
    return ITALY if start.nil?
    start = start.to_f if Float === start
    if Numeric === start && !(Float === start && (start.infinite? || start.nan?))
      start = start.to_i
      # CRuby's `valid_sg`: a reform day outside its range is ITALY, warned.
      unless start >= 2298874 && start <= 2426355
        warn "invalid start is ignored" if $VERBOSE
        return ITALY
      end
      return start
    end
    return start if Float === start && start.infinite?
    ITALY
  end

  # `c_civil_to_jd`: the Julian day of a civil date, and whether it is in
  # the Gregorian calendar.
  def self.__civil_to_jd__(y, m, d, sg)
    if m <= 2
      y -= 1
      m += 12
    end
    a = y.div(100)
    b = 2 - a + a.div(4)
    jd = (1461 * (y + 4716)).div(4) + (306001 * (m + 1)).div(10000) + d + b - 1524
    if jd < sg
      jd -= b
      [jd, false]
    else
      [jd, true]
    end
  end

  # `c_jd_to_civil`: year, month and day of a Julian day.
  def self.__jd_to_civil__(jd, sg)
    if jd < sg
      a = jd
    else
      x = (4 * jd - 7468865).div(146097)
      a = jd + 1 + x - x.div(4)
    end
    b = a + 1524
    c = (20 * b - 2442).div(7305)
    d = (1461 * c).div(4)
    e = ((b - d) * 10000).div(306001)
    dom = b - d - (306001 * e).div(10000)
    if e <= 13
      [c - 4716, e - 1, dom]
    else
      [c - 4715, e - 13, dom]
    end
  end

  def self.__gregorian_leap__(y) = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
  def self.__julian_leap__(y) = y % 4 == 0

  # The first day of a month on or after the reform that has one, as
  # `c_find_fdom` finds it, and the last as `c_find_ldom` does.
  def self.__find_fdom__(y, m, sg)
    (1..30).each do |d|
      jd, = __civil_to_jd__(y, m, d, sg)
      return jd if __jd_to_civil__(jd, sg) == [y, m, d]
    end
    nil
  end

  def self.__find_ldom__(y, m, sg)
    31.downto(1) do |d|
      jd, = __civil_to_jd__(y, m, d, sg)
      return jd if __jd_to_civil__(jd, sg) == [y, m, d]
    end
    nil
  end

  # `c_valid_civil_p`: the Julian day of a civil date, negative month and
  # day counting from the end, or nil when there is no such date.
  def self.__valid_civil__(y, m, d, sg)
    m += 13 if m < 0
    return nil if m < 1 || m > 12
    if d < 0
      last = __find_ldom__(y, m, sg)
      return nil if last.nil?
      jd = last + d + 1
      y2, m2, = __jd_to_civil__(jd, sg)
      return nil unless y2 == y && m2 == m
      return jd
    end
    return nil if d < 1
    jd, = __civil_to_jd__(y, m, d, sg)
    return nil unless __jd_to_civil__(jd, sg) == [y, m, d]
    jd
  end

  # `c_valid_ordinal_p`.
  def self.__valid_ordinal__(y, d, sg)
    if d < 0
      last = __find_ldoy__(y, sg)
      return nil if last.nil?
      jd = last + d + 1
      y2, = __jd_to_civil__(jd, sg)
      return nil unless y2 == y
      return jd
    end
    first = __find_fdoy__(y, sg)
    return nil if first.nil? || d < 1
    jd = first + d - 1
    y2, = __jd_to_civil__(jd, sg)
    y2 == y ? jd : nil
  end

  def self.__find_fdoy__(y, sg)
    (1..30).each do |d|
      jd = __valid_civil__(y, 1, d, sg)
      return jd unless jd.nil?
    end
    nil
  end

  def self.__find_ldoy__(y, sg)
    31.downto(1) do |d|
      jd = __valid_civil__(y, 12, d, sg)
      return jd unless jd.nil?
    end
    nil
  end

  # `c_commercial_to_jd` and its inverse: ISO weeks, Monday 1.
  def self.__commercial_to_jd__(y, w, d, sg)
    rjd2 = __find_fdoy__(y, sg) + 3
    rjd2 - ((rjd2 - 1 + 1) % 7) + 7 * (w - 1) + (d - 1)
  end

  def self.__jd_to_commercial__(jd, sg)
    a, = __jd_to_civil__(jd - 3, sg)
    y = a
    j = __commercial_to_jd__(y + 1, 1, 1, sg)
    y += 1 if jd >= j
    w = 1 + (jd - __commercial_to_jd__(y, 1, 1, sg)).div(7)
    d = (jd + 1) % 7
    d = 7 if d == 0
    [y, w, d]
  end

  def self.__valid_commercial__(y, w, d, sg)
    d += 8 if d < 0
    if w < 0
      ry, rw, = __jd_to_commercial__(__commercial_to_jd__(y + 1, 1, 1, sg) + w * 7, sg)
      return nil unless ry == y
      w = rw
    end
    jd = __commercial_to_jd__(y, w, d, sg)
    __jd_to_commercial__(jd, sg) == [y, w, d] ? jd : nil
  end

  # --- making one ---------------------------------------------------------------

  def self.__integer__(value, name = nil)
    return value if Integer === value
    unless Numeric === value
      raise TypeError, "invalid #{name || "argument"} (not numeric)" if name
      raise TypeError, "expected numeric"
    end
    value
  end

  # A number that may have a fraction: the whole part, and the fraction.
  def self.__split__(value)
    return [value, 0] if Integer === value
    whole = value.floor
    [whole, value - whole]
  end

  # Nanoseconds as CRuby keeps them: an Integer, or a Rational when there is
  # a fraction of one.
  def self.__norm__(nanos)
    Rational === nanos && nanos.denominator == 1 ? nanos.to_i : nanos
  end

  def self.__alloc__(jd, df = 0, sf = 0, of = 0, sg = ITALY)
    date = allocate
    date.__send__(:__init__, jd, df, sf, of, sg)
    date
  end

  def __init__(jd, df, sf, of, sg)
    @jd = jd
    @df = df
    @sf = sf
    @of = of
    @sg = sg
    self
  end
  private :__init__

  # The day and fraction a number of days names, as `jd` does: a fraction is
  # kept as seconds and nanoseconds.
  def self.__from_days__(days, of, sg)
    jd, fraction = __split__(days)
    df = 0
    sf = 0
    unless fraction == 0
      seconds = fraction.to_r * DAY_IN_SECONDS
      df = seconds.floor
      sf = __norm__((seconds - df) * 1_000_000_000)
    end
    __alloc__(jd, df, sf, of, sg)
  end

  def self.civil(year = -4712, month = 1, mday = 1, start = ITALY)
    __check_numeric__(year, "year")
    __check_numeric__(month, "month")
    __check_numeric__(mday, "day")
    sg = __sg__(start)
    y = year.to_i
    year_fraction = Integer === year ? 0 : year - year.floor
    m = month.to_i
    d, fraction = __split__(mday)
    jd = __valid_civil__(y, m, d, sg)
    raise Error, "invalid date" if jd.nil?
    __finish_date__(jd, fraction, sg)
  end

  # A Date keeps no part of a day it was given: CRuby's `d_simple_new_internal`.
  def self.__finish_date__(jd, fraction, sg)
    __alloc__(jd, 0, 0, 0, sg)
  end

  def self.__check_numeric__(value, name)
    return if Numeric === value
    raise TypeError, "invalid #{name} (not numeric)"
  end

  def self.new(*args)
    civil(*args)
  end

  def self.jd(jd = 0, start = ITALY)
    __check_numeric__(jd, "jd")
    __from_days__(jd, 0, __sg__(start))
  end

  def self.ordinal(year = -4712, yday = 1, start = ITALY)
    __check_numeric__(year, "year")
    __check_numeric__(yday, "yday")
    sg = __sg__(start)
    jd = __valid_ordinal__(year.to_i, yday.to_i, sg)
    raise Error, "invalid date" if jd.nil?
    __alloc__(jd, 0, 0, 0, sg)
  end

  def self.commercial(cwyear = -4712, cweek = 1, cwday = 1, start = ITALY)
    __check_numeric__(cwyear, "cwyear")
    __check_numeric__(cweek, "cweek")
    __check_numeric__(cwday, "cwday")
    sg = __sg__(start)
    jd = __valid_commercial__(cwyear.to_i, cweek.to_i, cwday.to_i, sg)
    raise Error, "invalid date" if jd.nil?
    __alloc__(jd, 0, 0, 0, sg)
  end

  def self.today(start = ITALY)
    now = Time.now
    civil(now.year, now.mon, now.mday, start)
  end

  def self.valid_jd?(jd, start = ITALY)
    return false unless Numeric === jd
    true
  end

  def self.valid_civil?(year, month, mday, start = ITALY)
    return false unless Numeric === year && Numeric === month && Numeric === mday
    !__valid_civil__(year.to_i, month.to_i, mday.to_i, __sg__(start)).nil?
  end

  def self.valid_ordinal?(year, yday, start = ITALY)
    return false unless Numeric === year && Numeric === yday
    !__valid_ordinal__(year.to_i, yday.to_i, __sg__(start)).nil?
  end

  def self.valid_commercial?(cwyear, cweek, cwday, start = ITALY)
    return false unless Numeric === cwyear && Numeric === cweek && Numeric === cwday
    !__valid_commercial__(cwyear.to_i, cweek.to_i, cwday.to_i, __sg__(start)).nil?
  end

  class << self
    alias valid_date? valid_civil?
  end

  def self.gregorian_leap?(year)
    raise TypeError, "invalid year (not numeric)" unless Numeric === year
    __gregorian_leap__(year.to_i)
  end

  def self.julian_leap?(year)
    raise TypeError, "invalid year (not numeric)" unless Numeric === year
    __julian_leap__(year.to_i)
  end

  class << self
    alias leap? gregorian_leap?
  end

  # --- what it is -----------------------------------------------------------------

  # The local day and second, the offset applied.
  def __local__
    @__local__ ||= begin
      seconds = @df + @of
      [@jd + seconds.div(DAY_IN_SECONDS), seconds % DAY_IN_SECONDS]
    end
  end

  def __civil__ = (@__civil__ ||= Date.__jd_to_civil__(__local__[0], @sg))

  def jd = __local__[0]
  def year = __civil__[0]
  def mon = __civil__[1]
  alias month mon
  def mday = __civil__[2]
  alias day mday

  def start = @sg.to_f

  def wday = (jd + 1) % 7

  def yday
    jd - Date.__find_fdoy__(year, @sg) + 1
  end

  def __commercial__ = (@__commercial__ ||= Date.__jd_to_commercial__(jd, @sg))
  def cwyear = __commercial__[0]
  def cweek = __commercial__[1]
  def cwday = __commercial__[2]

  def mjd = jd - 2400001
  def ld = jd - 2299160

  def ajd
    Rational(@jd * DAY_IN_SECONDS * 1_000_000_000 + @df * 1_000_000_000 + @sf, DAY_IN_SECONDS * 1_000_000_000) - Rational(1, 2)
  end

  def amjd = ajd - Rational(4800001, 2)

  def day_fraction
    seconds = __local__[1]
    return 0 if seconds == 0 && @sf == 0 && !is_a?(DateTime) && @df == 0
    Rational(seconds * 1_000_000_000 + @sf, DAY_IN_SECONDS * 1_000_000_000)
  end

  def julian? = jd < @sg
  def gregorian? = !julian?

  def leap?
    if gregorian?
      Date.__gregorian_leap__(year)
    else
      Date.__julian_leap__(year)
    end
  end

  def sunday? = wday == 0
  def monday? = wday == 1
  def tuesday? = wday == 2
  def wednesday? = wday == 3
  def thursday? = wday == 4
  def friday? = wday == 5
  def saturday? = wday == 6

  def new_start(start = ITALY)
    self.class.__alloc__(@jd, @df, @sf, @of, Date.__sg__(start))
  end

  def italy = new_start(ITALY)
  def england = new_start(ENGLAND)
  def julian = new_start(JULIAN)
  def gregorian = new_start(GREGORIAN)

  # --- arithmetic -----------------------------------------------------------------

  def +(other)
    case other
    when Integer
      self.class.__alloc__(@jd + other, @df, @sf, @of, @sg)
    when Rational, Float
      raise TypeError, "expected numeric" if Float === other && (other.nan? || other.infinite?)
      nanos = (other.to_r * DAY_IN_SECONDS * 1_000_000_000).round
      total = @df * 1_000_000_000 + @sf + nanos
      days, rest = total.divmod(DAY_IN_SECONDS * 1_000_000_000)
      df, sf = rest.divmod(1_000_000_000)
      self.class.__alloc__(@jd + days, df, sf, @of, @sg)
    when Numeric
      self + other.to_r
    else
      raise TypeError, "expected numeric"
    end
  end

  def -(other)
    case other
    when Date
      ajd - other.ajd
    when Numeric
      self + -other
    else
      raise TypeError, "expected numeric or date"
    end
  end

  def >>(n)
    raise TypeError, "expected numeric" unless Numeric === n
    t = year * 12 + (mon - 1) + n.to_i
    y = t.div(12)
    m = t % 12 + 1
    d = mday
    sg = @sg
    jd = nil
    until (jd = Date.__valid_civil__(y, m, d, sg))
      d -= 1
      raise Error, "invalid date" if d < 1
    end
    self + (jd - self.jd)
  end

  def <<(n)
    raise TypeError, "expected numeric" unless Numeric === n
    self >> -n
  end

  def next_day(n = 1) = self + n
  def prev_day(n = 1) = self - n
  def next_month(n = 1) = self >> n
  def prev_month(n = 1) = self << n
  def next_year(n = 1) = self >> (n * 12)
  def prev_year(n = 1) = self << (n * 12)

  def next = next_day
  alias succ next

  def step(limit, by = 1)
    return to_enum(:step, limit, by) unless block_given?
    raise ArgumentError, "step can't be 0" if by == 0
    date = self
    if by > 0
      while date <= limit
        yield date
        date += by
      end
    else
      while date >= limit
        yield date
        date += by
      end
    end
    self
  end

  def upto(max, &block)
    return to_enum(:upto, max) unless block
    step(max, 1, &block)
  end

  def downto(min, &block)
    return to_enum(:downto, min) unless block
    step(min, -1, &block)
  end

  # --- comparing --------------------------------------------------------------------

  def <=>(other)
    case other
    when Date
      ajd <=> other.ajd
    when Numeric
      ajd <=> other
    else
      nil
    end
  end

  def ==(other)
    return false unless Date === other || Numeric === other
    (self <=> other) == 0
  end

  def ===(other)
    case other
    when Date
      jd == other.jd
    when Numeric
      jd == other.to_i
    else
      false
    end
  end

  def eql?(other)
    Date === other && ajd == other.ajd && ((is_a?(DateTime) == other.is_a?(DateTime)) || true)
  end

  def hash = ajd.hash

  # --- conversions ------------------------------------------------------------------

  def to_date
    return self if instance_of?(Date)
    Date.__alloc__(jd, 0, 0, 0, @sg)
  end

  def to_datetime
    DateTime.__alloc__(@jd, @df, @sf, @of, @sg)
  end

  # A `Time` is proleptic Gregorian, so its fields are the Gregorian date's.
  def to_time
    g = gregorian
    Time.local(g.year, g.mon, g.mday)
  end

  def to_s = format("%.4d-%02d-%02d", year, mon, mday).encode(Encoding::US_ASCII)

  def inspect
    sg = Float === @sg ? (@sg > 0 ? "Inf" : "-Inf") : @sg.to_s
    "#<#{self.class}: #{self} ((#{@jd}j,#{@df}s,#{@sf.inspect}n),#{@of >= 0 ? "+" : "-"}#{@of.abs}s,#{sg}j)>".encode(Encoding::US_ASCII)
  end

  def marshal_dump
    [0, @jd, @df, @sf, @of, @sg.to_f]
  end

  def marshal_load(array)
    raise TypeError, "expected an array" unless Array === array
    case array.size
    when 2
      ajd, sg = array
      __init_from_ajd__(ajd, 0, sg)
    when 3
      ajd, of, sg = array
      __init_from_ajd__(ajd, (of * DAY_IN_SECONDS).to_i, sg)
    when 6
      _nth, jd, df, sf, of, sg = array
      __init__(jd, df, sf.to_i, of, Float === sg && sg.finite? ? sg.to_i : sg)
    else
      raise TypeError, "invalid size"
    end
    self
  end

  def __init_from_ajd__(ajd, of, sg)
    days = ajd + Rational(1, 2)
    jd = days.floor
    seconds = (days - jd) * DAY_IN_SECONDS
    df = seconds.floor
    __init__(jd, df, ((seconds - df) * 1_000_000_000).round, of, sg)
  end
  private :__init_from_ajd__

  def self._load(string)
    array = Marshal.load(string)
    date = allocate
    date.marshal_load(array)
    date
  end

  def deconstruct_keys(keys)
    all = { year: year, month: mon, day: mday, yday: yday, wday: wday }
    if is_a?(DateTime)
      all.merge!(hour: hour, min: min, sec: sec, sec_fraction: sec_fraction, zone: zone)
    end
    return all if keys.nil?
    raise TypeError, "wrong argument type #{keys.class} (expected Array or nil)" unless Array === keys
    keys.each_with_object({}) { |key, out| out[key] = all[key] if all.key?(key) }
  end

  def initialize_copy(other)
    __init__(*other.__parts__)
  end

  def __parts__ = [@jd, @df, @sf, @of, @sg]
end

class DateTime < Date
  def self.civil(*args)
    if args.size > 8
      raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..8)"
    end
    year, month, mday, hour, minute, second, offset, start = args
    year = -4712 if args.size < 1
    month = 1 if args.size < 2
    mday = 1 if args.size < 3
    hour = 0 if args.size < 4
    minute = 0 if args.size < 5
    second = 0 if args.size < 6
    offset = 0 if args.size < 7
    start = Date::ITALY if args.size < 8
    # The last of day, hour, minute and second given: a fraction may only be
    # in that one, measured.
    last = (args.size.clamp(3, 6)) - 3
    __check_numeric__(year, "year")
    __check_numeric__(month, "month")
    __check_numeric__(mday, "day")
    __check_numeric__(hour, "hour")
    __check_numeric__(minute, "minute")
    __check_numeric__(second, "second")
    sg = Date.__sg__(start)
    d, day_fraction = Date.__split__(mday)
    jd = Date.__valid_civil__(year.to_i, month.to_i, d, sg)
    raise Date::Error, "invalid date" if jd.nil?
    __at__(jd, day_fraction, hour, minute, second, offset, sg, last)
  end

  def self.new(*args) = civil(*args)

  def self.jd(*args)
    if args.size > 6
      raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..6)"
    end
    jd, hour, minute, second, offset, start = args
    jd ||= 0
    hour ||= 0
    minute ||= 0
    second ||= 0
    offset ||= 0
    start = Date::ITALY if args.size < 6
    __check_numeric__(jd, "jd")
    d, fraction = Date.__split__(jd)
    __at__(d, fraction, hour, minute, second, offset, Date.__sg__(start), args.size.clamp(1, 4) - 1)
  end

  def self.ordinal(year = -4712, yday = 1, hour = 0, minute = 0, second = 0, offset = 0, start = Date::ITALY)
    sg = Date.__sg__(start)
    jd = Date.__valid_ordinal__(year.to_i, yday.to_i, sg)
    raise Date::Error, "invalid date" if jd.nil?
    __at__(jd, 0, hour, minute, second, offset, sg)
  end

  def self.commercial(cwyear = -4712, cweek = 1, cwday = 1, hour = 0, minute = 0, second = 0, offset = 0, start = Date::ITALY)
    sg = Date.__sg__(start)
    jd = Date.__valid_commercial__(cwyear.to_i, cweek.to_i, cwday.to_i, sg)
    raise Date::Error, "invalid date" if jd.nil?
    __at__(jd, 0, hour, minute, second, offset, sg)
  end

  # A local time on a local day, given as numbers each of which may have a
  # fraction carried down, and an offset: kept in UTC.
  def self.__at__(jd, day_fraction, hour, minute, second, offset, sg, last = 3)
    of = __offset__(offset)
    h, hf = Date.__split__(hour)
    mi, mf = Date.__split__(minute)
    s, sf = Date.__split__(second)
    h += 24 if h < 0
    mi += 60 if mi < 0
    s += 60 if s < 0
    raise Date::Error, "invalid date" unless (0..24).cover?(h) && (0..59).cover?(mi) && (0..59).cover?(s)
    raise Date::Error, "invalid date" if h == 24 && (mi != 0 || s != 0)
    # A fraction may only be the last field given: measured.
    [day_fraction, hf, mf].each_with_index do |fraction, index|
      raise Date::Error, "invalid fraction" if fraction != 0 && index < last
    end
    nanos = (day_fraction.to_r * 86400 + hf.to_r * 3600 + mf.to_r * 60 + sf.to_r) * 1_000_000_000
    local = (h * 3600 + mi * 60 + s) * 1_000_000_000 + nanos
    utc = jd * 86400 * 1_000_000_000 + local - of * 1_000_000_000
    day, rest = utc.divmod(86400 * 1_000_000_000)
    df, sfn = rest.divmod(1_000_000_000)
    __alloc__(day, df, Date.__norm__(sfn), of, sg)
  end

  # An offset as seconds east: a fraction of a day, or `+HH:MM` and kin.
  def self.__offset__(offset)
    case offset
    when String
      seconds = Date.__zone_to_diff__(offset)
      if seconds.nil?
        warn "invalid offset is ignored", uplevel: 2
        return 0
      end
      seconds
    when Numeric
      seconds = (offset.to_r * 86400)
      unless seconds.abs <= 86400
        warn "invalid offset is ignored", uplevel: 2
        return 0
      end
      seconds.round
    else
      raise TypeError, "invalid offset"
    end
  end

  def self.now(start = Date::ITALY)
    now = Time.now
    __at__(Date.__valid_civil__(now.year, now.mon, now.mday, Date.__sg__(start)), 0, now.hour, now.min,
           now.sec + Rational(now.nsec, 1_000_000_000), Rational(now.utc_offset, 86400), Date.__sg__(start))
  end

  def hour = __local__[1] / 3600
  alias __hour__ hour
  def min = (__local__[1] % 3600) / 60
  alias minute min
  def sec = __local__[1] % 60
  alias second sec

  def sec_fraction = Rational(@sf, 1_000_000_000)
  alias second_fraction sec_fraction

  def offset = Rational(@of, 86400)

  def zone
    sign = @of < 0 ? "-" : "+"
    hours, minutes = @of.abs.divmod(3600)
    format("%s%02d:%02d", sign, hours, minutes / 60).encode(Encoding::US_ASCII)
  end

  def new_offset(offset = 0)
    of = DateTime.__offset__(offset)
    self.class.__alloc__(@jd, @df, @sf, of, @sg)
  end

  def day_fraction
    seconds = __local__[1]
    Rational(seconds * 1_000_000_000 + @sf, 86400 * 1_000_000_000)
  end

  def to_s
    format("%.4d-%02d-%02dT%02d:%02d:%02d%s", year, mon, mday, hour, min, sec, zone).encode(Encoding::US_ASCII)
  end

  def to_date = Date.__alloc__(jd, 0, 0, 0, @sg)
  def to_datetime = self

  def to_time
    g = gregorian
    seconds = g.sec + Rational(@sf, 1_000_000_000)
    Time.new(g.year, g.mon, g.mday, g.hour, g.min, seconds, @of)
  end
end

class Time
  def to_date
    Date.civil(year, mon, mday, Date::GREGORIAN).new_start(Date::ITALY)
  end

  def to_datetime
    DateTime.civil(year, mon, mday, hour, min, sec + Rational(nsec, 1_000_000_000), Rational(utc_offset, 86400),
                   Date::GREGORIAN).new_start(Date::ITALY)
  end

  def to_time = self
end

# --- strftime: `date_strftime.c` ------------------------------------------------

class Date
  # Width and padding each conversion has unless the format says otherwise.
  STRFTIME_NUMBERS = {
    "d" => [2, "0"], "e" => [2, " "], "j" => [3, "0"], "m" => [2, "0"],
    "H" => [2, "0"], "k" => [2, " "], "I" => [2, "0"], "l" => [2, " "],
    "M" => [2, "0"], "S" => [2, "0"], "u" => [1, "0"], "w" => [1, "0"],
    "U" => [2, "0"], "W" => [2, "0"], "V" => [2, "0"], "y" => [2, "0"],
    "g" => [2, "0"], "C" => [2, "0"], "s" => [1, "0"], "Q" => [1, "0"],
  }.freeze
  private_constant :STRFTIME_NUMBERS

  STRFTIME_COMPOSITES = {
    "c" => "%a %b %e %H:%M:%S %Y", "D" => "%m/%d/%y", "F" => "%Y-%m-%d", "v" => "%e-%^b-%Y",
    "x" => "%m/%d/%y", "X" => "%H:%M:%S", "r" => "%I:%M:%S %p", "R" => "%H:%M", "T" => "%H:%M:%S",
    "+" => "%a %b %e %H:%M:%S %Z %Y",
  }.freeze
  private_constant :STRFTIME_COMPOSITES

  def strftime(format = "%F")
    format = IO.__str__(format)
    out = __strftime__(format)
    out.force_encoding(format.ascii_only? ? Encoding::US_ASCII : format.encoding)
  end

  def __hour__ = 0
  def __minute__ = 0
  def __second__ = 0
  def __nanos__ = 0
  def __offset_seconds__ = 0
  def __zone__ = "+00:00"

  def __strftime__(format)
    out = +""
    i = 0
    n = format.bytesize
    bytes = format.b
    while i < n
      c = bytes.getbyte(i)
      unless c == 37 # %
        out << bytes.byteslice(i, 1)
        i += 1
        next
      end
      start = i
      i += 1
      flags = +""
      while i < n && "-_0^#".include?(bytes.byteslice(i, 1))
        flags << bytes.byteslice(i, 1)
        i += 1
      end
      width = nil
      digits = +""
      while i < n && bytes.byteslice(i, 1).match?(/\d/)
        digits << bytes.byteslice(i, 1)
        i += 1
      end
      width = digits.to_i unless digits.empty?
      colons = 0
      while i < n && bytes.byteslice(i, 1) == ":"
        colons += 1
        i += 1
      end
      # `E` and `O` modify the conversion that follows, which keeps its own
      # meaning when the pair is one POSIX names.
      if i < n && "EO".include?(bytes.byteslice(i, 1))
        modifier = bytes.byteslice(i, 1)
        following = bytes.byteslice(i + 1, 1)
        allowed = modifier == "E" ? "cCxXyY" : "deHkIlmMSuUVwWy"
        if following && allowed.include?(following)
          i += 1
        else
          out << bytes.byteslice(start, i - start + 1)
          i += 1
          next
        end
      end
      conversion = bytes.byteslice(i, 1)
      if conversion.nil?
        out << bytes.byteslice(start, i - start)
        break
      end
      i += 1
      piece = colons > 0 && conversion != "z" ? nil : __strftime_one__(conversion, flags, width, colons)
      if piece.nil?
        out << bytes.byteslice(start, i - start)
      else
        out << piece
      end
    end
    out
  end

  def __pad__(text, width, pad, flags)
    return text if flags.include?("-") || width.nil? || text.size >= width
    # The last of `_` and `0` says how: `%_010h` pads with zeros.
    last = flags.rindex(/[_0]/)
    pad = flags[last] == "_" ? " " : "0" unless last.nil?
    if pad == "0" && (text.start_with?("-") || text.start_with?("+"))
      text[0] + text[1..].rjust(width - 1, "0")
    else
      text.rjust(width, pad)
    end
  end

  def __text__(text, flags, width)
    text = text.upcase if flags.include?("^") || flags.include?("#")
    __pad__(text, width, " ", flags)
  end

  def __strftime_one__(conversion, flags, width, colons)
    if (composite = STRFTIME_COMPOSITES[conversion])
      text = __strftime__(composite)
      text = text.upcase if flags.include?("^")
      return __pad__(text, width, flags.include?("0") ? "0" : " ", flags)
    end
    if (number = STRFTIME_NUMBERS[conversion])
      default_width, pad = number
      value = __strftime_number__(conversion)
      text = value.to_s
      width ||= default_width
      if pad == " " && flags.include?("0")
        pad = "0"
      end
      return __pad__(text, width, pad, flags)
    end
    case conversion
    when "Y", "G"
      value = conversion == "Y" ? year : cwyear
      if width.nil?
        return value < 0 ? "-" + value.abs.to_s.rjust(4, "0") : value.to_s.rjust(flags.include?("-") ? 0 : 4, flags.include?("_") ? " " : "0")
      end
      __pad__(value.to_s, width, "0", flags)
    when "B" then __text__(MONTHNAMES[mon], flags, width)
    when "b", "h" then __text__(ABBR_MONTHNAMES[mon], flags, width)
    when "A" then __text__(DAYNAMES[wday], flags, width)
    when "a" then __text__(ABBR_DAYNAMES[wday], flags, width)
    when "p"
      text = __hour__ < 12 ? "AM" : "PM"
      text = text.downcase if flags.include?("#")
      __pad__(text, width, " ", flags)
    when "P"
      text = __hour__ < 12 ? "am" : "pm"
      text = text.upcase if flags.include?("^")
      __pad__(text, width, " ", flags)
    when "L", "N"
      digits = width || (conversion == "L" ? 3 : 9)
      text = (__nanos__ * 10**digits / 1_000_000_000).floor.to_s.rjust(digits, "0")
      text
    when "z" then __strftime_zone__(flags, width, colons)
    when "Z" then __text__(__zone__.dup, flags.delete("^"), width)
    when "n" then "\n"
    when "t" then "\t"
    when "%" then __pad__(+"%", width, " ", flags)
    end
  end

  def __strftime_number__(conversion)
    case conversion
    when "d", "e" then mday
    when "j" then yday
    when "m" then mon
    when "H", "k" then __hour__
    when "I", "l" then h = __hour__ % 12; h == 0 ? 12 : h
    when "M" then __minute__
    when "S" then __second__
    when "u" then cwday
    when "w" then wday
    when "U" then (yday - 1 - wday + 7) / 7
    when "W" then (yday - 1 - (wday + 6) % 7 + 7) / 7
    when "V" then cweek
    when "y" then year % 100
    when "g" then cwyear % 100
    when "C" then year.div(100)
    when "s" then ((ajd - Rational(4881175, 2)) * 86400).floor
    when "Q" then ((ajd - Rational(4881175, 2)) * 86_400_000).floor
    end
  end

  def __strftime_zone__(flags, width, colons)
    of = __offset_seconds__
    sign = of < 0 ? "-" : "+"
    hours, rest = of.abs.divmod(3600)
    minutes, seconds = rest.divmod(60)
    hh = flags.include?("-") || flags.include?("_") ? hours.to_s : hours.to_s.rjust(2, "0")
    body = case colons
           when 0 then "#{hh}#{minutes.to_s.rjust(2, "0")}"
           when 1 then "#{hh}:#{minutes.to_s.rjust(2, "0")}"
           when 2 then "#{hh}:#{minutes.to_s.rjust(2, "0")}:#{seconds.to_s.rjust(2, "0")}"
           else
             text = hh
             text += ":#{minutes.to_s.rjust(2, "0")}" if minutes != 0 || seconds != 0
             text += ":#{seconds.to_s.rjust(2, "0")}" if seconds != 0
             text
           end
    text = sign + body
    width ||= 5 if flags.include?("_") && colons == 0
    return text if width.nil? || text.size >= width
    if flags.include?("_")
      text.rjust(width, " ")
    elsif flags.include?("-")
      text
    else
      sign + body.rjust(width - 1, "0")
    end
  end
end

class DateTime < Date
  def strftime(format = "%FT%T%:z") = super(format)

  def __hour__ = hour
  def __minute__ = min
  def __second__ = sec
  def __nanos__ = @sf
  def __offset_seconds__ = @of
  def __zone__ = zone
end

# --- parsing: `date_parse.c` ---------------------------------------------------------

class Date
  # `zonetab.list`: abbreviations and names, hours east of UTC.
  ZONES = {
    "ut" => 0, "gmt" => 0, "est" => -5, "edt" => -4, "cst" => -6, "cdt" => -5, "mst" => -7, "mdt" => -6,
    "pst" => -8, "pdt" => -7, "utc" => 0, "z" => 0, "zulu" => 0,
    "a" => 1, "b" => 2, "c" => 3, "d" => 4, "e" => 5, "f" => 6, "g" => 7, "h" => 8, "i" => 9, "k" => 10,
    "l" => 11, "m" => 12, "n" => -1, "o" => -2, "p" => -3, "q" => -4, "r" => -5, "s" => -6, "t" => -7,
    "u" => -8, "v" => -9, "w" => -10, "x" => -11, "y" => -12,
    "akst" => -9, "akdt" => -8, "hst" => -10, "hast" => -10, "hadt" => -9, "ast" => -4, "adt" => -3,
    "nst" => -3.5, "ndt" => -2.5, "wet" => 0, "west" => 1, "bst" => 1, "wat" => 1, "cet" => 1,
    "cest" => 2, "met" => 1, "mest" => 2, "mewt" => 1, "mez" => 1, "mesz" => 2, "swt" => 1, "sst" => 2,
    "fwt" => 1, "fst" => 2, "eet" => 2, "eest" => 3, "bt" => 3, "zp4" => 4, "zp5" => 5, "zp6" => 6,
    "wast" => 7, "wadt" => 8, "cct" => 8, "jst" => 9, "east" => 10, "eadt" => 11, "gst" => 10,
    "nzt" => 12, "nzst" => 12, "nzdt" => 13, "idle" => 12, "idlw" => -12, "msk" => 3, "msd" => 4,
    "ist" => 5.5, "kst" => 9, "hkt" => 8, "sgt" => 8, "awst" => 8, "acst" => 9.5, "acdt" => 10.5,
    "aest" => 10, "aedt" => 11, "brt" => -3, "brst" => -2, "art" => -3, "clt" => -4, "clst" => -3,
    "pht" => 8, "ict" => 7, "wib" => 7, "pkt" => 5, "npt" => 5.75, "irst" => 3.5, "aft" => 4.5,
    "eat" => 3, "cat" => 2, "sast" => 2, "yst" => -9, "ydt" => -8, "ahst" => -10, "cat_" => -10,
    "nt" => -11, "chst" => 10,
    "afghanistan" => 4.5, "alaskan" => -9, "arab" => 3, "arabian" => 4, "arabic" => 3, "atlantic" => -4,
    "aus central" => 9.5, "aus eastern" => 10, "azores" => -1, "canada central" => -6,
    "cape verde" => -1, "caucasus" => 4, "cen. australia" => 9.5, "central america" => -6,
    "central asia" => 6, "central europe" => 1, "central european" => 1, "central pacific" => 11,
    "central" => -6, "china" => 8, "dateline" => -12, "e. africa" => 3, "e. australia" => 10,
    "e. europe" => 2, "e. south america" => -3, "eastern" => -5, "egypt" => 2, "ekaterinburg" => 5,
    "fiji" => 12, "fle" => 2, "greenland" => -3, "greenwich" => 0, "gtb" => 2, "hawaiian" => -10,
    "india" => 5.5, "iran" => 3.5, "jerusalem" => 2, "korea" => 9, "mexico" => -6, "mid-atlantic" => -2,
    "mountain" => -7, "myanmar" => 6.5, "n. central asia" => 6, "nepal" => 5.75, "new zealand" => 12,
    "newfoundland" => -3.5, "north asia east" => 8, "north asia" => 7, "pacific sa" => -4, "pacific" => -8,
    "romance" => 1, "russian" => 3, "sa eastern" => -3, "sa pacific" => -5, "sa western" => -4,
    "samoa" => -11, "se asia" => 7, "malay peninsula" => 8, "south africa" => 2, "sri lanka" => 6,
    "taipei" => 8, "tasmania" => 10, "tokyo" => 9, "tonga" => 13, "us eastern" => -5,
    "us mountain" => -7, "vladivostok" => 10, "w. australia" => 8, "w. central africa" => 1,
    "w. europe" => 1, "west asia" => 5, "west pacific" => 10, "yakutsk" => 9,
  }.freeze
  private_constant :ZONES

  # `date_zone_to_diff`: seconds east of UTC for a zone as `_parse` finds
  # them, or nil.
  def self.__zone_to_diff__(zone)
    text = zone.to_s.strip.downcase.gsub(/\s+/, " ")
    dst = false
    if text.sub!(/ (?:standard|daylight) time\z/) { |m| dst = m.include?("daylight"); "" }
    elsif text.sub!(/ dst\z/, "")
      dst = true
    end
    if ZONES.key?(text)
      hours = ZONES[text]
      return ((hours + (dst ? 1 : 0)) * 3600).to_i
    end
    text = text.sub(/\A(?:gmt|utc)(?=[-+])/, "")
    return nil unless (m = text.match(/\A([-+])(.*)\z/))
    sign = m[1] == "-" ? -1 : 1
    rest = m[2]
    seconds =
      if (n = rest.match(/\A(\d+):(\d+)(?::(\d+))?\z/))
        n[1].to_i * 3600 + n[2].to_i * 60 + n[3].to_i
      elsif (n = rest.match(/\A(\d+)[,.](\d+)\z/))
        (n[1].to_i * 3600 + Rational("0.#{n[2]}") * 3600).round
      elsif rest.match?(/\A\d+\z/)
        case rest.size
        when 1, 2 then rest.to_i * 3600
        when 3, 4 then rest[0, rest.size - 2].to_i * 3600 + rest[-2, 2].to_i * 60
        else rest[0, rest.size - 4].to_i * 3600 + rest[-4, 2].to_i * 60 + rest[-2, 2].to_i
        end
      end
    seconds && sign * seconds
  end

  ABBR_DAYS_RE = "sun|mon|tue|wed|thu|fri|sat"
  ABBR_MONTHS_RE = "jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec"
  ABBR_DAYS = { "sun" => 0, "mon" => 1, "tue" => 2, "wed" => 3, "thu" => 4, "fri" => 5, "sat" => 6 }.freeze
  ABBR_MONTHS = { "jan" => 1, "feb" => 2, "mar" => 3, "apr" => 4, "may" => 5, "jun" => 6, "jul" => 7,
                  "aug" => 8, "sep" => 9, "oct" => 10, "nov" => 11, "dec" => 12 }.freeze
  private_constant :ABBR_DAYS_RE, :ABBR_MONTHS_RE, :ABBR_DAYS, :ABBR_MONTHS

  def self.__string_arg__(string, limit)
    string = IO.__str__(string) unless string.nil?
    unless limit.nil? || string.nil? || string.bytesize <= limit
      raise ArgumentError, "string length (#{string.bytesize}) exceeds the limit #{limit}"
    end
    string
  end

  def self._parse(string, comp = true, limit: 128)
    raise TypeError, "no implicit conversion of nil into String" if string.nil?
    string = __string_arg__(string, limit)
    str = string.gsub(/[^-+',.\/:@[:alnum:]\[\]]+/, " ")
    h = {}
    h[:_comp] = comp ? true : false
    alpha = str.match?(/[[:alpha:]]/)
    digit = str.match?(/\d/)
    __parse_day__(str, h) if alpha
    __parse_time__(str, h) if digit
    catch(:ok) do
      if alpha && digit
        throw :ok if __parse_eu__(str, h)
        throw :ok if __parse_us__(str, h)
      end
      if digit && str.include?("-")
        throw :ok if __parse_iso__(str, h)
      end
      if digit && str.include?(".")
        throw :ok if __parse_jis__(str, h)
      end
      if alpha && digit && str.include?("-")
        throw :ok if __parse_vms__(str, h)
      end
      if digit && str.include?("/")
        throw :ok if __parse_sla__(str, h)
      end
      if digit && str.include?(".")
        throw :ok if __parse_dot__(str, h)
      end
      if digit
        throw :ok if __parse_iso2__(str, h)
        throw :ok if __parse_year__(str, h)
      end
      throw :ok if alpha && __parse_mon__(str, h)
      if digit
        throw :ok if __parse_mday__(str, h)
        throw :ok if __parse_ddd__(str, h)
      end
    end
    __parse_bc__(str, h) if alpha
    __parse_frag__(str, h) if digit
    if h.delete(:_bc)
      h[:cwyear] = -h[:cwyear] + 1 if h.key?(:cwyear)
      h[:year] = -h[:year] + 1 if h.key?(:year)
    end
    if h.delete(:_comp)
      if h.key?(:cwyear) && h[:cwyear].between?(0, 99)
        h[:cwyear] += h[:cwyear] >= 69 ? 1900 : 2000
      end
      if h.key?(:year) && h[:year].between?(0, 99)
        h[:year] += h[:year] >= 69 ? 1900 : 2000
      end
    end
    h.delete(:_comp)
    h.delete(:_merid)
    if h.key?(:zone) && !h.key?(:offset)
      h[:offset] = __zone_to_diff__(h[:zone])
    end
    h
  end

  def self.__subx__(str, re)
    m = re.match(str)
    return nil if m.nil?
    str[m.begin(0), m.end(0) - m.begin(0)] = " " * (m.end(0) - m.begin(0))
    m
  end

  def self.__parse_day__(str, h)
    m = __subx__(str, /\b(#{ABBR_DAYS_RE})[^-\/\d\s]*/i)
    return false if m.nil?
    h[:wday] = ABBR_DAYS[m[1].downcase]
    true
  end

  TIME_RE = /(
      (?:
        \d+\s*:\s*\d+
        (?:
          \s*:\s*\d+(?:[,.]\d*)?
        )?
      |
        \d+\s*h(?:\s*\d+m?(?:\s*\d+s?)?)?
      )
      (?:
        \s*
        [ap](?:m\b|\.m\.)
      )?
    |
      \d+\s*[ap](?:m\b|\.m\.)
    )
    (?:
      \s*
      (
        (?:gmt|utc?)?[-+]\d+(?:[,.:]\d+(?::\d+)?)?
      |
        (?-i:[[:alpha:].\s]+)(?:standard|daylight)\stime\b
      |
        (?-i:[[:alpha:]]+)(?:\sdst)?\b
      )
    )?/ix
  private_constant :TIME_RE

  def self.__parse_time__(str, h)
    m = __subx__(str, TIME_RE)
    return false if m.nil?
    h[:zone] = m[2] if m[2]
    t = m[1]
    n = t.match(/\A(\d+)h?
          (?:\s*:?\s*(\d+)m?
            (?:
              \s*:?\s*(\d+)(?:[,.](\d+))?s?
            )?
          )?
        (?:\s*([ap])(?:m\b|\.m\.))?/ix)
    hour = n[1].to_i
    h[:hour] = hour
    h[:min] = n[2].to_i if n[2]
    h[:sec] = n[3].to_i if n[3]
    h[:sec_fraction] = Rational(n[4].to_i, 10**n[4].size) if n[4]
    if n[5]
      hour %= 12
      hour += 12 if n[5].downcase == "p"
      h[:hour] = hour
    end
    if h.key?(:zone)
      zone = h.delete(:zone)
      h.delete(:hour)
      hour_value = hour
      h[:zone] = zone
      h[:hour] = hour_value
      %i[min sec sec_fraction].each { |k| h[k] = h.delete(k) if h.key?(k) }
    end
    true
  end

  BC_RE = "c(?:e|\\.e\\.)|b(?:ce|\\.c\\.e\\.)|a(?:d|\\.d\\.)|b(?:c|\\.c\\.)"
  private_constant :BC_RE

  def self.__parse_eu__(str, h)
    m = __subx__(str, /'?\b(\d+)[^-\d\s']*\s*(#{ABBR_MONTHS_RE})[^-\d\s']*(?:\s*(?:\s*,?\s*)(#{BC_RE})?\s*('?-?\d+(?:(?:st|nd|rd|th)\b)?))?/i)
    return false if m.nil?
    bc = m[3] && m[3][0].downcase == "b"
    __s3e__(h, m[4], ABBR_MONTHS[m[2].downcase].to_s, m[1], bc)
    true
  end

  def self.__parse_us__(str, h)
    m = __subx__(str, /\b(#{ABBR_MONTHS_RE})[^-\d\s']*\s*('?\d+)[^-\d\s']*(?:\s*,?\s*(#{BC_RE})?\s*('?-?\d+))?/i)
    return false if m.nil?
    bc = m[3] && m[3][0].downcase == "b"
    __s3e__(h, m[4], ABBR_MONTHS[m[1].downcase].to_s, m[2], bc)
    true
  end

  def self.__parse_iso__(str, h)
    m = __subx__(str, /('?[-+]?\d+)-(\d+)-('?-?\d+)/)
    return false if m.nil?
    __s3e__(h, m[1], m[2], m[3], false)
    true
  end

  def self.__parse_iso2__(str, h)
    if (m = __subx__(str, /\b(\d{2}|\d{4})?-?w(\d{2})(?:-?(\d))?\b/i))
      h[:cwyear] = m[1].to_i if m[1]
      h[:cweek] = m[2].to_i
      h[:cwday] = m[3].to_i if m[3]
      true
    elsif (m = __subx__(str, /-w-(\d)\b/i))
      h[:cwday] = m[1].to_i
      true
    elsif (m = __subx__(str, /--(\d{2})?-(\d{2})\b/))
      h[:mon] = m[1].to_i if m[1]
      h[:mday] = m[2].to_i
      true
    elsif (m = __subx__(str, /--(\d{2})(\d{2})?\b/))
      h[:mon] = m[1].to_i
      h[:mday] = m[2].to_i if m[2]
      true
    elsif !str.match?(/[,.](\d{2}|\d{4})-\d{3}\b/) && (m = __subx__(str, /\b(\d{2}|\d{4})-(\d{3})\b/))
      h[:year] = m[1].to_i
      h[:yday] = m[2].to_i
      true
    elsif !str.match?(/\d-\d{3}\b/) && (m = __subx__(str, /\b-(\d{3})\b/))
      h[:yday] = m[1].to_i
      true
    else
      false
    end
  end

  ERAS = { "m" => 1867, "t" => 1911, "s" => 1925, "h" => 1988, "r" => 2018 }.freeze
  private_constant :ERAS

  def self.__parse_jis__(str, h)
    m = __subx__(str, /\b([mtshr])(\d+)\.(\d+)\.(\d+)/i)
    return false if m.nil?
    h[:year] = m[2].to_i + ERAS[m[1].downcase]
    h[:mon] = m[3].to_i
    h[:mday] = m[4].to_i
    true
  end

  def self.__parse_vms__(str, h)
    if (m = __subx__(str, /('?-?\d+)-(#{ABBR_MONTHS_RE})[^-\/.]*-('?-?\d+)/i))
      __s3e__(h, m[3], ABBR_MONTHS[m[2].downcase].to_s, m[1], false)
      true
    elsif (m = __subx__(str, /\b(#{ABBR_MONTHS_RE})[^-\/.]*-('?-?\d+)(?:-('?-?\d+))?/i))
      __s3e__(h, m[3], ABBR_MONTHS[m[1].downcase].to_s, m[2], false)
      true
    else
      false
    end
  end

  def self.__parse_sla__(str, h)
    m = __subx__(str, %r{('?-?\d+)/\s*('?\d+)(?:\D\s*('?-?\d+))?})
    return false if m.nil?
    __s3e__(h, m[1], m[2], m[3], false)
    true
  end

  def self.__parse_dot__(str, h)
    m = __subx__(str, %r{('?-?\d+)\.\s*('?\d+)\.\s*('?-?\d+)})
    return false if m.nil?
    __s3e__(h, m[1], m[2], m[3], false)
    true
  end

  def self.__parse_year__(str, h)
    m = __subx__(str, /'(\d+)\b/)
    return false if m.nil?
    h[:year] = m[1].to_i
    true
  end

  def self.__parse_mon__(str, h)
    m = __subx__(str, /\b(#{ABBR_MONTHS_RE})\S*/i)
    return false if m.nil?
    h[:mon] = ABBR_MONTHS[m[1].downcase]
    true
  end

  def self.__parse_mday__(str, h)
    m = __subx__(str, /(\d+)(st|nd|rd|th)\b/i)
    return false if m.nil?
    h[:mday] = m[1].to_i
    true
  end

  DDD_RE = /([-+]?)(\d{2,14})
      (?:
        \s*
        t?
        \s*
        (\d{2,6})?(?:[,.](\d*))?
      )?
      (?:
        \s*
        (
          z\b
        |
          [-+]\d{1,4}\b
        |
          \[[-+]?\d[^\]]*\]
        )
      )?/ix
  private_constant :DDD_RE

  def self.__parse_ddd__(str, h)
    m = __subx__(str, DDD_RE)
    return false if m.nil?
    sign, s2, s3, s4, s5 = m[1], m[2], m[3], m[4], m[5]
    time_only = s3.nil? && s4
    case s2.size
    when 2
      if time_only
        h[:sec] = s2[-2, 2].to_i
      else
        h[:mday] = s2[0, 2].to_i
      end
    when 4
      if time_only
        h[:sec] = s2[-2, 2].to_i
        h[:min] = s2[-4, 2].to_i
      else
        h[:mon] = s2[0, 2].to_i
        h[:mday] = s2[2, 2].to_i
      end
    when 6
      if time_only
        h[:sec] = s2[-2, 2].to_i
        h[:min] = s2[-4, 2].to_i
        h[:hour] = s2[-6, 2].to_i
      else
        h[:year] = (sign + s2[0, 2]).to_i
        h[:mon] = s2[2, 2].to_i
        h[:mday] = s2[4, 2].to_i
      end
    when 8, 10, 12, 14
      if time_only
        h[:sec] = s2[-2, 2].to_i
        h[:min] = s2[-4, 2].to_i
        h[:hour] = s2[-6, 2].to_i
        h[:mday] = s2[-8, 2].to_i
        h[:mon] = s2[-10, 2].to_i if s2.size >= 10
        h[:year] = (sign + s2[0, 2]).to_i if s2.size == 12
        if s2.size == 14
          h[:year] = (sign + s2[0, 4]).to_i
          h[:_comp] = false
        end
      else
        h[:year] = (sign + s2[0, 4]).to_i
        h[:mon] = s2[4, 2].to_i
        h[:mday] = s2[6, 2].to_i
        h[:hour] = s2[8, 2].to_i if s2.size >= 10
        h[:min] = s2[10, 2].to_i if s2.size >= 12
        h[:sec] = s2[12, 2].to_i if s2.size >= 14
        h[:_comp] = false
      end
    when 3
      if time_only
        h[:sec] = s2[-2, 2].to_i
        h[:min] = s2[-3, 1].to_i
      else
        h[:yday] = s2[0, 3].to_i
      end
    when 5
      if time_only
        h[:sec] = s2[-2, 2].to_i
        h[:min] = s2[-4, 2].to_i
        h[:hour] = s2[-5, 1].to_i
      else
        h[:year] = (sign + s2[0, 2]).to_i
        h[:yday] = s2[2, 3].to_i
      end
    when 7
      if time_only
        h[:sec] = s2[-2, 2].to_i
        h[:min] = s2[-4, 2].to_i
        h[:hour] = s2[-6, 2].to_i
        h[:mday] = s2[-7, 1].to_i
      else
        h[:year] = (sign + s2[0, 4]).to_i
        h[:yday] = s2[4, 3].to_i
      end
    end
    if s3
      if s4
        if [2, 4, 6].include?(s3.size)
          h[:sec] = s3[-2, 2].to_i
          h[:min] = s3[-4, 2].to_i if s3.size >= 4
          h[:hour] = s3[-6, 2].to_i if s3.size >= 6
        end
      elsif [2, 4, 6].include?(s3.size)
        h[:hour] = s3[0, 2].to_i
        h[:min] = s3[2, 2].to_i if s3.size >= 4
        h[:sec] = s3[4, 2].to_i if s3.size >= 6
      end
    end
    h[:sec_fraction] = Rational(s4.to_i, 10**s4.size) if s4 && !s4.empty?
    if s5
      zone = s5
      if zone.start_with?("[")
        o, n, = zone[1..-2].split(":")
        zone = n || o
        o = "+#{o}" if o.match?(/\A\d/)
        h[:zone] = zone
        h[:offset] = __zone_to_diff__(o)
      else
        h[:zone] = zone
      end
    end
    true
  end

  def self.__parse_bc__(str, h)
    m = __subx__(str, /\b(bc\b|bce\b|b\.c\.|b\.c\.e\.)/i)
    return false if m.nil?
    h[:_bc] = true
    true
  end

  def self.__parse_frag__(str, h)
    m = str.match(/\A\s*(\d{1,2})\s*\z/)
    return false if m.nil?
    n = m[1].to_i
    if h.key?(:hour) && !h.key?(:mday)
      h[:mday] = n if n.between?(1, 31)
    end
    if h.key?(:mday) && !h.key?(:hour)
      h[:hour] = n if n.between?(0, 24)
    end
    true
  end

  # `s3e`: which of three numbers is the year, the month and the day.
  def self.__s3e__(h, y, m, d, bc)
    comp = nil
    if y && m && d.nil?
      y, m, d = d, y, m
    end
    if y.nil?
      if d && d.sub(/\A'/, "").size > 2 && !d.start_with?("'")
        y = d
        d = nil
      elsif d && d.start_with?("'")
        y = d
        d = nil
      end
    end
    if y
      if (n = y.match(/\A'?([-+]?\d+)(.+)?\z/)) && n[2]
        y, d = d, n[1]
      end
    end
    if m && (m.start_with?("'") || m.size > 2)
      y, m, d = m, d, y
    end
    if d && (d.start_with?("'") || d.sub(/\A[-+]/, "").match(/\d+/).to_s.size > 2)
      y, d = d, y
    end
    if y
      n = y.match(/([-+])?(\d+)/)
      comp = false if n[1] || n[2].size > 2
      year = n[0].to_i
      year = -year + 1 if bc
      h[:year] = year
    end
    h[:mon] = m.match(/\d+/)[0].to_i if m
    h[:mday] = d.match(/\d+/)[0].to_i if d
    h[:_comp] = comp unless comp.nil?
  end
end

# --- from fragments: `rt_complete_frags`, `rt__valid_date_frags_p` ---------------

class Date
  FRAG_PATTERNS = [
    [:time, %i[hour min sec]],
    [nil, %i[jd]],
    [:ordinal, %i[year yday hour min sec]],
    [:civil, %i[year mon mday hour min sec]],
    [:commercial, %i[cwyear cweek cwday hour min sec]],
    [:wday, %i[wday hour min sec]],
    [:wnum0, %i[year wnum0 wday hour min sec]],
    [:wnum1, %i[year wnum1 wday hour min sec]],
    [nil, %i[cwyear cweek wday hour min sec]],
    [nil, %i[year wnum0 cwday hour min sec]],
    [nil, %i[year wnum1 cwday hour min sec]],
  ].freeze
  private_constant :FRAG_PATTERNS

  # `seconds` (from `%s` and `%Q`) as the fields it means.
  def self.__rewrite_frags__(h)
    seconds = h.delete(:seconds)
    return h if seconds.nil?
    offset = h[:offset] || 0
    seconds += offset
    days, rest = seconds.divmod(86400)
    whole = rest.floor
    fraction = rest - whole
    h[:jd] = 2440588 + days
    h[:hour] = whole / 3600
    h[:min] = whole % 3600 / 60
    h[:sec] = whole % 60
    h[:sec_fraction] = fraction
    h
  end

  def self.__complete_frags__(h, datetime)
    best = nil
    FRAG_PATTERNS.each_with_index do |(kind, keys), index|
      present = keys.count { |k| h.key?(k) && !h[k].nil? }
      next if present == 0
      # The most fields present, the earliest pattern on a tie.
      best = [kind, keys, present] if best.nil? || present > best[2]
    end
    today = nil
    if best && best[0] && best[1].size != best[2]
      today = Date.today
      kind, keys = best
      case kind
      when :ordinal
        h[:year] ||= today.year
        h[:yday] ||= 1
      when :civil
        keys.each do |k|
          break if h[k]
          h[k] = today.__send__(k)
        end
        h[:mon] ||= 1
        h[:mday] ||= 1
      when :commercial
        keys.each do |k|
          break if h[k]
          h[k] = today.__send__(k)
        end
        h[:cweek] ||= 1
        h[:cwday] ||= 1
      when :wday
        h[:jd] ||= (today - today.wday + h[:wday]).jd
      when :wnum0
        keys.each do |k|
          break if h[k]
          h[k] = k == :wnum0 ? today.strftime("%U").to_i : today.__send__(k)
        end
        h[:wnum0] ||= 0
        h[:wday] ||= 0
      when :wnum1
        keys.each do |k|
          break if h[k]
          h[k] = k == :wnum1 ? today.strftime("%W").to_i : today.__send__(k)
        end
        h[:wnum1] ||= 0
        h[:wday] ||= 1
      end
    end
    if best && best[0] == :time && datetime
      today ||= Date.today
      h[:jd] ||= today.jd
    end
    h[:hour] ||= 0
    h[:min] ||= 0
    h[:sec] ||= 0
    h[:sec] = 59 if h[:sec] == 60
    h
  end

  def self.__weeknum_to_jd__(y, w, d, f, sg)
    rjd2 = __find_fdoy__(y, sg) + 6
    (rjd2 - ((rjd2 - f + 1) % 7) - 7) + 7 * w + d
  end

  def self.__valid_date_frags__(h, sg)
    return h[:jd] if h[:jd]
    if h[:year] && h[:yday]
      jd = __valid_ordinal__(h[:year], h[:yday], sg)
      return jd if jd
    end
    if h[:year] && h[:mon] && h[:mday]
      jd = __valid_civil__(h[:year], h[:mon], h[:mday], sg)
      return jd if jd
    end
    cwday = h[:cwday] || (h[:wday] && (h[:wday] == 0 ? 7 : h[:wday]))
    if h[:cwyear] && h[:cweek] && cwday
      jd = __valid_commercial__(h[:cwyear], h[:cweek], cwday, sg)
      return jd if jd
    end
    wday = h[:wday] || (h[:cwday] && h[:cwday] % 7)
    if h[:year] && h[:wnum0] && wday
      jd = __weeknum_to_jd__(h[:year], h[:wnum0], wday, 0, sg)
      return jd if __weeknum_valid__(jd, h[:year], sg)
    end
    wday = h[:wday] ? (h[:wday] - 1) % 7 : (h[:cwday] && (h[:cwday] - 1) % 7)
    if h[:year] && h[:wnum1] && wday
      jd = __weeknum_to_jd__(h[:year], h[:wnum1], wday, 1, sg)
      return jd if __weeknum_valid__(jd, h[:year], sg)
    end
    nil
  end

  def self.__weeknum_valid__(jd, year, sg)
    __jd_to_civil__(jd, sg)[0] == year
  end

  def self.__new_by_frags__(h, start)
    raise Error, "invalid date" if h.nil? || h.key?(:leftover)
    h = h.dup
    sg = __sg__(start)
    if !h.key?(:jd) && !h.key?(:yday) && h[:year] && h[:mon] && h[:mday]
      jd = __valid_civil__(h[:year], h[:mon], h[:mday], sg)
      raise Error, "invalid date" if jd.nil?
      return __alloc__(jd, 0, 0, 0, sg)
    end
    h = __rewrite_frags__(h)
    h = __complete_frags__(h, false)
    jd = __valid_date_frags__(h, sg)
    raise Error, "invalid date" if jd.nil?
    __alloc__(jd, 0, 0, 0, sg)
  end

  def self.parse(string = "-4712-01-01", comp = true, start = ITALY, limit: 128)
    __new_by_frags__(_parse(string, comp, limit: limit), start)
  end

  # --- the strict formats ----------------------------------------------------------

  def self.__complete_year__(text, h, key)
    year = text.to_i
    year += year >= 69 ? 1900 : 2000 if text.match?(/\A\d{2}\z/)
    h[key] = year
  end

  def self.__frac__(text) = Rational(text.to_i, 10**text.size)

  def self.__time_frags__(h, hour, min, sec, frac, zone)
    h[:hour] = hour.to_i if hour
    h[:min] = min.to_i if min
    h[:sec] = sec.to_i if sec
    h[:sec_fraction] = __frac__(frac) if frac
    if zone
      h[:zone] = zone
      h[:offset] = __zone_to_diff__(zone)
    end
    h
  end

  def self._iso8601(string, limit: 128)
    return {} if string.nil?
    string = __string_arg__(string, limit)
    h = {}
    if (m = string.match(/\A\s*(?:([-+]?\d{2,}|-)-(\d{2})?(?:-(\d{2}))?|([-+]?\d{2,})?-(\d{3})|(\d{4}|\d{2})?-w(\d{2})-(\d)|-w-(\d))(?:t(\d{2}):(\d{2})(?::(\d{2})(?:[,.](\d+))?)?(z|[-+]\d{2}(?::?\d{2})?)?)?\s*\z/i))
      if m[1]
        h[:mday] = m[3].to_i if m[3]
        __complete_year__(m[1], h, :year) unless m[1] == "-"
        h[:mon] = m[2].to_i if m[2]
        h.delete(:year) if m[1] == "-"
      elsif m[5]
        h[:yday] = m[5].to_i
        __complete_year__(m[4], h, :year) if m[4]
      elsif m[8]
        h[:cweek] = m[7].to_i
        h[:cwday] = m[8].to_i
        __complete_year__(m[6], h, :cwyear) if m[6]
      elsif m[9]
        h[:cwday] = m[9].to_i
      end
      __time_frags__(h, m[10], m[11], m[12], m[13], m[14]) if m[10]
    elsif (m = string.match(/\A\s*(?:([-+]?(?:\d{4}|\d{2})|--)(\d{2}|-)(\d{2})|([-+]?(?:\d{4}|\d{2}))(\d{3})|-(\d{3})|(\d{4}|\d{2})w(\d{2})(\d)|-w(\d{2})(\d)|-w-(\d))(?:t?(\d{2})(\d{2})(?:(\d{2})(?:[,.](\d+))?)?(z|[-+]\d{2}(?:\d{2})?)?)?\s*\z/i))
      if m[3]
        h[:mday] = m[3].to_i
        __complete_year__(m[1], h, :year) unless m[1] == "--"
        h[:mon] = m[2].to_i unless m[2] == "-"
      elsif m[5]
        h[:yday] = m[5].to_i
        __complete_year__(m[4], h, :year)
      elsif m[6]
        h[:yday] = m[6].to_i
      elsif m[9]
        h[:cweek] = m[8].to_i
        h[:cwday] = m[9].to_i
        __complete_year__(m[7], h, :cwyear)
      elsif m[11]
        h[:cweek] = m[10].to_i
        h[:cwday] = m[11].to_i
      elsif m[12]
        h[:cwday] = m[12].to_i
      end
      __time_frags__(h, m[13], m[14], m[15], m[16], m[17]) if m[13]
    elsif (m = string.match(/\A\s*(\d{2}):(\d{2})(?::(\d{2})(?:[,.](\d+))?)?(z|[-+]\d{2}(:?\d{2})?)?\s*\z/i))
      __time_frags__(h, m[1], m[2], m[3], m[4], m[5])
    elsif (m = string.match(/\A\s*(\d{2})(\d{2})(?:(\d{2})(?:[,.](\d+))?)?(z|[-+]\d{2}(\d{2})?)?\s*\z/i))
      __time_frags__(h, m[1], m[2], m[3], m[4], m[5])
    end
    h
  end

  def self._rfc3339(string, limit: 128)
    return {} if string.nil?
    string = __string_arg__(string, limit)
    h = {}
    m = string.match(/\A\s*(-?\d{4})-(\d{2})-(\d{2})(?:t|\s)(\d{2}):(\d{2}):(\d{2})(?:[,.](\d+))?(z|[-+]\d{2}:\d{2})\s*\z/i)
    return h if m.nil?
    h[:year] = m[1].to_i
    h[:mon] = m[2].to_i
    h[:mday] = m[3].to_i
    h[:hour] = m[4].to_i
    h[:min] = m[5].to_i
    h[:sec] = m[6].to_i
    h[:zone] = m[8]
    h[:offset] = __zone_to_diff__(m[8])
    h[:sec_fraction] = __frac__(m[7]) if m[7]
    h
  end

  def self._xmlschema(string, limit: 128)
    return {} if string.nil?
    string = __string_arg__(string, limit)
    h = {}
    if (m = string.match(/\A\s*(-?\d{4,})(?:-(\d{2})(?:-(\d{2}))?)?(?:t(\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?)?(z|[-+]\d{2}:\d{2})?\s*\z/i))
      h[:year] = m[1].to_i
      h[:mon] = m[2].to_i if m[2]
      h[:mday] = m[3].to_i if m[3]
      __time_frags__(h, m[4], m[5], m[6], m[7], m[8])
    elsif (m = string.match(/\A\s*(\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?(z|[-+]\d{2}:\d{2})?\s*\z/i))
      __time_frags__(h, m[1], m[2], m[3], m[4], m[5])
    elsif (m = string.match(/\A\s*(?:--(\d{2})(?:-(\d{2}))?|---(\d{2}))(z|[-+]\d{2}:\d{2})?\s*\z/i))
      h[:mon] = m[1].to_i if m[1]
      h[:mday] = m[2].to_i if m[2]
      h[:mday] = m[3].to_i if m[3]
      __time_frags__(h, nil, nil, nil, nil, m[4])
    end
    h
  end

  DAYS_RE = "sunday|monday|tuesday|wednesday|thursday|friday|saturday"
  private_constant :DAYS_RE

  def self._rfc2822(string, limit: 128)
    return {} if string.nil?
    string = __string_arg__(string, limit)
    h = {}
    m = string.match(/\A\s*(?:(#{ABBR_DAYS_RE})\s*,\s+)?(\d{1,2})\s+(#{ABBR_MONTHS_RE})\s+(-?\d{2,})\s+(\d{2}):(\d{2})(?::(\d{2}))?\s*([-+]\d{4}|ut|gmt|e[sd]t|c[sd]t|m[sd]t|p[sd]t|[a-ik-z])\s*\z/i)
    return h if m.nil?
    h[:wday] = ABBR_DAYS[m[1].downcase] if m[1]
    h[:mday] = m[2].to_i
    h[:mon] = ABBR_MONTHS[m[3].downcase]
    year = m[4].to_i
    if m[4].size < 4
      year += year >= 50 ? 1900 : 2000
    end
    h[:year] = year
    h[:hour] = m[5].to_i
    h[:min] = m[6].to_i
    h[:sec] = m[7].to_i if m[7]
    h[:zone] = m[8]
    h[:offset] = __zone_to_diff__(m[8])
    h
  end

  class << self
    alias _rfc822 _rfc2822
  end

  def self._httpdate(string, limit: 128)
    return {} if string.nil?
    string = __string_arg__(string, limit)
    h = {}
    if (m = string.match(/\A\s*(#{ABBR_DAYS_RE})\s*,\s+(\d{2})\s+(#{ABBR_MONTHS_RE})\s+(-?\d{4})\s+(\d{2}):(\d{2}):(\d{2})\s+(gmt)\s*\z/i))
      h[:wday] = ABBR_DAYS[m[1].downcase]
      h[:mday] = m[2].to_i
      h[:mon] = ABBR_MONTHS[m[3].downcase]
      h[:year] = m[4].to_i
      __time_frags__(h, m[5], m[6], m[7], nil, m[8])
    elsif (m = string.match(/\A\s*(#{DAYS_RE})\s*,\s+(\d{2})\s*-\s*(#{ABBR_MONTHS_RE})\s*-\s*(\d{2})\s+(\d{2}):(\d{2}):(\d{2})\s+(gmt)\s*\z/i))
      h[:wday] = ABBR_DAYS[m[1][0, 3].downcase]
      h[:mday] = m[2].to_i
      h[:mon] = ABBR_MONTHS[m[3].downcase]
      year = m[4].to_i
      year += year < 50 ? 2000 : 1900 if year.between?(0, 99)
      h[:year] = year
      __time_frags__(h, m[5], m[6], m[7], nil, m[8])
    elsif (m = string.match(/\A\s*(#{ABBR_DAYS_RE})\s+(#{ABBR_MONTHS_RE})\s+(\d{1,2})\s+(\d{2}):(\d{2}):(\d{2})\s+(\d{4})\s*\z/i))
      h[:wday] = ABBR_DAYS[m[1].downcase]
      h[:mon] = ABBR_MONTHS[m[2].downcase]
      h[:mday] = m[3].to_i
      __time_frags__(h, m[4], m[5], m[6], nil, nil)
      h[:year] = m[7].to_i
    end
    h
  end

  def self._jisx0301(string, limit: 128)
    return {} if string.nil?
    string = __string_arg__(string, limit)
    m = string.match(/\A\s*([mtshr])?(\d{2})\.(\d{2})\.(\d{2})(?:t(?:(\d{2}):(\d{2})(?::(\d{2})(?:[,.](\d*))?)?(z|[-+]\d{2}(?::?\d{2})?)?)?)?\s*\z/i)
    return _iso8601(string, limit: limit) if m.nil?
    h = {}
    era = ERAS[(m[1] || "h").downcase]
    h[:year] = m[2].to_i + era
    h[:mon] = m[3].to_i
    h[:mday] = m[4].to_i
    if m[5]
      h[:hour] = m[5].to_i
      h[:min] = m[6].to_i if m[6]
      h[:sec] = m[7].to_i if m[7]
    end
    h[:sec_fraction] = __frac__(m[8]) if m[8] && !m[8].empty?
    if m[9]
      h[:zone] = m[9]
      h[:offset] = __zone_to_diff__(m[9])
    end
    h
  end

  %i[iso8601 rfc3339 xmlschema rfc2822 httpdate jisx0301].each do |name|
    default = { iso8601: "-4712-01-01", rfc3339: "-4712-01-01T00:00:00+00:00", xmlschema: "-4712-01-01",
                rfc2822: "Mon, 1 Jan -4712 00:00:00 +0000", httpdate: "Mon, 01 Jan -4712 00:00:00 GMT",
                jisx0301: "-4712-01-01" }[name]
    define_singleton_method(name) do |string = default, start = ITALY, limit: 128|
      __new_by_frags__(__send__(:"_#{name}", string, limit: limit), start)
    end
  end

  class << self
    alias rfc822 rfc2822
  end

  # --- formatting them ---------------------------------------------------------------

  def iso8601 = strftime("%Y-%m-%d")
  alias xmlschema iso8601
  def rfc3339 = strftime("%Y-%m-%dT%H:%M:%S%:z")
  def rfc2822 = strftime("%a, %-d %b %Y %T %z")
  alias rfc822 rfc2822
  def httpdate = strftime("%a, %d %b %Y %T GMT")
  def asctime = strftime("%a %b %e %H:%M:%S %Y")
  alias ctime asctime

  def __jisx0301_date__
    j = jd
    if j < 2405160
      strftime("%Y-%m-%d")
    else
      era, base = if j < 2419614 then ["M", 1867]
                  elsif j < 2424875 then ["T", 1911]
                  elsif j < 2447535 then ["S", 1925]
                  elsif j < 2458605 then ["H", 1988]
                  else ["R", 2018]
                  end
      format("%s%02d", era, year - base) + strftime(".%m.%d")
    end
  end

  def jisx0301 = __jisx0301_date__.encode(Encoding::US_ASCII)
end

class DateTime < Date
  def self.__new_by_frags__(h, start)
    raise Date::Error, "invalid date" if h.nil? || h.key?(:leftover)
    h = h.dup
    sg = Date.__sg__(start)
    h = Date.__rewrite_frags__(h)
    h = Date.__complete_frags__(h, true)
    jd = Date.__valid_date_frags__(h, sg)
    raise Date::Error, "invalid date" if jd.nil?
    hour = h[:hour]
    min = h[:min]
    sec = h[:sec]
    hour += 24 if hour < 0
    min += 60 if min < 0
    sec += 60 if sec < 0
    unless hour.between?(0, 24) && min.between?(0, 59) && sec.between?(0, 59) && !(hour == 24 && (min != 0 || sec != 0))
      raise Date::Error, "invalid date"
    end
    fraction = h[:sec_fraction] || 0
    offset = h[:offset] || 0
    unless offset.abs <= 86400
      warn "invalid offset is ignored"
      offset = 0
    end
    __at__(jd, 0, hour, min, sec + fraction, Rational(offset, 86400), sg)
  end

  def self.parse(string = "-4712-01-01T00:00:00+00:00", comp = true, start = Date::ITALY, limit: 128)
    __new_by_frags__(Date._parse(string, comp, limit: limit), start)
  end

  %i[iso8601 rfc3339 xmlschema rfc2822 httpdate jisx0301].each do |name|
    default = { iso8601: "-4712-01-01T00:00:00+00:00", rfc3339: "-4712-01-01T00:00:00+00:00",
                xmlschema: "-4712-01-01T00:00:00+00:00", rfc2822: "Mon, 1 Jan -4712 00:00:00 +0000",
                httpdate: "Mon, 01 Jan -4712 00:00:00 GMT", jisx0301: "-4712-01-01T00:00:00+00:00" }[name]
    define_singleton_method(name) do |string = default, start = Date::ITALY, limit: 128|
      __new_by_frags__(Date.__send__(:"_#{name}", string, limit: limit), start)
    end
  end

  class << self
    alias rfc822 rfc2822
  end

  def __fraction__(n)
    n = Integer.__index__(n)
    n > 0 ? ".%#{n}N" : ""
  end

  def iso8601(n = 0) = strftime("%Y-%m-%dT%H:%M:%S#{__fraction__(n)}%:z")
  def httpdate = new_offset(0).strftime("%a, %d %b %Y %T GMT")
  alias xmlschema iso8601
  def rfc3339(n = 0) = iso8601(n)
  def jisx0301(n = 0) = (__jisx0301_date__ + strftime("T%H:%M:%S#{__fraction__(n)}%:z")).encode(Encoding::US_ASCII)
end

# --- strptime: `date_strptime.c` -----------------------------------------------------

class Date
  STRPTIME_COMPOSITES = {
    "c" => "%a %b %e %H:%M:%S %Y", "D" => "%m/%d/%y", "x" => "%m/%d/%y", "F" => "%Y-%m-%d",
    "R" => "%H:%M", "r" => "%I:%M:%S %p", "T" => "%H:%M:%S", "X" => "%H:%M:%S", "v" => "%e-%b-%Y",
    "+" => "%a %b %e %H:%M:%S %Z %Y",
  }.freeze
  NUM_PATTERN = "CDdeFGgHIjkLlMmNQRrSsTUuVvWwXxYy"
  private_constant :STRPTIME_COMPOSITES, :NUM_PATTERN

  STRPTIME_ZONE_RE = /\A((?:gmt|utc?)?[-+]\d+(?:[,.:]\d+(?::\d+)?)?|(?-i:[[:alpha:].\s]+)(?:standard|daylight)\s+time\b|(?-i:[[:alpha:]]+)(?:\sdst)?\b)/i
  private_constant :STRPTIME_ZONE_RE

  def self._strptime(string, format = "%F")
    string = IO.__str__(string)
    format = IO.__str__(format)
    h = {}
    at = __strptime__(string, 0, format, h)
    return nil if at.nil?
    h[:leftover] = string.byteslice(at, string.bytesize - at) if at < string.bytesize
    if h.key?(:_cent)
      cent = h.delete(:_cent)
      h[:cwyear] = cent * 100 + h[:cwyear] if h.key?(:cwyear)
      h[:year] = cent * 100 + h[:year] if h.key?(:year)
    end
    if h.key?(:_merid)
      merid = h.delete(:_merid)
      h[:hour] = h[:hour] % 12 + merid if h.key?(:hour)
    end
    h
  end

  # Whether what follows in the format reads digits, which bounds the
  # width of the number before it.
  def self.__num_pattern__(format, at)
    c = format.byteslice(at, 1)
    return false if c.nil? || c.empty?
    return true if c.match?(/\d/)
    return false unless c == "%"
    c = format.byteslice(at + 1, 1)
    c = format.byteslice(at + 2, 1) if c == "E" || c == "O"
    !c.nil? && !c.empty? && NUM_PATTERN.include?(c)
  end

  # Digits at `at`, at most `width` of them: the value, the count, and where
  # they end; nil for none.
  def self.__read_digits__(string, at, width, signed = false)
    rest = string.byteslice(at, string.bytesize - at)
    pattern = signed ? /\A([-+]?)(\d+)/ : /\A()(\d+)/
    m = rest.match(pattern)
    return nil if m.nil?
    digits = m[2]
    digits = digits[0, width] if width
    value = digits.to_i
    value = -value if m[1] == "-"
    [value, digits.size, at + m[1].size + digits.size]
  end

  def self.__strptime__(string, at, format, h)
    fi = 0
    n = format.bytesize
    while fi < n
      c = format.byteslice(fi, 1)
      if c.match?(/\s/)
        fi += 1
        at += 1 while at < string.bytesize && string.byteslice(at, 1).match?(/\s/)
        next
      end
      unless c == "%" && fi + 1 < n
        return nil unless string.byteslice(at, 1) == c
        at += 1
        fi += 1
        next
      end
      fi += 1
      c = format.byteslice(fi, 1)
      if c == "E" && "cCxXyY".include?(format.byteslice(fi + 1, 1).to_s) && !format.byteslice(fi + 1, 1).to_s.empty?
        fi += 1
        c = format.byteslice(fi, 1)
      elsif c == "O" && "deHImMSuUVwWy".include?(format.byteslice(fi + 1, 1).to_s) && !format.byteslice(fi + 1, 1).to_s.empty?
        fi += 1
        c = format.byteslice(fi, 1)
      end
      # `%:z` and `%::z` are `%z`.
      while c == ":" && fi + 1 < n
        fi += 1
        c = format.byteslice(fi, 1)
      end
      fi += 1
      bounded = __num_pattern__(format, fi)
      if (composite = STRPTIME_COMPOSITES[c])
        at = __strptime__(string, at, composite, h)
        return nil if at.nil?
        next
      end
      # A proc, so that a bad number ends the whole match.
      read = proc do |width, lo, hi, signed = false|
        got = __read_digits__(string, at, width, signed)
        return nil if got.nil?
        value, _count, ending = got
        return nil if (lo && value < lo) || (hi && value > hi)
        at = ending
        value
      end
      case c
      when "A", "a"
        rest = string.byteslice(at, string.bytesize - at)
        found = nil
        DAYNAMES.each_with_index do |name, i|
          if rest[0, name.size].casecmp?(name)
            found = [i, name.size]
            break
          end
        end
        if found.nil?
          ABBR_DAYNAMES.each_with_index do |name, i|
            if rest[0, 3].to_s.casecmp?(name)
              found = [i, 3]
              break
            end
          end
        end
        return nil if found.nil?
        h[:wday] = found[0]
        at += found[1]
      when "B", "b", "h"
        rest = string.byteslice(at, string.bytesize - at)
        found = nil
        MONTHNAMES.each_with_index do |name, i|
          next if name.nil?
          if rest[0, name.size].casecmp?(name)
            found = [i, name.size]
            break
          end
        end
        if found.nil?
          ABBR_MONTHNAMES.each_with_index do |name, i|
            next if name.nil?
            if rest[0, 3].to_s.casecmp?(name)
              found = [i, 3]
              break
            end
          end
        end
        return nil if found.nil?
        h[:mon] = found[0]
        at += found[1]
      when "C"
        value = read.(bounded ? 2 : nil, nil, nil, true)
        h[:_cent] = value
      when "d", "e"
        at += 1 if string.byteslice(at, 1) == " "
        h[:mday] = read.(2, 1, 31)
      when "G"
        h[:cwyear] = read.(bounded ? 4 : nil, nil, nil, true)
      when "g"
        value = read.(2, 0, 99)
        h[:cwyear] = value
        h[:_cent] ||= value >= 69 ? 19 : 20
      when "H", "k"
        at += 1 if string.byteslice(at, 1) == " "
        h[:hour] = read.(2, 0, 24)
      when "I", "l"
        at += 1 if string.byteslice(at, 1) == " "
        h[:hour] = read.(2, 1, 12)
      when "j"
        h[:yday] = read.(3, 1, 366)
      when "L", "N"
        sign = 1
        if string.byteslice(at, 1) == "-" || string.byteslice(at, 1) == "+"
          sign = -1 if string.byteslice(at, 1) == "-"
          at += 1
        end
        got = __read_digits__(string, at, bounded ? (c == "L" ? 3 : 9) : nil)
        return nil if got.nil?
        value, count, at = got
        h[:sec_fraction] = Rational(sign * value, 10**count)
      when "M"
        h[:min] = read.(2, 0, 59)
      when "m"
        h[:mon] = read.(2, 1, 12)
      when "n", "t"
        at = __strptime__(string, at, " ", h)
      when "P", "p"
        rest = string.byteslice(at, string.bytesize - at)
        m = rest.match(/\A([ap])(?:m\b|\.m\.)/i)
        return nil if m.nil?
        h[:_merid] = m[1].downcase == "p" ? 12 : 0
        at += m[0].bytesize
      when "Q"
        value = read.(nil, nil, nil, true)
        h[:seconds] = Rational(value, 1000)
      when "S"
        h[:sec] = read.(2, 0, 60)
      when "s"
        h[:seconds] = read.(nil, nil, nil, true)
      when "U"
        h[:wnum0] = read.(2, 0, 53)
      when "W"
        h[:wnum1] = read.(2, 0, 53)
      when "u"
        h[:cwday] = read.(1, 1, 7)
      when "V"
        h[:cweek] = read.(2, 1, 53)
      when "w"
        h[:wday] = read.(1, 0, 6)
      when "Y"
        h[:year] = read.(bounded ? 4 : nil, nil, nil, true)
      when "y"
        value = read.(2, 0, 99)
        h[:year] = value
        h[:_cent] ||= value >= 69 ? 19 : 20
      when "Z", "z"
        rest = string.byteslice(at, string.bytesize - at)
        m = rest.match(STRPTIME_ZONE_RE)
        return nil if m.nil?
        h[:zone] = m[1]
        h[:offset] = __zone_to_diff__(m[1])
        at += m[1].bytesize
      when "%"
        return nil unless string.byteslice(at, 1) == "%"
        at += 1
      else
        return nil unless string.byteslice(at, 1) == "%"
        at += 1
        return nil unless string.byteslice(at, 1) == c
        at += 1
      end
    end
    at
  end

  def self.strptime(string = "-4712-01-01", format = "%F", start = ITALY)
    h = _strptime(string, format)
    raise Error, "invalid date" if h.nil?
    h.delete(:leftover)
    __new_by_frags__(h, start)
  end
end

class DateTime < Date
  def self.strptime(string = "-4712-01-01T00:00:00+00:00", format = "%FT%T%z", start = Date::ITALY)
    h = Date._strptime(string, format)
    raise Date::Error, "invalid date" if h.nil?
    h.delete(:leftover)
    __new_by_frags__(h, start)
  end
end
