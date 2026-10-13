//! `File::SEPARATOR` and `File::PATH_SEPARATOR` are Strings and
//! `File::NOFOLLOW` and `File::NONBLOCK` are open(2) flags, as Ruby defines
//! them, and the emitted program answers what Ruby answers.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SOURCE: &str = r#"class FilePathProbe
  def self.inside?(path, root)
    path.start_with?(root + File::SEPARATOR)
  end
  def self.search_path(dirs)
    dirs.join(File::PATH_SEPARATOR)
  end
  def self.read_regular(path)
    File.open(path, File::RDONLY | File::NOFOLLOW | File::NONBLOCK) { |file| file.read }
  rescue Errno::ELOOP
    "symlink"
  end
end
"#;

const SCRIPT: &str = r#"
require "tmpdir"
raise "SEPARATOR" unless FilePathProbe.inside?("/a/b", "/a") && !FilePathProbe.inside?("/ab", "/a")
raise "PATH_SEPARATOR" unless FilePathProbe.search_path(["/x", "/y"]) == "/x:/y"
Dir.mktmpdir do |dir|
  target = File.join(dir, "target.txt")
  File.write(target, "hi")
  link = File.join(dir, "link.txt")
  File.symlink(target, link)
  raise "regular file" unless FilePathProbe.read_regular(target) == "hi"
  raise "NOFOLLOW" unless FilePathProbe.read_regular(link) == "symlink"
end
puts "File value constants passed"
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/file_path_probe.rb", SOURCE)
}

#[test]
fn file_separator_and_open_flag_constants_execute_after_emission() {
    app().run_ruby(SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn file_separator_and_open_flag_constants_execute_in_native_spinel() {
    app().run_spinel(SCRIPT).assert_passes();
}
