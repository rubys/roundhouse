# frozen_string_literal: true

# Host-only experimental input. A manifest is executable Ruby, not configuration
# safe to load from an untrusted repository. Never invoked by check or the LSP.
require "optparse"
require "fileutils"
require "json"
require "digest"

begin
  output = nil
  trusted = false
  parser = OptionParser.new do |o|
    o.banner = "Usage: bin/rh materialize --trust-boot MANIFEST.rb -o NEW_APP"
    o.separator "Experimental: execute trusted Ruby and export a declared instance-method cut."
    o.on("--trust-boot", "Allow executing the manifest (not a sandbox)") { trusted = true }
    o.on("-o", "--output DIR", "New source project; never overwrite existing paths") { |v| output = v }
    o.on("-h", "--help", "Show help without loading Prism or executing Ruby") { puts o; exit 0 }
  end
  parser.parse!(ARGV)
  raise ArgumentError, parser.banner unless trusted && output && ARGV.length == 1
  manifest = File.expand_path(ARGV.first)
  output = File.expand_path(output)
  raise ArgumentError, "manifest is not a file: #{manifest}" unless File.file?(manifest)
  raise ArgumentError, "output already exists: #{output}" if File.exist?(output) || File.symlink?(output)
  raise ArgumentError, "output parent must exist" unless File.directory?(File.dirname(output))

  require_relative "capture"
  load manifest
  unless NativeDefineMethodObserver.failures.empty?
    raise BootToCore::Unsupported, "observer collector failed: #{NativeDefineMethodObserver.failures.map { |_, _, error| error.class.name }.uniq.join(', ')}"
  end
  spec = BootToCore.input_spec
  raise ArgumentError, "manifest must call BootToCore.input(roots:, signatures:)" unless spec
  roots = spec.fetch(:roots)
  unless roots.is_a?(Hash) && !roots.empty? && roots.values.all? { |names| names.is_a?(Array) && !names.empty? }
    raise ArgumentError, "roots must be a nonempty { Class/Module => [public_method_names] }"
  end
  core, inventory, cells = BootToCore.export(roots.keys, roots: roots)
  signatures = spec.fetch(:signatures).map do |path|
    path = File.expand_path(path, File.dirname(manifest))
    [path, File.read(path)]
  end
  report = {
    scope: "experimental declared instance-method cut; not a whole-app snapshot or inferred ABI",
    ruby: RUBY_DESCRIPTION, prism: Prism::VERSION,
    manifest: manifest, manifest_sha256: Digest::SHA256.file(manifest).hexdigest,
    exporter_sha256: Digest::SHA256.file(File.join(__dir__, "capture.rb")).hexdigest,
    gems: Gem.loaded_specs.transform_values { |gem| gem.version.to_s },
    roots: roots.to_h { |owner, names| [owner.name, names] },
    methods: inventory, cells: cells, core_sha256: Digest::SHA256.hexdigest(core),
    omitted_initializers: BootToCore.omitted_initializers,
    opaque_definitions: BootToCore.records.values.count { |record| record.reason },
    observer_failures: NativeDefineMethodObserver.failures.map { |_, name, error| { method: name, kind: error.class.name } },
    scalar_constants: BootToCore.events.select { |event| event[:kind] == "scalar_constant" },
    immutable_constants: BootToCore.events.select { |event| event[:kind] == "immutable_constant" },
    signatures: signatures.map { |path, text| { path: path, sha256: Digest::SHA256.hexdigest(text) } }
  }
  # Exclusive creation also catches a competing writer after the pre-boot check.
  # Boot/export failures leave no output project. Boot's own effects are not undone.
  Dir.mkdir(output)
  FileUtils.mkdir_p(File.join(output, "app"))
  FileUtils.mkdir_p(File.join(output, "lib"))
  File.write(File.join(output, "lib/core.rb"), core)
  unless signatures.empty?
    FileUtils.mkdir_p(File.join(output, "sig"))
    signatures.each_with_index { |(_, text), i| File.write(File.join(output, "sig/input#{i}.rbs"), text) }
  end
  File.write(File.join(output, "materialization.json"), JSON.pretty_generate(report) + "\n")
  puts "Materialized #{inventory.length} methods, #{cells} cells into #{output}"
  puts "Next: roundhouse check --strict #{output} (types come from ordinary analysis, not boot observations)"
rescue StandardError, LoadError, SyntaxError => error
  warn "rh materialize: #{error.class}: #{error.message}"
  exit 1
end
