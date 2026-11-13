# Spinel's Gemfile evaluator (docs/package-manager.md, "1. Gemfile
# evaluation", which documents the JSON it prints).
#
#   spinel run shims/gemfile_eval.rb
#
# The Gemfile is BUNDLE_GEMFILE when that is set and not empty, otherwise the
# nearest `gems.rb` or `Gemfile` from the current directory upward, as Bundler
# finds it. It is evaluated as Ruby, so `if ENV[...]` and anything else a
# Gemfile can do works, by an object with Bundler::Dsl's methods that records
# what Bundler::Dsl records and rejects what it rejects, except that an
# unknown `gem` option is kept, in `options`, for the resolver to judge. The
# result goes to stdout as JSON; an error goes to stderr with the Gemfile line,
# and the exit status is 1.
#
# ponytail: `gemspec` records the directive and the gemspec files it finds,
# but does not evaluate them, because RubyGems does not load yet (#319).
# Turning one into the gem and its development dependencies is the gemspec
# evaluation of git and path sources (#88).

require "json"

class GemfileEval
  class Error < StandardError; end

  # Bundler::CurrentRuby::PLATFORM_MAP's keys as of Bundler 4.0.20.
  # ponytail: Bundler adds a `<platform>_<version>` for each new Ruby; so does this table.
  VERSIONS = %w[18 19 20 21 22 23 24 25 26 27 30 31 32 33 34 40 41].freeze
  PLATFORMS = {
    "ruby" => VERSIONS, "mri" => VERSIONS, "rbx" => [], "truffleruby" => [], "jruby" => %w[18 19],
    "windows" => VERSIONS, "mswin" => VERSIONS, "mswin64" => VERSIONS - %w[18],
    "mingw" => VERSIONS, "x64_mingw" => VERSIONS - %w[18 19],
  }.flat_map { |name, versions| [name, *versions.map { |v| "#{name}_#{v}" }] }.freeze
  # Bundler::Dsl::VALID_KEYS, the names a git_source may not take.
  RESERVED = %w[group groups git path glob name branch ref tag require submodules
                platform platforms source install_if force_ruby_platform].freeze
  # Gem::Requirement::PATTERN.
  REQUIREMENT = /\A\s*(=|!=|>=|<=|~>|>|<)?\s*([0-9]+(?:\.[0-9a-zA-Z]+)*(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?)\s*\z/

  attr_reader :gemfiles

  def initialize
    @dependencies = []
    @gemfiles = []
    @gemspecs = []
    @groups = []
    @optional_groups = []
    @platforms = []
    @install_if = []
    @env = nil
    @source = nil
    @global_source = nil
    @global_path = false
    @ruby = nil
    @gemfile = nil
    @git_sources = {}
    git_source(:github) do |repo|
      if (m = %r{\Ahttps://github\.com/([A-Za-z0-9_\-\.]+/[A-Za-z0-9_\-\.]+)/pull/(\d+)\z}.match(repo))
        { "git" => "https://github.com/#{m[1]}.git", "branch" => nil, "ref" => "refs/pull/#{m[2]}/head", "tag" => nil }
      else
        "https://github.com/#{repo.include?("/") ? repo : "#{repo}/#{repo}"}.git"
      end
    end
    git_source(:gist) { |repo| "https://gist.github.com/#{repo}.git" }
    git_source(:bitbucket) do |repo|
      user, name = repo.split("/")
      "https://#{user}@bitbucket.org/#{user}/#{name || user}.git"
    end
    git_source(:gitlab) do |repo|
      if (m = %r{\Ahttps://gitlab\.com/([A-Za-z0-9_\-\./]+)/-/merge_requests/(\d+)\z}.match(repo))
        { "git" => "https://gitlab.com/#{m[1]}.git", "branch" => nil, "ref" => "refs/merge-requests/#{m[2]}/head", "tag" => nil }
      else
        "https://gitlab.com/#{repo.include?("/") ? repo : "#{repo}/#{repo}"}.git"
      end
    end
  end

  def eval_gemfile(gemfile, contents = nil)
    path = File.expand_path(gemfile.to_s, @gemfile && File.dirname(@gemfile))
    outer = @gemfile
    begin
      @gemfile = path
      @gemfiles << path
      instance_eval(contents || File.read(path, encoding: "UTF-8"), path, 1)
    ensure
      @gemfile = outer
    end
  end

  def gem(name, *args)
    opts = args.last.is_a?(Hash) ? strings(args.pop) : {}
    raise Error, %(You need to specify gem names as Strings. Use 'gem "#{name}"' instead) if name.is_a?(Symbol)
    raise Error, %('#{name}' is not a valid gem name because it contains whitespace) if /\s/.match?(name)
    raise Error, "an empty gem name is not valid" if name.empty?
    if opts["branch"] && !(opts["git"] || (opts.keys & @git_sources.keys).any?)
      raise Error, "The `branch` option for `gem '#{name}'` is not allowed. Only gems with a git source can specify a branch"
    end

    groups = @groups.dup
    listed = opts.delete("groups")
    group = opts.delete("group")
    groups.concat(Array(listed || group))
    groups = [:default] if groups.empty?

    install_if = (@install_if + Array(opts.delete("install_if"))).reduce(true) do |memo, cond|
      memo && (cond.respond_to?(:call) ? cond.call : cond)
    end

    singular = opts.delete("platform")
    plural = opts.delete("platforms")
    platforms = (@platforms + Array(singular || plural)).map(&:to_s)
    platforms.each do |platform|
      next if PLATFORMS.include?(platform)
      raise Error, "`#{platform}` is not a valid platform. The available options are: #{PLATFORMS.map(&:to_sym).inspect}"
    end

    source = @source
    source = rubygems_source(opts.delete("source"), nil) if opts.key?("source")
    git_name = (@git_sources.keys & opts.keys).last
    if git_name
      repo = opts.delete(git_name)
      expanded = @git_sources[git_name].call(repo)
      expanded = { "git" => expanded } if expanded.is_a?(String)
      strings(expanded).each do |key, value|
        raise Error, %(The :#{key} option can't be used with `#{git_name}: #{repo.inspect}`) if opts.key?(key)
        opts[key] = value
      end
    end
    source = git_source_of(opts.delete("git"), opts) if opts["git"]
    source = path_source_of(opts.delete("path"), opts) if opts["path"]
    %w[branch ref tag submodules glob].each { |key| opts.delete(key) }

    autorequire = opts.key?("require") ? Array(opts.delete("require") || []) : nil
    add_dependency(
      "name" => name,
      "requirements" => requirements(args, name),
      "groups" => groups.map(&:to_s),
      "platforms" => platforms,
      "source" => source,
      "require" => autorequire,
      "install_if" => install_if ? true : false,
      "env" => @env.is_a?(Hash) ? @env.to_h { |k, v| [k.to_s, v.is_a?(Regexp) ? v.inspect : v] } : @env&.to_s,
      "force_ruby_platform" => opts.delete("force_ruby_platform"),
      "gemfile" => @gemfile,
      "options" => opts,
    )
  end

  def source(source, *args, &blk)
    opts = args.last.is_a?(Hash) ? strings(args.pop) : {}
    cooldown = opts["cooldown"]
    if cooldown && !(cooldown.is_a?(Integer) && cooldown >= 0)
      raise Error, "Expected `cooldown` to be a non-negative integer, got #{cooldown.inspect}"
    end
    if opts.key?("type")
      raise Error, "You need to pass a block to #source with :type option" unless blk
      with_source({ "type" => "plugin", "plugin" => opts.delete("type").to_s, "uri" => normalize_source(source), "options" => opts }, &blk)
    elsif blk
      with_source(rubygems_source(source, cooldown), &blk)
    elsif @global_source && remote_key(@global_source["remote"]) != remote_key(normalize_source(source))
      raise Error, "This Gemfile contains multiple global sources. Each source after the first must include a block to indicate which gems should come from that source"
    else
      @global_source = rubygems_source(source, cooldown || @global_source&.fetch("cooldown"))
    end
  end

  def git_source(name, &blk)
    raise Error, "You need to pass a block to #git_source" unless blk
    if RESERVED.include?(name.to_s)
      raise Error, "You cannot use #{name} as a git source. It is a reserved key. Reserved keys are: #{RESERVED.join(", ")}"
    end
    @git_sources[name.to_s] = blk
  end

  def git(uri, options = {}, &blk)
    unless blk
      raise Error, "You can no longer specify a git source by itself. Instead, \n" \
        "either use the :git option on a gem, or specify the gems that \n" \
        "bundler should find in the git source by passing a block to \n" \
        "the git method, like: \n\n  git 'git://github.com/rails/rails.git' do\n    gem 'rails'\n  end"
    end
    with_source(git_source_of(uri, strings(options)), &blk)
  end

  def github(repo, options = {}, &blk)
    raise Error, "GitHub sources require a block" unless blk
    expanded = @git_sources["github"].call(repo)
    options = strings(options)
    options = options.merge(strings(expanded)) if expanded.is_a?(Hash)
    with_source(git_source_of(expanded.is_a?(Hash) ? expanded["git"] : expanded, options), &blk)
  end

  def path(path, options = {}, &blk)
    source = path_source_of(path, strings(options))
    return with_source(source, &blk) if blk
    @global_path = true
    source
  end

  def group(*args)
    opts = args.last.is_a?(Hash) ? strings(args.pop) : {}
    invalid = opts.keys - ["optional"]
    unless invalid.empty?
      raise Error, "You passed #{invalid.map { |k| ":#{k}" }.join(", ")} as #{invalid.size > 1 ? "options" : "an option"} " \
        "for group #{args.map { |g| ":#{g}" }.join(", ")}, but #{invalid.size > 1 ? "they are" : "it is"} invalid. Valid options are: optional."
    end
    @groups.concat(args)
    @optional_groups.concat(args - @optional_groups) if opts["optional"]
    yield
  ensure
    args.each { @groups.pop }
  end

  def platforms(*platforms)
    @platforms.concat(platforms)
    yield
  ensure
    platforms.each { @platforms.pop }
  end
  alias_method :platform, :platforms

  def install_if(*conditions)
    @install_if.concat(conditions)
    yield
  ensure
    conditions.each { @install_if.pop }
  end

  def env(name)
    outer = @env
    @env = name
    yield
  ensure
    @env = outer
  end

  # Bundler::Dsl#plugin passes plugins on to Bundler's plugin DSL; installing plugins is not Spinel's.
  def plugin(*); end

  def ruby(*versions)
    opts = versions.last.is_a?(Hash) ? versions.pop : nil
    versions.flatten!
    patchlevel = engine = engine_version = nil
    if opts
      patchlevel = opts[:patchlevel]
      engine = opts[:engine]
      engine_version = opts[:engine_version]
      raise Error, "Please define :engine_version" if engine && engine_version.nil?
      raise Error, "Please define :engine" if engine_version && engine.nil?
      if opts[:file]
        raise Error, "Do not pass version argument when using :file option" unless versions.empty?
        versions << ruby_file(opts[:file])
      end
      if engine == "ruby" && engine_version && versions != Array(engine_version)
        raise Error, "ruby_version must match the :engine_version for MRI"
      end
    end
    versions = versions.map do |v|
      m = REQUIREMENT.match(v.to_s.tr("-", ".")) or raise Error, "#{v} is not a valid requirement on the Ruby version"
      m[1].nil? || m[1] == "=" ? m[2] : "#{m[1]} #{m[2]}"
    end
    @ruby = {
      "versions" => versions,
      "engine" => engine&.to_s || "ruby",
      "engine_versions" => engine_version ? Array(engine_version).map(&:to_s) : versions,
      "patchlevel" => patchlevel || (versions.first.to_s.match?(/[a-zA-Z]/) ? "-1" : nil),
    }
  end

  def gemspec(opts = nil)
    opts ||= {}
    path = (opts[:path] || ".").to_s
    dir = File.expand_path(path, gemfile_root)
    files = Dir.glob("{,*}.gemspec", base: dir).sort.map { |file| File.join(dir, file) }
    raise Error, "There are no gemspecs at #{dir}" if files.empty?
    if files.size > 1 && !opts[:name]
      raise Error, "There are multiple gemspecs at #{dir}. Please use the :name option to specify which one should be used"
    end
    @gemspecs << {
      "path" => path,
      "root" => gemfile_root,
      "files" => files,
      "name" => opts[:name]&.to_s,
      "glob" => opts[:glob],
      "development_group" => (opts[:development_group] || :development).to_s,
      "gemfile" => @gemfile,
    }
  end

  def method_missing(name, *)
    raise Error, "Undefined local variable or method `#{name}' for Gemfile"
  end

  def to_h
    if @global_path
      raise Error, "You can no longer specify a path source by itself. Instead, \n" \
        "either use the :path option on a gem, or specify the gems that \n" \
        "bundler should find in the path source by passing a block to \n" \
        "the path method, like: \n\n    path 'dir/containing/rails' do\n      gem 'rails'\n    end"
    end
    {
      "gemfiles" => @gemfiles,
      "source" => @global_source,
      "ruby" => @ruby,
      "optional_groups" => @optional_groups.map(&:to_s),
      "gemspecs" => @gemspecs,
      "dependencies" => @dependencies,
    }
  end

  private

  def strings(hash)
    hash.each_with_object({}) { |(key, value), out| out[key.to_s] = value }
  end

  def gemfile_root
    File.dirname(@gemfile)
  end

  def with_source(source)
    outer = @source
    @source = source
    yield
    source
  ensure
    @source = outer
  end

  def normalize_source(source)
    case source
    when :gemcutter, :rubygems, :rubyforge
      raise Error, "The source :#{source} is disallowed because HTTP requests are insecure.\n" \
        "Please change your source to 'https://rubygems.org' if possible, or 'http://rubygems.org' if not."
    when String then source
    else raise Error, "Unknown source '#{source}'"
    end
  end

  # Bundler compares remotes with a trailing slash.
  def remote_key(remote)
    remote.end_with?("/") ? remote : "#{remote}/"
  end

  def rubygems_source(remote, cooldown)
    { "type" => "rubygems", "remote" => normalize_source(remote), "cooldown" => cooldown }
  end

  def git_source_of(uri, opts)
    { "type" => "git", "uri" => uri.to_s, "branch" => opts["branch"], "ref" => opts["ref"],
      "tag" => opts["tag"], "submodules" => opts["submodules"], "glob" => opts["glob"] }
  end

  def path_source_of(path, opts)
    { "type" => "path", "path" => path.to_s, "root" => gemfile_root, "glob" => opts["glob"] }
  end

  def ruby_file(name)
    content = File.read(File.join(gemfile_root, name.to_s))
    m = /^ruby[\s-]*(?:=\s*)?(?:"([^"]+)"|'([^']+)'|([^\s#"']+))/.match(content)
    m ? m[1] || m[2] || m[3] : content.strip
  rescue Errno::ENOENT
    raise Error, "Could not find version file #{name}"
  end

  # Gem::Requirement#as_list: each `op version`, `=` spelled out, `-` as `.pre.`.
  def requirements(args, name)
    list = args.flatten.compact.uniq.map do |req|
      m = REQUIREMENT.match(req.to_s) or raise Error, "Illformed requirement [#{req.inspect}] for gem '#{name}'"
      "#{m[1] || "="} #{m[2].gsub("-", ".pre.")}"
    end
    list.empty? ? [">= 0"] : list
  end

  def describe(source)
    case source&.fetch("type")
    when nil then "an unspecified source"
    when "rubygems" then "rubygems repository #{remote_key(source["remote"])}"
    when "path" then "source at `#{source["path"]}`"
    else source["uri"]
    end
  end

  def add_dependency(dep)
    current = @dependencies.find { |d| d["name"] == dep["name"] }
    if current
      shown = ->(d) { "#{d["name"]} (#{d["requirements"].join(", ")})" }
      # ponytail: compares requirement strings, where Gem::Requirement#== compares
      # versions, so `= 1` and `= 1.0` count as different here.
      if current["requirements"].sort != dep["requirements"].sort
        raise Error, "You cannot specify the same gem twice with different version requirements.\n" \
          "You specified: #{shown.(current)} and #{shown.(dep)}"
      end
      if current["source"] != dep["source"]
        raise Error, "You cannot specify the same gem twice coming from different sources.\n" \
          "You specified that #{shown.(dep)} should come from #{describe(current["source"])} and #{describe(dep["source"])}\n"
      end
      warn "Your Gemfile lists the gem #{shown.(current)} more than once.\n" \
        "You should probably keep only one of them.\n" \
        "Remove any duplicate entries and specify the gem only once.\n" \
        "While it's not a problem now, it could cause errors if you change the version of one of them later."
    end
    @dependencies << dep
  end
end

gemfile = ENV["BUNDLE_GEMFILE"]
if gemfile.nil? || gemfile.empty?
  dir = Dir.pwd
  loop do
    gemfile = %w[gems.rb Gemfile].map { |name| File.join(dir, name) }.find { |file| File.file?(file) }
    break if gemfile || File.dirname(dir) == dir
    dir = File.dirname(dir)
  end
end
unless gemfile
  warn "Could not locate Gemfile"
  exit 1
end

dsl = GemfileEval.new
begin
  dsl.eval_gemfile(gemfile)
  result = dsl.to_h
rescue StandardError, ScriptError => e
  where = e.backtrace.to_a.find { |line| dsl.gemfiles.any? { |file| line.start_with?("#{file}:") } }
  file = where ? dsl.gemfiles.find { |f| where.start_with?("#{f}:") } : File.expand_path(gemfile)
  warn "There was an error parsing `#{File.basename(file)}`: #{e.message}"
  warn "  from #{where[/\A.*?:\d+/]}" if where
  exit 1
end
puts JSON.pretty_generate(result)
