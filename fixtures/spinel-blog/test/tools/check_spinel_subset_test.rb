require_relative "../test_helper"
require "tempfile"
require "tmpdir"

# Smoke test for the linter: runs it against synthetic fixture trees
# and asserts the expected exit codes / output. The linter is enforced
# via Rakefile against the real spinel-blog tree; this test pins the
# rule semantics independently.
class CheckSpinelSubsetTest < Minitest::Test
  LINTER = File.expand_path("../../tools/check_spinel_subset.rb", __dir__)

  def with_fixture_tree(files)
    Dir.mktmpdir do |dir|
      Dir.mkdir(File.join(dir, "runtime"))
      Dir.mkdir(File.join(dir, "tools"))
      File.write(File.join(dir, "tools", "check_spinel_subset.rb"), File.read(LINTER))
      files.each do |relpath, content|
        path = File.join(dir, relpath)
        Dir.mkdir(File.dirname(path)) unless File.directory?(File.dirname(path))
        File.write(path, content)
      end
      yield dir
    end
  end

  def run_linter_in(dir)
    output = `cd #{dir} && ruby tools/check_spinel_subset.rb 2>&1`
    [$?.exitstatus, output]
  end

  def test_clean_code_passes
    with_fixture_tree("runtime/clean.rb" => "module Foo; def self.bar; 42; end; end\n") do |dir|
      status, out = run_linter_in(dir)
      assert_equal 0, status, "expected clean exit, got: #{out}"
      assert_match(/spinel-subset OK/, out)
    end
  end

  def test_instance_variable_get_flagged
    with_fixture_tree("runtime/dirty.rb" => "x = obj.instance_variable_get(:@foo)\n") do |dir|
      status, out = run_linter_in(dir)
      refute_equal 0, status
      assert_match(/instance_variable_get/, out)
    end
  end

  def test_define_method_flagged
    with_fixture_tree("runtime/dirty.rb" => "define_method(:foo) { 42 }\n") do |dir|
      status, out = run_linter_in(dir)
      refute_equal 0, status
      assert_match(/define_method/, out)
    end
  end

  def test_send_with_paren_flagged
    with_fixture_tree("runtime/dirty.rb" => "x.send(:foo)\n") do |dir|
      status, out = run_linter_in(dir)
      refute_equal 0, status
      assert_match(/\.send\(/, out)
    end
  end

  def test_eval_flagged
    with_fixture_tree("runtime/dirty.rb" => %(eval("1 + 1")\n)) do |dir|
      status, out = run_linter_in(dir)
      refute_equal 0, status
      assert_match(/eval/, out)
    end
  end

  def test_class_eval_flagged
    with_fixture_tree("runtime/dirty.rb" => "Klass.class_eval { def foo; end }\n") do |dir|
      status, out = run_linter_in(dir)
      refute_equal 0, status
      assert_match(/class_eval/, out)
    end
  end

  def test_thread_flagged
    with_fixture_tree("runtime/dirty.rb" => "Thread.new { do_work }\n") do |dir|
      status, out = run_linter_in(dir)
      refute_equal 0, status
      assert_match(/Thread/, out)
    end
  end

  def test_comment_does_not_trigger
    with_fixture_tree("runtime/clean.rb" => "# instance_variable_get is forbidden\nx = 1\n") do |dir|
      status, out = run_linter_in(dir)
      assert_equal 0, status, "comment-only mention triggered the linter: #{out}"
    end
  end
end
