//! `File.mtime(path)` answers a Time, as in CRuby, so a row hash that
//! carries it (`mtime = nil`, then `File.mtime` inside `begin`/`rescue`)
//! keeps `Time | nil` for that key and a helper can format it. Before,
//! the call was untyped and `row[:mtime].strftime` failed dispatch on
//! the other values' union (`Integer | bool | nil`).

use super::emit_and_run;

const MODEL: &str = r#"class FileRows
  def initialize(root)
    @root = root
  end

  def list
    rows = []
    Dir.children(@root).sort.each do |name|
      rows << row(File.join(@root, name), name)
    end
    rows
  end

  def row(path, name)
    dir = File.directory?(path)
    size = 0
    mtime = nil
    begin
      size = dir ? 0 : File.size(path)
      mtime = File.mtime(path)
    rescue StandardError
      size = 0
    end
    { name: name, dir: dir, size: size, mtime: mtime }
  end
end
"#;

const HELPER: &str = r#"module FilesHelper
  def files_meta(row)
    when_s = row[:mtime] ? row[:mtime].strftime("%Y-%m-%d %H:%M") : ""
    return when_s if row[:dir]

    [ row[:size].to_s, when_s ].reject(&:empty?).join(" ")
  end
end
"#;

const TEST: &str = r#"require "test_helper"

class FilesMetaControllerTest < ActionDispatch::IntegrationTest
  test "a listing row shows the file's size and mtime" do
    dir = File.join(Dir.pwd, "files_meta_probe")
    Dir.mkdir(dir) unless Dir.exist?(dir)
    path = File.join(dir, "notes.txt")
    File.write(path, "hello")
    stamp = Time.local(2026, 10, 16, 12, 40)
    File.utime(stamp, stamp, path)
    get articles_url(dir: dir)
    assert_includes response.body, "<span class=\"meta\">5 2026-10-16 12:40</span>"
  end
end
"#;

#[test]
fn a_file_mtime_in_a_row_hash_formats_as_a_time() {
    emit_and_run::real_blog()
        .write("app/models/file_rows.rb", MODEL)
        .write("app/helpers/files_helper.rb", HELPER)
        .edit(
            "app/controllers/articles_controller.rb",
            "@articles = Article.includes(:comments).order(created_at: :desc)",
            "@articles = Article.includes(:comments).order(created_at: :desc)\n    root = params[:dir].to_s\n    root = \".\" if root.empty?\n    @entries = FileRows.new(root).list",
        )
        .edit(
            "app/views/articles/index.html.erb",
            "<h1",
            "<% @entries.each do |r| %><span class=\"meta\"><%= files_meta(r) %></span><% end %>\n<h1",
        )
        .write("test/controllers/files_meta_controller_test.rb", TEST)
        .run_test("test/controllers/files_meta_controller_test.rb")
        .assert_passes();
}
