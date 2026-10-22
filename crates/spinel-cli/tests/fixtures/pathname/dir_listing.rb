# Pathname's directory listing, over a tree the test made at ARGV[0]:
# tree/a.txt and tree/sub/.b, each subdirectory with one entry, so a one
# element `sort!` is on the path `find` takes.
require "pathname"

tree = Pathname(ARGV.fetch(0))
p tree.children(false).map(&:to_s).sort
p tree.children.map { |c| c.relative_path_from(tree).to_s }.sort
names = []
tree.each_child(false) { |c| names << c.to_s }
p names.sort
p tree.each_entry.map(&:to_s).sort
p tree.find.map { |f| f.relative_path_from(tree).to_s }
p tree.opendir { |dir| [dir.class, dir.path == tree.to_s, dir.children.sort] }
dir = tree.opendir
p dir.read.class
dir.close
p((dir.read rescue $!.class))
p Dir.foreach(tree.to_s).to_a.sort
p Dir.children(tree.to_s, encoding: "ASCII-8BIT").map(&:encoding).uniq
p tree.rmtree.equal?(tree)
p tree.exist?
