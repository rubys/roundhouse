# frozen_string_literal: true

require "digest"
require "fileutils"
require "json"
require "open3"
require "rbconfig"
require "tmpdir"
require_relative "capture"

ROOT = File.expand_path("../..", __dir__)
OUT = File.expand_path(ARGV.fetch(0))
raise "output already exists: #{OUT}" if File.exist?(OUT)
FileUtils.mkdir_p(OUT)

report = {
  ruby: RUBY_DESCRIPTION, prism: Prism::VERSION,
  compiler_head: `git -C #{ROOT} rev-parse HEAD`.strip,
  input_sha256: Dir[File.join(__dir__, "*")].select { |path| File.file?(path) }.to_h { |path| [File.basename(path), Digest::SHA256.file(path).hexdigest] },
  commands: [], rejections: []
}

def command(report, name, *argv)
  stdout, stderr, status = Open3.capture3(*argv, chdir: ROOT)
  File.write(File.join(OUT, "#{name}.stdout"), stdout)
  File.write(File.join(OUT, "#{name}.stderr"), stderr)
  report[:commands] << { name: name, argv: argv, exit: status.exitstatus, signal: status.termsig }
  [stdout, status.success?]
end

def required(report, name, *argv)
  stdout, success = command(report, name, *argv)
  raise "#{name} failed; inspect #{name}.stderr" unless success
  stdout
end

def rejection(report, name)
  yield
  raise "#{name} was incorrectly accepted"
rescue BootToCore::Unsupported => error
  report[:rejections] << { name: name, reason: error.message }
ensure
  BootToCore.active = false
end

def collect_expressions(value, counts)
  case value
  when Hash
    if value.key?("node") && value.key?("span")
      counts[:total] += 1
      type = value["ty"]
      counts[:missing] += 1 unless type
      counts[:unresolved] += 1 if type && JSON.generate(type).match?(/"kind":"(?:var|untyped)"/)
    end
    value.each_value { |child| collect_expressions(child, counts) }
  when Array
    value.each { |child| collect_expressions(child, counts) }
  end
end

begin
  bins = %w[roundhouse dump_ir].to_h do |name|
    path = File.join(ROOT, "target/debug", name)
    raise "build first: cargo build --locked --jobs 4 --bin roundhouse --bin dump_ir" unless File.executable?(path)
    [name, path]
  end
  report[:binary_sha256] = bins.transform_values { |path| Digest::SHA256.file(path).hexdigest }
  contract = File.join(__dir__, "contract.rb")
  fixture = File.join(__dir__, "fixture.rb")
  reference = JSON.parse(required(report, "reference", RbConfig.ruby, contract, fixture))

  BootToCore.active = true
  load fixture
  BootToCore.active = false
  core, inventory, cells = BootToCore.export([EvalProduct, LeftProduct, RightProduct])
  report[:export] = { methods: inventory, cells: cells }
  raise "generator escaped into output" if core.match?(/UnknownGenerator|class_eval|define_method/)
  core_path = File.join(OUT, "core.rb")
  File.write(core_path, core)
  exported = JSON.parse(required(report, "exported_core", RbConfig.ruby, contract, core_path))
  raise "exported core differs from reference" unless exported == reference
  report[:reference_contract] = reference
  report[:exported_contract] = exported

  Dir.mktmpdir("roundhouse-boot-to-core-") do |scratch|
    original = File.join(scratch, "original")
    app = File.join(scratch, "core")
    [original, app].each do |dir|
      FileUtils.mkdir_p(File.join(dir, "lib"))
      FileUtils.mkdir_p(File.join(dir, "app"))
    end
    FileUtils.cp(fixture, File.join(original, "lib", "generator.rb"))
    _, original_ok = command(report, "original_static_check", bins.fetch("roundhouse"), "check", "--strict", original)
    report[:original_static_check] = original_ok ? "accepted" : "rejected"
    File.write(File.join(app, "lib", "core.rb"), core)
    # First measure what boot extraction alone gets us: no invented signatures.
    _, no_signatures_ok = command(report, "core_without_signatures", bins.fetch("roundhouse"), "check", "--strict", app)
    report[:core_without_signatures] = no_signatures_ok ? "accepted_with_diagnostics_in_log" : "rejected"
    classes = core.scan(/^class ([A-Z][A-Za-z0-9]*)$/).flatten
    unsigned_ir = classes.map do |name|
      json = required(report, "unsigned_ir_#{name}", bins.fetch("dump_ir"), app, "--select", name, "--format", "json")
      JSON.parse(json.lines.reject { |line| line.start_with?("// ===") }.join)
    end
    unsigned_counts = { total: 0, missing: 0, unresolved: 0 }
    collect_expressions(unsigned_ir, unsigned_counts)
    report[:ir_without_input_contract] = unsigned_counts
    File.write(File.join(OUT, "unsigned_ir.json"), JSON.pretty_generate(unsigned_ir) + "\n")
    FileUtils.mkdir_p(File.join(app, "sig"))
    FileUtils.cp(File.join(__dir__, "contract.rbs"), File.join(app, "sig", "core.rbs"))
    required(report, "core_with_input_contract", bins.fetch("roundhouse"), "check", "--strict", app)

    ir = []
    classes.each do |name|
      json = required(report, "ir_#{name}", bins.fetch("dump_ir"), app, "--select", name, "--format", "json")
      ir << JSON.parse(json.lines.reject { |line| line.start_with?("// ===") }.join)
    end
    File.write(File.join(OUT, "typed_ir.json"), JSON.pretty_generate(ir) + "\n")
    counts = { total: 0, missing: 0, unresolved: 0 }
    collect_expressions(ir, counts)
    report[:ir_expressions] = counts
    report[:method_parameter_type_gaps] = ir.flat_map do |owner|
      owner.fetch("methods").filter_map do |method|
        next if method.fetch("params").empty?
        signature = method["signature"]
        if !signature || signature.fetch("params").any? { |param| JSON.generate(param.fetch("ty")).match?(/"kind":"(?:var|untyped)"/) }
          "#{owner.fetch('name')}##{method.fetch('name')}"
        end
      end
    end
    project = File.join(scratch, "ruby-output")
    required(report, "transpile_ruby", bins.fetch("roundhouse"), "--target", "ruby", app, "-o", project)
    files = Dir[File.join(project, "**", "*.rb")]
    emitted = classes.map do |name|
      matches = files.select { |path| File.read(path).match?(/^class #{Regexp.escape(name)}$/) }
      raise "expected one emitted source file for #{name}, found #{matches.length}" unless matches.length == 1
      relative = matches.first.delete_prefix(project + "/")
      destination = File.join(OUT, "emitted_files", relative)
      FileUtils.mkdir_p(File.dirname(destination))
      FileUtils.cp(matches.first, destination)
      "require_relative #{("emitted_files/" + relative).inspect}"
    end
    emitted_path = File.join(OUT, "roundhouse_core.rb")
    File.write(emitted_path, emitted.join("\n"))
    roundhouse = JSON.parse(required(report, "roundhouse_emitted", RbConfig.ruby, contract, emitted_path))
    raise "Roundhouse emission differs from reference" unless roundhouse == reference
    report[:roundhouse_contract] = roundhouse
  end

  rejection(report, "mutable_object_graph") do
    BootToCore.active = true
    Object.const_set(:ObjectCaptureProduct, Class.new)
    UnknownGenerator.closures(ObjectCaptureProduct, "prefix", {}, true)
    BootToCore.active = false
    BootToCore.export([ObjectCaptureProduct])
  end
  rejection(report, "mutable_string_snapshot") do
    BootToCore.active = true
    Object.const_set(:MutableStringProduct, Class.new)
    UnknownGenerator.closures(MutableStringProduct, "prefix".dup, 1, true)
    BootToCore.active = false
    BootToCore.export([MutableStringProduct])
  end
  rejection(report, "native_proc_without_source") do
    BootToCore.active = true
    Object.const_set(:NativeProcProduct, Class.new)
    NativeProcProduct.define_method(:native, Kernel.method(:puts).to_proc)
    BootToCore.active = false
    BootToCore.export([NativeProcProduct])
  end
  rejection(report, "late_method_generation") do
    BootToCore.active = true
    Object.const_set(:LateProduct, Class.new)
    LateProduct.class_eval("def late\n class_eval('def extra; 1; end')\nend")
    BootToCore.active = false
    BootToCore.export([LateProduct])
  end
  rejection(report, "optional_parameters") do
    BootToCore.active = true
    Object.const_set(:OptionalProduct, Class.new)
    OptionalProduct.class_eval("def optional(value = 3)\n value\nend")
    BootToCore.active = false
    BootToCore.export([OptionalProduct])
  end
  rejection(report, "uncaptured_override") do
    BootToCore.active = true
    Object.const_set(:ReplacedProduct, Class.new)
    ReplacedProduct.class_eval("def value\n 3\nend")
    BootToCore.active = false
    ReplacedProduct.class_eval("def value\n 41\nend")
    BootToCore.export([ReplacedProduct])
  end

  rails_probe = File.join(__dir__, "rails_probe.rb")
  rails_reference = JSON.parse(required(report, "rails_reference", RbConfig.ruby, rails_probe))
  rails_capture, capture_ok = command(report, "rails_capture", RbConfig.ruby, rails_probe, "--capture")
  report[:rails_probe] = { reference: rails_reference, capture: JSON.parse(rails_capture), capture_exit_zero: capture_ok }
  report[:status] = capture_ok ? "micro_passed_rails_export_unverified" : "micro_passed_rails_blocked"
  puts "PASS: #{inventory.length} generated methods, #{cells} shared cells, #{reference.fetch('checks')} assertions in each of 3 fresh Ruby processes"
  puts "IR: #{report[:ir_expressions].inspect}; #{report[:rejections].length} explicit unsupported controls"
  puts "Rails 8.1.4: #{report[:rails_probe][:capture].inspect} (not a full-app boot)"
rescue StandardError => error
  report[:status] = "failed"
  report[:failure] = { class: error.class.name, message: error.message }
  warn "FAIL: #{error.class}: #{error.message}"
ensure
  BootToCore.active = false
  File.write(File.join(OUT, "results.json"), JSON.pretty_generate(report) + "\n")
end

# A blocked or execution-unverified real-Rails lane is not an overall pass.
exit(report[:status] == "passed" ? 0 : 1)
