# A missing method is an ordinary Ruby raise since #170, so a program can catch
# one. Before that it was a Rust-level error that unwound past every handler and
# ended the run, and this file printed nothing.
begin
  "spinel".upcase
rescue NoMethodError => e
  puts e.class
  puts e.message
  puts e.name.inspect
  puts e.receiver
end
