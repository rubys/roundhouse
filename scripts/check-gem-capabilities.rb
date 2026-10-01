#!/usr/bin/env ruby
require "optparse"
require "pathname"
require_relative "../fixtures/gem-capabilities/evidence"
require_relative "../fixtures/gem-capabilities/cases"

class CorpusRun
  ROOT = File.expand_path("..", __dir__)
  BUNDLE = [RbConfig.ruby, File.join(File.dirname(RbConfig.ruby), "bundle"), "_2.6.9_"].freeze

  def initialize(options)
    @options = options
    @out = File.expand_path(options.fetch(:out))
    raise "output already exists: #{@out}" if File.exist?(@out)
    FileUtils.mkdir_p(@out)
    @tools = %w[roundhouse spinel].to_h do |name|
      path = options[name.to_sym]
      receipt = nil
      actual = nil
      receipt = JSON.parse(File.read(path)) if path && File.file?(path)
      actual = Evidence.observe_tool(receipt, name)
      provenance = Evidence.provenance(receipt, actual, name)
      [name, {"receipt" => receipt, "observed_source" => actual, "provenance" => provenance}]
    end
    bundler = Evidence.capture(*BUNDLE, "--version", dir: ROOT)
    @reference_provenance = RUBY_VERSION == "4.0.7" && RUBY_PATCHLEVEL == 0 && bundler["exit"] == 0 && bundler["stdout"] == "Bundler version 2.6.9\n" ? "verified" : "provenance-mismatch"
    @report = {"schema" => 1, "started" => Time.now.utc.iso8601, "ruby" => RUBY_DESCRIPTION,
               "ruby_executable" => RbConfig.ruby, "bundler" => bundler,
               "tools" => @tools, "runner_sources" => Evidence.files(File.join(ROOT, "fixtures/gem-capabilities")).merge("scripts/check-gem-capabilities.rb" => Evidence.sha(__FILE__)),
               "cases" => {}, "scope" => "synthetic module consumers; Ruby output boot without dispatch; no Rails engine, app/DB/HTTP, no native replacement passes"}
  end

  def step(lane, name, argv, dir:, env: {}, provenance: "verified", artifacts: [], expected: nil, failure: "runtime-fail")
    result = Evidence.capture(*argv, dir: dir, env: env) if provenance == "verified"
    lane[name] = {"status" => Evidence.classify(result, provenance: provenance, artifacts: artifacts, expected: expected, failure: failure), "execution" => result}
    lane[name]
  end

  def skip(reason, status = "not-run")
    {"status" => status, "reason" => reason}
  end

  def bundle_env(dir)
    {"BUNDLE_GEMFILE" => File.join(dir, "Gemfile"), "BUNDLE_FROZEN" => "true", "RUBYOPT" => nil, "RUBYLIB" => nil}
  end

  def input(row, dir)
    FileUtils.cp_r(File.join(ROOT, "fixtures/gem-capabilities/historical/base"), dir)
    FileUtils.mkdir_p(File.join(dir, "app/lib"))
    FileUtils.cp(row.fetch("source"), File.join(dir, "app/lib/survey_probe.rb"))
    Dir.glob("#{row.fetch('classes')}/*.rb").each { |file| FileUtils.cp(file, File.join(dir, "app/lib")) }
    gem = row.fetch("gem")
    manifest = "source \"https://rubygems.org\"\nruby \"4.0.7\"\ngem #{gem.inspect}, #{row.fetch('version').inspect}\n"
    manifest += "gem \"alba\", \"4.0.0\"\n" if gem == "typelizer"
    manifest += "gem \"railties\", \"8.1.4\"\n" if gem == "oj_serializers"
    manifest += "gem \"activerecord\", \"8.1.4\"\n" if gem == "panko_serializer"
    File.write(File.join(dir, "Gemfile"), manifest)
    FileUtils.cp(row.fetch("lock"), File.join(dir, "Gemfile.lock"))
  end

  def reference(row, dir)
    lane = {"scope" => "CRuby4 + original locked gem", "stages" => {}}
    stages = lane["stages"]
    env = bundle_env(dir)
    original_lock = Evidence.sha(File.join(dir, "Gemfile.lock"))
    step(stages, "install", BUNDLE + ["install", "--jobs", "4"], dir: dir, env: env, provenance: @reference_provenance, failure: "dependency-blocked")
    step(stages, "lock", BUNDLE + ["check"], dir: dir, env: env, provenance: @reference_provenance, failure: "dependency-blocked")
    stage = step(stages, "contract", [RbConfig.ruby, "-rbundler/setup", *row.fetch("requires").map { |r| "-r#{r}" }, "-r#{dir}/app/lib/survey_probe", row.fetch("contract")],
                 dir: dir, env: env, provenance: @reference_provenance, expected: row.fetch("expected_stdout"), failure: "invalid-reference")
    lane["lock_unchanged"] = original_lock == Evidence.sha(File.join(dir, "Gemfile.lock"))
    metadata_code = 'require "json"; puts JSON.generate(Bundler.load.specs.map { |s| {name: s.name, version: s.version.to_s, root: s.full_gem_path, paths: s.full_require_paths, extensions: s.extensions, package: s.cache_file} })'
    metadata = step(stages, "source_metadata", [RbConfig.ruby, "-rbundler/setup", "-e", metadata_code], dir: dir, env: env, provenance: @reference_provenance)
    if metadata["status"] == "pass"
      lane["gem_sources"] = JSON.parse(metadata["execution"]["stdout"]).map do |spec|
        spec.merge("files_sha256" => Evidence.files(File.join(spec.fetch("root"), "lib")),
                   "extension_source_sha256" => Evidence.files(File.join(spec.fetch("root"), "ext")),
                   "package_sha256" => Evidence.sha(spec.fetch("package")))
      end
    end
    lane["status"] = if stage["status"] == "pass" && stages["install"]["status"] == "pass" && stages["lock"]["status"] == "pass" && lane["lock_unchanged"] && lane["gem_sources"]
      "pass"
    elsif @reference_provenance != "verified"
      @reference_provenance
    else
      "invalid-reference"
    end
    lane
  end

  def ruby_lane(row, dir, input_dir)
    tool = @tools.fetch("roundhouse")
    provenance = tool.fetch("provenance")
    binary = tool.dig("receipt", "binary")
    lane = {"scope" => "Roundhouse consumer → Ruby4 output project; no-preload boot + independent contract", "stages" => {}}
    stages = lane["stages"]
    if provenance != "verified"
      lane["status"] = provenance
      return lane
    end
    step(stages, "library_strict", [binary, "check", input_dir], dir: ROOT, failure: "rejected")
    step(stages, "library_continue", [binary, "check", "--continue", input_dir], dir: ROOT, failure: "rejected")
    mirror = File.join(dir, "consumer-input")
    FileUtils.cp_r(input_dir, mirror)
    # Explicit analysis consumers; not a library-only or Rails dispatch support claim.
    FileUtils.cp(row.fetch("consumer"), File.join(mirror, "app/controllers/surveys_controller.rb"))
    lane["consumer_input_sha256"] = Evidence.files(mirror)
    step(stages, "consumer_strict", [binary, "check", mirror], dir: ROOT, failure: "rejected")
    step(stages, "consumer_continue", [binary, "check", "--continue", mirror], dir: ROOT, failure: "rejected")
    emitted = File.join(dir, "ruby")
    emit = step(stages, "emit", [binary, "--target", "ruby", input_dir, "-o", emitted], dir: ROOT,
                artifacts: ["#{emitted}/main.rb", "#{emitted}/Gemfile", "#{emitted}/app/models/survey_probe.rb"], failure: "rejected")
    lane["lowering_residue"] = emit.dig("execution", "stderr").to_s.lines.select { |line| line.include?("lower_residue") }
    unless emit["status"] == "pass"
      lane["status"] = emit["status"]
      return lane
    end
    before = Evidence.files(emitted)
    lane["emitted_sha256"] = before
    ruby_files = Dir.glob("#{emitted}/**/*.rb")
    ruby_files.each do |file|
      step(stages, "syntax:#{file.delete_prefix(emitted + '/')}", [RbConfig.ruby, "-c", file], dir: ROOT, failure: "syntax-fail")
    end
    env = bundle_env(emitted).merge("BUNDLE_FROZEN" => nil, "SECRET_KEY_BASE" => "public-synthetic-corpus-value")
    step(stages, "output_install", BUNDLE + ["install", "--jobs", "4"], dir: emitted, env: env, failure: "dependency-blocked")
    bundle_check = step(stages, "output_check", BUNDLE + ["check"], dir: emitted, env: env, failure: "dependency-blocked")
    if bundle_check["status"] == "pass"
      step(stages, "boot_no_preload", [RbConfig.ruby, "-rbundler/setup", "-e", "require './main'; puts 'BOOT no dispatch/DB'"],
           dir: emitted, env: env, expected: "BOOT no dispatch/DB\n")
      step(stages, "contract_no_preload", [RbConfig.ruby, "-rbundler/setup", "-e", "require './main'; load #{row.fetch('contract').inspect}"],
           dir: emitted, env: env, expected: row.fetch("expected_stdout"))
      step(stages, "loaded_output_specs", [RbConfig.ruby, "-rbundler/setup", "-e", 'require "json"; puts JSON.generate(Gem.loaded_specs.transform_values { |s| s.version.to_s })'], dir: emitted, env: env)
    else
      stages["boot_no_preload"] = skip("output bundle unavailable", "dependency-blocked")
      stages["contract_no_preload"] = skip("output bundle unavailable", "dependency-blocked")
    end
    after = Evidence.files(emitted)
    lane["output_unchanged"] = before.all? { |path, digest| after[path] == digest }
    lane["output_lock_sha256"] = Evidence.sha("#{emitted}/Gemfile.lock")
    lane["status"] = lane["output_unchanged"] ? (stages.values.find { |stage| stage["status"] != "pass" } || stages.fetch("contract_no_preload"))["status"] : "artifact-mutated"
    lane
  end

  def spinel_lane(row, dir, input_dir, reference, emitted:)
    tool = @tools.fetch("spinel")
    provenance = tool.fetch("provenance")
    binary = tool.dig("receipt", "binary")
    lane = {"scope" => emitted ? "Roundhouse→Spinel module compile + binary (not full-project/Rails boot)" : "original gem Ruby sources + consumer; CRuby native extensions not replaced", "stages" => {}}
    stages = lane["stages"]
    lane["full_project"] = skip("module contract is not full-project Spin build/Rails boot")
    if provenance != "verified"
      lane["status"] = provenance
      return lane
    end
    roots = reference.fetch("gem_sources").flat_map { |spec| spec.fetch("paths") }.uniq
    native = reference.fetch("gem_sources").select { |spec| !spec.fetch("extensions").empty? }.map { |spec| spec.fetch("name") }
    lane["native_extension_dependencies"] = native unless emitted
    lane["transitive_source_providers"] = skip("Spinel bundled libraries/packages precede -I roots; original transitive providers unverified", "blocked") unless emitted
    work = File.join(dir, emitted ? "spinel-emitted" : "spinel-original")
    if emitted
      rh = @tools.fetch("roundhouse")
      if rh["provenance"] != "verified"
        lane["status"] = rh["provenance"]
        return lane
      end
      emit = step(stages, "emit", [rh.dig("receipt", "binary"), "--target", "spinel", input_dir, "-o", work], dir: ROOT,
                  artifacts: ["#{work}/app/models/survey_probe.rb", "#{work}/main.rb"], failure: "rejected")
      lane["lowering_residue"] = emit.dig("execution", "stderr").to_s.lines.select { |line| line.include?("lower_residue") }
      unless emit["status"] == "pass"
        lane["status"] = emit["status"]
        return lane
      end
      source = "#{work}/app/models.rb"
      requires = ""
      flags = [] # Never preload original gems into Roundhouse emission.
    else
      FileUtils.mkdir_p(work)
      source = "#{input_dir}/app/lib/survey_probe.rb"
      # Absolute original entry files prevent a same-named native package being
      # substituted for the requested gem itself. -I alone does not prove the
      # original transitive providers won over Spinel's bundled libraries/packages.
      entries = row.fetch("requires").map do |feature|
        roots.map { |root| File.join(root, feature + ".rb") }.find { |path| File.file?(path) }
      end
      unless entries.all?
        lane["status"] = "blocked"
        lane["reason"] = "original gem entry unavailable; no substitute"
        return lane
      end
      requires = entries.map { |entry| "require_relative #{Pathname.new(entry.delete_suffix('.rb')).relative_path_from(Pathname.new(work)).to_s.inspect}\n" }.join
      flags = roots.flat_map { |root| ["-I", root] }
      lane["original_entries"] = entries.to_h { |entry| [entry, Evidence.sha(entry)] }
      lane["load_roots"] = roots
    end
    before = Evidence.files(work)
    FileUtils.cp(row.fetch("contract"), "#{work}/independent_contract.rb")
    raise "contract copy changed" unless Evidence.sha(row.fetch("contract")) == Evidence.sha("#{work}/independent_contract.rb")
    relative_source = Pathname.new(source.delete_suffix('.rb')).relative_path_from(Pathname.new(work)).to_s
    File.write("#{work}/driver.rb", requires + "require_relative #{relative_source.inspect}\nrequire_relative \"independent_contract\"\n")
    native_bin = "#{dir}/#{emitted ? 'emitted' : 'original'}-contract"
    compile = step(stages, "compile", [binary, "--require-gate", *flags, "driver.rb", "-o", native_bin], dir: work, artifacts: [native_bin], failure: "rejected")
    lane["binary_sha256"] = Evidence.sha(native_bin)
    if compile["status"] == "pass"
      step(stages, "binary_contract", [native_bin], dir: work, expected: row.fetch("expected_stdout"))
    else
      stages["binary_contract"] = skip("compile did not produce a passing artifact")
    end
    lane["emission_unchanged"] = before.all? { |path, digest| Evidence.sha("#{work}/#{path}") == digest }
    lane["status"] = compile["status"] == "pass" ? stages["binary_contract"]["status"] : compile["status"]
    if !emitted && lane["status"] == "pass"
      lane["status"] = "blocked"
      lane["reason"] = native.empty? ? "Original transitive source providers unverified; compile/binary success alone is not original-gem-source support" :
        "Ruby-source projection cannot prove original CRuby C extensions: #{native.join(', ')}; native packages are not passes"
    end
    lane["status"] = "artifact-mutated" unless lane["emission_unchanged"]
    lane
  end

  def run
    all = Cases.all(ROOT)
    ids = @options[:cases] == "all" ? all.keys : @options.fetch(:cases).split(",")
    raise "select at least one distinct case" if ids.empty? || ids.uniq != ids
    raise "unknown cases: #{ids - all.keys}" unless (ids - all.keys).empty?
    (all.keys - ids).each do |id|
      @report["cases"][id] = {"lanes" => %w[cruby4-original roundhouse-ruby4 original-spinel roundhouse-spinel].to_h { |name| [name, skip("not selected in this execution")] }}
    end
    ids.each do |id|
      row = all.fetch(id)
      dir = File.join(@out, id)
      FileUtils.mkdir_p(dir)
      input_dir = File.join(dir, "input")
      input(row, input_dir)
      result = {"case" => row, "input_sha256" => Evidence.files(input_dir), "contract_sha256" => Evidence.sha(row.fetch("contract")), "lanes" => {}}
      lanes = result["lanes"]
      ref = lanes["cruby4-original"] = reference(row, input_dir)
      if @options[:reference_only] || ref["status"] != "pass"
        %w[roundhouse-ruby4 original-spinel roundhouse-spinel].each { |name| lanes[name] = skip(@options[:reference_only] ? "reference-only run" : "reference invalid", ref["status"] == "pass" ? "not-run" : "invalid-reference") }
      else
        lanes["roundhouse-ruby4"] = ruby_lane(row, dir, input_dir)
        lanes["original-spinel"] = spinel_lane(row, dir, input_dir, ref, emitted: false)
        lanes["roundhouse-spinel"] = spinel_lane(row, dir, input_dir, ref, emitted: true)
      end
      @report["cases"][id] = result
      Evidence.write("#{@out}/results.json", @report)
      puts "#{id}: #{lanes.map { |name, lane| "#{name}=#{lane['status']}" }.join(' ')}"
    end
    @report["tool_provenance_after"] = @tools.to_h { |name, tool| [name, Evidence.provenance(tool["receipt"], Evidence.observe_tool(tool["receipt"], name), name)] }
    @report["finished"] = Time.now.utc.iso8601
    @report["artifact_sha256"] = Evidence.files(@out).reject { |path, _| path == "results.json" }
    Evidence.write("#{@out}/results.json", @report)
    # Unlike the historical observation runner, any requested non-pass fails the gate.
    requested = @options[:reference_only] ? ["cruby4-original"] : %w[cruby4-original roundhouse-ruby4 original-spinel roundhouse-spinel]
    success = ids.all? { |id| requested.all? { |lane| @report["cases"][id]["lanes"][lane]["status"] == "pass" } }
    success && (@options[:reference_only] || @report["tool_provenance_after"].values.all? { |v| v == "verified" }) ? 0 : 1
  end
end

if $PROGRAM_NAME == __FILE__
  options = {cases: Cases::CORE.join(","), reference_only: false}
  parser = OptionParser.new do |opts|
    opts.banner = "ruby scripts/check-gem-capabilities.rb --out NEW_DIR [--cases all|id,id] [--reference-only] [--roundhouse RECEIPT] [--spinel RECEIPT]"
    opts.on("--out PATH") { |v| options[:out] = v }
    opts.on("--cases IDS") { |v| options[:cases] = v }
    opts.on("--reference-only") { options[:reference_only] = true }
    opts.on("--roundhouse PATH") { |v| options[:roundhouse] = v }
    opts.on("--spinel PATH") { |v| options[:spinel] = v }
  end
  parser.parse!
  abort parser.to_s unless options[:out]
  exit CorpusRun.new(options).run
end
