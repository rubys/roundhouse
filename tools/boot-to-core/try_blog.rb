# frozen_string_literal: true

# Two distinct claims: attempt whole-app materialization, then exercise an
# incremental Core overlay through the full, unchanged blog's test contract.
require "json"
require "fileutils"
require "open3"
require "rbconfig"
require "digest"

root = File.expand_path("../..", __dir__)
out = File.expand_path(ARGV.fetch(0))
compiler = File.expand_path(ARGV[1] || File.join(root, "target/debug/roundhouse"))
raise "output already exists" if File.exist?(out)
FileUtils.mkdir_p(out)
fixture = File.join(root, "fixtures/real-blog")
report = { scope: "real-blog: whole-app attempt versus incremental Core overlay; local execution, not hosted CI", commands: [] }
env = ENV.keys.grep(/TOKEN|PASSWORD|SECRET|API_KEY/).to_h { |key| [key, nil] }
env.merge!("RAILS_ENV" => "test", "CI" => "1", "DATABASE_URL" => nil, "BUNDLE_GEMFILE" => nil)
run = ->(name, cwd, extra_env, *argv) do
  stdout, stderr, status = Open3.capture3(env.merge(extra_env), *argv, chdir: cwd)
  File.write(File.join(out, "#{name}.stdout"), stdout)
  File.write(File.join(out, "#{name}.stderr"), stderr)
  report[:commands] << { name: name, cwd: cwd, argv: argv, exit: status.exitstatus }
  [stdout, stderr, status.success?]
end
required = ->(name, cwd, extra_env, *argv) do
  stdout, stderr, ok = run.call(name, cwd, extra_env, *argv)
  raise "#{name} failed; inspect #{name}.stderr: #{stderr.lines.last}" unless ok
  stdout
end

begin
  report[:compiler_sha256] = Digest::SHA256.file(compiler).hexdigest
  report[:compiler_head] = required.call("head", root, {}, "git", "rev-parse", "HEAD").strip
  report[:input_sha256] = Dir[File.join(__dir__, "*")].select { |path| File.file?(path) }.to_h { |path| [File.basename(path), Digest::SHA256.file(path).hexdigest] }
  app = File.join(out, "reference-blog")
  FileUtils.cp_r(fixture, app)
  # Only disposable test DBs and files are used; original fixture stays intact.
  required.call("bundle_check", app, {}, "bundle", "check")
  reference = required.call("rails_reference", app, {}, "bundle", "exec", "ruby", "bin/rails", "test", "test/models", "test/controllers")
  unless reference.match?(/^21 (?:runs|tests), \d+ assertions, 0 failures, 0 errors, 0 skips$/)
    raise "Rails reference did not execute all 21 tests without skips"
  end
  stdout, stderr, ok = run.call("whole_blog_materialize", root, { "CORE_RUBY_APP" => app }, RbConfig.ruby,
    File.join(root, "bin/rh"), "materialize", "--trust-boot", File.join(__dir__, "real_blog_input.rb"), "-o", File.join(out, "whole-core"))
  report[:whole_app] = { materialized: ok, stderr: stderr, stdout: stdout,
    emitted_or_executed: false, status: ok ? "exported_unverified" : "blocked" }

  # Incremental use keeps the existing Rails input/semantics, replacing only
  # the unknown generator with its Core. No original DSL is shipped in output.
  core_app = File.join(out, "generator-core")
  required.call("generator_materialize", root, {}, RbConfig.ruby, File.join(root, "bin/rh"), "materialize", "--trust-boot",
    File.join(__dir__, "micro_input.rb"), "-o", core_app)
  overlay = File.join(out, "overlay-blog")
  FileUtils.cp_r(fixture, overlay)
  FileUtils.cp(File.join(core_app, "lib/core.rb"), File.join(overlay, "lib/materialized_core.rb"))
  FileUtils.mkdir_p(File.join(overlay, "sig"))
  FileUtils.cp(File.join(core_app, "sig/input0.rbs"), File.join(overlay, "sig/materialized_core.rbs"))
  required.call("overlay_check", root, {}, compiler, "check", "--strict", overlay)
  emitted = File.join(out, "emitted-blog")
  required.call("overlay_emit", root, {}, compiler, "--target", "ruby", overlay, "-o", emitted)
  checks = {}
  %w[article comment].map { |name| "test/models/#{name}_test.rb" }.concat(
    %w[articles comments].map { |name| "test/controllers/#{name}_controller_test.rb" }
  ).each do |test|
    name = File.basename(test, ".rb")
    output = required.call(name, emitted, {}, RbConfig.ruby, "-Itest", "-I.", test)
    match = output.match(/^\w+: (\d+) tests passed$/)
    expected = { "article_test" => 4, "comment_test" => 5, "articles_controller_test" => 9, "comments_controller_test" => 3 }.fetch(name)
    raise "#{test} did not execute all #{expected} tests without skips" unless match && match[1].to_i == expected && !output.match?(/^SKIP |tests skipped/)
    checks[test] = { tests: match[1].to_i }
  end
  output = required.call("generator_emitted_contract", emitted, {}, RbConfig.ruby, "-e", <<~RUBY)
    require File.expand_path("main", Dir.pwd)
    Main.configure_default_adapter!
    raise "generator escaped into output" if defined?(UnknownGenerator)
    ARGV.replace([#{File.join(emitted, 'app/models.rb').inspect}])
    load #{File.join(__dir__, 'contract.rb').inspect}
  RUBY
  contract = JSON.parse(output.lines.last)
  raise "generator contract was not fully executed" unless contract.fetch("checks") == 22
  report[:incremental] = { blog_suites: checks, generator_contract: contract, status: "passed" }
  report[:status] = report[:whole_app][:materialized] ? "incremental_passed_whole_export_unverified" : "incremental_passed_whole_app_blocked"
  puts "PASS incremental real-blog: #{checks.values.sum { |c| c[:tests] }} blog tests, 22 generated-code assertions"
  puts "WHOLE APP: #{report[:whole_app][:status]} (not a full Rails materialization pass)"
rescue StandardError => error
  report[:status] = "failed"
  report[:failure] = { class: error.class.name, message: error.message }
  warn "FAIL: #{error.message}"
ensure
  File.write(File.join(out, "results.json"), JSON.pretty_generate(report) + "\n")
end
exit(report[:status] == "incremental_passed_whole_app_blocked" ? 0 : 1)
