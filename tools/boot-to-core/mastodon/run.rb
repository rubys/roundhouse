# frozen_string_literal: true

# Prerequisites and the disposable DB recipe are in RESULTS.md. No fetching,
# installation, DB writes or service management is performed by this runner.
require "json"
require "open3"
require "fileutils"
require "digest"
require "rbconfig"

app, compiler, output = ARGV.map { |path| File.expand_path(path) }
raise "usage: ruby run.rb APP COMPILER NEW_OUTPUT" unless app && compiler && output
raise "output exists" if File.exist?(output)
head, status = Open3.capture2("git", "-C", compiler, "rev-parse", "HEAD")
raise "wrong compiler commit" unless status.success? && head.strip == "83b6b1458adde2b2db728507fcabc0c2c0557612"
diff, status = Open3.capture2("git", "-C", compiler, "diff", "HEAD", "--", "src", "runtime", "Cargo.toml", "Cargo.lock", "build.rs")
raise "compiler source differs from controlled commit" unless status.success? && diff.empty?
FileUtils.mkdir_p(output)
binary = File.join(compiler, "target/debug/roundhouse")
sha = ->(path) { Digest::SHA256.file(path).hexdigest }
observer = ENV.fetch("MASTODON_PROBE_OBSERVER", "/tmp/core-observer-round3")
report = { compiler_commit: "83b6b1458adde2b2db728507fcabc0c2c0557612", binary_sha256: sha.call(binary),
  probe_sha256: Dir[File.join(__dir__, "*.{rb,rbs}")].sort.to_h { |path| [File.basename(path), sha.call(path)] },
  observer_sha256: %w[observer.c extconf.rb contract.rb].to_h { |name| [name, sha.call(File.join(compiler, "tools/native-observer", name))] }
    .merge("native_define_method_observer.so" => sha.call(File.join(observer, "native_define_method_observer.so"))),
  exporter_sha256: %w[capture.rb materialize.rb].to_h { |name| [name, sha.call(File.join(compiler, "tools/boot-to-core", name))] },
  source_sha256: ["Gemfile", "Gemfile.lock", "db/schema.rb", "lib/mastodon/snowflake.rb", "config/initializers/opentelemetry.rb",
    "app/lib/http_signature_draft.rb", "app/lib/response_with_limit.rb", "app/lib/ascii_folding.rb", "app/models/login_activity.rb", "app/models/user.rb",
    "app/models/status_edit.rb", "app/models/user_settings.rb", "app/models/fasp/capability.rb"].to_h { |name| [name, sha.call(File.join(app, name))] },
  commands: [], lanes: {} }
# In addition to discarding inherited credentials/configuration, unshare -n
# denies IP network effects. Only disposable local Unix sockets remain reachable.
environment = {
  "HOME" => ENV.fetch("HOME"), "PATH" => ENV.fetch("PATH"), "BUNDLE_GEMFILE" => File.join(app, "Gemfile"),
  "RUBYLIB" => observer,
  "BUNDLE_FROZEN" => "true", "BUNDLE_WITHOUT" => "development:test:opentelemetry:pam_authentication",
  "RAILS_ENV" => "production", "LOCAL_DOMAIN" => "mastodon.invalid", "SMTP_DELIVERY_METHOD" => "test",
  "SECRET_KEY_BASE" => "disposable-mastodon-probe-not-a-production-secret",
  "ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY" => "disposable-deterministic-key",
  "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT" => "disposable-salt",
  "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY" => "disposable-primary-key", "ES_ENABLED" => "false",
  "DB_HOST" => ENV.fetch("MASTODON_PROBE_PG_SOCKET", "/tmp/mastodon-pg"), "DB_PORT" => "55439",
  "DB_USER" => "mastodon_probe", "DB_NAME" => "mastodon_probe",
  "REDIS_URL" => "unix://#{ENV.fetch('MASTODON_PROBE_REDIS_SOCKET', '/tmp/mastodon-redis/redis.sock')}",
  "MASTODON_PROMETHEUS_EXPORTER_ENABLED" => "false", "MASTODON_APP" => app
}
report[:sandbox] = { prefix: ["sudo", "unshare", "-n", "--", "env", "-i"], environment: environment, timeout_seconds: 120, kill_after_seconds: 10, chdir: app }
run = ->(name, argv, lane = nil) do
  env = environment.merge(lane ? { "MASTODON_LANE" => lane } : {})
  command = ["sudo", "unshare", "-n", "--", "env", "-i", *env.map { |k, v| "#{k}=#{v}" }, "timeout", "--kill-after=10", "120", *argv]
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  stdout, stderr, status = Open3.capture3(*command, chdir: app)
  elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  File.write(File.join(output, "#{name}.stdout"), stdout)
  File.write(File.join(output, "#{name}.stderr"), stderr)
  report[:commands] << { name: name, argv: argv, lane: lane, exit: status.exitstatus, signal: status.termsig, elapsed_seconds: elapsed.round(3) }
  [stdout, stderr, status.success?]
end
contract = File.join(__dir__, "contract.rb")
observations = ->(stdout) { JSON.parse(stdout.lines.find { |line| line.start_with?("MASTODON_CONTRACT=") }.delete_prefix("MASTODON_CONTRACT=")) }
failed = false
incomplete = false
begin
  boot = <<~'RUBY'
    require "./config/environment"
    require "json"
    require "digest"
    gems = Gem.loaded_specs.transform_values do |gem|
      files = Dir[File.join(gem.full_gem_path, "**/*.{rb,so}")].sort.select { |path| File.file?(path) }
      entries = files.map { |path| path.delete_prefix(gem.full_gem_path + "/") + "\0" + Digest::SHA256.file(path).hexdigest }
      { version: gem.version.to_s, gemspec_sha256: Digest::SHA256.file(gem.loaded_from).hexdigest,
        ruby_native_source_tree_sha256: Digest::SHA256.hexdigest(entries.join("\n")), hashed_files: files.length }
    end
    puts "MASTODON_BOOT_OK"
    puts JSON.generate(ruby: RUBY_DESCRIPTION, gems: gems)
  RUBY
  stdout, stderr, ok = run.call("boot", [RbConfig.ruby, "-e", boot])
  raise "original app boot failed: #{stderr.lines.first}" unless ok && stdout.include?("MASTODON_BOOT_OK")
  report[:boot] = JSON.parse(stdout.lines.last)
  stdout, stderr, ok = run.call("generated_reference", [RbConfig.ruby, contract, "generated_reference", "original"])
  raise "generated original contract failed: #{stderr.lines.first}" unless ok
  report[:generated_reference] = observations.call(stdout)
  %w[ordinary accessor full_capture enum attribute delegate settings namespaced_attribute enum_root_only attribute_root_only delegate_root_only namespaced_root_only enum_generation delegate_generation constant].each do |lane|
    core_app = File.join(output, "#{lane}-core")
    stdout, stderr, ok = run.call("#{lane}_materialize", [RbConfig.ruby, "-rbundler/setup", File.join(compiler, "tools/boot-to-core/materialize.rb"), "--trust-boot", File.join(__dir__, "input.rb"), "-o", core_app], lane)
    result = report[:lanes][lane] = { materialized: ok }
    summary = stderr.lines.find { |line| line.start_with?("MASTODON_OBSERVATION=") }
    unless summary
      result[:observation_status] = "missing; boot phase, opaque records and observer failures unknown"
      result[:boundary] = stderr.strip
      incomplete = true
      raise "#{lane}: successful export without observation summary" if ok
      next
    end
    result[:observation] = JSON.parse(summary.delete_prefix("MASTODON_OBSERVATION="))
    failures = result.fetch(:observation).fetch("observer_failures")
    raise "#{lane}: unexpected observer failures: #{failures.inspect}" unless failures.empty?
    unless ok
      result[:boundary] = stderr.strip
      failed = true if %w[ordinary accessor constant].include?(lane)
      next
    end
    result[:inventory] = JSON.parse(File.read(File.join(core_app, "materialization.json")))
    stdout, stderr, ok = run.call("#{lane}_strict", [binary, "check", "--strict", core_app])
    result[:strict] = { passed: ok, stdout: stdout, stderr: stderr }
    if %w[ordinary accessor constant full_capture].include?(lane)
      contract_lane = lane == "full_capture" ? "ordinary" : lane
      samples = {}
      { original: "original", core: File.join(core_app, "lib/core.rb") }.each do |kind, source|
        stdout, stderr, passed = run.call("#{lane}_#{kind}", [RbConfig.ruby, contract, contract_lane, source])
        raise "#{lane} #{kind} failed: #{stderr.lines.first}" unless passed
        samples[kind] = observations.call(stdout)
      end
      emitted_app = File.join(output, "#{lane}-emitted")
      stdout, stderr, passed = run.call("#{lane}_emit", [binary, "--target", "ruby", core_app, "-o", emitted_app])
      raise "#{lane} emit failed: #{stderr.lines.first}" unless passed
      loader = File.join(output, "#{lane}-loader.rb")
      File.write(loader, Dir[File.join(emitted_app, "app/models/*.rb")].sort.map { |path| "require #{path.inspect}" }.join("\n") + "\n")
      stdout, stderr, passed = run.call("#{lane}_emitted", [RbConfig.ruby, contract, contract_lane, loader])
      raise "#{lane} emitted failed: #{stderr.lines.first}" unless passed
      samples[:emitted] = observations.call(stdout)
      raise "#{lane} observations differ" unless samples.values.uniq.length == 1
      result[:contracts] = samples
      failed = true unless ok
    end
  end
  report[:status] = if failed
    "positive_lane_failed"
  elsif incomplete
    "incomplete_capture_with_verified_bounded_contracts"
  else
    "bounded_contracts_verified_with_explicit_boundaries"
  end
rescue StandardError => error
  report[:status] = "failed"
  report[:failure] = error.message
  warn error.message
ensure
  File.write(File.join(output, "results.json"), JSON.pretty_generate(report) + "\n")
end
puts "#{report[:status]}: #{output}/results.json"
exit(report[:status] == "bounded_contracts_verified_with_explicit_boundaries" ? 0 : 1)
