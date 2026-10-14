# `io/nonblock`: `O_NONBLOCK` on an IO's descriptor, through `IO#fcntl`.

class IO
  def nonblock?
    fcntl(IO.__fcntl__("F_GETFL")) & File::NONBLOCK != 0
  end

  def nonblock=(value)
    flags = fcntl(IO.__fcntl__("F_GETFL"))
    wanted = value ? flags | File::NONBLOCK : flags & ~File::NONBLOCK
    fcntl(IO.__fcntl__("F_SETFL"), wanted) unless wanted == flags
    value
  end

  # With a block, nonblocking only while it runs.
  def nonblock(value = true)
    before = nonblock?
    self.nonblock = value
    return self unless block_given?
    begin
      yield self
    ensure
      self.nonblock = before
    end
  end
end
