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
      FileUtils.mkdir_p(out_dir)
      iseq_dir = File.join(out_dir, "iseq")
      # Stage into a fresh tree; leave the live iseq/ alone until every
      # unit compiles. A mid-run SyntaxError must not orphan an old
      # manifest.json that still names deleted binaries.
      staging = File.join(out_dir, ".iseq.staging-#{Process.pid}-#{Thread.current.object_id}")
      FileUtils.rm_rf(staging)
      FileUtils.mkdir_p(staging)

      begin
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
          dest = File.join(staging, rel)
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

        swap_iseq_dir!(iseq_dir, staging)

        manifest = {
          "version" => 1,
          "ruby_description" => RUBY_DESCRIPTION,
          "entry" => units.empty? ? nil : sanitize_key!(stringify_keys(units.first).fetch("key")),
          "units" => entries,
        }
        File.write(File.join(out_dir, "manifest.json"), JSON.pretty_generate(manifest) + "\n")
        manifest
      ensure
        FileUtils.rm_rf(staging) if File.directory?(staging)
      end
    end

    # Replace live iseq/ with the staged tree. Same-filesystem rename so
    # readers never see a half-deleted directory.
    def swap_iseq_dir!(iseq_dir, staging)
      parent = File.dirname(iseq_dir)
      retired = File.join(parent, ".iseq.retired-#{Process.pid}-#{Thread.current.object_id}")
      FileUtils.rm_rf(retired)
      File.rename(iseq_dir, retired) if File.directory?(iseq_dir)
      File.rename(staging, iseq_dir)
      FileUtils.rm_rf(retired)
    end
    private_class_method :swap_iseq_dir!

    def stringify_keys(h)
      h.each_with_object({}) { |(k, v), out| out[k.to_s] = v }
    end
    private_class_method :stringify_keys
  end
end
