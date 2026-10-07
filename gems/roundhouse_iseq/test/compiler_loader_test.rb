# frozen_string_literal: true

require "minitest/autorun"
require "fileutils"
require "tmpdir"
require "json"

$LOAD_PATH.unshift File.expand_path("../lib", __dir__)
require "roundhouse_iseq"

class CompilerLoaderTest < Minitest::Test
  def setup
    @dir = Dir.mktmpdir("roundhouse_iseq_")
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

    RoundhouseIseq::Compiler.compile!(units: units, out_dir: @dir)
    assert File.file?(File.join(@dir, "manifest.json"))
    assert File.file?(File.join(@dir, "iseq/leaf.iseq"))

    loader = RoundhouseIseq::Loader.install!(root: @dir)
    loader.require("leaf")
    loader.require("lib")

    err = assert_raises(RuntimeError) { run_lib }
    frames = err.backtrace
    assert frames.any? { |f| f.include?("app/views/greeting.html.erb") },
           "expected original path in backtrace, got:\n#{frames.first(5).join("\n")}"
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
    RoundhouseIseq::Compiler.compile!(units: units, out_dir: @dir)
    RoundhouseIseq::Loader.install!(root: @dir).require("probe")
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
    RoundhouseIseq::Compiler.compile!(units: units, out_dir: @dir)
    loader = RoundhouseIseq::Loader.install!(root: @dir)
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
    RoundhouseIseq::Compiler.compile!(units: units, out_dir: @dir)
    RoundhouseIseq::Loader.install!(root: @dir)
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
    exe = File.expand_path("../exe/roundhouse-iseq-compile", __dir__)
    FileUtils.chmod("+x", exe)
    system(RbConfig.ruby, exe, "--out", out, "--units", units_path)
    assert $?.success?, "cli failed"
    loader = RoundhouseIseq::Loader.install!(root: out)
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
    RoundhouseIseq::Compiler.compile!(units: units, out_dir: @dir)
    RoundhouseIseq::Loader.install!(root: @dir).boot!("a")
    assert A_LOADED
    assert B_SEES_A
  end
end
