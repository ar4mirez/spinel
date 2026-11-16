# `RbConfig`, which CRuby generates at build time and has loaded before a
# program starts (#145: mspec's guards read it). The keys are the ones
# ruby/spec and mspec look at, from the platform Spinel was built for.
module RbConfig
  cpu, os = RUBY_PLATFORM.split("-", 2)
  major, minor, teeny = RUBY_VERSION.split(".")
  # CRuby's `bindir` is where `make install` put `ruby`; Spinel is one binary,
  # so it is wherever that binary runs from.
  exe = __exe_path__
  bindir = exe && exe[0, exe.rindex("/") || 0]
  # CRuby's `prefix` is where `make install` put Ruby, and the library
  # directories hang off it. Spinel installs nothing beside its binary, so
  # its prefix is the home it keeps the extracted stdlib in, found as
  # `crates/spinel-cli/src/stdlib.rs` finds it: `$SPINEL_HOME`, else
  # `~/.spinel`. When neither `SPINEL_HOME` nor `HOME` is set, stdlib.rs has
  # no home, and RbConfig's own fallback is the directory above the binary's.
  # RubyGems derives its default gem directory from these (`Gem.default_dir`,
  # #319).
  prefix = ENV["SPINEL_HOME"]
  if prefix.nil? || prefix.empty?
    home = ENV["HOME"]
    prefix = home.nil? || home.empty? ? nil : File.join(home, ".spinel")
  end
  prefix ||= bindir ? File.dirname(bindir) : "/"
  ruby_version = "#{major}.#{minor}.0"
  rubylibprefix = File.join(prefix, "lib", "ruby")
  sitedir = File.join(rubylibprefix, "site_ruby")
  vendordir = File.join(rubylibprefix, "vendor_ruby")
  CONFIG = {
    # A copy: CONFIG's values are not frozen, and RUBY_PLATFORM is.
    "arch" => RUBY_PLATFORM.dup,
    "bindir" => bindir,
    "prefix" => prefix,
    "exec_prefix" => prefix,
    "libdir" => File.join(prefix, "lib"),
    "datadir" => File.join(prefix, "share"),
    "sysconfdir" => File.join(prefix, "etc"),
    "rubylibprefix" => rubylibprefix,
    "sitedir" => sitedir,
    "sitelibdir" => File.join(sitedir, ruby_version),
    "vendordir" => vendordir,
    "vendorlibdir" => File.join(vendordir, ruby_version),
    "host_cpu" => cpu,
    "host_os" => os,
    "target_cpu" => cpu,
    "target_os" => os,
    "ruby_install_name" => "spinel",
    "RUBY_INSTALL_NAME" => "spinel",
    "EXEEXT" => "",
    "DLEXT" => os.start_with?("darwin") ? "bundle" : "so",
    "ruby_version" => ruby_version,
    "MAJOR" => major,
    "MINOR" => minor,
    "TEENY" => teeny,
    "PATCHLEVEL" => RUBY_PATCHLEVEL.to_s,
    "ENABLE_SHARED" => "no",
    # Spinel is one binary with no libruby, as a CRuby built without
    # `--enable-shared`: these name a library that is not installed.
    "LIBRUBY_A" => "libspinel-static.a",
    "LIBRUBY" => "libspinel-static.a",
    "LIBRUBY_SO" => os.start_with?("darwin") ?
      "libspinel.#{major}.#{minor}.#{teeny}.dylib" :
      "libspinel.so.#{major}.#{minor}.#{teeny}",
    "libdirname" => "libdir",
    # The extensions Spinel ships are inside the binary, as a CRuby built
    # with `--with-static-linked-ext` has them: `require "etc"` records
    # `etc.so` with no directory, and `resolve_feature_path` says so.
    "EXTSTATIC" => "static",
    # The tables in `crates/spinel-vm/src/encoding_table.rs` are generated
    # from ruby 4.0, which ships these.
    "UNICODE_VERSION" => "17.0.0",
    "UNICODE_EMOJI_VERSION" => "17.0",
  }
  # Not frozen, as CRuby's is not: `mkmf` writes to it, and the CLI adds
  # `rubylibdir` and `rubyarchdir` once it knows where the stdlib is.

  # Where Ruby is installed: CRuby's is the prefix its `rbconfig.rb` sits
  # under, and Spinel's is its home, the prefix above. RubyGems reads it
  # (`Gem::Installer`), and ruby/spec guards what only an installed Ruby has
  # on it.
  TOPDIR = prefix

  # The interpreter's own path: what a program runs to start another Ruby
  # (WEBrick's CGI handler, #54), and what `Gem.ruby` answers (#70). CRuby's
  # `rbconfig.rb` joins `bindir` and `ruby_install_name`, which name the
  # binary `make install` put there; Spinel's is wherever it runs from under
  # whatever name it has, so this is the running binary itself, as the OS
  # reports it: on Linux, as `/proc/self/exe` names it, absolute with
  # symlinks resolved. Only when the platform cannot say does it fall back
  # to CRuby's join.
  def self.ruby
    __exe_path__ || File.join(CONFIG["bindir"], CONFIG["ruby_install_name"] + CONFIG["EXEEXT"])
  end

  # `rbconfig/sizeof`'s table, for the 64-bit targets Spinel builds for:
  # `SIZEOF`, defined when that feature is required, as in CRuby.
  def self.__define_sizeof__
    return if const_defined?(:SIZEOF, false)
    const_set(:SIZEOF, {
      "short" => 2, "int" => 4, "long" => 8, "long long" => 8, "void*" => 8,
      "size_t" => 8, "intptr_t" => 8, "float" => 4, "double" => 8,
    }.freeze)
  end
end

# CRuby's rbconfig.rb sets this beside RbConfig: nil unless Ruby was built
# to run on another platform than the one building it. Bundler reads it.
CROSS_COMPILING = nil unless defined? CROSS_COMPILING
