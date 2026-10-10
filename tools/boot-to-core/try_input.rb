# frozen_string_literal: true

# Bounded end-to-end experiment: original Ruby -> declared Core Ruby -> actual
# Roundhouse Ruby output. This is not a full Rails application or target matrix.
require "json"
require "fileutils"
require "open3"
require "rbconfig"
require "digest"

root = File.expand_path("../..", __dir__)
out = File.expand_path(ARGV.fetch(0))
raise "output already exists" if File.exist?(out)
FileUtils.mkdir_p(out)
report = { scope: "bounded Ruby input contracts only; no full Rails support claim", commands: [], lanes: {} }
run = ->(name, *argv) do
  stdout, stderr, status = Open3.capture3(*argv, chdir: root)
  File.write(File.join(out, "#{name}.stdout"), stdout)
  File.write(File.join(out, "#{name}.stderr"), stderr)
  report[:commands] << { name: name, argv: argv, exit: status.exitstatus }
  [stdout, status.success?]
end
required = ->(name, *argv) do
  stdout, ok = run.call(name, *argv)
  raise "#{name} failed: inspect #{name}.stderr" unless ok
  stdout
end
counts = ->(value, result) do
  case value
  when Hash
    if value.key?("node") && value.key?("span")
      result[:total] += 1
      result[:missing] += 1 unless value["ty"]
      result[:unresolved] += 1 if value["ty"] && JSON.generate(value["ty"]).match?(/"kind":"(?:var|untyped)"/)
    end
    value.each_value { |child| counts.call(child, result) }
  when Array
    value.each { |child| counts.call(child, result) }
  end
end

begin
  bins = %w[roundhouse dump_ir].to_h { |name| [name, File.join(root, "target/debug", name)] }
  report[:binary_sha256] = bins.transform_values { |path| Digest::SHA256.file(path).hexdigest }
  report[:input_sha256] = Dir[File.join(__dir__, "*")].select { |path| File.file?(path) }.to_h { |path| [File.basename(path), Digest::SHA256.file(path).hexdigest] }
  %w[micro rails].each do |lane|
    contract = File.join(__dir__, lane == "micro" ? "contract.rb" : "rails_contract.rb")
    original = File.join(__dir__, lane == "micro" ? "fixture.rb" : "rails_fixture.rb")
    manifest = File.join(__dir__, "#{lane}_input.rb")
    app = File.join(out, "#{lane}-app")
    core_args = lane == "rails" ? ["--core"] : []
    reference = JSON.parse(required.call("#{lane}_original", RbConfig.ruby, contract, original))
    required.call("#{lane}_materialize", RbConfig.ruby, File.join(root, "bin/rh"), "materialize", "--trust-boot", manifest, "-o", app)
    core = File.join(app, "lib/core.rb")
    actual = JSON.parse(required.call("#{lane}_core", RbConfig.ruby, contract, core, *core_args))
    raise "#{lane} Core differs from original" unless actual == reference
    required.call("#{lane}_check", bins.fetch("roundhouse"), "check", "--strict", app)
    names = File.read(core).scan(/^(?:class|module) ([A-Z][A-Za-z0-9]*)$/).flatten
    ir = names.map do |name|
      json = required.call("#{lane}_ir_#{name}", bins.fetch("dump_ir"), app, "--select", name, "--format", "json")
      JSON.parse(json.lines.reject { |line| line.start_with?("// ===") }.join)
    end
    typing = { total: 0, missing: 0, unresolved: 0 }
    counts.call(ir, typing)
    gaps = ir.flat_map do |owner|
      owner.fetch("methods").filter_map do |method|
        next if method.fetch("params").empty?
        signature = method["signature"]
        if !signature || signature.fetch("params").any? { |param| JSON.generate(param.fetch("ty")).match?(/"kind":"(?:var|untyped)"/) }
          "#{owner.fetch('name')}##{method.fetch('name')}"
        end
      end
    end
    project = File.join(out, "#{lane}-emitted")
    required.call("#{lane}_emit", bins.fetch("roundhouse"), "--target", "ruby", app, "-o", project)
    loader = File.join(out, "#{lane}-emitted.rb")
    files = Dir[File.join(project, "app/models/*.rb")]
    File.write(loader, files.map { |path| "require #{path.inspect}" }.join("\n") + "\n")
    emitted = JSON.parse(required.call("#{lane}_execution", RbConfig.ruby, contract, loader, *core_args))
    raise "#{lane} emitted output differs from original" unless emitted == reference
    report[:lanes][lane] = { checks_per_process: reference.fetch("checks"), original: reference, core: actual,
      emitted: emitted, expression_types: typing, parameter_signature_gaps: gaps }
  end

  # Execute real Rails callback dispatch as a reference, without claiming to
  # export that scheduler. Both valid and invalid branches must be exercised.
  required.call("rails_valid_reference", RbConfig.ruby, "-r", File.join(__dir__, "rails_fixture.rb"), "-e", <<~RUBY)
    [[2, 6, false, 52, 17, 1], [5, 8, true, 115, 19, 0]].each do |seed, weight, valid, ledger, transformed, errors|
      object = CallbackProduct.new
      object.prime(seed, weight)
      actual = [object.valid?, object.ledger, object.probe_weight, object.errors.count]
      expected = [valid, ledger, transformed, errors]
      raise "callback reference mismatch: \#{actual.inspect}" unless actual == expected
      puts actual.inspect
    end
  RUBY

  # Same Rails code generator, now with its default forwarding ABI. Capturing
  # that body is not a support claim: strict Roundhouse must still reject it.
  forwarding = File.join(out, "forwarding_input.rb")
  File.write(forwarding, <<~RUBY)
    # frozen_string_literal: true
    require #{File.join(__dir__, 'rails_fixture.rb').inspect}
    class RailsCoreProduct
      attribute_method_suffix "_forward"
      private
      def attribute_forward(attribute)
        "\#{attribute}:\#{@name}"
      end
    end
    BootToCore.capture { RailsCoreProduct.define_attribute_methods(:name) }
    BootToCore.input(roots: { RailsCoreProduct => [:name_forward] })
  RUBY
  forward_app = File.join(out, "forwarding-app")
  required.call("forward_materialize", RbConfig.ruby, File.join(root, "bin/rh"), "materialize", "--trust-boot", forwarding, "-o", forward_app)
  stdout, accepted = run.call("forward_strict_check", bins.fetch("roundhouse"), "check", "--strict", forward_app)
  raise "forwarding control unexpectedly passed" if accepted
  unless stdout.include?("forwarding") || File.read(File.join(out, "forward_strict_check.stderr")).include?("forwarding")
    raise "forwarding control failed for an unrelated reason"
  end
  report[:forwarding] = "captured, rejected by strict compiler; diagnostic retained"
  report[:status] = "bounded_input_verified"
  report[:lanes].each { |lane, result| puts "PASS #{lane}: #{result[:checks_per_process]} assertions in each of original/Core/emitted processes; types #{result[:expression_types]}; #{result[:parameter_signature_gaps].length} parameter-signature gaps" }
  puts "BOUNDARY: #{report[:forwarding]}"
rescue StandardError => error
  report[:status] = "failed"
  report[:failure] = { class: error.class.name, message: error.message }
  warn "FAIL: #{error.message}"
ensure
  File.write(File.join(out, "results.json"), JSON.pretty_generate(report) + "\n")
end
exit(report[:status] == "bounded_input_verified" ? 0 : 1)
