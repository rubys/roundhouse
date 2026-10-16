//! `File::SEPARATOR` and `File::PATH_SEPARATOR` are Strings and
//! File's open(2) flags are Integers, as Ruby defines
//! them, and the emitted program answers what Ruby answers.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SOURCE: &str = r#"class FilePathProbe
  OPEN_FLAGS = File::RDONLY | File::NONBLOCK | File::NOCTTY
  SYNC_FLAGS = File::SYNC | File::DSYNC | File::BINARY

  def self.open_flags
    OPEN_FLAGS
  end
  def self.sync_flags
    SYNC_FLAGS
  end
  def self.inside?(path, root)
    path.start_with?(root + File::SEPARATOR)
  end
  def self.search_path(dirs)
    dirs.join(File::PATH_SEPARATOR)
  end
  def self.read_regular(path)
    File.open(path, OPEN_FLAGS | File::NOFOLLOW) { |file| file.read }
  rescue Errno::ELOOP
    "symlink"
  end
end
"#;

const SCRIPT: &str = r#"
require "tmpdir"
raise "OPEN_FLAGS" unless FilePathProbe.open_flags == File::RDONLY | File::NONBLOCK | File::NOCTTY
raise "SYNC_FLAGS" unless FilePathProbe.sync_flags == File::SYNC | File::DSYNC | File::BINARY
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
fn open_flags_type_as_integers_and_survive_class_body_emission() {
    use roundhouse::project::BuildTarget;
    use roundhouse::ty::Ty;

    for name in ["NOCTTY", "SYNC", "DSYNC", "BINARY"] {
        let source = format!("class FlagProbe\n  FLAGS = File::{name}\n  def value\n    File::{name}\n  end\nend\n");
        let (emitted, analyzed, errors) = app()
            .write("app/models/flag_probe.rb", &source)
            .emit_with_app(BuildTarget::Spinel);
        assert!(errors.is_empty(), "{name}: {errors:#?}");
        let probe = analyzed.library_classes.iter().find(|c| c.name.0.as_str() == "FlagProbe").unwrap();
        let method = probe.methods.iter().find(|m| m.name.as_str() == "value").unwrap();
        let Some(Ty::Fn { ret, .. }) = &method.signature else { panic!("{name}: no signature") };
        assert_eq!(ret.as_ref(), &Ty::Int, "{name}");
        let ruby = std::fs::read_to_string(emitted.join("app/models/flag_probe.rb")).unwrap();
        assert!(ruby.contains(&format!("FLAGS = File::{name}")), "{ruby}");
        assert!(!ruby.contains("raise"), "{ruby}");
    }
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn file_separator_and_open_flag_constants_execute_in_native_spinel() {
    app().run_spinel(SCRIPT).assert_passes();
}
