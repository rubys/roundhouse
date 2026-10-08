# frozen_string_literal: true

require "digest"
require "json"
require "fileutils"
require "tmpdir"

module Roundsnap
  # Compiles lowered Ruby source units to MRI ISeq binaries.
  #
  # Each unit is a Hash (string or symbol keys) with:
  #   key          — logical require key (e.g. "app/models/article")
  #   source       — Ruby source string
  #   file         — path stored on the ISeq (__FILE__ / backtraces)
  #   first_lineno — optional Integer, default 1
  #
  # Uses InstructionSequence.compile (string + metadata), not compile_file,
  # so `file` need not exist on disk and can name the original app path.
  module Compiler
    module_function

    # Unit keys become relative paths under iseq/. Reject anything that
    # would escape that directory (absolute paths, `..` segments).
    def sanitize_key!(key)
      k = key.to_s
      raise ArgumentError, "roundsnap: empty unit key" if k.empty?
      if k.start_with?("/", "\\") || k.match?(/\A[A-Za-z]:/) || k.include?("\\") || k.include?("\0")
        raise ArgumentError, "roundsnap: unsafe unit key #{k.inspect}"
      end
      parts = k.split(%r{[/\\]})
      if parts.any? { |p| p.empty? || p == "." || p == ".." }
        raise ArgumentError, "roundsnap: unsafe unit key #{k.inspect}"
      end
      k
    end

    def compile!(units:, out_dir:)
      raise ArgumentError, "units must be an Array" unless units.is_a?(Array)
      raise "roundsnap requires MRI (RUBY_ENGINE=ruby)" unless RUBY_ENGINE == "ruby"

      out_dir = File.expand_path(out_dir)
      FileUtils.mkdir_p(out_dir)
      iseq_dir = File.join(out_dir, "iseq")
      # Publish a complete generation, then atomically replace the manifest.
      # Old generations stay available to already-installed loaders. Never
      # swap/delete their binaries before publishing the new manifest.
      staging = Dir.mktmpdir(".iseq.staging-", out_dir)
      generation = File.basename(staging).delete_prefix(".")
      manifest_tmp = "#{staging}.json"

      begin
        entries = {}
        # Markers become a sidecar, never a transformation of program text.
        expanded = []
        units.each do |raw|
          unit = stringify_keys(raw)
          key = unit.fetch("key")
          source = unit.fetch("source")
          if source.include?(SourceMap::MARKER_PREFIX)
            SourceMap.units_from(source, emit_key: key).each { |u| expanded << (u["mapped"] ? u : unit) }
          else
            expanded << unit
          end
        end

        expanded.each do |raw|
          unit = stringify_keys(raw)
          key = sanitize_key!(unit.fetch("key"))
          raise ArgumentError, "roundsnap: duplicate unit key #{key.inspect}" if entries.key?(key)

          source = unit.fetch("source")
          file = unit.fetch("file")
          first_lineno = Integer(unit.fetch("first_lineno", 1))
          absolute_file = File.expand_path(file, out_dir)

          iseq = RubyVM::InstructionSequence.compile(
            source,
            absolute_file,
            absolute_file,
            first_lineno,
          )
          binary = iseq.to_binary
          rel = "#{key}.iseq"
          dest = File.join(staging, rel)
          FileUtils.mkdir_p(File.dirname(dest))
          File.binwrite(dest, binary)

          entries[key] = {
            "iseq" => File.join("iseq", generation, rel),
            "file" => file,
            "first_lineno" => first_lineno,
            "mapped" => unit["mapped"] == true,
            "source_map" => unit.fetch("source_map", {}),
            "size" => source.bytesize,
            "digest" => Digest::SHA256.hexdigest(source),
          }
        end

        # Bootsnap keys ISeq caches on compile_option as well as Ruby
        # version; a mismatched option loads as "broken binary". Record
        # the option hash so the loader can fail with a clear rebuild hint.
        entry_key = nil
        unless units.empty?
          entry_key = sanitize_key!(stringify_keys(units.first).fetch("key"))
          entry_key = entries.key?(entry_key) ? entry_key : entries.keys.first
        end
        manifest = {
          "version" => 2,
          "compile_root" => out_dir,
          "ruby_description" => RUBY_DESCRIPTION,
          "compile_option" => compile_option_fingerprint,
          "entry" => entry_key,
          "units" => entries,
        }
        File.write(manifest_tmp, JSON.pretty_generate(manifest) + "\n")
        FileUtils.mkdir_p(iseq_dir)
        File.rename(staging, File.join(iseq_dir, generation))
        File.rename(manifest_tmp, File.join(out_dir, "manifest.json"))
        manifest
      ensure
        FileUtils.rm_rf(staging) if File.directory?(staging)
        FileUtils.rm_f(manifest_tmp)
      end
    end

    def stringify_keys(h)
      h.each_with_object({}) { |(k, v), out| out[k.to_s] = v }
    end
    private_class_method :stringify_keys

    # Stable, JSON-friendly fingerprint of InstructionSequence.compile_option.
    def compile_option_fingerprint
      opt = RubyVM::InstructionSequence.compile_option
      return {} unless opt.is_a?(Hash)

      opt.keys.map(&:to_s).sort.each_with_object({}) do |k, out|
        out[k] = opt[k.to_sym]
      end
    end
    module_function :compile_option_fingerprint
  end
end
