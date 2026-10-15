# `socket` (#61), as CRuby 4.0.6 ships it. CRuby's is `ext/socket`: a C
# half (`socket.c`, `basicsocket.c`, `ipsocket.c`, `raddrinfo.c`, `option.c`,
# `ancdata.c`, `ifaddr.c` and the rest) and a Ruby half, `lib/socket.rb`.
# Here both are Ruby, over the system calls in `spinel-vm/src/socket.rs`;
# the methods of upstream's Ruby half are its own code, kept close to it, except
# where it starts a thread, which waits on #45.
#
# An address is the bytes of the platform's `struct sockaddr_*`, as in CRuby:
# `Addrinfo#to_sockaddr`, `Socket.sockaddr_in` and what `accept` answers.

class SocketError < StandardError
end

class BasicSocket < IO
end

class Socket < BasicSocket
  class ResolutionError < SocketError
    def initialize(message = nil, error_code = nil)
      super(message)
      @error_code = error_code
    end

    attr_reader :error_code
  end

  module Constants
  end
  __sock_constants__.each do |name, value|
    Constants.const_set(name, value)
    const_set(name, value)
  end

  # --- names for numbers, both ways -------------------------------------------

  # Where two names share a number, the one CRuby prints.
  PREFERRED = { "AF" => { Constants::AF_UNIX => "UNIX" }, "PF" => { Constants::PF_UNIX => "UNIX" } }
  if Constants.const_defined?(:AF_ROUTE)
    PREFERRED["AF"][Constants::AF_ROUTE] = "ROUTE"
    PREFERRED["PF"][Constants::PF_ROUTE] = "ROUTE"
  end
  private_constant :PREFERRED

  def self.__names__(prefix)
    @__names__ ||= {}
    @__names__[prefix] ||= begin
      table = {}
      Constants.constants.sort.each do |name|
        name = name.to_s
        next unless name.start_with?("#{prefix}_")
        value = Constants.const_get(name)
        table[value] ||= name[(prefix.length + 1)..]
      end
      (PREFERRED[prefix] || {}).each { |value, name| table[value] = name }
      table
    end
  end

  def self.__name_of__(prefix, value) = __names__(prefix)[value]

  # A constant named by a Symbol or String, with or without its prefix, or an
  # Integer as it is: CRuby's `rsock_family_arg` and its kin.
  def self.__name_arg__(value)
    return value.to_s if Symbol === value
    return value if String === value
    return nil unless value.respond_to?(:to_str)
    name = value.to_str
    unless String === name
      raise TypeError, "can't convert #{value.class} to String (#{value.class}#to_str gives #{name.class})"
    end
    name
  end

  def self.__constant_arg__(value, prefixes, what)
    name = __name_arg__(value)
    unless name.nil?
      prefixes.each do |prefix|
        full = name.start_with?("#{prefix}_") ? name : "#{prefix}_#{name}"
        return Constants.const_get(full) if Constants.const_defined?(full, false)
      end
      raise SocketError, "unknown #{what}: #{name}"
    end
    Integer.__index__(value)
  end

  def self.__family__(value)
    return AF_UNSPEC if value.nil?
    __constant_arg__(value, %w[AF PF], "socket domain")
  end

  def self.__socktype__(value)
    return 0 if value.nil?
    __constant_arg__(value, %w[SOCK], "socket type")
  end

  def self.__inet__(family) = family == AF_INET || family == AF_INET6

  def self.__level__(family, value)
    name = __name_arg__(value)
    unless name.nil?
      return SOL_SOCKET if name == "SOCKET" || name == "SOL_SOCKET"
      if __inet__(family) || family == AF_UNSPEC
        full = name.start_with?("IPPROTO_") ? name : "IPPROTO_#{name}"
        return Constants.const_get(full) if Constants.const_defined?(full, false)
      end
      raise SocketError, "unknown protocol level: #{name}"
    end
    Integer.__index__(value)
  end

  def self.__level_prefix__(level)
    case level
    when SOL_SOCKET then "SO"
    when IPPROTO_IP then "IP"
    when IPPROTO_IPV6 then "IPV6"
    when IPPROTO_TCP then "TCP"
    when IPPROTO_UDP then "UDP"
    end
  end

  def self.__optname__(family, level, value)
    name = __name_arg__(value)
    return Integer.__index__(value) if name.nil?
    prefix = __level_prefix__(level)
    if prefix
      full = name.start_with?("#{prefix}_") ? name : "#{prefix}_#{name}"
      return Constants.const_get(full) if Constants.const_defined?(full, false)
    end
    what = { "SO" => "socket level option name", "IP" => "IP level option name",
             "IPV6" => "IPv6 level option name", "TCP" => "TCP level option name",
             "UDP" => "UDP level option name" }[prefix] || "option name"
    raise SocketError, "unknown #{what}: #{name}"
  end

  # A control message type, named for its level's prefix; a level without
  # named types takes only an Integer, as CRuby's `rsock_cmsg_type_arg`.
  def self.__cmsg_type__(family, level, value)
    prefix = level == SOL_SOCKET ? "SCM" : (__inet__(family) && __level_prefix__(level))
    prefix = nil unless %w[SCM IP IPV6 TCP UDP].include?(prefix)
    name = prefix && __name_arg__(value)
    return Integer.__index__(value) if name.nil?
    full = name.start_with?("#{prefix}_") ? name : "#{prefix}_#{name}"
    return Constants.const_get(full) if Constants.const_defined?(full, false)
    what = { "SCM" => "UNIX", "IP" => "IP", "IPV6" => "IPv6", "TCP" => "TCP", "UDP" => "UDP" }[prefix]
    raise SocketError, "unknown #{what} control message: #{name}"
  end

  def self.__family_name__(family)
    name = __name_of__("AF", family)
    name ? "AF_#{name}" : "unknown:#{family}"
  end

  # --- addresses ---------------------------------------------------------------

  # The bytes of an address given as an `Addrinfo` or a String.
  def self.__sockaddr__(address)
    return address.to_sockaddr if Addrinfo === address
    address = String === address ? address : IO.__str__(address)
    address.b
  end

  # A host as `getaddrinfo` takes it: nil for none, the wildcard and
  # broadcast spellings, or an Integer as an IPv4 address.
  def self.__host__(host)
    return nil if host.nil?
    if Integer === host
      return [host].pack("N").unpack("C4").join(".")
    end
    host = String === host ? host : IO.__str__(host)
    return "0.0.0.0" if host.empty? || host == "<any>"
    return "255.255.255.255" if host == "<broadcast>"
    host
  end

  def self.__service__(port)
    return nil if port.nil?
    return port.to_s if Integer === port
    String === port ? port : IO.__str__(port)
  end

  def self.__getaddrinfo__(host, port, family, socktype, protocol, flags)
    host = __host__(host)
    service = __service__(port)
    family = __family__(family)
    socktype = __socktype__(socktype)
    protocol = protocol.nil? ? 0 : Integer.__index__(protocol)
    flags = flags.nil? ? 0 : Integer.__index__(flags)
    numeric = __numeric_getaddrinfo__(host, service, family, socktype, protocol)
    return numeric unless numeric.nil?
    list = __sock_getaddrinfo__(host, service, family, socktype, protocol, flags)
    if Integer === list[0]
      raise ResolutionError.new("getaddrinfo: #{list[1]}", list[0])
    end
    list
  end

  # CRuby's `numeric_getaddrinfo`: a numeric address and port need no
  # resolver, and answer a stream, a datagram and a raw socket, as the hints
  # allow. Anything else is `getaddrinfo(3)`'s.
  NUMERIC_KINDS = [[Constants::SOCK_STREAM, Constants::IPPROTO_TCP],
                   [Constants::SOCK_DGRAM, Constants::IPPROTO_UDP],
                   [Constants::SOCK_RAW, 0]].freeze
  private_constant :NUMERIC_KINDS

  def self.__numeric_getaddrinfo__(host, service, family, socktype, protocol)
    return nil if host.nil? || service.nil? || !service.match?(/\A[0-9]+\z/)
    port = service.to_i
    return nil if port > 65535
    if (family == AF_UNSPEC || family == AF_INET6) && host.match?(/\A[0-9a-fA-F.:]+\z/) &&
       (address = __inet_pton__(AF_INET6, host))
      family = AF_INET6
    elsif (family == AF_UNSPEC || family == AF_INET) && host.match?(/\A[0-9.]+\z/) &&
          (address = __inet_pton__(AF_INET, host))
      family = AF_INET
    else
      return nil
    end
    list = NUMERIC_KINDS.filter_map do |kind, default|
      next unless socktype == 0 || socktype == kind
      next unless protocol == 0 || default == 0 || protocol == default
      [family, kind, protocol == 0 ? default : protocol, __sockaddr_in__(address, port, 0, 0), nil]
    end
    list.empty? ? nil : list
  end

  def self.__getnameinfo__(sockaddr, flags)
    result = __sock_getnameinfo__(sockaddr, flags)
    if Integer === result[0]
      raise ResolutionError.new("getnameinfo: #{result[1]}", result[0])
    end
    result.map { |s| s.force_encoding(Encoding::ASCII_8BIT).encode(Encoding::US_ASCII) rescue s }
  end

  # A sockaddr's host and port as text: numeric unless `reverse`.
  def self.__numeric__(sockaddr)
    parsed = __sockaddr_parse__(sockaddr)
    raise ArgumentError, "not an AF_INET/AF_INET6 sockaddr" if parsed.nil? || parsed.size < 3
    [parsed[1], __ip_text__(parsed)]
  end

  def self.__ip_text__(parsed)
    text = __inet_ntop__(parsed[2])
    if parsed[0] == AF_INET6 && parsed[4] != 0
      name = __sock_ifname__(parsed[4])
      text = "#{text}%#{name || parsed[4]}"
    end
    text
  end

  def self.getaddrinfo(host, port, family = nil, socktype = nil, protocol = nil, flags = nil, reverse_lookup = nil, timeout: nil)
    reverse = BasicSocket.__reverse_lookup__(reverse_lookup)
    __getaddrinfo__(host, port, family, socktype, protocol, flags).map do |afamily, type, proto, sockaddr, _|
      port_number, address = __numeric__(sockaddr)
      name = reverse ? __getnameinfo__(sockaddr, 0)[0] : address
      [__family_name__(afamily), port_number, name, address, afamily, type, proto]
    end
  end

  def self.getnameinfo(sockaddr, flags = 0)
    flags = Integer.__index__(flags)
    if Array === sockaddr
      family, port, host, numeric = sockaddr
      host = numeric if sockaddr.size >= 4 && !numeric.nil?
      raise ArgumentError, "array size should be 3 or 4, #{sockaddr.size} given" unless [3, 4].include?(sockaddr.size)
      list = __getaddrinfo__(host, port, family.nil? ? nil : family, :DGRAM, nil, AI_NUMERICHOST & 0)
      sockaddr = list[0][3]
    else
      sockaddr = __sockaddr__(sockaddr)
    end
    __getnameinfo__(sockaddr, flags)
  end

  def self.gethostname
    name = __sock_hostname__
    IO.__check__(name) if Integer === name
    name.force_encoding(Encoding.default_external)
  end

  def self.getservbyname(name, proto = "tcp")
    name = IO.__str__(name)
    proto = IO.__str__(proto)
    port = __sock_servport__(name, proto)
    return port unless port.nil?
    return Integer(name, 10) if name.match?(/\A\d+\z/)
    raise SocketError, "no such service #{name}/#{proto}"
  end

  def self.getservbyport(port, proto = "tcp")
    port = Integer.__index__(port)
    proto = IO.__str__(proto)
    raise RangeError, "integer #{port} too big to convert into 'unsigned short'" if port > 0xffff
    raise RangeError, "integer #{port} too small to convert into 'unsigned short'" if port < -0x8000
    name = __sock_servname__(port & 0xffff, proto)
    raise SocketError, "no such service for port #{port}/#{proto}" if name.nil?
    name.force_encoding(Encoding::US_ASCII)
  end

  def self.sockaddr_in(port, host)
    __getaddrinfo__(host, port, nil, 0, nil, 0)[0][3]
  end

  class << self
    alias pack_sockaddr_in sockaddr_in
  end

  def self.unpack_sockaddr_in(sockaddr)
    sockaddr = __sockaddr__(sockaddr)
    parsed = __sockaddr_parse__(sockaddr)
    if parsed.nil? || ![AF_INET, AF_INET6].include?(parsed[0])
      raise ArgumentError, "not an AF_INET/AF_INET6 sockaddr"
    end
    [parsed[1], __ip_text__(parsed)]
  end

  def self.sockaddr_un(path)
    path = String === path ? path : IO.__str__(path)
    sockaddr = __sockaddr_un__(path.b)
    raise ArgumentError, "too long unix socket path (#{path.bytesize} bytes given but #{__sockaddr_un_max__} bytes max)" if sockaddr.nil?
    sockaddr
  end

  def self.__sockaddr_un_max__
    @__sockaddr_un_max__ ||= begin
      n = 1
      n += 1 until __sockaddr_un__("a" * (n + 1)).nil?
      n
    end
  end

  class << self
    alias pack_sockaddr_un sockaddr_un
  end

  def self.unpack_sockaddr_un(sockaddr)
    sockaddr = __sockaddr__(sockaddr)
    parsed = __sockaddr_parse__(sockaddr)
    if parsed.nil? || parsed[0] != AF_UNIX
      raise ArgumentError, "not an AF_UNIX sockaddr"
    end
    parsed[1]
  end

  def self.pair(domain, type, protocol = 0)
    domain = __family__(domain)
    type = __socktype__(type)
    fds = IO.__check__(__sock_pair__(domain, type, protocol.nil? ? 0 : Integer.__index__(protocol)), nil, "socketpair(2)")
    pair = fds.map { |fd| for_fd(fd) }
    return pair unless block_given?
    begin
      yield(*pair)
    ensure
      pair.each { |s| s.close unless s.closed? }
    end
  end

  class << self
    alias socketpair pair
  end

  def self.ip_address_list
    getifaddrs.map(&:addr).compact.select(&:ip?)
  end

  def self.getifaddrs
    list = __sock_ifaddrs__
    IO.__check__(list, nil, "getifaddrs") if Integer === list
    list.map { |fields| Ifaddr.__new__(*fields) }
  end

  # `Socket.gethostbyname`, deprecated in CRuby for `Addrinfo`.
  def self.gethostbyname(name)
    warn "Socket.gethostbyname is deprecated; use Addrinfo.getaddrinfo instead.", uplevel: 1, category: :deprecated
    list = __getaddrinfo__(name, nil, nil, :STREAM, nil, AI_CANONNAME)
    canonical = list[0][4] || name.to_s
    family = list[0][0]
    addresses = list.select { |l| l[0] == family }.map { |l| __sockaddr_parse__(l[3])[2] }.uniq
    [canonical.dup.force_encoding(Encoding::US_ASCII), [], family, *addresses]
  end

  def self.gethostbyaddr(address, family = nil)
    warn "Socket.gethostbyaddr is deprecated; use Addrinfo#getnameinfo instead.", uplevel: 1, category: :deprecated
    address = IO.__str__(address).b
    family = family.nil? ? (address.bytesize == 16 ? AF_INET6 : AF_INET) : __family__(family)
    sockaddr = __sockaddr_in__(address, 0, 0, 0)
    # An address of the other family's length names no host.
    wanted = { AF_INET => 4, AF_INET6 => 16 }[family]
    raise SocketError, "host not found" if sockaddr.nil? || wanted != address.bytesize
    host = __getnameinfo__(sockaddr, NI_NAMEREQD)[0]
    [host, [], family, address]
  end

  # --- a socket of any kind ------------------------------------------------------

  def initialize(domain, socktype, protocol = nil)
    domain = Socket.__family__(domain)
    socktype = Socket.__socktype__(socktype)
    protocol = protocol.nil? ? 0 : Integer.__index__(protocol)
    fd = IO.__check__(__sock_socket__(domain, socktype, protocol), nil, "socket(2)")[0]
    __setup_socket__(fd)
  end

  def bind(address)
    address = Socket.__sockaddr__(address)
    result = __sock_bind__(fileno, address)
    Socket.__fail__(result, "bind(2)", address) if Integer === result
    0
  end

  def connect(address)
    address = Socket.__sockaddr__(address)
    __connect__(address, nil, nil)
    0
  end

  def listen(backlog)
    IO.__check__(__sock_listen__(fileno, Integer.__index__(backlog)), nil, "listen(2)")
    0
  end

  def accept
    fd, address = __accept__(false)
    [Socket.for_fd(fd), Addrinfo.__from__(address, local_address)]
  end

  def __accept_nonblock__(exception)
    result = __accept__(true, exception)
    return result if Symbol === result
    fd, address = result
    [Socket.for_fd(fd), Addrinfo.__from__(address, local_address)]
  end

  def sysaccept
    fd, address = __accept__(false)
    [fd, Addrinfo.__from__(address, local_address)]
  end

  def recvfrom(length, flags = 0)
    data, from = __recv__(length, flags, nil)
    return nil if data.nil?
    [data, Addrinfo.__from__(from, local_address)]
  end

  def __recvfrom_nonblock__(length, flags, buffer, exception)
    result = __recv_nonblock__(length, flags, buffer, exception)
    return result if Symbol === result || result.nil?
    data, from = result
    [data, Addrinfo.__from__(from, local_address)]
  end

  def self.__fail__(errno, call, sockaddr)
    described = sockaddr && __describe__(sockaddr)
    raise SystemCallError.new(described ? "#{call} for #{described}" : call, errno)
  end

  # A sockaddr for an error message: `host:port` or the path.
  def self.__describe__(sockaddr)
    parsed = __sockaddr_parse__(sockaddr)
    return nil if parsed.nil?
    case parsed[0]
    when AF_INET then "#{__ip_text__(parsed)}:#{parsed[1]}"
    when AF_INET6 then "[#{__ip_text__(parsed)}]:#{parsed[1]}"
    when AF_UNIX then parsed[1]
    end
  end
end

class BasicSocket < IO
  @do_not_reverse_lookup = true

  def self.do_not_reverse_lookup = BasicSocket.instance_variable_get(:@do_not_reverse_lookup)

  def self.do_not_reverse_lookup=(value)
    BasicSocket.instance_variable_set(:@do_not_reverse_lookup, value ? true : false)
  end

  # Whether to look a name up: the argument, which may be a Symbol, or the
  # default.
  def self.__reverse_lookup__(value, default = !do_not_reverse_lookup)
    case value
    when nil then default
    when true, :hostname then true
    when false, :numeric then false
    when Symbol then raise ArgumentError, "invalid reverse_lookup flag: #{value.inspect}"
    else raise TypeError, "wrong argument type #{value.class} (expected Symbol)"
    end
  end

  def self.for_fd(fd)
    socket = allocate
    socket.__setup_socket__(Integer.__index__(fd))
    socket
  end

  def __setup_socket__(fd)
    __setup__(fd, "r+", { binmode: true })
    @sync = true
    # Nonblocking, as every socket CRuby 3 makes; the blocking calls wait.
    __nonblock__
    @do_not_reverse_lookup = BasicSocket.do_not_reverse_lookup
    self
  end

  def do_not_reverse_lookup = @do_not_reverse_lookup

  # Wait for a nonblocking socket to be ready, for as long as its timeout.
  def __wait_for__(events)
    IO.__wait__([[self, events]], @timeout) || raise(IO::TimeoutError, "Blocking operation timed out!")
  end

  def do_not_reverse_lookup=(value)
    @do_not_reverse_lookup = value ? true : false
  end

  def shutdown(how = Socket::SHUT_RDWR)
    name = Integer === how ? nil : Socket.__name_arg__(how)
    how = case
          when Integer === how then how
          when name
            name = name.delete_prefix("SHUT_")
            { "RD" => Socket::SHUT_RD, "WR" => Socket::SHUT_WR, "RDWR" => Socket::SHUT_RDWR }[name] ||
              raise(SocketError, "unknown shutdown argument: #{how}")
          else Integer.__index__(how)
          end
    unless [Socket::SHUT_RD, Socket::SHUT_WR, Socket::SHUT_RDWR].include?(how)
      raise ArgumentError, "`how' should be either :SHUT_RD, :SHUT_WR, :SHUT_RDWR"
    end
    IO.__check__(__sock_shutdown__(fileno, how), nil, "shutdown(2)")
    0
  end

  def close_read
    __check_open__
    if @wclosed_socket
      close
    else
      __sock_shutdown__(@fileno, Socket::SHUT_RD)
      @rclosed_socket = true
    end
    nil
  end

  def close_write
    __check_open__
    if @rclosed_socket
      close
    else
      __sock_shutdown__(@fileno, Socket::SHUT_WR)
      @wclosed_socket = true
    end
    nil
  end

  def __readable__
    super
    raise IOError, "not opened for reading" if @rclosed_socket
  end

  def __writable__
    super
    raise IOError, "not opened for writing" if @wclosed_socket
  end

  def __family__
    Socket.__sockaddr_parse__(__sock_name__(fileno, false).then { |a| Integer === a ? "" : a })&.first || Socket::AF_UNSPEC
  end

  def getsockopt(level, optname)
    family = __family__
    level = Socket.__level__(family, level)
    optname = Socket.__optname__(family, level, optname)
    data = IO.__check__(__sock_getopt__(fileno, level, optname), nil, "getsockopt(2)")
    Socket::Option.new(family, level, optname, data)
  end

  def setsockopt(*args)
    if args.size == 1
      option = args[0]
      level, optname, value = option.level, option.optname, option.data
    elsif args.size == 3
      level, optname, value = args
      family = __family__
      level = Socket.__level__(family, level)
      optname = Socket.__optname__(family, level, optname)
    else
      raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1 or 3)"
    end
    value = case value
            when Integer then [value].pack("i")
            when true then [1].pack("i")
            when false then [0].pack("i")
            else IO.__str__(value)
            end
    IO.__check__(__sock_setopt__(fileno, level, optname, value.b), nil, "setsockopt(2)")
    0
  end

  def getsockname
    IO.__check__(__sock_name__(fileno, false), nil, "getsockname(2)")
  end

  def getpeername
    IO.__check__(__sock_name__(fileno, true), nil, "getpeername(2)")
  end

  def local_address
    Addrinfo.__from__(getsockname, self)
  end

  def remote_address
    Addrinfo.__from__(getpeername, self)
  end

  def __socktype__
    data = __sock_getopt__(fileno, Socket::SOL_SOCKET, Socket::SO_TYPE)
    Integer === data ? 0 : data.unpack1("i")
  end

  def getpeereid
    if Socket.const_defined?(:SO_PEERCRED)
      data = IO.__check__(__sock_getopt__(fileno, Socket::SOL_SOCKET, Socket::SO_PEERCRED), nil, "getsockopt(2)")
      _pid, uid, gid = data.unpack("iII")
      [uid, gid]
    else
      raise NotImplementedError, "getpeereid() function is unimplemented on this machine"
    end
  end

  def send(message, flags, *rest)
    if rest.size > 1
      raise ArgumentError, "wrong number of arguments (given #{rest.size + 2}, expected 2..3)"
    end
    message = String === message ? message : IO.__str__(message)
    to = rest.empty? || rest[0].nil? ? nil : Socket.__sockaddr__(rest[0])
    __send_to__(message, flags, to)
  end

  def __send_to__(message, flags, to)
    flags = Integer.__index__(flags)
    loop do
      result = __sock_send__(fileno, message.b, flags, to)
      return result[0] unless Integer === result
      next if result == Errno::EINTR::Errno
      next __wait_for__(IO::WRITABLE) if result == IO.__eagain__
      IO.__check__(result, nil, to ? "sendto(2)" : "send(2)")
    end
  end

  # `recv` and kin: the bytes, or nil at the end of a stream (CRuby 3.3 on).
  def __recv__(length, flags, buffer)
    length = Integer.__index__(length)
    flags = Integer.__index__(flags || 0)
    raise ArgumentError, "negative length #{length} given" if length < 0
    __readable__
    loop do
      result = __sock_recv__(fileno, length, flags)
      unless Integer === result
        data, from = result
        if data.empty? && length > 0 && __socktype__ == Socket::SOCK_STREAM
          buffer&.replace("")
          return [nil, from]
        end
        return [__into__(data, buffer), from]
      end
      next if result == Errno::EINTR::Errno
      next __wait_for__(IO::READABLE) if result == IO.__eagain__
      IO.__check__(result, nil, "recvfrom(2)")
    end
  end

  # The data in the caller's buffer, which keeps its encoding.
  def __into__(data, buffer)
    return data if buffer.nil?
    buffer = IO.__str__(buffer)
    encoding = buffer.encoding
    buffer.replace(data)
    buffer.force_encoding(encoding)
  end

  def __recv_nonblock__(length, flags, buffer, exception)
    IO.__exception_flag__(exception)
    length = Integer.__index__(length)
    flags = Integer.__index__(flags || 0)
    __readable__
    result = __sock_recv__(fileno, length, flags | Socket::MSG_DONTWAIT)
    if Integer === result
      if result == IO.__eagain__
        return :wait_readable unless exception
        raise IO::EAGAINWaitReadable, "recvfrom(2) would block"
      end
      IO.__check__(result, nil, "recvfrom(2)")
    end
    data, from = result
    if data.empty? && length > 0 && __socktype__ == Socket::SOCK_STREAM
      buffer&.replace("")
      return nil
    end
    [__into__(data, buffer), from]
  end

  def recv(length, flags = 0, buffer = nil)
    __recv__(length, flags, buffer)[0]
  end

  def recv_nonblock(length, flags = 0, buffer = nil, exception: true)
    result = __recv_nonblock__(length, flags, buffer, exception)
    return result if result.nil? || Symbol === result
    result[0]
  end

  def __connect__(address, timeout, exception)
    result = __sock_connect__(fileno, address)
    return 0 if result == true
    if result == Errno::EINPROGRESS::Errno
      __wait_for__(IO::WRITABLE)
      result = __sock_getopt__(fileno, Socket::SOL_SOCKET, Socket::SO_ERROR).unpack1("i")
      return 0 if result == 0
    end
    Socket.__fail__(result, "connect(2)", address)
  end

  def __accept__(nonblock, exception = true)
    __check_open__
    if nonblock
      IO.__exception_flag__(exception)
      __nonblock__
    end
    loop do
      result = __sock_accept__(fileno)
      return result unless Integer === result
      next if result == Errno::EINTR::Errno && !nonblock
      next __wait_for__(IO::READABLE) if result == IO.__eagain__ && !nonblock
      if nonblock && (result == IO.__eagain__ || result == Errno::ECONNABORTED::Errno || result == Errno::EPROTO::Errno || result == Errno::EINTR::Errno)
        return :wait_readable unless exception
        raise IO::EAGAINWaitReadable, "accept(2) would block"
      end
      IO.__check__(result, nil, "accept(2)")
    end
  end

  def sendmsg(message, flags = 0, dest_sockaddr = nil, *controls)
    __sendmsg__(message, flags, dest_sockaddr, controls, false, true)
  end

  def sendmsg_nonblock(message, flags = 0, dest_sockaddr = nil, *controls, exception: true)
    __sendmsg__(message, flags, dest_sockaddr, controls, true, exception)
  end

  def __sendmsg__(message, flags, dest, controls, nonblock, exception)
    IO.__exception_flag__(exception)
    message = IO.__str__(message)
    flags = Integer.__index__(flags || 0)
    flags |= Socket::MSG_DONTWAIT if nonblock
    dest = dest.nil? ? nil : Socket.__sockaddr__(dest)
    family = __family__
    controls = controls.map do |control|
      if Socket::AncillaryData === control
        [control.level, control.type, control.data.b]
      else
        level, type, data = control
        level = Socket.__level__(family, level)
        [level, Socket.__cmsg_type__(family, level, type), IO.__str__(data).b]
      end
    end
    loop do
      result = __sock_sendmsg__(fileno, message.b, flags, dest, controls)
      return result[0] unless Integer === result
      next if result == Errno::EINTR::Errno && !nonblock
      next __wait_for__(IO::WRITABLE) if result == IO.__eagain__ && !nonblock
      if nonblock && result == IO.__eagain__
        return :wait_writable unless exception
        raise IO::EAGAINWaitWritable, "sendmsg(2) would block"
      end
      IO.__check__(result, nil, "sendmsg(2)")
    end
  end

  def recvmsg(dlen = nil, flags = 0, clen = nil, scm_rights: false)
    __recvmsg__(dlen, flags, clen, scm_rights, false, true)
  end

  def recvmsg_nonblock(dlen = nil, flags = 0, clen = nil, scm_rights: false, exception: true)
    __recvmsg__(dlen, flags, clen, scm_rights, true, exception)
  end

  def __recvmsg__(dlen, flags, clen, scm_rights, nonblock, exception)
    IO.__exception_flag__(exception)
    flags = Integer.__index__(flags || 0)
    flags |= Socket::MSG_DONTWAIT if nonblock
    grow = dlen.nil?
    dlen = grow ? 4096 : Integer.__index__(dlen)
    clen = clen.nil? ? 4096 : Integer.__index__(clen)
    __readable__
    loop do
      peek = grow ? flags | Socket::MSG_PEEK : flags
      result = __sock_recvmsg__(fileno, dlen, peek, clen)
      if Integer === result
        next if result == Errno::EINTR::Errno && !nonblock
        next __wait_for__(IO::READABLE) if result == IO.__eagain__ && !nonblock
        if nonblock && result == IO.__eagain__
          return :wait_readable unless exception
          raise IO::EAGAINWaitReadable, "recvmsg(2) would block"
        end
        IO.__check__(result, nil, "recvmsg(2)")
      end
      data, from, msg_flags, controls = result
      # With no length given, a whole datagram: peek until it fits, then
      # read it.
      if grow
        if msg_flags & Socket::MSG_TRUNC != 0 && dlen < (1 << 20)
          dlen *= 2
          next
        end
        result = __sock_recvmsg__(fileno, dlen, flags, clen)
        IO.__check__(result, nil, "recvmsg(2)") if Integer === result
        data, from, msg_flags, controls = result
      end
      family = __family__
      socktype = __socktype__
      if data.empty? && socktype == Socket::SOCK_STREAM && dlen > 0 && controls.empty?
        return nil
      end
      sender = from.empty? ? Addrinfo.__new__("".b, 0, socktype, 0) : Addrinfo.__from__(from, self)
      ancillary = controls.map do |level, type, bytes|
        control = Socket::AncillaryData.new(family, level, type, bytes)
        if scm_rights && level == Socket::SOL_SOCKET && type == Socket::SCM_RIGHTS
          control.__ios__ = bytes.unpack("i*").map { |fd| IO.for_fd(fd) }
        elsif level == Socket::SOL_SOCKET && type == Socket::SCM_RIGHTS
          bytes.unpack("i*").each { |fd| __fd_close__(fd) }
        end
        control
      end
      return [data, sender, msg_flags, *ancillary]
    end
  end

  # From upstream's `lib/socket.rb`: the address to connect to for one this
  # socket is bound to, which is the loopback for a wildcard.
  def connect_address
    addr = local_address
    afamily = addr.afamily
    if afamily == Socket::AF_INET
      raise SocketError, "unbound IPv4 socket" if addr.ip_port == 0
      if addr.ip_address == "0.0.0.0"
        addr = Addrinfo.new(["AF_INET", addr.ip_port, nil, "127.0.0.1"], addr.pfamily, addr.socktype, addr.protocol)
      end
    elsif defined?(Socket::AF_INET6) && afamily == Socket::AF_INET6
      raise SocketError, "unbound IPv6 socket" if addr.ip_port == 0
      if addr.ip_address == "::"
        addr = Addrinfo.new(["AF_INET6", addr.ip_port, nil, "::1"], addr.pfamily, addr.socktype, addr.protocol)
      elsif addr.ip_address == "0.0.0.0" # MacOS X 10.4 returns "a.b.c.d" for IPv4-mapped IPv6 address.
        addr = Addrinfo.new(["AF_INET6", addr.ip_port, nil, "::1"], addr.pfamily, addr.socktype, addr.protocol)
      elsif addr.ip_address == "::ffff:0.0.0.0" # MacOS X 10.6 returns "::ffff:a.b.c.d" for IPv4-mapped IPv6 address.
        addr = Addrinfo.new(["AF_INET6", addr.ip_port, nil, "::1"], addr.pfamily, addr.socktype, addr.protocol)
      end
    elsif defined?(Socket::AF_UNIX) && afamily == Socket::AF_UNIX
      raise SocketError, "unbound Unix socket" if addr.unix_path == ""
    end
    addr
  end

  # As upstream's `__read_nonblock`: `recv` with `MSG_DONTWAIT`, which leaves
  # the descriptor's own mode alone.
  def read_nonblock(len, str = nil, exception: true)
    IO.__exception_flag__(exception)
    __readable__
    len = Integer.__index__(len)
    unless str.nil?
      str = IO.__str__(str)
      raise FrozenError, "can't modify frozen String: #{str.inspect}" if str.frozen?
    end
    if len == 0
      str&.clear
      return str || ""
    end
    return __partial__(len, str, "read_nonblock", false) unless @rbuf.empty?
    result = __sock_recv__(fileno, len, Socket::MSG_DONTWAIT)
    if Integer === result
      if result == IO.__eagain__
        return :wait_readable unless exception
        raise IO::EAGAINWaitReadable, "read would block"
      end
      IO.__check__(result, @path)
    end
    data = result[0]
    if data.empty?
      str&.clear
      return nil unless exception
      raise EOFError, "end of file reached"
    end
    __into__(data, str)
  end

  def write_nonblock(buf, exception: true)
    IO.__exception_flag__(exception)
    __writable__
    text = String === buf ? buf : buf.to_s
    result = __sock_send__(fileno, text.b, Socket::MSG_DONTWAIT, nil)
    if Integer === result
      if result == IO.__eagain__
        return :wait_writable unless exception
        raise IO::EAGAINWaitWritable, "write would block"
      end
      IO.__check__(result, @path)
    end
    result[0]
  end
end

class Socket < BasicSocket
  def connect_nonblock(address, exception: true)
    IO.__exception_flag__(exception)
    address = Socket.__sockaddr__(address)
    __nonblock__
    result = __sock_connect__(fileno, address)
    return 0 if result == true
    if result == Errno::EINPROGRESS::Errno
      return :wait_writable unless exception
      raise IO::EINPROGRESSWaitWritable, "Operation now in progress - connect(2) would block"
    end
    if result == Errno::EISCONN::Errno && !exception
      return 0
    end
    Socket.__fail__(result, "connect(2)", address)
  end

  def accept_nonblock(exception: true)
    __accept_nonblock__(exception)
  end

  def recvfrom_nonblock(len, flag = 0, str = nil, exception: true)
    __recvfrom_nonblock__(len, flag, str, exception)
  end

  # From upstream's `lib/socket.rb`.
  def ipv6only!
    if defined? Socket::IPV6_V6ONLY
      self.setsockopt(:IPV6, :V6ONLY, 1)
    end
  end

  # `Socket.tcp`, from upstream's `tcp_without_fast_fallback`: the happy
  # eyeballs one starts threads (#45).
  def self.tcp(host, port, local_host = nil, local_port = nil, connect_timeout: nil, resolv_timeout: nil, open_timeout: nil, fast_fallback: nil, &block)
    if open_timeout && (connect_timeout || resolv_timeout)
      raise ArgumentError, "Cannot specify open_timeout along with connect_timeout or resolv_timeout"
    end
    last_error = nil
    ret = nil

    local_addr_list = nil
    if local_host != nil || local_port != nil
      local_addr_list = Addrinfo.getaddrinfo(local_host, local_port, nil, :STREAM, nil)
    end

    Addrinfo.foreach(host, port, nil, :STREAM, timeout: resolv_timeout || open_timeout) {|ai|
      if local_addr_list
        local_addr = local_addr_list.find {|local_ai| local_ai.afamily == ai.afamily }
        next unless local_addr
      else
        local_addr = nil
      end
      begin
        sock = local_addr ?
          ai.connect_from(local_addr, timeout: connect_timeout || open_timeout) :
          ai.connect(timeout: connect_timeout || open_timeout)
      rescue SystemCallError
        last_error = $!
        next
      end
      ret = sock
      break
    }
    unless ret
      if last_error
        raise last_error
      else
        raise SocketError, "no appropriate local address"
      end
    end
    if block
      begin
        yield ret
      ensure
        ret.close
      end
    else
      ret
    end
  end

  def self.tcp_fast_fallback = @tcp_fast_fallback.nil? ? true : @tcp_fast_fallback

  def self.tcp_fast_fallback=(value)
    @tcp_fast_fallback = value
  end

  # From upstream's `lib/socket.rb`.
  def self.ip_sockets_port0(ai_list, reuseaddr)
    sockets = []
    begin
      sockets.clear
      port = nil
      ai_list.each {|ai|
        begin
          s = Socket.new(ai.pfamily, ai.socktype, ai.protocol)
        rescue SystemCallError
          next
        end
        sockets << s
        s.ipv6only! if ai.ipv6?
        if reuseaddr
          s.setsockopt(:SOCKET, :REUSEADDR, 1)
        end
        unless port
          s.bind(ai)
          port = s.local_address.ip_port
        else
          s.bind(ai.family_addrinfo(ai.ip_address, port))
        end
      }
    rescue Errno::EADDRINUSE
      sockets.each(&:close)
      retry
    rescue Exception
      sockets.each(&:close)
      raise
    end
    sockets
  end

  def self.tcp_server_sockets_port0(host)
    ai_list = Addrinfo.getaddrinfo(host, 0, nil, :STREAM, nil, Socket::AI_PASSIVE)
    sockets = ip_sockets_port0(ai_list, true)
    begin
      sockets.each {|s|
        s.listen(Socket::SOMAXCONN)
      }
    rescue Exception
      sockets.each(&:close)
      raise
    end
    sockets
  end

  # From upstream's `lib/socket.rb`.
  def self.tcp_server_sockets(host=nil, port)
    if port == 0
      sockets = tcp_server_sockets_port0(host)
    else
      last_error = nil
      sockets = []
      begin
        Addrinfo.foreach(host, port, nil, :STREAM, nil, Socket::AI_PASSIVE) {|ai|
          begin
            s = ai.listen
          rescue SystemCallError
            last_error = $!
            next
          end
          sockets << s
        }
        if sockets.empty?
          raise last_error
        end
      rescue Exception
        sockets.each(&:close)
        raise
      end
    end
    if block_given?
      begin
        yield sockets
      ensure
        sockets.each(&:close)
      end
    else
      sockets
    end
  end

  # From upstream's `lib/socket.rb`.
  def self.accept_loop(*sockets) # :yield: socket, client_addrinfo
    sockets.flatten!(1)
    if sockets.empty?
      raise ArgumentError, "no sockets"
    end
    loop {
      readable, _, _ = IO.select(sockets)
      readable.each {|r|
        sock, addr = r.accept_nonblock(exception: false)
        next if sock == :wait_readable
        yield sock, addr
      }
    }
  end

  def self.tcp_server_loop(host=nil, port, &b) # :yield: socket, client_addrinfo
    tcp_server_sockets(host, port) {|sockets|
      accept_loop(sockets, &b)
    }
  end

  # From upstream's `lib/socket.rb`.
  def self.udp_server_sockets(host=nil, port)
    last_error = nil
    sockets = []

    ipv6_recvpktinfo = nil
    if defined? Socket::AncillaryData
      if defined? Socket::IPV6_RECVPKTINFO # RFC 3542
        ipv6_recvpktinfo = Socket::IPV6_RECVPKTINFO
      elsif defined? Socket::IPV6_PKTINFO # RFC 2292
        ipv6_recvpktinfo = Socket::IPV6_PKTINFO
      end
    end

    local_addrs = Socket.ip_address_list

    ip_list = []
    Addrinfo.foreach(host, port, nil, :DGRAM, nil, Socket::AI_PASSIVE) {|ai|
      if ai.ipv4? && ai.ip_address == "0.0.0.0"
        local_addrs.each {|a|
          next unless a.ipv4?
          ip_list << Addrinfo.new(a.to_sockaddr, :INET, :DGRAM, 0);
        }
      elsif ai.ipv6? && ai.ip_address == "::" && !ipv6_recvpktinfo
        local_addrs.each {|a|
          next unless a.ipv6?
          ip_list << Addrinfo.new(a.to_sockaddr, :INET6, :DGRAM, 0);
        }
      else
        ip_list << ai
      end
    }
    ip_list.uniq!(&:to_sockaddr)

    if port == 0
      sockets = ip_sockets_port0(ip_list, false)
    else
      ip_list.each {|ip|
        ai = Addrinfo.udp(ip.ip_address, port)
        begin
          s = ai.bind
        rescue SystemCallError
          last_error = $!
          next
        end
        sockets << s
      }
      if sockets.empty?
        raise last_error
      end
    end

    sockets.each {|s|
      ai = s.local_address
      if ipv6_recvpktinfo && ai.ipv6? && ai.ip_address == "::"
        s.setsockopt(:IPV6, ipv6_recvpktinfo, 1)
      end
    }

    if block_given?
      begin
        yield sockets
      ensure
        sockets.each(&:close) if sockets
      end
    else
      sockets
    end
  end

  # From upstream's `lib/socket.rb`.
  def self.udp_server_recv(sockets)
    sockets.each {|r|
      msg, sender_addrinfo, _, *controls = r.recvmsg_nonblock(exception: false)
      next if msg == :wait_readable
      ai = r.local_address
      if ai.ipv6? and pktinfo = controls.find {|c| c.cmsg_is?(:IPV6, :PKTINFO) }
        ai = Addrinfo.udp(pktinfo.ipv6_pktinfo_addr.ip_address, ai.ip_port)
        yield msg, UDPSource.new(sender_addrinfo, ai) {|reply_msg|
          r.sendmsg reply_msg, 0, sender_addrinfo, pktinfo
        }
      else
        yield msg, UDPSource.new(sender_addrinfo, ai) {|reply_msg|
          r.send reply_msg, 0, sender_addrinfo
        }
      end
    }
  end

  def self.udp_server_loop_on(sockets, &b) # :yield: msg, msg_src
    loop {
      readable, _, _ = IO.select(sockets)
      udp_server_recv(readable, &b)
    }
  end

  def self.udp_server_loop(host=nil, port, &b) # :yield: message, message_source
    udp_server_sockets(host, port) {|sockets|
      udp_server_loop_on(sockets, &b)
    }
  end

  # From upstream's `lib/socket.rb`.
  class UDPSource
    def initialize(remote_address, local_address, &reply_proc)
      @remote_address = remote_address
      @local_address = local_address
      @reply_proc = reply_proc
    end

    attr_reader :remote_address, :local_address

    def inspect # :nodoc:
      "\#<#{self.class}: #{@remote_address.inspect_sockaddr} to #{@local_address.inspect_sockaddr}>".dup
    end

    def reply(msg)
      @reply_proc.call msg
    end
  end

  # From upstream's `lib/socket.rb`.
  def self.unix(path) # :yield: socket
    addr = Addrinfo.unix(path)
    sock = addr.connect
    if block_given?
      begin
        yield sock
      ensure
        sock.close
      end
    else
      sock
    end
  end

  def self.unix_server_socket(path)
    unless unix_socket_abstract_name?(path)
      begin
        st = File.lstat(path)
      rescue Errno::ENOENT
      end
      if st&.socket? && st.owned?
        File.unlink path
      end
    end
    s = Addrinfo.unix(path).listen
    if block_given?
      begin
        yield s
      ensure
        s.close
        unless unix_socket_abstract_name?(path)
          File.unlink path
        end
      end
    else
      s
    end
  end

  def self.unix_socket_abstract_name?(path)
    RUBY_PLATFORM.include?("linux") && path.empty? || path.start_with?("\0")
  end
  private_class_method :unix_socket_abstract_name?

  def self.unix_server_loop(path, &b) # :yield: socket, client_addrinfo
    unix_server_socket(path) {|serv|
      accept_loop(serv, &b)
    }
  end
end

# --- addresses ------------------------------------------------------------------

class Addrinfo
  # An Addrinfo of everything known, without checks: what the system calls
  # answer.
  def self.__new__(sockaddr, pfamily, socktype, protocol, canonname = nil, inspectname = nil)
    info = allocate
    info.__init__(sockaddr, pfamily, socktype, protocol, canonname, inspectname)
    info
  end

  # One a socket answered, with that socket's family and type.
  def self.__from__(sockaddr, socket)
    if socket.nil?
      return __new__(sockaddr, Socket.__sockaddr_parse__(sockaddr)&.first || 0, 0, 0)
    end
    socket = socket.instance_variable_get(:@__socket__) || socket if Addrinfo === socket
    if Addrinfo === socket
      return __new__(sockaddr, socket.pfamily, socket.socktype, socket.protocol)
    end
    family = Socket.__sockaddr_parse__(sockaddr)&.first || socket.__family__
    type = socket.__socktype__
    # Measured: CRuby does not ask the socket its protocol.
    __new__(sockaddr, family, type, 0)
  end

  def __init__(sockaddr, pfamily, socktype, protocol, canonname, inspectname)
    @sockaddr = sockaddr.b.freeze
    @pfamily = pfamily
    @socktype = socktype
    @protocol = protocol
    @canonname = canonname
    @inspectname = inspectname
    self
  end

  def initialize(sockaddr, family = nil, socktype = nil, protocol = nil)
    pfamily = family.nil? ? nil : Socket.__family__(family)
    socktype = Socket.__socktype__(socktype)
    protocol = protocol.nil? ? 0 : Integer.__index__(protocol)
    inspectname = nil
    if Array === sockaddr
      afamily = Socket.__family__(sockaddr[0])
      if afamily == Socket::AF_UNIX
        path = IO.__str__(sockaddr[1])
        bytes = Socket.sockaddr_un(path)
        pfamily ||= Socket::PF_UNIX
      elsif Socket.__inet__(afamily)
        raise ArgumentError, "array size should be 3 or 4, #{sockaddr.size} given" unless [3, 4].include?(sockaddr.size)
        _, port, host, numeric = sockaddr
        numeric = host if sockaddr.size == 3
        numeric = IO.__str__(numeric)
        # Measured: with no socket type, the lookup is a datagram one.
        hints = socktype == 0 ? Socket::SOCK_DGRAM : socktype
        list = Socket.__getaddrinfo__(numeric, port, afamily, hints, protocol, Socket::AI_NUMERICHOST | Socket::AI_NUMERICSERV)
        bytes = list[0][3]
        inspectname = host unless host.nil? || host == numeric
        pfamily ||= afamily
      else
        raise SocketError, "unknown address family: #{sockaddr[0]}"
      end
      __init__(bytes, pfamily, socktype, protocol, nil, inspectname)
    else
      bytes = String === sockaddr ? sockaddr : IO.__str__(sockaddr)
      bytes = bytes.b
      pfamily ||= Socket::PF_UNSPEC
      __init__(bytes, pfamily, socktype, protocol, nil, nil)
    end
  end

  def self.getaddrinfo(nodename, service, family = nil, socktype = nil, protocol = nil, flags = nil, timeout: nil)
    list = Socket.__getaddrinfo__(nodename, service, family, socktype, protocol, flags)
    inspectname = __inspectname__(nodename, service, list)
    list.map do |afamily, type, proto, sockaddr, canonname|
      __new__(sockaddr, afamily, type, proto, canonname, inspectname)
    end
  end

  # The node as written, when it is a name and not an address.
  def self.__inspectname__(nodename, service, list)
    return nil if nodename.nil?
    node = Socket.__host__(nodename)
    numeric = list.any? do |l|
      parsed = Socket.__sockaddr_parse__(l[3])
      parsed && parsed.size >= 3 && Socket.__inet_ntop__(parsed[2]) == node
    end
    numeric ? nil : node
  end

  def self.ip(host)
    list = Socket.__getaddrinfo__(host, nil, nil, 0, 0, 0)
    afamily, _, _, sockaddr = list[0]
    parsed = Socket.__sockaddr_parse__(sockaddr)
    sockaddr = Socket.__sockaddr_in__(parsed[2], 0, parsed[3] || 0, parsed[4] || 0)
    __new__(sockaddr, afamily, 0, 0, nil, __inspectname__(host, nil, list))
  end

  def self.tcp(host, port)
    list = Socket.__getaddrinfo__(host, port, nil, Socket::SOCK_STREAM, Socket::IPPROTO_TCP, 0)
    afamily, type, proto, sockaddr, canonname = list[0]
    __new__(sockaddr, afamily, type, proto, canonname, __inspectname__(host, port, list))
  end

  def self.udp(host, port)
    list = Socket.__getaddrinfo__(host, port, nil, Socket::SOCK_DGRAM, Socket::IPPROTO_UDP, 0)
    afamily, type, proto, sockaddr, canonname = list[0]
    __new__(sockaddr, afamily, type, proto, canonname, __inspectname__(host, port, list))
  end

  def self.unix(path, socktype = nil)
    socktype = socktype.nil? ? Socket::SOCK_STREAM : Socket.__socktype__(socktype)
    __new__(Socket.sockaddr_un(path), Socket::PF_UNIX, socktype, 0)
  end

  # From upstream's `lib/socket.rb`.
  def self.foreach(nodename, service, family=nil, socktype=nil, protocol=nil, flags=nil, timeout: nil, &block)
    Addrinfo.getaddrinfo(nodename, service, family, socktype, protocol, flags, timeout: timeout).each(&block)
  end

  attr_reader :pfamily, :socktype, :protocol, :canonname

  def __parsed__ = (@__parsed__ ||= Socket.__sockaddr_parse__(@sockaddr))

  def afamily = __parsed__&.first || Socket::AF_UNSPEC

  def to_sockaddr = @sockaddr
  alias to_s to_sockaddr

  def ip? = Socket.__inet__(afamily)
  def ipv4? = afamily == Socket::AF_INET
  def ipv6? = afamily == Socket::AF_INET6
  def unix? = afamily == Socket::AF_UNIX

  def ip_unpack
    raise SocketError, "need IPv4 or IPv6 address" unless ip?
    [ip_address, ip_port]
  end

  def ip_address
    raise SocketError, "need IPv4 or IPv6 address" unless ip?
    Socket.__ip_text__(__parsed__)
  end

  def ip_port
    raise SocketError, "need IPv4 or IPv6 address" unless ip?
    __parsed__[1]
  end

  def unix_path
    raise SocketError, "need AF_UNIX address" unless unix?
    __parsed__[1]
  end

  def __ipv4_number__ = __parsed__[2].unpack1("N")

  def ipv4_private?
    return false unless ipv4?
    n = __ipv4_number__
    (n & 0xff000000) == 0x0a000000 || (n & 0xfff00000) == 0xac100000 || (n & 0xffff0000) == 0xc0a80000
  end

  def ipv4_loopback? = ipv4? && (__ipv4_number__ & 0xff000000) == 0x7f000000
  def ipv4_multicast? = ipv4? && (__ipv4_number__ & 0xf0000000) == 0xe0000000

  def __ipv6_bytes__ = __parsed__[2].bytes

  def ipv6_unspecified? = ipv6? && __ipv6_bytes__.all?(0)
  def ipv6_loopback? = ipv6? && __ipv6_bytes__ == [0] * 15 + [1]
  def ipv6_multicast? = ipv6? && __ipv6_bytes__[0] == 0xff
  def ipv6_linklocal? = ipv6? && __ipv6_bytes__[0] == 0xfe && (__ipv6_bytes__[1] & 0xc0) == 0x80
  def ipv6_sitelocal? = ipv6? && __ipv6_bytes__[0] == 0xfe && (__ipv6_bytes__[1] & 0xc0) == 0xc0
  def ipv6_unique_local? = ipv6? && (__ipv6_bytes__[0] & 0xfe) == 0xfc
  def ipv6_v4mapped? = ipv6? && __ipv6_bytes__[0, 12] == [0] * 10 + [0xff, 0xff]

  def ipv6_v4compat?
    return false unless ipv6?
    b = __ipv6_bytes__
    b[0, 12].all?(0) && !(b[12, 3].all?(0) && b[15] <= 1)
  end

  def __ipv6_mc__(scope) = ipv6_multicast? && (__ipv6_bytes__[1] & 0x0f) == scope

  def ipv6_mc_nodelocal? = __ipv6_mc__(1)
  def ipv6_mc_linklocal? = __ipv6_mc__(2)
  def ipv6_mc_sitelocal? = __ipv6_mc__(5)
  def ipv6_mc_orglocal? = __ipv6_mc__(8)
  def ipv6_mc_global? = __ipv6_mc__(14)

  def ipv6_to_ipv4
    return nil unless ipv6?
    return nil unless ipv6_v4mapped? || ipv6_v4compat?
    bytes = __parsed__[2].byteslice(12, 4)
    Addrinfo.__new__(Socket.__sockaddr_in__(bytes, ip_port, 0, 0), Socket::PF_INET, @socktype, @protocol)
  end

  def getnameinfo(flags = 0)
    Socket.__getnameinfo__(@sockaddr, Integer.__index__(flags))
  end

  def inspect_sockaddr
    parsed = __parsed__
    return "UNSPEC" if parsed.nil? || (@sockaddr.bytesize <= 2)
    case parsed[0]
    when Socket::AF_INET
      text = Socket.__ip_text__(parsed)
      parsed[1] == 0 ? text : "#{text}:#{parsed[1]}"
    when Socket::AF_INET6
      text = Socket.__ip_text__(parsed)
      parsed[1] == 0 ? text : "[#{text}]:#{parsed[1]}"
    when Socket::AF_UNIX
      path = parsed[1]
      if path.empty?
        "empty-path-AF_UNIX-sockaddr"
      elsif path.bytes.all? { |b| b >= 0x21 && b <= 0x7e }
        path.start_with?("/") ? path : "UNIX #{path}"
      else
        "UNIX:" + path.bytes.map { |b| "%02x" % b }.join(":")
      end
    else
      name = Socket.__name_of__("AF", parsed[0])
      name ? "#{name} address format unknown" : "unknown address family #{parsed[0]}"
    end
  end

  def inspect
    out = "#<Addrinfo: #{inspect_sockaddr}"
    afamily = self.afamily
    if @pfamily != 0 && @pfamily != afamily && !(@sockaddr.bytesize <= 2)
      name = Socket.__name_of__("PF", @pfamily)
      out << " #{name ? "PF_#{name}" : "PF_\?\?\?(#{@pfamily})"}"
    end
    internet = Socket.__inet__(@pfamily)
    if internet && @socktype == Socket::SOCK_STREAM && (@protocol == 0 || @protocol == Socket::IPPROTO_TCP)
      out << " TCP"
    elsif internet && @socktype == Socket::SOCK_DGRAM && (@protocol == 0 || @protocol == Socket::IPPROTO_UDP)
      out << " UDP"
    else
      if @socktype != 0
        name = Socket.__name_of__("SOCK", @socktype)
        out << " #{name ? "SOCK_#{name}" : "SOCK_\?\?\?(#{@socktype})"}"
      end
      if @protocol != 0
        name = internet ? Socket.__name_of__("IPPROTO", @protocol) : nil
        out << " #{name ? "IPPROTO_#{name}" : "UNKNOWN_PROTOCOL(#{@protocol})"}"
      end
    end
    if @inspectname
      out << " (#{@inspectname})"
    elsif @canonname
      out << " #{@canonname}"
    end
    out << ">"
  end

  def marshal_dump
    afamily = self.afamily
    sockaddr = unix? ? unix_path : getnameinfo(Socket::NI_NUMERICHOST | Socket::NI_NUMERICSERV)
    pfamily = Socket.__name_of__("PF", @pfamily)
    socktype = Socket.__name_of__("SOCK", @socktype)
    protocol = Socket.__inet__(@pfamily) ? Socket.__name_of__("IPPROTO", @protocol) : nil
    [Socket.__family_name__(afamily), sockaddr,
     pfamily ? "PF_#{pfamily}" : @pfamily,
     socktype ? "SOCK_#{socktype}" : @socktype,
     protocol ? "IPPROTO_#{protocol}" : @protocol,
     @canonname, @inspectname]
  end

  def marshal_load(data)
    afamily, sockaddr, pfamily, socktype, protocol, canonname, inspectname = data
    afamily = Socket.__family__(afamily)
    pfamily = Socket.__family__(pfamily)
    socktype = Socket.__socktype__(socktype)
    protocol = Integer === protocol ? protocol : Socket::Constants.const_get(protocol)
    if afamily == Socket::AF_UNIX
      bytes = Socket.sockaddr_un(sockaddr)
    else
      host, port = sockaddr
      bytes = Socket.__getaddrinfo__(host, port, afamily, socktype, protocol, Socket::AI_NUMERICHOST | Socket::AI_NUMERICSERV)[0][3]
    end
    __init__(bytes, pfamily, socktype, protocol, canonname, inspectname)
  end

  # From upstream's `lib/socket.rb`.
  def family_addrinfo(*args)
    if args.empty?
      raise ArgumentError, "no address specified"
    elsif Addrinfo === args.first
      raise ArgumentError, "too many arguments" if args.length != 1
      addrinfo = args.first
      if (self.pfamily != addrinfo.pfamily) ||
         (self.socktype != addrinfo.socktype)
        raise ArgumentError, "Addrinfo type mismatch"
      end
      addrinfo
    elsif self.ip?
      raise ArgumentError, "IP address needs host and port but #{args.length} arguments given" if args.length != 2
      host, port = args
      Addrinfo.getaddrinfo(host, port, self.pfamily, self.socktype, self.protocol)[0]
    elsif self.unix?
      raise ArgumentError, "too many arguments" if args.length != 1
      path, = args
      Addrinfo.unix(path)
    else
      raise ArgumentError, "unexpected family"
    end
  end

  # From upstream's `lib/socket.rb`, less its `Timeout` around connect.
  def connect_internal(local_addrinfo, timeout=nil) # :yields: socket
    sock = Socket.new(self.pfamily, self.socktype, self.protocol)
    begin
      sock.ipv6only! if self.ipv6?
      sock.bind local_addrinfo if local_addrinfo
      if timeout
        case sock.connect_nonblock(self, exception: false)
        when 0 # success or EISCONN, other errors raise
          break
        when :wait_writable
          sock.wait_writable(timeout) or
            raise Errno::ETIMEDOUT, "user specified timeout for #{inspect_sockaddr}"
        end while true
      else
        sock.connect(self)
      end
    rescue Exception
      sock.close
      raise
    end
    if block_given?
      begin
        yield sock
      ensure
        sock.close
      end
    else
      sock
    end
  end
  protected :connect_internal

  def connect_from(*args, timeout: nil, &block)
    connect_internal(family_addrinfo(*args), timeout, &block)
  end

  def connect(timeout: nil, &block)
    connect_internal(nil, timeout, &block)
  end

  def connect_to(*args, timeout: nil, &block)
    remote_addrinfo = family_addrinfo(*args)
    remote_addrinfo.connect_internal(self, timeout, &block)
  end

  def bind
    sock = Socket.new(self.pfamily, self.socktype, self.protocol)
    begin
      sock.ipv6only! if self.ipv6?
      sock.setsockopt(:SOCKET, :REUSEADDR, 1)
      sock.bind(self)
    rescue Exception
      sock.close
      raise
    end
    if block_given?
      begin
        yield sock
      ensure
        sock.close
      end
    else
      sock
    end
  end

  def listen(backlog=Socket::SOMAXCONN)
    sock = Socket.new(self.pfamily, self.socktype, self.protocol)
    begin
      sock.ipv6only! if self.ipv6?
      sock.setsockopt(:SOCKET, :REUSEADDR, 1) unless self.pfamily == Socket::PF_UNIX
      sock.bind(self)
      sock.listen(backlog)
    rescue Exception
      sock.close
      raise
    end
    if block_given?
      begin
        yield sock
      ensure
        sock.close
      end
    else
      sock
    end
  end
end

# --- the IP sockets --------------------------------------------------------------

class IPSocket < BasicSocket
  undef_method :getpeereid

  def self.getaddress(host)
    list = Socket.__getaddrinfo__(host, nil, nil, 0, 0, 0)
    Socket.__ip_text__(Socket.__sockaddr_parse__(list[0][3]))
  end

  # `["AF_INET", port, host, address]`, the host looked up when asked to.
  def __describe__(sockaddr, reverse)
    reverse = BasicSocket.__reverse_lookup__(reverse, !@do_not_reverse_lookup)
    parsed = Socket.__sockaddr_parse__(sockaddr)
    address = Socket.__ip_text__(parsed)
    host = reverse ? Socket.__getnameinfo__(sockaddr, 0)[0] : address
    [Socket.__family_name__(parsed[0]), parsed[1], host, address]
  end

  def addr(reverse_lookup = nil)
    __describe__(getsockname, reverse_lookup)
  end

  def peeraddr(reverse_lookup = nil)
    __describe__(getpeername, reverse_lookup)
  end

  def recvfrom(length, flags = 0)
    data, from = __recv__(length, flags, nil)
    return nil if data.nil?
    parsed = Socket.__sockaddr_parse__(from)
    return [data, nil] if parsed.nil? || parsed.size < 3
    [data, __describe__(from, nil)]
  end

  def inspect
    out = super.chomp(">")
    begin
      family, port, _, address = addr(false)
      out << ", #{family}, #{address}, #{port}"
    rescue SystemCallError, SocketError, IOError
    end
    out << ">"
  end
end

class TCPSocket < IPSocket
  def self.gethostbyname(host)
    warn "TCPSocket.gethostbyname is deprecated; use Addrinfo.getaddrinfo instead.", uplevel: 1, category: :deprecated
    list = Socket.__getaddrinfo__(host, nil, nil, Socket::SOCK_STREAM, 0, Socket::AI_CANONNAME)
    family = list[0][0]
    name = list[0][4] || host.to_s
    addresses = list.map { |l| Socket.__ip_text__(Socket.__sockaddr_parse__(l[3])) }.uniq
    [name, [], family, *addresses]
  end

  def initialize(remote_host, remote_port, local_host = nil, local_port = nil, connect_timeout: nil, resolv_timeout: nil, open_timeout: nil, fast_fallback: nil)
    if open_timeout && (connect_timeout || resolv_timeout)
      raise ArgumentError, "Cannot specify open_timeout along with connect_timeout or resolv_timeout"
    end
    local = nil
    unless local_host.nil? && local_port.nil?
      local = Socket.__getaddrinfo__(local_host, local_port, nil, Socket::SOCK_STREAM, 0, Socket::AI_PASSIVE)
    end
    list = Socket.__getaddrinfo__(remote_host, remote_port, nil, Socket::SOCK_STREAM, 0, 0)
    last = nil
    list.each do |family, type, protocol, sockaddr, _|
      bind = nil
      if local
        bind = local.find { |l| l[0] == family }
        next if bind.nil?
      end
      result = __sock_socket__(family, type, protocol)
      if Integer === result
        last = [result, "socket(2)"]
        next
      end
      fd = result[0]
      if bind
        done = __sock_bind__(fd, bind[3])
        if Integer === done
          __fd_close__(fd)
          last = [done, "bind(2)"]
          next
        end
      end
      done = __tcp_connect__(fd, sockaddr, connect_timeout || open_timeout)
      if done == true
        __setup_socket__(fd)
        return
      end
      __fd_close__(fd)
      last = [done, "connect(2)"]
    end
    raise SocketError, "no appropriate local address" if last.nil?
    errno, where = last
    if errno == :timeout
      raise IO::TimeoutError, "Connect timed out!"
    end
    raise SystemCallError.new("#{where} for #{remote_host.inspect} port #{remote_port}", errno)
  end

  def __tcp_connect__(fd, sockaddr, timeout)
    return __sock_connect__(fd, sockaddr) if timeout.nil?
    flags = IO.__check__(__fd_fcntl__(fd, IO.__fcntl__("F_GETFL"), 0))[0]
    __fd_fcntl__(fd, IO.__fcntl__("F_SETFL"), flags | File::NONBLOCK)
    done = __sock_connect__(fd, sockaddr)
    if done == Errno::EINPROGRESS::Errno
      ready = __fd_poll__([[fd, IO.__poll__("POLLOUT")]], (timeout * 1000).ceil)
      return :timeout if Array === ready && ready[0] == 0
      error = __sock_getopt__(fd, Socket::SOL_SOCKET, Socket::SO_ERROR).unpack1("i")
      done = error == 0 ? true : error
    end
    __fd_fcntl__(fd, IO.__fcntl__("F_SETFL"), flags)
    done
  end
end

class TCPServer < TCPSocket
  def initialize(host = nil, port)
    list = Socket.__getaddrinfo__(host, port, nil, Socket::SOCK_STREAM, 0, Socket::AI_PASSIVE)
    last = nil
    list.each do |family, type, protocol, sockaddr, _|
      result = __sock_socket__(family, type, protocol)
      if Integer === result
        last = [result, "socket(2)", sockaddr]
        next
      end
      fd = result[0]
      __sock_setopt__(fd, Socket::SOL_SOCKET, Socket::SO_REUSEADDR, [1].pack("i"))
      done = __sock_bind__(fd, sockaddr)
      done = __sock_listen__(fd, Socket::SOMAXCONN) if done == true
      if done == true
        __setup_socket__(fd)
        return
      end
      __fd_close__(fd)
      last = [done, "bind(2)", sockaddr]
    end
    errno, where, sockaddr = last
    raise SystemCallError.new("#{where} for #{host.nil? ? "nil" : host.inspect} port #{port}", errno)
  end

  def accept
    fd, = __accept__(false)
    TCPSocket.for_fd(fd)
  end

  def accept_nonblock(exception: true)
    result = __accept__(true, exception)
    return result if Symbol === result
    TCPSocket.for_fd(result[0])
  end

  def sysaccept
    __accept__(false)[0]
  end

  def listen(backlog)
    IO.__check__(__sock_listen__(fileno, Integer.__index__(backlog)), nil, "listen(2)")
    0
  end
end

class UDPSocket < IPSocket
  def initialize(family = Socket::AF_INET)
    family = Socket.__family__(family)
    fd = IO.__check__(__sock_socket__(family, Socket::SOCK_DGRAM, 0), nil, "socket(2)")[0]
    __setup_socket__(fd)
  end

  def __resolve__(host, port)
    family = __family__
    list = Socket.__getaddrinfo__(host, port, family == Socket::AF_UNSPEC ? nil : family, Socket::SOCK_DGRAM, 0, 0)
    list.map { |l| l[3] }
  end

  def bind(host, port)
    last = nil
    __resolve__(host, port).each do |sockaddr|
      done = __sock_bind__(fileno, sockaddr)
      return 0 if done == true
      last = done
    end
    raise SystemCallError.new("bind(2) for #{host.inspect} port #{port}", last)
  end

  def connect(host, port)
    last = nil
    __resolve__(host, port).each do |sockaddr|
      done = __sock_connect__(fileno, sockaddr)
      return 0 if done == true
      last = done
    end
    raise SystemCallError.new("connect(2) for #{host.inspect} port #{port}", last)
  end

  def send(message, flags, *rest)
    case rest.size
    when 0, 1
      super
    when 2
      host, port = rest
      message = String === message ? message : IO.__str__(message)
      last = nil
      __resolve__(host, port).each do |sockaddr|
        result = __sock_send__(fileno, message.b, Integer.__index__(flags), sockaddr)
        return result[0] unless Integer === result
        last = result
      end
      IO.__check__(last, nil, "sendto(2)")
    else
      raise ArgumentError, "wrong number of arguments (given #{rest.size + 2}, expected 2..4)"
    end
  end

  def recvfrom_nonblock(len, flag = 0, outbuf = nil, exception: true)
    result = __recv_nonblock__(len, flag, outbuf, exception)
    return result if Symbol === result || result.nil?
    data, from = result
    [data, __describe__(from, nil)]
  end
end

# --- the UNIX sockets ------------------------------------------------------------

class UNIXSocket < BasicSocket
  def self.pair(type = Socket::SOCK_STREAM, protocol = 0)
    type = Socket.__socktype__(type)
    fds = IO.__check__(__sock_pair__(Socket::AF_UNIX, type, protocol.nil? ? 0 : Integer.__index__(protocol)), nil, "socketpair(2)")
    fds.map { |fd| for_fd(fd) }
  end

  class << self
    alias socketpair pair
  end

  def initialize(path)
    __unix_connect__(path, Socket::SOCK_STREAM)
  end

  def __unix_connect__(path, type)
    path = File.__path__(path)
    sockaddr = Socket.sockaddr_un(path)
    fd = IO.__check__(__sock_socket__(Socket::AF_UNIX, type, 0), nil, "socket(2)")[0]
    done = __sock_connect__(fd, sockaddr)
    unless done == true
      __fd_close__(fd)
      raise SystemCallError.new("connect(2) for #{path}", done)
    end
    __setup_socket__(fd)
    @path = nil
  end

  def path
    @__path__ ||= begin
      parsed = Socket.__sockaddr_parse__(getsockname)
      parsed ? parsed[1] : ""
    end
  end

  def addr
    ["AF_UNIX", path]
  end

  def peeraddr
    parsed = Socket.__sockaddr_parse__(getpeername)
    ["AF_UNIX", parsed ? parsed[1] : ""]
  end

  def recvfrom(length, flags = 0, buffer = nil)
    data, from = __recv__(length, flags, buffer)
    parsed = from.empty? ? nil : Socket.__sockaddr_parse__(from)
    [data, ["AF_UNIX", parsed && parsed[0] == Socket::AF_UNIX ? parsed[1] : ""]]
  end

  def send_io(io)
    fd = Integer === io ? io : io.to_io.fileno
    sendmsg("\0", 0, nil, [Socket::SOL_SOCKET, Socket::SCM_RIGHTS, [fd].pack("i")])
    nil
  end

  def recv_io(klass = IO, mode = nil)
    _, _, _, *controls = recvmsg(1, 0, nil, scm_rights: true)
    control = controls.find { |c| c.cmsg_is?(:SOCKET, :RIGHTS) }
    raise SocketError, "file descriptor was not passed" if control.nil?
    io = control.unix_rights[0]
    return io.fileno if klass.nil?
    fd = io.fileno
    io.autoclose = false
    mode.nil? ? klass.for_fd(fd) : klass.for_fd(fd, mode)
  end
end

class UNIXServer < UNIXSocket
  def initialize(path)
    path = File.__path__(path)
    sockaddr = Socket.sockaddr_un(path)
    fd = IO.__check__(__sock_socket__(Socket::AF_UNIX, Socket::SOCK_STREAM, 0), nil, "socket(2)")[0]
    done = __sock_bind__(fd, sockaddr)
    done = __sock_listen__(fd, Socket::SOMAXCONN) if done == true
    unless done == true
      __fd_close__(fd)
      raise SystemCallError.new("bind(2) for #{path.inspect}", done) if done
    end
    __setup_socket__(fd)
  end

  def accept
    fd, = __accept__(false)
    UNIXSocket.for_fd(fd)
  end

  def accept_nonblock(exception: true)
    result = __accept__(true, exception)
    return result if Symbol === result
    UNIXSocket.for_fd(result[0])
  end

  def sysaccept
    __accept__(false)[0]
  end

  def listen(backlog)
    IO.__check__(__sock_listen__(fileno, Integer.__index__(backlog)), nil, "listen(2)")
    0
  end
end

# --- options, control messages, interfaces ---------------------------------------

class Socket
  class Option
    def self.int(family, level, optname, integer)
      new(family, level, optname, [Integer.__index__(integer)].pack("i"))
    end

    def self.bool(family, level, optname, value)
      new(family, level, optname, [value ? 1 : 0].pack("i"))
    end

    def self.byte(family, level, optname, integer)
      new(family, level, optname, [Integer.__index__(integer)].pack("C"))
    end

    def self.linger(onoff, secs)
      onoff = case onoff
              when true then 1
              when false, nil then 0
              else Integer.__index__(onoff)
              end
      new(:UNSPEC, :SOCKET, :LINGER, [onoff, Integer.__index__(secs)].pack("ii"))
    end

    def self.ipv4_multicast_loop(value)
      new(:INET, :IPPROTO_IP, :IP_MULTICAST_LOOP, [Integer.__index__(value)].pack("C"))
    end

    def self.ipv4_multicast_ttl(value)
      new(:INET, :IPPROTO_IP, :IP_MULTICAST_TTL, [Integer.__index__(value)].pack("C"))
    end

    def initialize(family, level, optname, data)
      @family = Socket.__family__(family)
      @level = Socket.__level__(@family, level)
      @optname = Socket.__optname__(@family, @level, optname)
      @data = IO.__str__(data).b.freeze
    end

    attr_reader :family, :level, :optname, :data

    def to_s = @data

    def int
      unless @data.bytesize == 4
        raise TypeError, "size differ.  expected as sizeof(int)=4 but #{@data.bytesize}"
      end
      @data.unpack1("i")
    end

    def bool
      case @data.bytesize
      when 1 then @data.unpack1("C") != 0
      when 4 then @data.unpack1("i") != 0
      else raise TypeError, "size differ.  expected as sizeof(int)=4 but #{@data.bytesize}"
      end
    end

    def byte
      unless @data.bytesize == 1
        raise TypeError, "size differ.  expected as sizeof(char)=1 but #{@data.bytesize}"
      end
      @data.unpack1("C")
    end

    def linger
      unless @level == SOL_SOCKET && @optname == SO_LINGER
        raise TypeError, "linger socket option expected"
      end
      unless @data.bytesize == 8
        raise TypeError, "size differ.  expected as sizeof(struct linger)=8 but #{@data.bytesize}"
      end
      onoff, secs = @data.unpack("ii")
      [onoff == 0 ? false : (onoff == 1 ? true : onoff), secs]
    end

    def ipv4_multicast_loop
      unless @level == IPPROTO_IP && @optname == IP_MULTICAST_LOOP
        raise TypeError, "ipv4_multicast_loop socket option expected"
      end
      @data.bytesize == 1 ? @data.unpack1("C") : @data.unpack1("i")
    end

    def ipv4_multicast_ttl
      unless @level == IPPROTO_IP && @optname == IP_MULTICAST_TTL
        raise TypeError, "ipv4_multicast_ttl socket option expected"
      end
      @data.bytesize == 1 ? @data.unpack1("C") : @data.unpack1("i")
    end

    def unpack(template) = @data.unpack(template)

    INTEGER_OPTIONS = {
      Constants::SOL_SOCKET => %w[DEBUG REUSEADDR REUSEPORT KEEPALIVE DONTROUTE BROADCAST OOBINLINE SNDBUF RCVBUF
                                 SNDLOWAT RCVLOWAT ACCEPTCONN PRIORITY PASSCRED DOMAIN PROTOCOL],
      Constants::IPPROTO_TCP => %w[NODELAY MAXSEG CORK KEEPIDLE KEEPINTVL KEEPCNT SYNCNT LINGER2 DEFER_ACCEPT
                                   WINDOW_CLAMP QUICKACK USER_TIMEOUT FASTOPEN],
      Constants::IPPROTO_UDP => %w[CORK],
    }
    private_constant :INTEGER_OPTIONS

    def __value_inspect__
      prefix = Socket.__level_prefix__(@level)
      name = prefix && Socket.__name_of__(prefix, @optname)
      if @level == SOL_SOCKET && @optname == SO_LINGER && @data.bytesize == 8
        onoff, secs = @data.unpack("ii")
        return "#{onoff == 0 ? "off" : (onoff == 1 ? "on" : onoff)} #{secs}sec"
      end
      if @level == SOL_SOCKET && @optname == SO_TYPE && @data.bytesize == 4
        type = Socket.__name_of__("SOCK", @data.unpack1("i"))
        return type ? "SOCK_#{type}" : @data.unpack1("i").to_s
      end
      if @level == SOL_SOCKET && @optname == SO_ERROR && @data.bytesize == 4
        errno = @data.unpack1("i")
        klass = SystemCallError.new(nil, errno).class
        return klass == SystemCallError ? errno.to_s : klass.name.delete_prefix("Errno::")
      end
      if name && (INTEGER_OPTIONS[@level] || []).include?(name) && @data.bytesize == 4
        return @data.unpack1("i").to_s
      end
      @data.inspect
    end

    def inspect
      family = Socket.__name_of__("AF", @family) || @family.to_s
      prefix = Socket.__level_prefix__(@level)
      level = @level == SOL_SOCKET ? "SOCKET" : (Socket.__inet__(@family) && Socket.__name_of__("IPPROTO", @level))
      optname = prefix && Socket.__name_of__(prefix, @optname)
      if level && optname && (Socket.__inet__(@family) || @level == SOL_SOCKET)
        "#<Socket::Option: #{family} #{level} #{optname} #{__value_inspect__}>"
      else
        "#<Socket::Option: #{family} level:#{level || @level} optname:#{@optname} #{@data.inspect}>"
      end
    end
  end

  class AncillaryData
    def self.int(family, cmsg_level, cmsg_type, integer)
      new(family, cmsg_level, cmsg_type, [Integer.__index__(integer)].pack("i"))
    end

    def self.unix_rights(*ios)
      fds = ios.map do |io|
        raise TypeError, "IO expected" unless IO === io
        io.fileno
      end
      data = new(:UNIX, :SOCKET, :RIGHTS, fds.pack("i*"))
      data.__ios__ = ios
      data
    end

    def self.ip_pktinfo(addr, ifindex, spec_dst = nil)
      spec_dst ||= addr
      bytes = [Integer.__index__(ifindex)].pack("I") + spec_dst.__parsed__[2] + addr.__parsed__[2]
      new(:INET, :IP, :PKTINFO, bytes)
    end

    def self.ipv6_pktinfo(addr, ifindex)
      new(:INET6, :IPV6, :PKTINFO, addr.__parsed__[2] + [Integer.__index__(ifindex)].pack("I"))
    end

    def initialize(family, cmsg_level, cmsg_type, data)
      @family = Socket.__family__(family)
      @level = Socket.__level__(@family, cmsg_level)
      @type = Socket.__cmsg_type__(@family, @level, cmsg_type)
      @data = IO.__str__(data).b
    end

    attr_reader :family, :level, :type, :data
    attr_writer :__ios__

    def cmsg_is?(level, type)
      level = Socket.__level__(@family, level)
      type = Socket.__cmsg_type__(@family, level, type)
      @level == level && @type == type
    end

    def int
      unless @data.bytesize == 4
        raise TypeError, "size differ.  expected as sizeof(int)=4 but #{@data.bytesize}"
      end
      @data.unpack1("i")
    end

    def unix_rights
      unless @level == SOL_SOCKET && @type == SCM_RIGHTS
        raise TypeError, "SCM_RIGHTS ancillary data expected"
      end
      @__ios__
    end

    def timestamp
      unless @level == SOL_SOCKET && @type == SCM_TIMESTAMP
        raise TypeError, "SCM_TIMESTAMP ancillary data expected"
      end
      sec, usec = @data.unpack("q!q!")
      Time.at(sec, usec)
    end

    def ip_pktinfo
      unless @level == IPPROTO_IP && @type == IP_PKTINFO && @data.bytesize >= 12
        raise TypeError, "IP_PKTINFO ancillary data expected"
      end
      ifindex = @data.unpack1("I")
      addr = Addrinfo.__new__(Socket.__sockaddr_in__(@data.byteslice(8, 4), 0, 0, 0), PF_INET, 0, 0)
      spec = Addrinfo.__new__(Socket.__sockaddr_in__(@data.byteslice(4, 4), 0, 0, 0), PF_INET, 0, 0)
      [addr, ifindex, spec]
    end

    def ipv6_pktinfo
      unless @level == IPPROTO_IPV6 && @type == IPV6_PKTINFO && @data.bytesize == 20
        raise TypeError, "IPV6_PKTINFO ancillary data expected"
      end
      ifindex = @data.byteslice(16, 4).unpack1("I")
      addr = Addrinfo.__new__(Socket.__sockaddr_in__(@data.byteslice(0, 16), 0, 0, 0), PF_INET6, 0, 0)
      [addr, ifindex]
    end

    def ipv6_pktinfo_addr = ipv6_pktinfo[0]
    def ipv6_pktinfo_ifindex = ipv6_pktinfo[1]

    def inspect
      family = Socket.__name_of__("AF", @family) || @family.to_s
      prefix = @level == SOL_SOCKET ? "SCM" : Socket.__level_prefix__(@level)
      level = @level == SOL_SOCKET ? "SOCKET" : Socket.__name_of__("IPPROTO", @level)
      type = prefix && Socket.__name_of__(prefix, @type)
      out = "#<Socket::AncillaryData: #{family} #{level || "cmsg_level:#{@level}"} #{type || "cmsg_type:#{@type}"}"
      if @level == SOL_SOCKET && @type == SCM_RIGHTS
        out << " " << @data.unpack("i*").join(" ")
      else
        out << " " << @data.inspect
      end
      out << ">"
    end
  end

  class Ifaddr
    def self.__new__(name, flags, ifindex, addr, netmask, broadaddr, dstaddr)
      ifaddr = allocate
      ifaddr.__init__(name, flags, ifindex, addr, netmask, broadaddr, dstaddr)
      ifaddr
    end

    class << self
      undef_method :new
    end

    def __init__(name, flags, ifindex, addr, netmask, broadaddr, dstaddr)
      @name = name.force_encoding(Encoding::US_ASCII)
      @flags = flags
      @ifindex = ifindex
      info = ->(bytes) { bytes && Addrinfo.__new__(bytes, Socket.__sockaddr_parse__(bytes)&.first || 0, 0, 0) }
      @addr = info.(addr)
      @netmask = info.(netmask)
      @broadaddr = info.(broadaddr)
      @dstaddr = info.(dstaddr)
    end

    attr_reader :name, :flags, :ifindex, :addr, :netmask, :broadaddr, :dstaddr

    def inspect
      flags = Socket.__names__("IFF").sort.filter_map { |bit, name| name if @flags & bit != 0 && bit.to_s(2).count("1") == 1 }
      out = "#<Socket::Ifaddr #{@name}"
      out << " #{flags.join(",")}" unless flags.empty?
      out << " #{@addr.inspect_sockaddr}" if @addr
      out << " netmask=#{@netmask.inspect_sockaddr}" if @netmask
      out << " broadcast=#{@broadaddr.inspect_sockaddr}" if @broadaddr
      out << " dstaddr=#{@dstaddr.inspect_sockaddr}" if @dstaddr
      out << ">"
    end
  end
end
