# frozen_string_literal: true

require "digest"
require "json"
require "fileutils"

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
      raise ArgumentError, "roundsnap: absolute unit key #{k.inspect}" if k.start_with?("/", "\\")
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
      iseq_dir = File.join(out_dir, "iseq")
      FileUtils.rm_rf(iseq_dir)
      FileUtils.mkdir_p(iseq_dir)

      entries = {}
      units.each do |raw|
        unit = stringify_keys(raw)
        key = sanitize_key!(unit.fetch("key"))
        source = unit.fetch("source")
        file = unit.fetch("file")
        first_lineno = (unit["first_lineno"] || 1).to_i
        first_lineno = 1 if first_lineno < 1

        iseq = RubyVM::InstructionSequence.compile(
          source,
          file,
          file,
          first_lineno,
        )
        binary = iseq.to_binary
        rel = "#{key}.iseq"
        dest = File.join(iseq_dir, rel)
        FileUtils.mkdir_p(File.dirname(dest))
        File.binwrite(dest, binary)

        entries[key] = {
          "iseq" => File.join("iseq", rel),
          "file" => file,
          "first_lineno" => first_lineno,
          "size" => source.bytesize,
          "digest" => Digest::SHA256.hexdigest(source),
        }
      end

      manifest = {
        "version" => 1,
        "ruby_description" => RUBY_DESCRIPTION,
        "entry" => units.empty? ? nil : sanitize_key!(stringify_keys(units.first).fetch("key")),
        "units" => entries,
      }
      File.write(File.join(out_dir, "manifest.json"), JSON.pretty_generate(manifest) + "\n")
      manifest
    end

    def stringify_keys(h)
      h.each_with_object({}) { |(k, v), out| out[k.to_s] = v }
    end
    private_class_method :stringify_keys
  end
end
