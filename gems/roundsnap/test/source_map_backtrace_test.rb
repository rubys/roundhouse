# frozen_string_literal: true

require "minitest/autorun"
require "fileutils"
require "tmpdir"
require_relative "../lib/roundsnap"

class SourceMapBacktraceTest < Minitest::Test
  def setup
    @dir = Dir.mktmpdir("roundsnap_map_")
  end

  def teardown
    Roundsnap::Loader.current = nil
    FileUtils.remove_entry(@dir)
  end

  def test_model_and_erb_source_frames_are_formatted_not_rewritten
    { "ModelProbe" => ["app/models/thing.rb", 12],
      "ErbProbe" => ["app/views/articles/_article.html.erb", 4] }.each do |name, (file, line)|
      source = "module #{name}\n#<SPINEL_SOURCE>#{file}:1\ndef self.boom\n#<SPINEL_SOURCE>#{file}:#{line}\nraise 'probe'\nend\nend\n"
      Roundsnap::Compiler.compile!(units: [{ "key" => name, "source" => source }], out_dir: @dir)
      loader = Roundsnap::Loader.install!(root: @dir)
      loader.require(name)
      err = assert_raises(RuntimeError) { Object.const_get(name).boom }
      original = err.backtrace.dup
      assert_match(/#{Regexp.escape(@dir)}\/#{name}\.rb:5/, original.first)
      assert_match(/#{Regexp.escape(file)}:#{line}:/, loader.format_backtrace(original).first)
      assert_equal original, err.backtrace, "formatting must not mutate native exception locations"
    end
  end

  def test_markers_preserve_caller_metadata_and_map_native_line_offsets
    source = <<~RUBY
      #<SPINEL_SOURCE>app/models/original.rb:42
      def metadata_probe
        [__FILE__, __dir__, __LINE__]
      end
      def metadata_boom
      #<SPINEL_SOURCE>app/views/original.html.erb:7
        raise 'metadata-probe'
      #<SPINEL_SOURCE>generated/wrapper.rb:99
      end
    RUBY
    # Adjacent markers make either off-by-one translation fail instead of
    # landing on a neighboring statement with the same held source span.
    # Include zero/negative MRI offsets and a string accepted by Integer().
    { 1 => [3, 7], 20 => [22, 26], 0 => [2, 6], -20 => [-18, -14], "20" => [22, 26] }.each do |first, (read_line, raise_line)|
      key = "logical/probe.rb"
      file = "chosen/probe.rb"
      unit = { key: key, source: source, file: file, first_lineno: first }
      manifest = Roundsnap::Compiler.compile!(units: [unit], out_dir: @dir)
      assert_equal [key], manifest.fetch("units").keys, "markers must not normalize the caller's key"
      entry = manifest.fetch("units").fetch(key)
      assert_equal file, entry.fetch("file")
      assert_equal Integer(first), entry.fetch("first_lineno")
      assert_equal({ "file" => "app/views/original.html.erb", "line" => 7 }, entry.fetch("source_map")["7"])
      loader = Roundsnap::Loader.install!(root: @dir)
      loader.require(key)
      assert_equal [File.join(@dir, file), File.join(@dir, "chosen"), read_line], metadata_probe
      error = assert_raises(RuntimeError) { metadata_boom }
      native = error.backtrace.dup
      assert_match(/\A#{Regexp.escape(File.join(@dir, file))}:#{raise_line}:in .*metadata_boom/, native.first)
      assert_match(/\Aapp\/views\/original\.html\.erb:7:in .*metadata_boom/, loader.format_backtrace(native).first)
      assert_equal native, error.backtrace
      assert_equal native.drop(1), loader.format_backtrace(native).drop(1), "unmapped caller frames stay unchanged"
    end
  end

  def test_marked_custom_paths_resolve_requires_without_using_source_origins
    units = [
      { "key" => "logical/entry", "file" => "chosen/entry.rb", "first_lineno" => 23,
        "source" => "#<SPINEL_SOURCE>original/entry.rb:90\nrequire_relative 'leaf'\n" },
      { "key" => "logical/leaf", "file" => "chosen/leaf.rb", "first_lineno" => 37,
        "source" => "#<SPINEL_SOURCE>original/leaf.rb:4\nCUSTOM_PATH_PROBE = [__FILE__, __dir__, __LINE__]\n" },
    ]
    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    loader = Roundsnap::Loader.install!(root: @dir)
    assert_equal "logical/entry", loader.resolve_key(File.join(@dir, "chosen/entry.rb"))
    assert_nil loader.resolve_key(File.join(@dir, "original/entry.rb"))
    assert require(File.join(@dir, "chosen/entry"))
    assert loader.loaded?("logical/leaf")
    assert_equal [File.join(@dir, "chosen/leaf.rb"), File.join(@dir, "chosen"), 38], CUSTOM_PATH_PROBE
  end

  def test_backwards_and_colliding_markers_never_reorder_statements
    # rubys' actual failure sequence: a body marker jumps before its def.
    source = <<~RUBY
      module OrderProbe
      #<SPINEL_SOURCE>app/models/t.rb:20
        def self.run
          log = []
      #<SPINEL_SOURCE>app/models/t.rb:5
          log << :first
      #<SPINEL_SOURCE>app/models/t.rb:5
          log << :second
          log
        end
      end
    RUBY
    unit = Roundsnap::SourceMap.units_from(source, emit_key: "order").first
    assert_equal source, unit["source"]
    Roundsnap::Compiler.compile!(units: [unit], out_dir: @dir)
    Roundsnap::Loader.install!(root: @dir).require("order")
    assert_equal [:first, :second], OrderProbe.run
    assert_equal({ "file" => "app/models/t.rb", "line" => 5 }, unit["source_map"]["9"])
  end

  def test_spliced_concerns_can_map_different_files_without_splitting_wrappers
    source = <<~RUBY
      module ConcernProbe
      #<SPINEL_SOURCE>app/models/a.rb:10
        def self.a = 3
      #<SPINEL_SOURCE>app/models/a/concern.rb:2
        def self.b = 7
      end
    RUBY
    unit = Roundsnap::SourceMap.units_from(source, emit_key: "mixed").first
    assert_equal source, unit["source"]
    assert_equal({ "file" => "app/models/a/concern.rb", "line" => 2 }, unit["source_map"]["5"])
    Roundsnap::Compiler.compile!(units: [unit], out_dir: @dir)
    Roundsnap::Loader.install!(root: @dir).require("mixed")
    assert_equal [3, 7], [ConcernProbe.a, ConcernProbe.b]
  end

  def test_heredocs_blank_lines_and_marker_looking_data_are_unchanged
    source = <<~'RUBY'
      #<SPINEL_SOURCE>app/x.rb:1
      HEREDOC_PROBE = <<~TEXT
        a

        #<SPINEL_SOURCE>not-a-marker.rb:0
        b
      TEXT
    RUBY
    unit = Roundsnap::SourceMap.units_from(source, emit_key: "heredoc").first
    assert_equal source, unit["source"]
    assert_equal({ "file" => "app/x.rb", "line" => 1 }, unit["source_map"]["6"])
    Roundsnap::Compiler.compile!(units: [unit], out_dir: @dir)
    Roundsnap::Loader.install!(root: @dir).require("heredoc")
    assert_equal "a\n\n#<SPINEL_SOURCE>not-a-marker.rb:0\nb\n", HEREDOC_PROBE
  end

  def test_line_zero_fails_instead_of_dropping_code
    assert_raises(ArgumentError) do
      Roundsnap::SourceMap.units_from("#<SPINEL_SOURCE>x.rb:0\ndef kept = 1\n", emit_key: "zero")
    end
  end

  def test_unmarked_units_have_honest_emitted_locations
    source = "def unmarked_boom\n  raise 'x'\nend\n"
    unit = Roundsnap::SourceMap.units_from(source, emit_key: "app/models/thing").first
    refute unit["mapped"]
    assert_equal "app/models/thing.rb", unit["file"]
    assert_equal source, unit["source"]
    assert_empty unit["source_map"]
  end
end
