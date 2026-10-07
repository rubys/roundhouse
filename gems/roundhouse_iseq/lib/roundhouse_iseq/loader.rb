# frozen_string_literal: true

require "json"

module RoundhouseIseq
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
      raise LoadError, "roundhouse_iseq: unknown key #{key.inspect}" unless entry

      if @manifest["ruby_description"] && @manifest["ruby_description"] != RUBY_DESCRIPTION
        raise LoadError,
              "roundhouse_iseq: ISeq built for #{@manifest["ruby_description"].inspect}, " \
              "running #{RUBY_DESCRIPTION.inspect}"
      end

      path = File.join(@root, entry.fetch("iseq"))
      binary = File.binread(path)
      iseq = RubyVM::InstructionSequence.load_from_binary(binary)
      @loaded[key] = true
      iseq.eval
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
      @loaded[key.to_s]
    end

    module RequireHook
      def require(name)
        loader = RoundhouseIseq::Loader.current
        if loader
          key = loader.resolve_key(name)
          return loader.require(key) if key
        end
        super
      end

      def require_relative(name)
        loc = RequireHook.outside_gem_caller
        loader = RoundhouseIseq::Loader.current
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

      def self.outside_gem_caller
        caller_locations(2, 32)&.find do |l|
          path = l.path.to_s
          !path.include?("/roundhouse_iseq/") &&
            !path.end_with?("roundhouse_iseq.rb")
        end
      end
    end
  end
end
