# FileUtils (#49) against a scratch directory given as ARGV[0]. Every
# result is printed on stdout, as are the verbose and noop messages.
require "fileutils"
require "pathname"
Dir.chdir(ARGV[0])

def t(label)
  r = yield
  puts "#{label}: #{r.inspect}"
rescue Exception => e
  puts "#{label}: #{e.class}: #{e.message.sub(Dir.pwd, 'ROOT')}"
end

t("mkdir_p") { FileUtils.mkdir_p("a/b/c") }
t("mkdir") { FileUtils.mkdir("d") }
t("touch") { FileUtils.touch(["a/x.txt", "a/b/y.txt"]) }
t("exists") { [File.file?("a/x.txt"), File.directory?("a/b/c")] }
t("write") { File.write("a/x.txt", "hello") }
t("cp") { FileUtils.cp("a/x.txt", "d/x2.txt") }
t("read cp") { File.read("d/x2.txt") }
t("compare") { FileUtils.compare_file("a/x.txt", "d/x2.txt") }
t("identical?") { FileUtils.identical?("a/x.txt", "d/x2.txt") }
t("uptodate?") { FileUtils.uptodate?("d/x2.txt", ["a/x.txt"]) }
t("cp_r") { FileUtils.cp_r("a", "e") }
t("cp_r tree") { Dir.glob("e/**/*").sort }
t("mv") { FileUtils.mv("d/x2.txt", "d/x3.txt") }
t("mv ls") { Dir.children("d").sort }
t("ln") { FileUtils.ln("a/x.txt", "d/hard") }
t("ln_s") { FileUtils.ln_s("../a/x.txt", "d/soft") }
t("ln_sf") { FileUtils.ln_sf("../a/x.txt", "d/soft") }
t("readlink") { File.readlink("d/soft") }
t("chmod") { FileUtils.chmod(0600, "d/x3.txt"); File.stat("d/x3.txt").mode & 0777 }
t("chmod sym") { FileUtils.chmod("u+x,g+r", "d/x3.txt"); File.stat("d/x3.txt").mode & 0777 }
t("chmod_R") { FileUtils.chmod_R(0755, "e"); File.stat("e/b").mode & 0777 }
t("install") { FileUtils.install("a/x.txt", "d/inst", mode: 0644); File.stat("d/inst").mode & 0777 }
t("copy_entry") { FileUtils.copy_entry("a", "f"); Dir.glob("f/**/*").sort }
t("pwd") { FileUtils.pwd == Dir.pwd }
t("cd") { FileUtils.cd("a") { Dir.pwd.end_with?("/a") } }
t("rm") { FileUtils.rm("d/x3.txt") }
t("rm_f") { FileUtils.rm_f("nope") }
t("rmdir") { FileUtils.rmdir("a/b/c") }
t("rm_r") { FileUtils.rm_r("e") }
t("rm_rf") { FileUtils.rm_rf("f") }
t("remove_entry_secure") { FileUtils.remove_entry_secure("d") }
t("verbose") { FileUtils.mkdir_p("v", verbose: true) }
t("noop") { FileUtils::DryRun.rm_rf("a"); File.exist?("a") }
t("noop opt") { FileUtils.rm_rf("a", noop: true); File.exist?("a") }
t("Verbose module") { FileUtils::Verbose.touch("v/z") }
t("options") { FileUtils.options.size > 0 }
t("options_of") { FileUtils.options_of(:rm) }
t("have_option?") { FileUtils.have_option?(:cp, :preserve) }
t("cp preserve") { FileUtils.cp("a/x.txt", "v/p", preserve: true); File.mtime("v/p") == File.mtime("a/x.txt") }
t("rm missing") { FileUtils.rm("nope") }
t("mkdir exists") { FileUtils.mkdir("v") }
FileUtils.mkdir_p("s/t"); File.write("s/t/f", "1"); File.write("s/g", "22")
t("ln_sr") { FileUtils.ln_sr("s/t/f", "lr"); File.readlink("lr") }
t("cp_lr") { FileUtils.cp_lr("s", "hl"); File.stat("hl/g").nlink }
t("mkdir_p mode") { FileUtils.mkdir_p("m/n", mode: 0700); File.stat("m/n").mode & 0777 }
t("ln_s dir") { FileUtils.ln_s(File.expand_path("s"), "sl") }
t("rm_r symlink dir") { FileUtils.rm_r("sl"); [File.exist?("sl"), File.exist?("s/g")] }
t("cp_r deref") { FileUtils.ln_s("t/f", "s/lf"); FileUtils.cp_r("s", "c2", dereference_root: true); File.symlink?("c2/lf") }
t("copy_file") { FileUtils.copy_file("s/g", "cf"); File.read("cf") }
t("copy_stream") { File.open("s/g") { |i| File.open("cs", "w") { |o| FileUtils.copy_stream(i, o) } }; File.read("cs") }
t("cp to self") { FileUtils.cp("s/g", "s/g") }
t("mv dir into dir") { FileUtils.mkdir("dest"); FileUtils.mv("m", "dest"); Dir.children("dest") }
t("mv list") { FileUtils.touch(%w[p1 p2]); FileUtils.mv(%w[p1 p2], "dest"); Dir.children("dest").sort }
t("rmdir parents") { FileUtils.mkdir_p("q/r/s"); FileUtils.rmdir("q/r/s", parents: true); File.exist?("q") }
t("rmdir nonempty") { FileUtils.rmdir("s") }
t("touch mtime") { FileUtils.touch("tm", mtime: Time.at(1000000000)); File.mtime("tm").to_i }
t("chown nil") { FileUtils.chown(nil, nil, "tm") }
t("chown_R nil") { FileUtils.chown_R(nil, nil, "s") }
t("remove_dir") { FileUtils.remove_dir("hl"); File.exist?("hl") }
t("remove_file") { FileUtils.remove_file("cf"); File.exist?("cf") }
t("remove_entry") { FileUtils.remove_entry("c2") }
t("compare_stream") { File.open("s/g") { |a| File.open("cs") { |b| FileUtils.compare_stream(a, b) } } }
t("install dir") { FileUtils.install("s/g", "dest", mode: "u=rw,go=r"); File.stat("dest/g").mode & 0777 }
t("collect_method") { FileUtils.collect_method(:noop).sort.first(3) }
t("commands") { FileUtils.commands.size > 10 }
t("private") { FileUtils.respond_to?(:fu_list) }
t("include") { Class.new { include FileUtils; def go; mkdir_p("inc"); end }.new.go }
t("NoWrite") { FileUtils::NoWrite.touch("nw"); File.exist?("nw") }
t("ln force") { FileUtils.ln("s/g", "cs", force: true); File.read("cs") }
t("rm_rf glob") { FileUtils.rm_rf(Dir.glob("p*")); Dir.glob("p*") }
t("mkdir_p pathname") { FileUtils.mkdir_p(Pathname("pn/x")) }
t("final") { Dir.glob("**/*").sort }
