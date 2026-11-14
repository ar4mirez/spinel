# `io/console` on a terminal: the window size, the modes and their round
# trips, `getch`, `getpass`, `ttyname` and `IO.console`, each line as ruby
# 4.0.9 answers it. A mode is read back with `stty -a`, which is the
# terminal's own account of it.
require "io/console"

lines = []
check = lambda do |label, &block|
  value = begin
    block.call
  rescue Exception => e
    [e.class, e.message]
  end
  lines << "#{label}: #{value.inspect}"
end
flags = ->(*names) { words = `stty -a`.split(/[\s;]+/); names.map { |n| words.include?(n) } }
cc = ->(name) { `stty -a`[/\b#{name} = (\d+)/, 1] }
io = $stdin
initial = `stty -g`

check.("winsize=") { io.winsize = [30, 100] }
check.("winsize") { [io.winsize, `stty size`.chomp] }
check.("winsize= 4") { io.winsize = [31, 101, 7, 9]; [io.winsize, $stdout.winsize] }
check.("winsize= wraps") { io.winsize = [65_537, -1]; io.winsize }
check.("winsize= 3") { io.winsize = [1, 2, 3] }
check.("echo?") { io.echo? }
check.("echo= false") { io.echo = false; [io.echo?, flags.("-echo", "-echoe", "-echok", "-echonl")] }
check.("echo= true") { io.echo = true; [io.echo?, flags.("echo", "echoe", "echok", "echonl")] }
# `echo = true` turns on all four, which a fresh terminal need not have had.
initial = `stty -g`
check.("raw") { io.raw { |x| [x.equal?(io), flags.("-icanon", "-echo", "-isig", "-iexten", "-opost", "-icrnl", "-ixon", "cs8", "-parenb", "-echoe", "-echok"), cc.("min"), cc.("time")] } }
check.("restored") { `stty -g` == initial }
check.("raw min time") { io.raw(min: 0, time: 1.5) { [cc.("min"), cc.("time")] } }
check.("raw min wraps") { io.raw(min: 300, time: -1) { [cc.("min"), cc.("time")] } }
check.("raw intr") { io.raw(intr: true) { flags.("isig", "opost", "brkint", "-icanon") } }
check.("raw intr bad") { io.raw(intr: 1) { } }
check.("raw raises") { begin; io.raw { raise "boom" }; rescue => e; [e.message, `stty -g` == initial]; end }
check.("raw no block") { begin; io.raw; rescue LocalJumpError => e; [e.message, `stty -g` == initial]; end }
check.("cooked") { io.raw { io.cooked { flags.("icanon", "echo", "isig", "opost", "icrnl", "istrip") } } }
check.("noecho") { [io.noecho { io.echo? }, io.echo?] }
base = io.console_mode
check.("raw!") { [io.raw!(min: 4).equal?(io), flags.("-icanon"), cc.("min")] }
check.("console_mode=") { io.console_mode = base; `stty -g` == initial }
check.("cooked!") { io.raw!; io.cooked!; r = flags.("icanon", "echo"); io.console_mode = base; r }
check.("mode raw") { io.console_mode = base.raw(min: 2); r = [flags.("-icanon"), cc.("min")]; io.console_mode = base; r }
check.("mode raw!") { m = base.dup; m.raw!; m.echo = true; io.console_mode = m; r = flags.("-icanon", "echo"); io.console_mode = base; r }
check.("mode dup") { m = io.console_mode; d = m.dup; m.echo = false; io.console_mode = d; `stty -g` == initial }
check.("mode= type") { io.console_mode = nil }
check.("getch") { [io.getch, io.getch(min: 1), io.getch, `stty -g` == initial] }
check.("getpass") { [io.getpass, io.getpass("pw: "), `stty -g` == initial] }
check.("ttyname") { name = io.ttyname; [name.start_with?("/dev/"), name.frozen?, name.encoding, $stdout.ttyname == name] }
check.("console") { c = IO.console; [c.class, c.path, c.sync, c.tty?, IO.console.equal?(c), File.console.equal?(c)] }
check.("console send") { IO.console(:winsize) }
check.("console type") { [nil, false, "x"].map { |a| begin; IO.console(a); rescue TypeError => e; e.message; end } }
check.("console close") { c = IO.console; [IO.console(:close), c.closed?, IO.console.equal?(c)] }
check.("flushes") { [io.iflush, $stdout.oflush, io.ioflush, $stdout.beep].all? }
check.("final") { `stty -g` == initial }
File.write(ARGV[0], lines.join("\n") + "\n")
