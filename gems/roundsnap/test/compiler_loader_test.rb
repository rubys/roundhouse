# frozen_string_literal: true

require "minitest/autorun"
require "fileutils"
require "tmpdir"
require "json"

$LOAD_PATH.unshift File.expand_path("../lib", __dir__)
require "roundsnap"

class CompilerLoaderTest < Minitest::Test
  def setup
    @dir = Dir.mktmpdir("roundsnap_")
  end

  def teardown
    Roundsnap::Loader.current = nil
    FileUtils.remove_entry(@dir) if @dir && File.directory?(@dir)
  end

  def test_compile_and_load_sets_original_file_on_iseq
    units = [
      {
        "key" => "leaf",
        "source" => <<~RUBY,
          def leaf_boom
            raise "boom-from-leaf"
          end
        RUBY
        "file" => "app/views/greeting.html.erb",
        "first_lineno" => 12,
      },
      {
        "key" => "lib",
        "source" => <<~RUBY,
          def run_lib
            leaf_boom
          end
        RUBY
        "file" => "app/models/thing.rb",
        "first_lineno" => 1,
      },
    ]

    manifest = Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    assert File.file?(File.join(@dir, "manifest.json"))
    assert File.file?(File.join(@dir, manifest["units"]["leaf"]["iseq"]))

    loader = Roundsnap::Loader.install!(root: @dir)
    loader.require("leaf")
    loader.require("lib")

    err = assert_raises(RuntimeError) { run_lib }
    frames = err.backtrace
    leaf = frames.find { |f| f.include?("app/views/greeting.html.erb") }
    assert leaf, "expected original path in backtrace, got:\n#{frames.first(5).join("\n")}"
    # first_lineno 12 → `def` on 12, `raise` on 13 (not iseq/leaf.iseq noise)
    assert_match(%r{app/views/greeting\.html\.erb:13}, leaf, leaf)
    assert frames.any? { |f| f.include?("app/models/thing.rb") },
           "expected caller frame with original path, got:\n#{frames.first(5).join("\n")}"
    refute frames.any? { |f| f.include?("iseq/") || f.end_with?(".iseq") },
           "backtrace must not name iseq blob paths:\n#{frames.first(8).join("\n")}"
  end

  def test_file_constant_is_original_path
    units = [
      {
        "key" => "probe",
        "source" => "PROBE_FILE = __FILE__\n",
        "file" => "real-blog/app/models/article.rb",
        "first_lineno" => 1,
      },
    ]
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    Roundsnap::Loader.install!(root: @dir).require("probe")
    assert_equal File.join(@dir, "real-blog/app/models/article.rb"), PROBE_FILE
  end

  def test_second_require_is_idempotent
    units = [
      {
        "key" => "once",
        "source" => "ONCE_COUNT = (defined?(ONCE_COUNT) ? ONCE_COUNT : 0) + 1\n",
        "file" => "once.rb",
        "first_lineno" => 1,
      },
    ]
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    loader = Roundsnap::Loader.install!(root: @dir)
    assert loader.require("once")
    assert_equal false, loader.require("once")
    assert_equal 1, ONCE_COUNT
  end

  def test_stdlib_require_still_works
    units = [
      {
        "key" => "noop",
        "source" => "NOOP = true\n",
        "file" => "noop.rb",
        "first_lineno" => 1,
      },
    ]
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    Roundsnap::Loader.install!(root: @dir)
    require "json"
    assert_equal 1, JSON.parse('{"a":1}')["a"]
  end

  def test_cli_compile
    units_path = File.join(@dir, "units.json")
    out = File.join(@dir, "out")
    File.write(units_path, JSON.generate([
      {
        "key" => "cli",
        "source" => "CLI_OK = true\n",
        "file" => "cli.rb",
        "first_lineno" => 1,
      },
    ]))
    exe = File.expand_path("../exe/roundsnap-compile", __dir__)
    FileUtils.chmod("+x", exe)
    system(RbConfig.ruby, exe, "--out", out, "--units", units_path)
    assert $?.success?, "cli failed"
    loader = Roundsnap::Loader.install!(root: out)
    loader.require("cli")
    assert CLI_OK
  end

  def test_boot_loads_only_entry_and_its_dependencies
    units = [
      {
        "key" => "a",
        "source" => "A_LOADED = true\nrequire_relative 'b'\n",
        "file" => "a.rb",
        "first_lineno" => 1,
      },
      {
        "key" => "b",
        "source" => "B_SEES_A = defined?(A_LOADED)\n",
        "file" => "b.rb",
        "first_lineno" => 1,
      },
      { "key" => "lazy", "source" => "raise 'must stay lazy'\n", "file" => "lazy.rb" },
    ]
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    Roundsnap::Loader.install!(root: @dir).boot!("a")
    assert A_LOADED
    assert B_SEES_A
  end

  def test_rejects_path_traversal_key
    units = [
      {
        "key" => "../escape",
        "source" => "X = 1\n",
        "file" => "x.rb",
        "first_lineno" => 1,
      },
    ]
    assert_raises(ArgumentError) do
      Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    end
  end

  def test_failed_eval_allows_retry
    units = [
      {
        "key" => "boom",
        "source" => "raise 'unit-boom'\n",
        "file" => "boom.rb",
        "first_lineno" => 1,
      },
    ]
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    loader = Roundsnap::Loader.install!(root: @dir)
    assert_raises(RuntimeError) { loader.require("boom") }
    assert_equal false, loader.loaded?("boom")
    # Still raises (source unchanged) but was not stuck as "loaded".
    assert_raises(RuntimeError) { loader.require("boom") }
  end

  def test_circular_require_relative_does_not_stack_overflow
    units = [
      {
        "key" => "a",
        "source" => "require_relative \"b\"\nCIRC_A = 1\n",
        "file" => "a.rb",
        "first_lineno" => 1,
      },
      {
        "key" => "b",
        "source" => "require_relative \"a\"\nCIRC_B = 1\n",
        "file" => "b.rb",
        "first_lineno" => 1,
      },
    ]
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    Roundsnap::Loader.install!(root: @dir).boot!
    assert_equal 1, CIRC_A
    assert_equal 1, CIRC_B
  end

  def test_yjit_description_mismatch_is_tolerated
    units = [
      {
        "key" => "ok",
        "source" => "OK = true\n",
        "file" => "ok.rb",
        "first_lineno" => 1,
      },
    ]
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    path = File.join(@dir, "manifest.json")
    man = JSON.parse(File.read(path))
    # Simulate compile without YJIT, run with +YJIT in the description string.
    bare = Roundsnap::Loader.normalize_ruby_description(RUBY_DESCRIPTION)
    man["ruby_description"] = bare
    File.write(path, JSON.pretty_generate(man) + "\n")
    Roundsnap::Loader.install!(root: @dir).require("ok")
    assert OK
  end

  def test_manifest_records_compile_option
    units = [
      {
        "key" => "opt",
        "source" => "OPT = true\n",
        "file" => "opt.rb",
        "first_lineno" => 1,
      },
    ]
    man = Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    assert_equal Roundsnap::Compiler.compile_option_fingerprint, man["compile_option"]
    Roundsnap::Loader.install!(root: @dir).require("opt")
    assert OPT
  end

  def test_compile_option_mismatch_raises_rebuild_hint
    units = [
      {
        "key" => "opt",
        "source" => "OPT2 = true\n",
        "file" => "opt.rb",
        "first_lineno" => 1,
      },
    ]
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    path = File.join(@dir, "manifest.json")
    man = JSON.parse(File.read(path))
    man["compile_option"] = { "tailcall_optimization" => true }
    File.write(path, JSON.pretty_generate(man) + "\n")
    err = assert_raises(LoadError) do
      Roundsnap::Loader.install!(root: @dir).require("opt")
    end
    assert_match(/compile_option mismatch/, err.message)
    assert_match(/rebuild/i, err.message)
  end

  def test_failed_compile_leaves_prior_iseq_intact
    good = [
      {
        "key" => "keep",
        "source" => "KEEP = true\n",
        "file" => "keep.rb",
        "first_lineno" => 1,
      },
    ]
    manifest = Roundsnap::Compiler.compile!(units: good, out_dir: @dir)
    binary = File.join(@dir, manifest["units"]["keep"]["iseq"])
    prior = File.binread(binary)
    prior_manifest = File.read(File.join(@dir, "manifest.json"))

    bad = [
      {
        "key" => "keep",
        "source" => "def broken(\n",
        "file" => "keep.rb",
        "first_lineno" => 1,
      },
    ]
    assert_raises(SyntaxError) do
      Roundsnap::Compiler.compile!(units: bad, out_dir: @dir)
    end
    assert_equal prior, File.binread(binary)
    assert_equal prior_manifest, File.read(File.join(@dir, "manifest.json"))
    assert_empty Dir.glob(File.join(@dir, ".iseq.staging-*")), "staging must not linger"
  end

  def test_previous_manifest_version_asks_for_rebuild
    File.write(File.join(@dir, "manifest.json"), JSON.generate({ "version" => 1, "units" => {} }))
    error = assert_raises(LoadError) { Roundsnap::Loader.new(root: @dir) }
    assert_match(/unsupported manifest version; rebuild/, error.message)
  end

  def test_load_error_and_syntax_error_reservations_are_cleared
    ["LoadError", "SyntaxError"].each do |type|
      Roundsnap::Compiler.compile!(units: [{ "key" => "retry", "source" => "raise #{type}, 'retry'", "file" => "retry.rb" }], out_dir: @dir)
      loader = Roundsnap::Loader.install!(root: @dir)
      2.times do
        assert_raises(Object.const_get(type)) { loader.require("retry") }
        refute loader.loaded?("retry")
      end
    end
  end

  def test_resolve_key_uses_exact_emitted_paths_not_substrings
    units = [
      {
        "key" => "app/models/thing",
        "source" => "THING = 1\n",
        "file" => "fixture/app/models/thing.rb",
        "first_lineno" => 1,
      },
      {
        "key" => "runtime/gzip_cache",
        "source" => "GZ = 1\n",
        "file" => "runtime/gzip_cache.rb",
        "first_lineno" => 1,
      },
    ]
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    loader = Roundsnap::Loader.new(root: @dir)
    assert_equal "app/models/thing", loader.resolve_key(File.join(@dir, "fixture/app/models/thing"))
    assert_nil loader.resolve_key("other/app/models/thing")
    assert_nil loader.resolve_key("real-blog/runtime/gzip_cache")
    # Absolute host paths must not steal manifest keys (Lambda /var/runtime, …).
    assert_nil loader.resolve_key("/var/runtime/gzip_cache")
    assert_nil loader.resolve_key("/opt/deploy/app/models/thing")
  end

  def test_gem_relative_requires_and_nested_load_errors_are_not_hijacked
    Roundsnap::Compiler.compile!(units: [{ "key" => "main", "source" => "raise 'hijacked'", "file" => "main.rb" }], out_dir: @dir)
    loader = Roundsnap::Loader.install!(root: @dir)
    other = File.join(@dir, "roundsnap", "other-gem")
    FileUtils.mkdir_p(other)
    File.write(File.join(other, "entry.rb"), "require_relative 'main'\n")
    File.write(File.join(other, "main.rb"), "GEM_MAIN_OK = 7\n")
    require File.join(other, "entry")
    assert_equal 7, GEM_MAIN_OK
    refute loader.loaded?("main")
    assert_nil loader.resolve_relative("main", nil)
    File.write(File.join(other, "broken.rb"), "require 'roundsnap_missing_dependency'\n")
    File.write(File.join(other, "nested.rb"), "require_relative 'broken'\n")
    err = assert_raises(LoadError) { require File.join(other, "nested") }
    assert_equal "roundsnap_missing_dependency", err.path, "preserve dependency LoadError instead of retrying .rb"
  end

  def test_old_loader_can_load_its_generation_after_recompile
    units = [{ "key" => "versioned", "source" => "GENERATION_OLD = 3", "file" => "versioned.rb" }]
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    old = Roundsnap::Loader.new(root: @dir)
    units[0]["source"] = "GENERATION_NEW = 7"
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    old.require("versioned")
    Roundsnap::Loader.new(root: @dir).require("versioned")
    assert_equal [3, 7], [GENERATION_OLD, GENERATION_NEW]
  end

  def test_duplicate_keys_fail_without_replacing_manifest
    unit = { "key" => "dup", "source" => "1", "file" => "dup.rb" }
    Roundsnap::Compiler.compile!(units: [unit], out_dir: @dir)
    before = File.read(File.join(@dir, "manifest.json"))
    assert_raises(ArgumentError) { Roundsnap::Compiler.compile!(units: [unit, unit], out_dir: @dir) }
    assert_equal before, File.read(File.join(@dir, "manifest.json"))
  end

  def test_file_and_dir_do_not_depend_on_runtime_cwd
    source = "FILE_DIR_PROBE = [__FILE__, __dir__]\n"
    Roundsnap::Compiler.compile!(units: [{ "key" => "app/probe", "source" => source, "file" => "app/probe.rb" }], out_dir: @dir)
    Dir.chdir("/") { Roundsnap::Loader.install!(root: @dir).require("app/probe") }
    assert_equal [File.join(@dir, "app/probe.rb"), File.join(@dir, "app")], FILE_DIR_PROBE
  end

  def test_moved_artifact_keeps_relative_requires_working
    units = [
      { "key" => "app/parent", "file" => "app/parent.rb", "source" => "require_relative 'child'\nrequire_relative '../config'\n" },
      { "key" => "app/child", "file" => "app/child.rb", "source" => "MOVED_CHILD_PROBE = 17\n" },
    ]
    original = File.join(@dir, "original")
    moved = File.join(@dir, "moved")
    Roundsnap::Compiler.compile!(units: units, out_dir: original)
    File.write(File.join(original, "config.rb"), "MOVED_CONFIG_PROBE = 19\n")
    FileUtils.mv(original, moved)
    Dir.chdir("/") { Roundsnap::Loader.install!(root: moved).require("app/parent") }
    assert_equal [17, 19], [MOVED_CHILD_PROBE, MOVED_CONFIG_PROBE]
    refute File.exist?(original)
  end

  def test_marker_looking_data_keeps_explicit_iseq_metadata
    unit = { "key" => "literal", "file" => "custom.rb", "first_lineno" => 23,
             "source" => "MARKER_LITERAL_PROBE = '#<SPINEL_SOURCE>data.rb:0'\nraise 'literal-probe'\n" }
    Roundsnap::Compiler.compile!(units: [unit], out_dir: @dir)
    error = assert_raises(RuntimeError) { Roundsnap::Loader.install!(root: @dir).require("literal") }
    assert_equal "#<SPINEL_SOURCE>data.rb:0", MARKER_LITERAL_PROBE
    assert_match(%r{#{Regexp.escape(@dir)}/custom\.rb:24:}, error.backtrace.first)
  end

  def test_concurrent_requires_wait_for_completed_initialization
    source = "sleep 0.05\nTHREAD_PROBE = 9\n"
    Roundsnap::Compiler.compile!(units: [{ "key" => "threaded", "source" => source, "file" => "threaded.rb" }], out_dir: @dir)
    loader = Roundsnap::Loader.install!(root: @dir)
    results = 2.times.map { Thread.new { [loader.require("threaded"), THREAD_PROBE] } }.map(&:value)
    assert_equal [false, true], results.map(&:first).sort_by(&:to_s)
    assert_equal [9, 9], results.map(&:last)
  end

  def test_unit_can_join_thread_loading_another_unit
    units = [
      { "key" => "parent", "file" => "parent.rb", "source" => "Thread.new { require_relative 'child' }.value\n" },
      { "key" => "child", "file" => "child.rb", "source" => "THREAD_CHILD_PROBE = 13\n" },
    ]
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    loader = Roundsnap::Loader.install!(root: @dir)
    assert loader.require("parent")
    assert_equal 13, THREAD_CHILD_PROBE
    assert loader.loaded?("child")
  end
end
