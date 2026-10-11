# frozen_string_literal: true

# Installs/fetches nothing. Use a disposable exact pinned app and frozen bundle.
require "json"
require "open3"
require "fileutils"
require "digest"
require "rbconfig"

raise "usage: ruby run.rb APP COMPILER OBSERVER NEW_OUTPUT" unless ARGV.length == 4
app, compiler, observer, output = ARGV.map { |path| File.expand_path(path) }
raise "output exists" if File.exist?(output)
binary = File.join(compiler, "target/debug/roundhouse")
sha = ->(path) { Digest::SHA256.file(path).hexdigest }
source_paths = Dir[File.join(app, "{app,lib,extras,config,db}/**/*")].select { |path| File.file?(path) } +
  %w[Gemfile Gemfile.lock].map { |path| File.join(app, path) }
identity = -> { source_paths.sort.to_h { |path| [path.delete_prefix(app + "/"), sha.call(path)] } }
before = identity.call
tree_sha = Digest::SHA256.hexdigest(before.sort.map { |path, hash| path + "\0" + hash }.join("\n"))
raise "not the repository-pinned unmodified Lobsters fixture" unless tree_sha == "a1d3870d50ddac0db9017c80cdc747fc0da5f17c63b0825d645e0624727aa203"
head, status = Open3.capture2("git", "-C", compiler, "rev-parse", "HEAD")
raise "cannot identify compiler" unless status.success?
snapshot = ENV["LOBSTERS_GENERIC_SNAPSHOT"]
snapshot_files = {}
if snapshot
  snapshot = File.expand_path(snapshot)
  raise "wrong round4 snapshot" unless sha.call(snapshot) == "cdf501542d7b6a386cabedc059975d672187bd4b2bca234c9f0b5b4e1ec35f28"
  members, status = Open3.capture2("tar", "-tzf", snapshot)
  raise "cannot inspect snapshot" unless status.success?
  members.lines.map(&:strip).each do |path|
    bytes, status = Open3.capture2("tar", "-xOf", snapshot, path)
    raise "snapshot content mismatch: #{path}" unless status.success? && sha.call(File.join(compiler, path)) == Digest::SHA256.hexdigest(bytes)
    snapshot_files[path] = sha.call(File.join(compiler, path))
  end
  raise "wrong snapshot member count" unless snapshot_files.length == 22
  raise "round4 compiler/binary baseline changed" unless head.strip == "4480841589a8fedca066822f6730bc3959abd316" && sha.call(binary) == "7c8a5638b91cb08fba78044720915310e5899ad57db7a0eb6035d5e689af392e"
end
controlled = %w[src runtime Cargo.toml Cargo.lock build.rs]
controlled += %w[tools/boot-to-core/capture.rb tools/boot-to-core/materialize.rb tools/native-observer] unless snapshot
diff, status = Open3.capture2("git", "-C", compiler, "diff", "HEAD", "--", *controlled)
raise "compiler/exporter differs from the declared control" unless status.success? && diff.empty?
FileUtils.mkdir_p(output)
report = { fixture_pin: "ruby-bench d771f81f1ce9db51376e03ca7b8e6a83160556d4",
  compiler_commit: head.strip, binary_sha256: sha.call(binary),
  generic_snapshot_sha256: snapshot && sha.call(snapshot), generic_snapshot_files: snapshot_files,
  exporter_sha256: sha.call(File.join(compiler, "tools/boot-to-core/capture.rb")),
  observer_sha256: sha.call(File.join(observer, "native_define_method_observer.so")),
  source_sha256: before, source_tree_sha256: tree_sha,
  probe_sha256: Dir[File.join(__dir__, "*.{rb,rbs}")].sort.to_h { |path| [File.basename(path), sha.call(path)] },
  commands: [], lanes: {} }
environment = { "HOME" => ENV.fetch("HOME"), "PATH" => ENV.fetch("PATH"),
  "BUNDLE_GEMFILE" => File.join(app, "Gemfile"), "BUNDLE_FROZEN" => "true",
  "BUNDLE_PATH" => ENV.fetch("BUNDLE_PATH"), "BUNDLE_WITHOUT" => "development:test",
  "RAILS_ENV" => "production", "RAILS_LOG_LEVEL" => "error", "RUBYLIB" => observer,
  "SECRET_KEY_BASE" => "disposable-lobsters-contract-not-a-production-secret",
  "LOBSTERS_APP" => app }
# A fresh process gets the fixture's real SQLite :memory: configuration; no
# production credentials or network. No schema file is patched or regenerated.
run = ->(name, argv, lane = nil) do
  env = environment.merge(lane ? { "LOBSTERS_LANE" => lane } : {})
  command = ["sudo", "unshare", "-n", "--", "env", "-i", *env.map { |key, value| "#{key}=#{value}" },
    "timeout", "--kill-after=10", "120", *argv]
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  stdout, stderr, status = Open3.capture3(*command, chdir: app)
  File.write(File.join(output, "#{name}.stdout"), stdout)
  File.write(File.join(output, "#{name}.stderr"), stderr)
  report[:commands] << { name: name, argv: argv, exit: status.exitstatus, signal: status.termsig,
    seconds: (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started).round(3) }
  [stdout, stderr, status.success?]
end
observations = ->(stdout) {
  line = stdout.lines.find { |entry| entry.start_with?("LOBSTERS_CONTRACT=") }
  raise "contract summary missing" unless line
  JSON.parse(line.delete_prefix("LOBSTERS_CONTRACT="))
}
contract = File.join(__dir__, "contract.rb")
begin
  boot = 'require "./config/environment"; require "json"; puts "LOBSTERS_BOOT=" + JSON.generate(ruby: RUBY_DESCRIPTION, rails: Rails.version, gems: Gem.loaded_specs.transform_values { |gem| gem.version.to_s })'
  stdout, stderr, ok = run.call("boot", [RbConfig.ruby, "-e", boot])
  raise "original app boot failed; see boot.stderr" unless ok
  report[:boot] = JSON.parse(stdout.lines.find { |line| line.start_with?("LOBSTERS_BOOT=") }.delete_prefix("LOBSTERS_BOOT="))
  %w[search short_id namespace pagination paginator inherited delegate dynamic full_capture].each do |lane|
    result = report[:lanes][lane] = {}
    stdout, stderr, ok = run.call("#{lane}_original", [RbConfig.ruby, contract, lane, "original"])
    result[:original] = ok ? observations.call(stdout) : { failure: stderr.lines.last(12).join }
    core = File.join(output, "#{lane}-core")
    # Prism is the host observer dependency, not an added app Gemfile entry.
    # Set up the frozen bundle before materialize loads json/optparse; loading
    # Bundler 4 mid-script otherwise relaunches or conflicts with host json.
    stdout, stderr, ok = run.call("#{lane}_materialize", [RbConfig.ruby, "-rprism", "-rbundler/setup",
      File.join(compiler, "tools/boot-to-core/materialize.rb"), "--trust-boot", File.join(__dir__, "input.rb"), "-o", core], lane)
    result[:materialized] = ok
    line = stderr.lines.find { |entry| entry.start_with?("LOBSTERS_OBSERVATION=") }
    result[:observation] = JSON.parse(line.delete_prefix("LOBSTERS_OBSERVATION=")) if line
    line = stderr.lines.find { |entry| entry.start_with?("LOBSTERS_ROOTS=") }
    result[:roots] = JSON.parse(line.delete_prefix("LOBSTERS_ROOTS=")) if line
    unless ok
      result[:refusal] = stderr.lines.find { |entry| entry.start_with?("rh materialize:") }&.strip || stderr.lines.last(12).join
      result[:no_output_project] = !File.exist?(core)
      next
    end
    result[:inventory] = JSON.parse(File.read(File.join(core, "materialization.json")))
    stdout, stderr, ok = run.call("#{lane}_strict", [binary, "check", "--strict", core])
    result[:strict] = { passed: ok, stdout: stdout, stderr: stderr }
    stdout, stderr, ok = run.call("#{lane}_core", [RbConfig.ruby, contract, lane, File.join(core, "lib/core.rb")])
    result[:core] = ok ? observations.call(stdout) : { failure: stderr.lines.last(12).join }
    emitted = File.join(output, "#{lane}-emitted")
    stdout, stderr, ok = run.call("#{lane}_emit", [binary, "--target", "ruby", core, "-o", emitted])
    result[:emitted_project] = ok
    unless ok
      result[:emit_failure] = stderr
      next
    end
    stdout, stderr, ok = run.call("#{lane}_emitted_main", [RbConfig.ruby, "-e", "require #{File.join(emitted, 'main.rb').inspect}"])
    result[:emitted_main_boot] = { passed: ok, failure: ok ? nil : stderr.lines.first&.strip }
    # Test the actual compiler's class files, not its incomplete HTTP bootstrap
    # for a project with no controllers. No emitted body or require is patched.
    files = Dir[File.join(emitted, "app/models/**/*.rb")].sort
    raise "no emitted class files" if files.empty?
    result[:emitted_class_sha256] = files.to_h { |path| [path.delete_prefix(emitted + "/"), sha.call(path)] }
    loader = File.join(output, "#{lane}-loader.rb")
    File.write(loader, files.map { |path| "require #{path.inspect}" }.join("\n") + "\n")
    stdout, stderr, ok = run.call("#{lane}_emitted", [RbConfig.ruby, contract, lane, loader])
    result[:emitted] = ok ? observations.call(stdout) : { failure: stderr.lines.last(12).join }
    result[:three_way_equal] = [result[:original], result[:core], result[:emitted]].uniq.length == 1 && ok
    raise "emitted classes changed during execution" unless files.to_h { |path| [path.delete_prefix(emitted + "/"), sha.call(path)] } == result[:emitted_class_sha256]
  end
  report[:source_unchanged] = identity.call == before
  raise "fixture source identity changed" unless report[:source_unchanged]
  # Every materialized cut must match actual emitted execution, not just Core
  # or strict checking. Refusals must leave no project and retain real boot.
  positives = %w[search short_id full_capture]
  positives += %w[pagination paginator] if snapshot
  report[:status] = if positives.all? { |lane|
    report[:lanes][lane][:three_way_equal] && report[:lanes][lane].dig(:strict, :passed)
  } && report[:lanes].values.all? { |lane|
    lane[:original].key?("checks") && lane.dig(:observation, "booted") && lane.dig(:observation, "observer_failures") == [] &&
      (lane[:materialized] ? lane[:three_way_equal] && lane.dig(:strict, :passed) : lane[:no_output_project])
  }
    "bounded_contracts_verified_with_boundaries"
  else
    "incomplete_or_failed_contracts"
  end
rescue StandardError => error
  report[:status] = "failed"
  report[:failure] = error.message
ensure
  File.write(File.join(output, "results.json"), JSON.pretty_generate(report) + "\n")
end
puts "#{report[:status]}: #{output}/results.json"
exit(report[:status] == "bounded_contracts_verified_with_boundaries" ? 0 : 1)
