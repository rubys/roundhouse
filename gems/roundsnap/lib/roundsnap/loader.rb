# frozen_string_literal: true

require "json"
require "monitor"

module Roundsnap
  # Loads ISeq binaries by logical manifest key.
  #
  # Resolve only logical keys and exact emitted paths. Original source
  # locations live in a sidecar and never participate in require resolution.
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
      private :mark_hooked!
    end

    attr_reader :root, :manifest

    def initialize(root:)
      @root = File.expand_path(root)
      path = File.join(@root, "manifest.json")
      raise Errno::ENOENT, path unless File.file?(path)

      @manifest = JSON.parse(File.read(path))
      raise LoadError, "roundsnap: unsupported manifest version; rebuild" unless @manifest["version"] == 2

      @units = @manifest.fetch("units")
      @compile_root = @manifest.fetch("compile_root")
      @paths = {}
      @units.each do |key, entry|
        @paths[File.expand_path(entry.fetch("file"), @compile_root)] = key
        @paths[File.expand_path("#{key}.rb", @root)] = key
      end
      @loaded = {}
      @locks = @units.to_h { |key, _| [key, Monitor.new] }
      validate_manifest!
    end

    def install!
      self.class.current = self
      unless self.class.hooked?
        Kernel.prepend(RequireHook)
        self.class.send(:mark_hooked!)
      end
      self
    end

    def resolve_key(name)
      return nil if name.nil?

      n = name.to_s
      return n if @units.key?(n)
      return @paths[n] if @paths.key?(n)
      return @paths["#{n}.rb"] if @paths.key?("#{n}.rb")

      bare = n.sub(/\A\.\//, "").sub(/\.rb\z/, "")
      return bare if @units.key?(bare)

      nil
    end

    # No bare-name fallback: a gem's require_relative "main" must not
    # steal an application unit just because the process hook is installed.
    def resolve_relative(name, caller_path)
      return nil if caller_path.nil? || caller_path.empty?

      resolve_key(relative_path(name, caller_path))
    end

    def relative_path(name, caller_path)
      caller_file = File.expand_path(caller_path, @compile_root)
      if @paths.key?(caller_file) && caller_file.start_with?("#{@compile_root}/")
        caller_file = File.join(@root, caller_file.delete_prefix("#{@compile_root}/"))
      end
      File.expand_path(name.to_s, File.dirname(caller_file))
    end

    # Explicit presentation API: native backtraces, Coverage and profiler
    # locations remain emitted file:line; callers can format source frames
    # without mutating exceptions or claiming native per-method mapping.
    def format_backtrace(backtrace)
      Array(backtrace).map do |frame|
        match = /\A(.+):(-?\d+)(:.*)?\z/.match(frame)
        key = match && @paths[match[1]]
        entry = key && @units[key]
        # The sidecar indexes source text from 1, whereas MRI honors the
        # caller's first_lineno (including zero and negative offsets).
        line = entry && (Integer(match[2]) - entry.fetch("first_lineno", 1) + 1)
        location = entry && entry.fetch("source_map", {})[line.to_s]
        location ? "#{location.fetch('file')}:#{location.fetch('line')}#{match[3]}" : frame
      end
    end

    def require(key)
      key = key.to_s
      entry = @units[key]
      raise LoadError, "roundsnap: unknown key #{key.inspect}" unless entry

      @locks.fetch(key).synchronize do
        return false if @loaded[key]

        built = @manifest["ruby_description"]
        if built.nil? || built.to_s.empty?
          raise LoadError, "roundsnap: manifest missing ruby_description (rebuild required)"
        end
        # YJIT is a runtime JIT flag; it does not change ISeq binary layout,
        # but it does change RUBY_DESCRIPTION. Normalize that runtime flag.
        if normalize_ruby_description(built) != normalize_ruby_description(RUBY_DESCRIPTION)
          raise LoadError,
                "roundsnap: ISeq built for #{built.inspect}, " \
                "running #{RUBY_DESCRIPTION.inspect}"
        end
        built_opt = @manifest["compile_option"]
        if !built_opt.nil? && built_opt != Compiler.compile_option_fingerprint
          raise LoadError,
                "roundsnap: ISeq compile_option mismatch " \
                "(built #{built_opt.inspect}, running " \
                "#{Compiler.compile_option_fingerprint.inspect}); rebuild"
        end

        path = safe_iseq_path(entry.fetch("iseq"))
        binary = File.binread(path)
        begin
          iseq = RubyVM::InstructionSequence.load_from_binary(binary)
        rescue RuntimeError => e
          # Bootsnap rejects "broken binary format" and regenerates; we
          # have no source at deploy time, so ask for a rebuild.
          if e.message.include?("broken binary")
            raise LoadError,
                  "roundsnap: broken ISeq for #{key.inspect} (#{path}); " \
                  "rebuild with matching Ruby / compile_option"
          end
          raise
        end
        # Reentrant per-unit locks let circular requires on this thread
        # short-circuit, while other threads wait for initialization.
        # Unrelated units can load concurrently, including from a thread
        # started (and joined) by a unit's own top-level code.
        @loaded[key] = true
        completed = false
        begin
          iseq.eval
          completed = true
        ensure
          @loaded.delete(key) unless completed
        end
        true
      end
    end

    def boot!(entry_key = @manifest["entry"])
      install!
      # Evaluate only the entry and its actual requires, not every file in
      # the manifest. Conditional/lazy files must remain conditional/lazy.
      require(entry_key) if entry_key
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
        loc = caller_locations(1, 1)&.first
        loader = Roundsnap::Loader.current
        if loader
          key = loader.resolve_relative(name, loc&.absolute_path || loc&.path)
          return loader.require(key) if key
        end
        # Do not call super: with Kernel.prepend, super's require_relative
        # resolves against THIS method's path, not the real caller
        # (breaks stdlib openssl → openssl/bn). Expand ourselves.
        base = loc&.absolute_path || loc&.path
        raise LoadError, "require_relative: cannot infer base path for #{name.inspect}" if base.nil? || base.empty?

        path = loader ? loader.relative_path(name, base) : File.expand_path(name.to_s, File.dirname(base))
        require path
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
      Compiler.sanitize_key!(rel)
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
