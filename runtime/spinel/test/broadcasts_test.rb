require_relative "test_helper"
require "broadcasts"

class BroadcastsTest < Minitest::Test
  def setup
    Broadcasts.reset_log!
  end

  def test_append_records_action
    Broadcasts.append(stream: "articles", target: "articles", html: "<p>hi</p>")
    entry = Broadcasts.log.first
    assert_equal :append, entry[:action]
    assert_equal "articles", entry[:stream]
    assert_equal "articles", entry[:target]
    assert_equal "<p>hi</p>", entry[:html]
  end

  def test_prepend_records_action
    Broadcasts.prepend(stream: "s", target: "t", html: "<p>")
    assert_equal :prepend, Broadcasts.log.first[:action]
  end

  def test_replace_records_action
    Broadcasts.replace(stream: "s", target: "t", html: "<p>")
    assert_equal :replace, Broadcasts.log.first[:action]
  end

  def test_remove_records_with_empty_html
    Broadcasts.remove(stream: "s", target: "article_1")
    entry = Broadcasts.log.first
    assert_equal :remove, entry[:action]
    assert_equal "", entry[:html]
  end

  def test_log_accumulates_in_order
    Broadcasts.append(stream: "s", target: "t1", html: "a")
    Broadcasts.replace(stream: "s", target: "t2", html: "b")
    Broadcasts.remove(stream: "s", target: "t3")
    assert_equal 3, Broadcasts.log.length
    assert_equal [:append, :replace, :remove], Broadcasts.log.map { |e| e[:action] }
  end

  def test_reset_log_clears
    Broadcasts.append(stream: "s", target: "t", html: "a")
    Broadcasts.reset_log!
    assert_equal 0, Broadcasts.log.length
  end

  def test_log_returns_a_copy
    Broadcasts.append(stream: "s", target: "t", html: "a")
    snapshot = Broadcasts.log
    Broadcasts.append(stream: "s", target: "t2", html: "b")
    assert_equal 1, snapshot.length, "snapshot should not see later additions"
  end

  # ── render_fragment ──────────────────────────────────────────────

  def test_render_fragment_replace_includes_template
    out = Broadcasts.render_fragment(action: :replace, target: "article_1", html: "<p>x</p>")
    assert_includes out, %(<turbo-stream action="replace" target="article_1">)
    assert_includes out, "<template><p>x</p></template>"
    assert_includes out, "</turbo-stream>"
  end

  def test_render_fragment_remove_omits_template
    out = Broadcasts.render_fragment(action: :remove, target: "article_1")
    assert_includes out, %(<turbo-stream action="remove" target="article_1">)
    refute_includes out, "<template>"
  end

  def test_render_fragment_append_with_default_empty_html
    out = Broadcasts.render_fragment(action: :append, target: "items")
    assert_includes out, %(action="append")
    assert_includes out, "<template></template>"
  end

  # ── file IPC ──────────────────────────────────────────────────

  def test_no_files_written_when_BROADCAST_DIR_unset
    require "tmpdir"
    Dir.mktmpdir do |dir|
      ENV.delete("BROADCAST_DIR")
      Broadcasts.append(stream: "articles", target: "articles", html: "<p>")
      assert_empty Dir.children(dir)
    end
  end

  def test_writes_frag_file_when_BROADCAST_DIR_set
    require "tmpdir"
    Dir.mktmpdir do |dir|
      ENV["BROADCAST_DIR"] = dir
      Broadcasts.append(stream: "articles", target: "articles", html: "<p>hi</p>")
      files = Dir.children(dir)
      assert_equal 1, files.length
      assert files.first.start_with?("articles__")
      assert files.first.end_with?(".frag")
    ensure
      ENV.delete("BROADCAST_DIR")
    end
  end

  def test_frag_content_is_rendered_turbo_stream
    require "tmpdir"
    Dir.mktmpdir do |dir|
      ENV["BROADCAST_DIR"] = dir
      Broadcasts.replace(stream: "articles", target: "article_5", html: "<p>x</p>")
      content = File.read(File.join(dir, Dir.children(dir).first))
      assert_includes content, %(<turbo-stream action="replace" target="article_5">)
      assert_includes content, "<template><p>x</p></template>"
    ensure
      ENV.delete("BROADCAST_DIR")
    end
  end

  def test_special_chars_in_stream_name_get_sanitized
    require "tmpdir"
    Dir.mktmpdir do |dir|
      ENV["BROADCAST_DIR"] = dir
      Broadcasts.append(stream: "article_5_comments/foo bar!", target: "comments", html: "<p>")
      file = Dir.children(dir).first
      refute_nil file
      # Filename must round-trip safely on any FS — no slashes, spaces, etc.
      assert_match(/\A[a-zA-Z0-9_-]+__\d+T\d+\.frag\z/, file)
    ensure
      ENV.delete("BROADCAST_DIR")
    end
  end

  def test_in_memory_log_still_populated_alongside_file_writes
    require "tmpdir"
    Dir.mktmpdir do |dir|
      ENV["BROADCAST_DIR"] = dir
      Broadcasts.append(stream: "x", target: "t", html: "h")
      assert_equal 1, Broadcasts.log.length
      assert_equal 1, Dir.children(dir).length
    ensure
      ENV.delete("BROADCAST_DIR")
    end
  end
end
