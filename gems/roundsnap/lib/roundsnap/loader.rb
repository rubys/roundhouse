# frozen_string_literal: true

require "json"

module Roundsnap
  # Loads ISeq binaries by logical manifest key.
  #
  # Require is by key, not filesystem path, so the ISeq's stored `file`
  # (original app path) is free for __FILE__ and backtraces without
  # breaking resolution.
  class Loader
    @current = nil
    @hooked = false

    class << self
      attr_accessor :current

      def install!(root:)
        new(root: root).tap(&:install!)
      end

      def hooked?
        @hooked
      end

      def mark_hooked!
        @hooked = true
      end
    end

    attr_reader :root, :manifest

    def initialize(root:)
      @root = File.expand_path(root)
      path = File.join(@root, "manifest.json")
      raise Errno::ENOENT, path unless File.file?(path)

      @manifest = JSON.parse(File.read(path))
      @units = @manifest.fetch("units")
      @loaded = {}
      validate_manifest!
    end

    def install!
      self.class.current = self
      unless self.class.hooked?
        Kernel.prepend(RequireHook)
        self.class.mark_hooked!
      end
      self
    end

    def resolve_key(name)
      return nil if name.nil?

      n = name.to_s
      return n if @units.key?(n)

      bare = n.sub(/\A\.\//, "").sub(/\.rb\z/, "")
      return bare if @units.key?(bare)

      # Original-path ISeqs are labeled `fixture/app/...`; require_relative
      # joins against that and would miss emit keys `app/...`.
      if (idx = bare.index("/app/"))
        tail = bare[(idx + 1)..]
        return tail if @units.key?(tail)
      end
      if (idx = bare.index("/runtime/"))
        tail = bare[(idx + 1)..]
        return tail if @units.key?(tail)
      end
      %w[app/ runtime/].each do |prefix|
        return bare if bare.start_with?(prefix) && @units.key?(bare)
      end

      nil
    end

    # Resolve a require_relative name against the caller's ISeq path
    # (which may be an original app path that is not on disk).
    def resolve_relative(name, caller_path)
      return resolve_key(name) if caller_path.nil? || caller_path.empty?

      base = File.dirname(caller_path)
      joined = File.join(base, name.to_s.sub(/\.rb\z/, ""))
      # Collapse .. and . without requiring the path to exist.
      parts = []
      joined.split("/").each do |p|
        next if p.empty? || p == "."
        if p == ".."
          parts.pop unless parts.empty?
        else
          parts << p
        end
      end
      candidate = parts.join("/")
      resolve_key(candidate) || resolve_key(name)
    end

    def require(key)
      key = key.to_s
      return false if @loaded[key]

      entry = @units[key]
      raise LoadError, "roundsnap: unknown key #{key.inspect}" unless entry

      built = @manifest["ruby_description"]
      if built.nil? || built.to_s.empty?
        raise LoadError, "roundsnap: manifest missing ruby_description (rebuild required)"
      end
      # YJIT is a runtime JIT flag; it does not change ISeq binary layout, but
      # it does change RUBY_DESCRIPTION ("+YJIT"). Normalize so compile-without-
      # YJIT / run-with-YJIT (the campfire bench shape) still loads.
      if normalize_ruby_description(built) != normalize_ruby_description(RUBY_DESCRIPTION)
        raise LoadError,
              "roundsnap: ISeq built for #{built.inspect}, " \
              "running #{RUBY_DESCRIPTION.inspect}"
      end

      path = safe_iseq_path(entry.fetch("iseq"))
      binary = File.binread(path)
      iseq = RubyVM::InstructionSequence.load_from_binary(binary)
      # Reserve before eval so circular require_relative (common in Rails
      # model trees) short-circuits like MRI's $LOADED_FEATURES. Clear the
      # reservation on failure so a failed unit can be retried.
      @loaded[key] = true
      begin
        iseq.eval
      rescue StandardError
        @loaded.delete(key)
        raise
      end
      true
    end

    def boot!(entry_key = nil)
      install!
      # Load every unit in manifest order (boot-chain first). An explicit
      # entry_key is loaded too when present; default entry is informational.
      @units.each_key { |k| require(k) }
      if entry_key
        require(entry_key)
      elsif (key = @manifest["entry"])
        require(key) unless @loaded[key.to_s]
      end
      true
    end

    def loaded?(key)
      !!@loaded[key.to_s]
    end

    module RequireHook
      def require(name)
        loader = Roundsnap::Loader.current
        if loader
          key = loader.resolve_key(name)
          return loader.require(key) if key
        end
        super
      end

      def require_relative(name)
        loc = RequireHook.outside_gem_caller
        loader = Roundsnap::Loader.current
        if loader
          key = loader.resolve_relative(name, loc&.path)
          return loader.require(key) if key
        end
        # Do not call super: with Kernel.prepend, super's require_relative
        # resolves against THIS method's path, not the real caller
        # (breaks stdlib openssl → openssl/bn). Expand ourselves.
        base = loc&.absolute_path || loc&.path
        raise LoadError, "require_relative: cannot infer base path for #{name.inspect}" if base.nil? || base.empty?

        path = File.expand_path(name.to_s, File.dirname(base))
        begin
          require path
        rescue LoadError
          require "#{path}.rb"
        end
      end

      # Gem lib root (`…/lib`), not a `/roundsnap/` substring — emit trees
      # under `/tmp/campfire-roundsnap/` (or an app named roundsnap) must
      # still count as outside callers for require_relative.
      GEM_LIB = File.expand_path("..", __dir__).freeze

      def self.outside_gem_caller
        prefix = GEM_LIB + File::SEPARATOR
        caller_locations(2, 32)&.find do |l|
          raw = l.absolute_path || l.path
          next false if raw.nil? || raw.empty?

          path = File.expand_path(raw)
          !path.start_with?(prefix) && path != File.join(GEM_LIB, "roundsnap.rb")
        end
      end
    end

    def self.normalize_ruby_description(desc)
      desc.to_s.gsub(/ \+YJIT\b/, "")
    end

    def normalize_ruby_description(desc)
      self.class.normalize_ruby_description(desc)
    end

    private

    def validate_manifest!
      @units.each do |key, entry|
        Compiler.sanitize_key!(key)
        rel = entry.fetch("iseq")
        safe_iseq_path(rel)
      end
    end

    def safe_iseq_path(rel)
      rel = rel.to_s
      raise LoadError, "roundsnap: absolute iseq path #{rel.inspect}" if rel.start_with?("/", "\\")
      parts = rel.split(%r{[/\\]})
      if parts.any? { |p| p.empty? || p == "." || p == ".." }
        raise LoadError, "roundsnap: unsafe iseq path #{rel.inspect}"
      end
      unless rel.start_with?("iseq/")
        raise LoadError, "roundsnap: iseq path must be under iseq/: #{rel.inspect}"
      end
      path = File.expand_path(rel, @root)
      root_prefix = @root.end_with?("/") ? @root : "#{@root}/"
      unless path == @root || path.start_with?(root_prefix)
        raise LoadError, "roundsnap: iseq path escapes root: #{rel.inspect}"
      end
      path
    end
  end
end
