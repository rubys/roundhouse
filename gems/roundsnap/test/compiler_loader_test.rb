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

    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    assert File.file?(File.join(@dir, "manifest.json"))
    assert File.file?(File.join(@dir, "iseq/leaf.iseq"))

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
    assert_equal "real-blog/app/models/article.rb", PROBE_FILE
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

  def test_boot_loads_all_units_in_order
    units = [
      {
        "key" => "a",
        "source" => "A_LOADED = true\n",
        "file" => "a.rb",
        "first_lineno" => 1,
      },
      {
        "key" => "b",
        "source" => "B_SEES_A = defined?(A_LOADED)\n",
        "file" => "b.rb",
        "first_lineno" => 1,
      },
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
    Roundsnap::Compiler.compile!(units: good, out_dir: @dir)
    prior = File.binread(File.join(@dir, "iseq/keep.iseq"))
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
    assert_equal prior, File.binread(File.join(@dir, "iseq/keep.iseq"))
    assert_equal prior_manifest, File.read(File.join(@dir, "manifest.json"))
    refute File.exist?(File.join(@dir, ".iseq.staging-#{Process.pid}")),
           "staging dir must not linger"
  end

  def test_outside_gem_caller_ignores_app_paths_named_roundsnap
    # A frame under /tmp/campfire-roundsnap/ must count as OUTSIDE the gem.
    app_path = "/tmp/campfire-roundsnap/app/models/thing.rb"
    gem_lib = Roundsnap::Loader::RequireHook::GEM_LIB
    gem_frame = File.join(gem_lib, "roundsnap", "loader.rb")
    # Synthetic: expand_path of app path is not under gem_lib.
    refute app_path.start_with?(gem_lib + File::SEPARATOR)
    assert gem_frame.start_with?(gem_lib + File::SEPARATOR)
    # Real caller from this test file (not under gem lib).
    loc = Roundsnap::Loader::RequireHook.outside_gem_caller
    assert loc, "expected a caller outside the gem"
    path = File.expand_path(loc.absolute_path || loc.path)
    refute path.start_with?(gem_lib + File::SEPARATOR),
           "test file must not be treated as gem lib: #{path}"
  end
end
