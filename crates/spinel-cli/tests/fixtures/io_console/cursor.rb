# `IO#cursor` on the console: the query written, the terminal's answer read
# in raw mode; the movements after it are plain VT100 sequences.
require "io/console"

c = IO.console
position = c.cursor
c.cursor = [2, 3]
c.goto(0, 0)
c.cursor_up(2)
c.cursor_down(1)
c.cursor_left(3)
c.cursor_right(4)
c.erase_line(0)
c.erase_screen(1)
c.scroll_forward(1)
c.scroll_backward(1)
c.clear_screen
c.goto_column(5)
File.write(ARGV[0], position.inspect)
