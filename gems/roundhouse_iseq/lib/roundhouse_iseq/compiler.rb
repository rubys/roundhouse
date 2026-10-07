# frozen_string_literal: true

require "digest"
require "json"
require "fileutils"

module RoundhouseIseq
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

    def compile!(units:, out_dir:)
      raise ArgumentError, "units must be an Array" unless units.is_a?(Array)
      raise "roundhouse_iseq requires MRI (RUBY_ENGINE=ruby)" unless RUBY_ENGINE == "ruby"

      out_dir = File.expand_path(out_dir)
      iseq_dir = File.join(out_dir, "iseq")
      FileUtils.mkdir_p(iseq_dir)

      entries = {}
      units.each do |raw|
        unit = stringify_keys(raw)
        key = unit.fetch("key")
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
        "entry" => units.empty? ? nil : stringify_keys(units.first).fetch("key"),
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
