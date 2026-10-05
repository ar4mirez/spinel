# `RbConfig`, which CRuby generates at build time and has loaded before a
# program starts (#145: mspec's guards read it). The keys are the ones
# ruby/spec and mspec look at, from the platform Spinel was built for.
module RbConfig
  cpu, os = RUBY_PLATFORM.split("-", 2)
  major, minor, teeny = RUBY_VERSION.split(".")
  CONFIG = {
    "arch" => RUBY_PLATFORM,
    "host_cpu" => cpu,
    "host_os" => os,
    "target_cpu" => cpu,
    "target_os" => os,
    "ruby_install_name" => "spinel",
    "RUBY_INSTALL_NAME" => "spinel",
    "EXEEXT" => "",
    "DLEXT" => os.start_with?("darwin") ? "bundle" : "so",
    "ruby_version" => "#{major}.#{minor}.0",
    "MAJOR" => major,
    "MINOR" => minor,
    "TEENY" => teeny,
    "PATCHLEVEL" => RUBY_PATCHLEVEL.to_s,
    "ENABLE_SHARED" => "no",
    # The tables in `crates/spinel-vm/src/encoding_table.rs` are generated
    # from ruby 4.0, which ships these.
    "UNICODE_VERSION" => "17.0.0",
    "UNICODE_EMOJI_VERSION" => "17.0",
  }.freeze

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
