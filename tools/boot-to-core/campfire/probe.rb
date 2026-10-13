# frozen_string_literal: true

require "json"
require "open3"
require "fileutils"
require "digest"
require "rbconfig"

# Usage: ruby probe.rb DISPOSABLE_APP BUNDLE_PATH NEW_RESULTS_DIR [COMPILER] [OBSERVER_DIR]
# Install the pinned app's bundle and db:prepare before invoking this script.
app, bundle, out = ARGV.take(3).map { |path| File.realpath(path) rescue File.expand_path(path) }
root = File.expand_path("../../..", __dir__)
compiler = File.expand_path(ARGV[3] || File.join(root, "target/debug/roundhouse"))
observer = File.realpath(ARGV[4] || "/tmp/core-observer-round3")
raise "results already exist" if File.exist?(out)
FileUtils.mkdir_p(out)
env = {
  "HOME" => Dir.home, "PATH" => "#{File.dirname(RbConfig.ruby)}:#{Dir.home}/.local/bin:/usr/bin:/bin",
  "RUBYLIB" => observer,
  "LANG" => "C.UTF-8", "BUNDLE_PATH" => bundle, "BUNDLE_WITHOUT" => "development:test",
  "RAILS_ENV" => "test", "SKIP_TELEMETRY" => "1",
  "SECRET_KEY_BASE" => "disposable-campfire-core-probe-not-production",
  "REDIS_URL" => "redis://127.0.0.1:6399",
  "DATABASE_URL" => "sqlite3:#{File.join(File.dirname(out), 'probe.sqlite3')}",
  "CORE_RUBY_APP" => app
}
report = {
  scope: "Campfire application-specific research; real boot, bounded cuts, no whole-app support claim",
  roundhouse_commit: "83b6b1458adde2b2db728507fcabc0c2c0557612",
  campfire_commit: "66883b6fb1eda402245247592e0af5f54105c5eb",
  compiler_sha256: Digest::SHA256.file(compiler).hexdigest,
  snapshot_sha256: "cdf501542d7b6a386cabedc059975d672187bd4b2bca234c9f0b5b4e1ec35f28",
  observer_sha256: Digest::SHA256.file(File.join(observer, "native_define_method_observer.so")).hexdigest,
  ruby: RUBY_DESCRIPTION, commands: [], lanes: {},
  environment: env, source_sha256: {}, failure_source_sha256: {}
}
run = ->(name, extra, *argv) do
  stdout, stderr, status = Open3.capture3(env.merge(extra), "timeout", "120", *argv,
    chdir: app, unsetenv_others: true)
  File.write(File.join(out, "#{name}.stdout"), stdout)
  File.write(File.join(out, "#{name}.stderr"), stderr)
  stderr.scan(%r{/[^\s:]+\.rb}).uniq.each do |path|
    report[:failure_source_sha256][path] = Digest::SHA256.file(path).hexdigest if File.file?(path)
  end
  report[:commands] << { name: name, cwd: app, env: extra, argv: ["timeout", "120", *argv], exit: status.exitstatus }
  [stdout, stderr, status.success?]
end
required = ->(name, extra, *argv) do
  stdout, stderr, ok = run.call(name, extra, *argv)
  raise "#{name} failed: #{stderr.lines.last}" unless ok
  stdout
end
begin
  # Hash all app Ruby sources and each actual probe/exporter input. No DB,
  # credentials, generated log, or observation-based type signatures are used.
  paths = Dir[File.join(app, "{app,lib,config}/**/*.rb")] +
    %w[Gemfile Gemfile.lock .ruby-version config/database.yml config/cable.yml].map { |p| File.join(app, p) }
  paths.each { |p| report[:source_sha256]["app/#{p.delete_prefix(app + '/')}"] = Digest::SHA256.file(p).hexdigest }
  [File.join(root, "bin/rh"), File.join(root, "tests/rh_materialize_test.rb"),
   *Dir[File.join(root, "tools/boot-to-core/{capture,materialize}.rb")],
   *Dir[File.join(root, "tools/native-observer/*.{rb,c}")],
   *Dir[File.join(__dir__, "*.rb")]].each do |p|
    report[:source_sha256][p.delete_prefix(root + "/")] = Digest::SHA256.file(p).hexdigest
  end
  required.call("bundle_check", {}, "bundle", "check")
  required.call("native_contract", {}, RbConfig.ruby, File.join(root, "tools/native-observer/contract.rb"))
  # The contract spawns Ruby children; a parent-only --yjit flag is not inherited.
  required.call("native_contract_yjit", { "RUBYOPT" => "--yjit" }, RbConfig.ruby,
    File.join(root, "tools/native-observer/contract.rb"))
  required.call("cli_contract", { "BUNDLE_PATH" => nil, "BUNDLE_WITHOUT" => nil },
    RbConfig.ruby, File.join(root, "tests/rh_materialize_test.rb"))
  report[:dsl_inventory] = JSON.parse(required.call("inventory", {}, "bundle", "exec", RbConfig.ruby,
    File.join(__dir__, "inventory.rb"), app, root))
  reference = JSON.parse(required.call("reference", {}, "bundle", "exec", RbConfig.ruby, File.join(__dir__, "reference.rb")).lines.last)
  report[:reference] = reference
  originals = %w[leaf mention].to_h do |lane|
    output = required.call("#{lane}_original", {}, "bundle", "exec", RbConfig.ruby, File.join(__dir__, "contract.rb"), "original", lane)
    [lane, JSON.parse(output.lines.last)]
  end
  candidates = %w[namespace_accessor platform_inherited delegate_current delegate_filter delegate_presentation]
  candidates.each do |lane|
    output = required.call("#{lane}_original", {}, "bundle", "exec", RbConfig.ruby,
      File.join(__dir__, "next_contract.rb"), "original", lane)
    originals[lane] = JSON.parse(output.lines.last)
  end
  report[:original_contracts] = originals
  # Full round-one/two captures remain separate; these are public bounded cuts.
  (%w[leaf mention attribute_ready enum_capture] + candidates).each do |lane|
    core_app = File.join(out, "#{lane}-core")
    stdout, stderr, ok = run.call("#{lane}_materialize", { "CAMPFIRE_CORE_LANE" => lane }, "bundle", "exec", RbConfig.ruby,
      File.join(root, "bin/rh"), "materialize", "--trust-boot", File.join(__dir__, "input.rb"), "-o", core_app)
    provenance = stderr.lines.find { |line| line.start_with?("PROVENANCE ") }
    report[:lanes][lane] = { materialized: ok, stdout: stdout, stderr: stderr,
      refusal: stderr.lines.grep(/^rh materialize:/).map(&:strip),
      provenance: provenance && JSON.parse(provenance.delete_prefix("PROVENANCE ")),
      stage: stderr.include?("ROOT_EXPORT") ? "root_export" : "boot_or_generation_capture" }
    next unless ok
    report[:lanes][lane][:inventory] = JSON.parse(File.read(File.join(core_app, "materialization.json")))
    checked_stdout, checked_stderr, checked = run.call("#{lane}_strict", {}, compiler, "check", "--strict", core_app)
    report[:lanes][lane].merge!(strict: checked, diagnostics_stdout: checked_stdout, diagnostics_stderr: checked_stderr)
    next unless originals.key?(lane)
    contract = File.join(__dir__, candidates.include?(lane) ? "next_contract.rb" : "contract.rb")
    expected = originals.fetch(lane).reject { |key, _| key == "methods" }
    core_stdout, core_stderr, core_ok = run.call("#{lane}_core", {}, RbConfig.ruby, contract, File.join(core_app, "lib/core.rb"), lane)
    core = core_ok && JSON.parse(core_stdout.lines.last)
    report[:lanes][lane].merge!(original: originals.fetch(lane), core: core,
      core_matches_original: core_ok && core == expected, core_failure: core_ok ? nil : core_stderr)
    emitted_app = File.join(out, "#{lane}-emitted")
    _, emit_stderr, emit_ok = run.call("#{lane}_emit", {}, compiler, "--target", "ruby", core_app, "-o", emitted_app)
    report[:lanes][lane].merge!(emitted_project: emit_ok, emit_failure: emit_ok ? nil : emit_stderr)
    next unless emit_ok
    loader = File.join(out, "#{lane}-emitted-loader.rb")
    files = Dir[File.join(emitted_app, "app/models/**/*.rb")].sort
    raise "no emitted models" if files.empty?
    File.write(loader, files.map { |path| "require #{path.inspect}" }.join("\n") + "\n")
    emitted_stdout, emitted_stderr, emitted_ok = run.call("#{lane}_emitted", {}, RbConfig.ruby, contract, loader, lane)
    emitted = emitted_ok && JSON.parse(emitted_stdout.lines.last)
    report[:lanes][lane].merge!(emitted: emitted, emitted_matches_original: emitted_ok && emitted == expected,
      emitted_failure: emitted_ok ? nil : emitted_stderr,
      verified: checked && core_ok && core == expected && emitted_ok && emitted == expected)
  end
  unverified = report[:lanes].select { |_, lane| lane[:materialized] && !lane[:verified] }.keys
  raise "materialized cuts lack strict three-way verification: #{unverified.join(', ')}" unless unverified.empty?
  raise "leaf comparison regressed" unless report[:lanes].dig("leaf", :verified)
  raise "observer failures require investigation" if report[:lanes].values.any? { |lane| !lane.dig(:provenance, "observer_failures").to_a.empty? }
  { round1: "results", round2: "round2-final" }.each do |round, directory|
    baseline = File.join(File.dirname(out), directory, "results.json")
    next unless File.file?(baseline)
    initial = JSON.parse(File.read(baseline))
    raise "compiler differs from #{round}" unless initial.fetch("compiler_sha256") == report[:compiler_sha256]
    old_sources = initial.fetch("source_sha256").select { |path, _| path.start_with?("app/") }
    raise "app differs from #{round}" unless old_sources == report[:source_sha256].select { |path, _| path.start_with?("app/") }
    report[:"#{round}_comparison"] = initial.fetch("lanes").select { |lane, _| report[:lanes].key?(lane) }.to_h do |lane, old|
      [lane, { previous_stage: old.fetch("stage"), previous_refusals: old.fetch("stderr").lines.grep(/^rh materialize:/).map(&:strip),
        round4_stage: report[:lanes].fetch(lane)[:stage], round4_refusals: report[:lanes].fetch(lane)[:refusal] }]
    end
  end
  report[:verified_candidates] = candidates.select { |lane| report[:lanes].dig(lane, :verified) }
  report[:status] = "round4_cases_recorded"
  puts "PASS reference: #{reference.fetch('checks')} assertions, /up=200; leaf original/Core/emitted: 3 assertions each"
  report[:lanes].each do |lane, result|
    outcome = result[:verified] ? "original/Core/emitted verified" : result[:materialized] ? "materialized; not fully verified" : "blocked"
    puts "#{lane}: #{outcome} (#{result[:stage]})"
  end
rescue StandardError => error
  report[:status] = "failed"
  report[:failure] = { class: error.class.name, message: error.message }
  warn error.full_message
ensure
  File.write(File.join(out, "results.json"), JSON.pretty_generate(report) + "\n")
end
exit(report[:status] == "round4_cases_recorded" ? 0 : 1)
