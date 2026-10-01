require "tmpdir"
gem "minitest", "6.0.6"
require "minitest/autorun"
require_relative "../../scripts/check-gem-capabilities"

class GemCapabilityRunnerTest < Minitest::Test
  def setup
    @dir = Dir.mktmpdir("gem-capability-test-")
    @run = CorpusRun.new(out: "#{@dir}/output", cases: "alba", reference_only: true)
  end

  def teardown
    FileUtils.remove_entry(@dir)
  end

  def execute(code, **options)
    stages = {}
    @run.step(stages, "contract", [RbConfig.ruby, "-e", code], dir: @dir, expected: "PASS 41 false nil\n", **options)
  end

  def test_exit_zero_is_not_a_correct_result
    correct = execute('puts "PASS 41 false nil"')
    wrong = execute('puts "PASS 41 nil nil"')
    assert_equal 0, wrong["execution"]["exit"]
    assert_equal "pass", correct["status"]
    assert_equal "contract-fail", wrong["status"]
  end

  def test_nonzero_wins_even_with_the_right_stdout
    result = execute('puts "PASS 41 false nil"; warn "broken"; exit 7')
    assert_equal 7, result["execution"]["exit"]
    assert_equal "broken\n", result["execution"]["stderr"]
    assert_equal "runtime-fail", result["status"]
  end

  def test_missing_artifact_is_not_a_successful_emit
    artifact = "#{@dir}/emitted.rb"
    missing = execute('puts "PASS 41 false nil"', artifacts: [artifact])
    assert_equal "emission-missing", missing["status"]
    File.write(artifact, "class Probe; end\n")
    assert_equal "pass", execute('puts "PASS 41 false nil"', artifacts: [artifact])["status"]
  end

  def test_unverified_provenance_does_not_execute
    %w[provenance-missing provenance-mismatch].each do |provenance|
      result = execute("File.write(#{File.join(@dir, 'sentinel').inspect}, 'executed')", provenance: provenance)
      assert_equal provenance, result["status"]
      assert_nil result["execution"]
      refute File.exist?("#{@dir}/sentinel")
    end
  end

  def test_no_command_and_unavailable_command_are_distinct
    assert_equal "not-run", Evidence.classify(nil, provenance: "verified")
    unavailable = Evidence.capture("#{@dir}/missing-tool", dir: @dir)
    assert_equal "environment-blocked", Evidence.classify(unavailable, provenance: "verified")
  end

  def test_provenance_checks_real_binary_hash_pin_origin_diff_and_build
    binary = "#{@dir}/compiler"
    File.write(binary, "#!/bin/sh\nexit 0\n")
    File.chmod(0o755, binary)
    source = {"head" => Evidence::SPINEL, "tree" => "e" * 40, "compiler_pin" => Evidence::SPINEL,
              "origin" => Evidence::ORIGINS.fetch("spinel"), "status" => "", "diff" => "", "compiler_diff" => ""}
    receipt = {"source_root" => @dir, "source" => source, "source_after" => source,
               "binary" => binary, "binary_sha256" => Evidence.sha(binary), "build" => [{"exit" => 0, "command" => ["make", "-B", "-j4"], "cwd" => @dir}],
               "version" => {"exit" => 0, "stdout" => "spinel (7147b681f)"}}
    assert_equal "verified", Evidence.provenance(receipt, source, "spinel")
    assert_equal "provenance-missing", Evidence.provenance(nil, source, "spinel")
    assert_equal "provenance-missing", Evidence.provenance(receipt.reject { |key, _| key == "build" }, source, "spinel")
    [source.reject { |key, _| key == "head" }, source.merge("compiler_pin" => "3f5753ad"), source.merge("origin" => "https://example.invalid/fork"),
     source.merge("compiler_diff" => "compiler patch"), source.merge("status" => " M src/main.c")].each do |wrong|
      assert_equal "provenance-mismatch", Evidence.provenance(receipt, wrong, "spinel")
    end
    assert_equal "provenance-mismatch", Evidence.provenance(receipt.merge("build" => [{"exit" => 1}]), source, "spinel")
    assert_equal "provenance-mismatch", Evidence.provenance(receipt.merge("build" => [{"exit" => 0, "command" => ["make", "-j4"], "cwd" => @dir}]), source, "spinel")
    assert_equal "provenance-mismatch", Evidence.provenance(receipt.merge("build" => [{"exit" => 0, "command" => ["make", "-B", "-j4"], "cwd" => "/wrong-source"}]), source, "spinel")
    assert_equal "provenance-mismatch", Evidence.provenance(receipt.merge("version" => {"exit" => 0, "stdout" => "spinel (other)"}), source, "spinel")
    File.write(binary, "#!/bin/sh\nexit 3\n")
    assert_equal "provenance-mismatch", Evidence.provenance(receipt, source, "spinel")
  end

  def test_gate_aggregates_runtime_not_only_compile_and_scopes_reference_only
    @run.define_singleton_method(:reference) { |*| {"status" => "pass"} }
    @run.define_singleton_method(:ruby_lane) { |*| {"status" => "pass"} }
    failed_binary = {"status" => "runtime-fail", "stages" => {"compile" => {"status" => "pass"}, "binary_contract" => {"status" => "runtime-fail"}}}
    @run.define_singleton_method(:spinel_lane) { |*, **| failed_binary }
    @run.instance_variable_get(:@options)[:reference_only] = false
    original_provenance = Evidence.method(:provenance)
    Evidence.define_singleton_method(:provenance) { |*| "verified" }
    assert_equal 1, @run.run
    report = JSON.parse(File.read("#{@dir}/output/results.json"))
    assert_equal "pass", report.dig("cases", "alba", "lanes", "roundhouse-spinel", "stages", "compile", "status")
    assert_equal "runtime-fail", report.dig("cases", "alba", "lanes", "roundhouse-spinel", "stages", "binary_contract", "status")
    @run.define_singleton_method(:spinel_lane) { |*, **| {"status" => "pass"} }
    assert_equal 0, @run.run
    @run.instance_variable_get(:@options)[:reference_only] = true
    @run.define_singleton_method(:spinel_lane) { |*, **| raise "reference-only must not execute compiler" }
    Evidence.define_singleton_method(:provenance) { |*| "provenance-missing" }
    assert_equal 0, @run.run
    report = JSON.parse(File.read("#{@dir}/output/results.json"))
    assert_equal "not-run", report.dig("cases", "alba", "lanes", "roundhouse-spinel", "status")
    assert_equal "not-run", report.dig("cases", "faraday", "lanes", "cruby4-original", "status")
  ensure
    Evidence.define_singleton_method(:provenance, original_provenance) if original_provenance
  end

  def test_every_explicit_consumer_has_a_zero_argument_action
    Cases.all(CorpusRun::ROOT).each do |id, row|
      consumer = File.read(row.fetch("consumer"))
      assert_includes consumer, "def index\n", id
      refute_match(/def index\(/, consumer, id)
      assert_equal 0, Evidence.capture(RbConfig.ruby, "-c", row.fetch("consumer"), dir: @dir)["exit"], id
    end
    pagy = File.read(Cases.all(CorpusRun::ROOT).fetch("pagy").fetch("consumer"))
    assert_includes pagy, "include Pagy::Method"
    assert_includes pagy, "[last_page, overflow]"
  end

  def test_original_source_success_stays_blocked_without_provider_or_extension_proof
    FileUtils.mkdir_p("#{@dir}/gem/lib")
    File.write("#{@dir}/gem/lib/alba.rb", "module Alba; end\n")
    @run.instance_variable_set(:@tools, {"spinel" => {"provenance" => "verified", "receipt" => {"binary" => "unused"}}})
    @run.define_singleton_method(:step) { |lane, name, *, **| lane[name] = {"status" => "pass"} }
    [[], ["ext/extconf.rb"]].each_with_index do |extensions, index|
      dir = "#{@dir}/projection-#{index}"
      FileUtils.mkdir_p(dir)
      ref = {"gem_sources" => [{"name" => "alba", "paths" => ["#{@dir}/gem/lib"], "extensions" => extensions}]}
      lane = @run.spinel_lane(Cases.all(CorpusRun::ROOT).fetch("alba"), dir, "#{dir}/input", ref, emitted: false)
      assert_equal "pass", lane.dig("stages", "compile", "status")
      assert_equal "pass", lane.dig("stages", "binary_contract", "status")
      assert_equal "blocked", lane["status"]
      assert_equal "blocked", lane.dig("transitive_source_providers", "status")
      assert_includes lane["reason"], extensions.empty? ? "providers unverified" : "CRuby C extensions"
    end
  end

  def test_empty_duplicate_and_unknown_selections_fail_not_pass
    ["", "alba,alba", "not-a-case"].each_with_index do |ids, index|
      run = CorpusRun.new(out: "#{@dir}/selection-#{index}", cases: ids, reference_only: true)
      assert_raises(RuntimeError) { run.run }
    end
  end

  def test_frozen_historical_bytes_and_case_count
    root = File.expand_path(__dir__)
    lines = File.readlines("#{root}/historical.sha256")
    lines.each do |line|
      digest, path = line.split("  ", 2)
      assert_equal digest, Evidence.sha("#{root}/#{path.chomp}"), path
    end
    assert_equal 14, Cases::HISTORICAL.size
    assert_equal 16, Cases.all(CorpusRun::ROOT).size
  end
end
