# frozen_string_literal: true

require "minitest/autorun"
require "fileutils"
require "tmpdir"
require "json"

$LOAD_PATH.unshift File.expand_path("../lib", __dir__)
require "roundsnap"

# Hard assertions: backtrace lines name the *source* line, not the
# denser emitted line. Covers an app .rb method and an ERB view —
# matching the Spinel `#<SPINEL_SOURCE>` shape (unmarked wrappers,
# then markers on def / body).
class SourceMapBacktraceTest < Minitest::Test
  def setup
    @dir = Dir.mktmpdir("roundsnap_map_")
  end

  def teardown
    FileUtils.remove_entry(@dir) if @dir && File.directory?(@dir)
  end

  def test_mapped_ruby_method_backtrace_hits_source_line
    # Unmarked class wrapper, then markers — raise is source line 12.
    marked = <<~RUBY
      class Thing
      #<SPINEL_SOURCE>app/models/thing.rb:10
        def self.boom
      #<SPINEL_SOURCE>app/models/thing.rb:12
          raise "boom-from-model"
        end
      end
    RUBY

    units = Roundsnap::SourceMap.units_from(marked, emit_key: "app/models/thing")
    assert units[0]["mapped"]
    assert_equal "app/models/thing.rb", units[0]["file"]
    assert_equal 0, units[0]["first_lineno"], "one prefix line → first_lineno 0"

    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    Roundsnap::Loader.install!(root: @dir).boot!("app/models/thing")

    err = assert_raises(RuntimeError) { Thing.boom }
    top = err.backtrace.first
    assert_match(%r{app/models/thing\.rb:12}, top,
                 "backtrace must hit source line 12, got:\n#{err.backtrace.first(8).join("\n")}")
    refute_match(%r{iseq/}, top)
  end

  def test_mapped_erb_view_backtrace_hits_template_line
    # Real spinel shape: unmarked module wrappers, marker on def (:1),
    # marker on body (:4).
    marked = <<~RUBY
      module Views
        module Articles
      #<SPINEL_SOURCE>app/views/articles/_article.html.erb:1
          def self.render_boom
      #<SPINEL_SOURCE>app/views/articles/_article.html.erb:4
            raise "boom-from-erb"
          end
        end
      end
    RUBY

    units = Roundsnap::SourceMap.units_from(marked, emit_key: "app/views/articles/_article")
    assert units[0]["mapped"]
    assert_equal "app/views/articles/_article.html.erb", units[0]["file"]
    assert_equal(-1, units[0]["first_lineno"], "two prefix lines → first_lineno -1")

    Roundsnap::Compiler.compile!(units: units, out_dir: @dir)
    Roundsnap::Loader.install!(root: @dir).boot!("app/views/articles/_article")

    err = assert_raises(RuntimeError) { Views::Articles.render_boom }
    top = err.backtrace.first
    assert_match(%r{app/views/articles/_article\.html\.erb:4}, top,
                 "ERB backtrace must hit template line 4, got:\n#{err.backtrace.first(8).join("\n")}")
  end

  def test_unmarked_unit_keeps_emit_path_not_fake_original
    source = "def unmarked_boom\n  raise \"x\"\nend\n"
    units = Roundsnap::SourceMap.units_from(source, emit_key: "app/models/thing")
    assert_equal 1, units.size
    refute units[0]["mapped"]
    assert_equal "app/models/thing.rb", units[0]["file"]
  end

  def test_compiler_auto_expands_marked_units
    marked = <<~RUBY
      module Demo
      #<SPINEL_SOURCE>lib/demo.rb:5
        def self.boom
      #<SPINEL_SOURCE>lib/demo.rb:7
          raise "demo"
        end
      end
    RUBY
    Roundsnap::Compiler.compile!(
      units: [{ "key" => "lib/demo", "source" => marked, "file" => "ignored.rb", "first_lineno" => 1 }],
      out_dir: @dir,
    )
    man = JSON.parse(File.read(File.join(@dir, "manifest.json")))
    entry = man["units"]["lib/demo"]
    assert entry["mapped"]
    assert_equal "lib/demo.rb", entry["file"]
    Roundsnap::Loader.install!(root: @dir).boot!("lib/demo")
    err = assert_raises(RuntimeError) { Demo.boom }
    assert_match(%r{lib/demo\.rb:7}, err.backtrace.first,
                 "got:\n#{err.backtrace.first(6).join("\n")}")
  end
end
