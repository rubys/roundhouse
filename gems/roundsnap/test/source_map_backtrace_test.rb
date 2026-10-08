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
