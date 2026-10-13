# frozen_string_literal: true

# Prerequisites and the disposable DB recipe are in RESULTS.md. No fetching,
# installation, DB writes or service management is performed by this runner.
require "json"
require "open3"
require "fileutils"
require "digest"
require "rbconfig"

app, compiler, output = ARGV.first(3).map { |path| File.expand_path(path) }
raise "usage: ruby run.rb APP COMPILER NEW_OUTPUT [LANE...]" unless app && compiler && output
lanes = ARGV.drop(3)
lanes = %w[ordinary accessor full_capture enum attribute delegate settings namespaced_attribute enum_root_only attribute_root_only delegate_root_only namespaced_root_only enum_generation delegate_generation constant fasp_contract safe_delegate prefixed_delegate namespace_attributes media_delegate policy_generated policy_plain keyword_message] if lanes.empty?
raise "output exists" if File.exist?(output)
head, status = Open3.capture2("git", "-C", compiler, "rev-parse", "HEAD")
raise "wrong compiler commit" unless status.success? && head.strip == "83b6b1458adde2b2db728507fcabc0c2c0557612"
diff, status = Open3.capture2("git", "-C", compiler, "diff", "--no-color", "--no-ext-diff", "HEAD", "--", "src", "runtime", "Cargo.toml", "Cargo.lock", "build.rs")
patch = ENV["MASTODON_PROBE_COMPILER_PATCH"]
if patch
  raise "wrong keyword candidate patch" unless %w[f970c0d2393bccbfe3b8541311eebc15f8b30856974072b585fc628ce515988e 6bbf7c7ee8c8bf84a20998ff62c01b54cb450cb0e64386274520449761bdb212 8ad268408b2a0bf641e2ec4db0b5589991bf9c0534e4d6dae7c392a2dca2f97a c6be6d458fe6b36be652007e65e5630fcdbb47767129300c08b5ea020d235c65 85e2725bc2695d21141d263c7f8d12c0d0dc8db36179ed752681b6b2161abc93].include?(Digest::SHA256.file(patch).hexdigest)
  raise "compiler source differs from exact keyword candidate" unless status.success? && diff == File.read(patch)
else
  raise "compiler source differs from controlled commit" unless status.success? && diff.empty?
end
FileUtils.mkdir_p(output)
binary = patch ? File.expand_path(ENV.fetch("MASTODON_PROBE_COMPILER_BINARY")) : File.join(compiler, "target/debug/roundhouse")
sha = ->(path) { Digest::SHA256.file(path).hexdigest }
observer = ENV.fetch("MASTODON_PROBE_OBSERVER", "/tmp/core-observer-round3")
report = { compiler_commit: "83b6b1458adde2b2db728507fcabc0c2c0557612", binary_sha256: sha.call(binary),
  compiler_comparison: patch ? { mode: "separate keyword candidate", patch_sha256: sha.call(patch), exact_source_diff_sha256: Digest::SHA256.hexdigest(diff) } : { mode: "unchanged compiler control" },
  probe_sha256: Dir[File.join(__dir__, "*.{rb,rbs}")].sort.to_h { |path| [File.basename(path), sha.call(path)] },
  observer_sha256: %w[observer.c extconf.rb contract.rb].to_h { |name| [name, sha.call(File.join(compiler, "tools/native-observer", name))] }
    .merge("native_define_method_observer.so" => sha.call(File.join(observer, "native_define_method_observer.so"))),
  exporter_sha256: %w[capture.rb materialize.rb].to_h { |name| [name, sha.call(File.join(compiler, "tools/boot-to-core", name))] },
  source_sha256: ["Gemfile", "Gemfile.lock", "db/schema.rb", "lib/mastodon/snowflake.rb", "config/initializers/opentelemetry.rb",
    "app/lib/http_signature_draft.rb", "app/lib/response_with_limit.rb", "app/lib/ascii_folding.rb", "app/models/login_activity.rb", "app/models/user.rb",
    "app/models/status_edit.rb", "app/models/user_settings.rb", "app/models/fasp/capability.rb", "app/lib/web_push_request.rb",
    "app/models/account_suggestions/suggestion.rb", "app/lib/translation_service/translation.rb", "app/lib/interaction_policy.rb",
    "app/lib/admin/system_check/message.rb"].to_h { |name| [name, sha.call(File.join(app, name))] },
  commands: [], lanes: {} }
# In addition to discarding inherited credentials/configuration, unshare -n
# denies IP network effects. Only disposable local Unix sockets remain reachable.
yjit = ENV["MASTODON_PROBE_YJIT"] == "1"
runtime_probe = File.join(output, "child-runtime.rb")
File.write(runtime_probe, <<~'RUBY')
  at_exit do
    require "json"
    warn "MASTODON_CHILD_RUNTIME=" + JSON.generate(ruby: RUBY_DESCRIPTION,
      yjit: defined?(RubyVM::YJIT) && RubyVM::YJIT.enabled?)
  end
RUBY
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
report[:sandbox] = { prefix: ["sudo", "unshare", "-n", "--", "env", "-i"], environment: environment, ruby_yjit_requested: yjit, timeout_seconds: 120, kill_after_seconds: 10, chdir: app }
run = ->(name, argv, lane = nil) do
  env = environment.merge(lane ? { "MASTODON_LANE" => lane } : {})
  if argv.first == RbConfig.ruby
    argv = [RbConfig.ruby, *(yjit ? ["--yjit"] : []), "-r", runtime_probe, *argv.drop(1)]
  end
  command = ["sudo", "unshare", "-n", "--", "env", "-i", *env.map { |k, v| "#{k}=#{v}" }, "timeout", "--kill-after=10", "120", *argv]
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  stdout, stderr, status = Open3.capture3(*command, chdir: app)
  elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  File.write(File.join(output, "#{name}.stdout"), stdout)
  File.write(File.join(output, "#{name}.stderr"), stderr)
  runtime = stderr.lines.find { |line| line.start_with?("MASTODON_CHILD_RUNTIME=") }
  runtime = JSON.parse(runtime.delete_prefix("MASTODON_CHILD_RUNTIME=")) if runtime
  report[:commands] << { name: name, argv: argv, lane: lane, exit: status.exitstatus, signal: status.termsig, elapsed_seconds: elapsed.round(3), child_runtime: runtime }
  raise "#{name}: child YJIT is not enabled" if yjit && argv.first == RbConfig.ruby && (!runtime || runtime["yjit"] != true)
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
  lanes.each do |lane|
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
    if %w[ordinary accessor constant full_capture fasp_contract safe_delegate prefixed_delegate namespace_attributes media_delegate policy_generated policy_plain keyword_message].include?(lane)
      begin
        contract_lane = lane == "full_capture" ? "ordinary" : lane
        samples = result[:contracts] = {}
        { original: "original", core: File.join(core_app, "lib/core.rb") }.each do |kind, source|
          stdout, stderr, passed = run.call("#{lane}_#{kind}", [RbConfig.ruby, contract, contract_lane, source])
          raise "#{lane} #{kind} failed: #{stderr.lines.first}" unless passed
          samples[kind] = observations.call(stdout)
        end
        emitted_app = File.join(output, "#{lane}-emitted")
        stdout, stderr, passed = run.call("#{lane}_emit", [binary, "--target", "ruby", core_app, "-o", emitted_app])
        raise "#{lane} emit failed: #{stderr.lines.first}" unless passed
        loader = File.join(output, "#{lane}-loader.rb")
        File.write(loader, Dir[File.join(emitted_app, "app/models/**/*.rb")].sort.map { |path| "require #{path.inspect}" }.join("\n") + "\n")
        stdout, stderr, passed = run.call("#{lane}_emitted", [RbConfig.ruby, contract, contract_lane, loader])
        raise "#{lane} emitted failed: #{stderr.lines.first}" unless passed
        samples[:emitted] = observations.call(stdout)
        raise "#{lane} observations differ" unless samples.values.uniq.length == 1
        result[:verified] = ok
        failed = true unless ok
      rescue StandardError => error
        result[:verified] = false
        result[:failure] = error.message
        failed = true
        warn error.message
      end
    end
  end
  if ENV["MASTODON_PROBE_BARE_HASH"] == "1"
    raise "bare-Hash control requires a verified keyword_message lane" unless report[:lanes].dig("keyword_message", :verified)
    control_app = File.join(output, "bare-hash-core")
    FileUtils.cp_r(File.join(output, "keyword_message-core"), control_app)
    driver = File.join(control_app, "lib/bare_hash_control.rb")
    File.write(driver, <<~'RUBY')
      class MastodonKeywordCallControl
        def bare
          payload = {critical: true}
          Admin::SystemCheck::Message.new(:bare, nil, nil, payload)
        end
        def keyword
          Admin::SystemCheck::Message.new(:native, nil, nil, critical: true)
        end
      end
    RUBY
    helper = File.join(output, "bare-hash-contract.rb")
    File.write(helper, <<~RUBY)
      driver = ARGV.fetch(2)
      load #{contract.inspect}
      load driver unless driver == "-"
      result = begin
        ["returned", MastodonKeywordCallControl.new.bare.critical]
      rescue StandardError => error
        [error.class.name]
      end
      positive = MastodonKeywordCallControl.new.keyword.critical
      puts "MASTODON_BARE_HASH=" + JSON.generate(bare: result, keyword: positive)
      raise "bare Hash changed keyword/arity semantics" unless result == ["ArgumentError"] && positive == true
    RUBY
    control = report[:bare_hash_control] = { driver_sha256: sha.call(driver), helper_sha256: sha.call(helper), contracts: {} }
    stdout, stderr, passed = run.call("bare_hash_strict", [binary, "check", "--strict", control_app])
    control[:strict] = { passed: passed, stdout: stdout, stderr: stderr }
    raise "bare-Hash control strict failed" unless passed
    emitted_app = File.join(output, "bare-hash-emitted")
    stdout, stderr, passed = run.call("bare_hash_emit", [binary, "--target", "ruby", control_app, "-o", emitted_app])
    raise "bare-Hash control emit failed" unless passed
    loader = File.join(output, "bare-hash-loader.rb")
    File.write(loader, Dir[File.join(emitted_app, "app/models/**/*.rb")].sort.map { |path| "require #{path.inspect}" }.join("\n") + "\n")
    { original: ["original", driver], core: [File.join(control_app, "lib/core.rb"), driver], emitted: [loader, "-"] }.each do |kind, (source, driver_path)|
      stdout, stderr, passed = run.call("bare_hash_#{kind}", [RbConfig.ruby, helper, "keyword_message", source, driver_path])
      line = stdout.lines.find { |text| text.start_with?("MASTODON_BARE_HASH=") }
      control[:contracts][kind] = { passed: passed, observations: line && JSON.parse(line.delete_prefix("MASTODON_BARE_HASH=")) }
      raise "bare-Hash #{kind} failed: #{stderr.lines.first}" unless passed
    end
    control[:verified] = control[:contracts].values.map { |sample| sample[:observations] }.uniq.length == 1
    raise "bare-Hash observations differ" unless control[:verified]
  end
  report[:executed_contract_lanes] = report[:lanes].select { |_, result| result[:verified] }.keys
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
