# frozen_string_literal: true

require "json"
require "digest"
require "fileutils"
require "open3"
require "rbconfig"

# Usage: ruby run.rb COMPILER_WORKTREE PINNED_APP NEW_OUTPUT GEM_BUNDLE [OBSERVER_DIR]
# Run with the app's exact Ruby (3.4.7). Compiler and exporter are externally
# supplied inputs: this runner does not patch, fetch or build either of them.
# WRITEBOOK_LANES="qr current attributes" selects bounded reruns.
compiler, app, out, bundle, observer = ARGV.map { |path| File.expand_path(path) }
raise "expected four paths" unless bundle
raise "output already exists" if File.exist?(out)
FileUtils.mkdir_p(out)
lanes = ENV.fetch("WRITEBOOK_LANES", "full attributes current enum delegate markdown position slug qr embed lightbox").split
report = { commands: [], lanes: {}, selected_lanes: lanes, scope: "actual app cuts; no whole-app support claim" }
replacements = { compiler => "<COMPILER>", app => "<APP>", out => "<OUT>",
  bundle => "<BUNDLE>", __dir__ => "<PROBE>", Dir.home => "<HOME>" }
replacements[observer] = "<OBSERVER>" if observer
sanitize = ->(text) { replacements.reduce(text) { |value, (path, label)| value.gsub(path, label) } }
env = {
  "HOME" => Dir.home, "PATH" => "#{File.dirname(RbConfig.ruby)}:#{Dir.home}/.local/bin:/usr/bin:/bin",
  "BUNDLE_PATH" => bundle, "BUNDLE_GEMFILE" => File.join(app, "Gemfile"), "BUNDLE_FROZEN" => "true",
  "WRITEBOOK_ROOT" => app, "RAILS_ENV" => "test", "SECRET_KEY_BASE_DUMMY" => "1"
}
env["RUBYLIB"] = observer if observer
run = ->(name, argv, extra = {}, original: true) do
  process_env = original ? env : { "PATH" => "/usr/bin:/bin", "HOME" => Dir.home }
  process_env = process_env.merge("DATABASE_URL" => "sqlite3:#{out}/#{name}.sqlite3").merge(extra)
  stdout, stderr, status = Open3.capture3(process_env, *argv, chdir: app, unsetenv_others: true)
  File.write(File.join(out, "#{name}.stdout"), sanitize.call(stdout))
  File.write(File.join(out, "#{name}.stderr"), sanitize.call(stderr))
  report[:commands] << { name: name, argv: argv.map { |arg| sanitize.call(arg) },
    environment: process_env.reject { |key, _| key == "HOME" }.transform_values { |v| sanitize.call(v) },
    inherited_environment: false, exit: status.exitstatus }
  [stdout, stderr, status.success?]
end
sha = ->(path) { Digest::SHA256.file(path).hexdigest }
contract = File.join(__dir__, "contract.rb")
manifest = File.join(__dir__, "manifest.rb")
bin = File.join(compiler, "target/debug/roundhouse")

begin
  head, _, ok = run.call("compiler_head", ["git", "-C", compiler, "rev-parse", "HEAD"], {}, original: false)
  raise "cannot inspect compiler" unless ok
  report[:compiler] = head.strip
  report[:binary_sha256] = sha.call(bin)
  report[:observer_sha256] = sha.call(File.join(observer, "native_define_method_observer.so")) if observer
  report[:exporter_sha256] = %w[capture.rb materialize.rb].to_h { |name| [name, sha.call(File.join(compiler, "tools/boot-to-core", name))] }
  report[:probe_sha256] = Dir[File.join(__dir__, "*.{rb,rbs}")].to_h { |path| [File.basename(path), sha.call(path)] }
  report[:app_sha256] = %w[Gemfile Gemfile.lock .ruby-version db/schema.rb].to_h { |name| [name, sha.call(File.join(app, name))] }
  report[:ruby] = RUBY_DESCRIPTION
  stdout, _, booted = run.call("boot", [RbConfig.ruby, "-e", 'require "bundler/setup"; require "./config/environment"; Rails.application.eager_load!; puts "BOOT_OK"'])
  report[:boot] = booted && stdout.include?("BOOT_OK")
  raise "original app boot failed" unless report[:boot]
  strict, strict_err, strict_ok = run.call("app_strict", [bin, "check", "--strict", app], {}, original: false)
  report[:app_strict] = { accepted: strict_ok, output: sanitize.call(strict + strict_err),
    diagnostic_identities: sanitize.call(strict + strict_err).lines.grep(/(?:error|warning|note|info)\[[^\]]+\]:/).map(&:strip).tally }
  lanes.each do |lane|
    result = {}
    unless lane == "full"
      stdout, _, ok = run.call("#{lane}_original", [RbConfig.ruby, "-rbundler/setup", contract, lane, "original"])
      result[:original] = ok ? JSON.parse(stdout.lines.last) : "failed; see stderr"
    end
    %w[focused unobserved].each do |mode|
      next if lane == "full" && mode == "unobserved"
      name = "#{lane}_#{mode}"
      core_app = File.join(out, "#{name}-core")
      stdout, stderr, ok = run.call("#{name}_materialize",
        [RbConfig.ruby, "-rbundler/setup", File.join(compiler, "bin/rh"),
         "materialize", "--trust-boot", manifest, "-o", core_app],
        { "WRITEBOOK_LANE" => lane, "WRITEBOOK_CAPTURE" => mode })
      raise "materializer exited zero without Core artifact" if ok && !File.file?(File.join(core_app, "lib/core.rb"))
      attempt = { materialized: ok }
      metadata = stderr.lines.find { |line| line.start_with?("WRITEBOOK_CAPTURE_METADATA: ") }
      attempt[:capture] = JSON.parse(sanitize.call(metadata.delete_prefix("WRITEBOOK_CAPTURE_METADATA: "))) if metadata
      unless ok
        attempt[:failure] = sanitize.call(stderr.lines.find { |line| line.start_with?("rh materialize:") } || stderr.lines.last || "no stderr").strip
        result[mode] = attempt
        next
      end
      core = File.join(core_app, "lib/core.rb")
      File.write(File.join(out, "#{name}.core.rb"), File.read(core))
      attempt[:materialization] = JSON.parse(sanitize.call(File.read(File.join(core_app, "materialization.json"))))
      stdout, _, executed = run.call("#{name}_core", [RbConfig.ruby, contract, lane, core], {}, original: false)
      attempt[:core] = executed ? JSON.parse(stdout.lines.last) : "failed; see stderr"
      stdout, stderr, accepted = run.call("#{name}_check", [bin, "check", "--strict", core_app], {}, original: false)
      attempt[:strict] = { accepted: accepted, output: sanitize.call(stdout + stderr),
        diagnostic_identities: sanitize.call(stdout + stderr).lines.grep(/(?:error|warning|note|info)\[[^\]]+\]:/).map(&:strip).tally }
      emitted = File.join(out, "#{name}-emitted")
      _, _, emitted_ok = run.call("#{name}_emit", [bin, "--target", "ruby", core_app, "-o", emitted], {}, original: false)
      attempt[:emitted_project] = emitted_ok
      if emitted_ok
        loader = File.join(out, "#{name}-loader.rb")
        File.write(loader, Dir[File.join(emitted, "app/models/*.rb")].map { |path| "require #{path.inspect}" }.join("\n") + "\n")
        stdout, _, executed = run.call("#{name}_emitted", [RbConfig.ruby, contract, lane, loader], {}, original: false)
        attempt[:emitted] = executed ? JSON.parse(stdout.lines.last) : "failed; see stderr"
      end
      attempt[:contract_verified] = accepted && result[:original].is_a?(Hash) && result[:original] == attempt[:core] && result[:original] == attempt[:emitted]
      result[mode] = attempt
    end
    report[:lanes][lane] = result
    puts "#{lane}: #{result.reject { |key, _| key == :original }.transform_values { |attempt| attempt[:failure] || "contract_verified=#{attempt[:contract_verified]}" }.inspect}"
  end
  report[:status] = "experiment_recorded"
ensure
  File.write(File.join(out, "results.json"), JSON.pretty_generate(report) + "\n")
end
