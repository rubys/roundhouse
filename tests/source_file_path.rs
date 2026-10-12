//! `__FILE__` and `__dir__` name the loaded file's absolute location, so
//! a path built from them does not depend on the process's cwd. A
//! `ROOT = File.expand_path("../../..", __FILE__)` finds an app's config
//! directory that way. The Ruby family emits
//! `app/lib/locator.rb` as `app/models/locator.rb`, so the emitted
//! literal is the source's app-relative path anchored on the emitted
//! file's own `__dir__`: the app root maps to the output root, where the
//! `config/` directory is.
//!
//! Native Ruby is the oracle. Both runs start from a cwd outside the app.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCATOR: &str = r#"module Locator
  ROOT = File.expand_path("../../..", __FILE__)
  HERE = __dir__

  def self.file
    __FILE__
  end

  def self.marker
    File.directory?(File.join(ROOT, "config"))
  end
end
"#;

/// Gives the native app a `config/` directory like the emitted tree has.
const CONFIG_STUB: &str = "# config\n";

const REPORT: &str = r#"puts Locator.marker
puts Locator.file.start_with?("/")
puts File.expand_path("app/lib/locator.rb", Locator::ROOT) == Locator.file
puts Locator::HERE == File.dirname(Locator.file)
puts Dir.pwd != Locator::ROOT
"#;

const EXPECTED: &str = "true\ntrue\ntrue\ntrue\ntrue\n";

fn elsewhere(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("roundhouse-cwd-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn native_ruby_answers_from_another_cwd() {
    let app = elsewhere("native-app");
    for (path, text) in [("app/lib/locator.rb", LOCATOR), ("config/application.rb", CONFIG_STUB)] {
        let full = app.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let cwd = elsewhere("native-cwd");
    let output = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!("require {:?}\n{REPORT}", app.join("app/lib/locator").display().to_string()))
        .current_dir(&cwd)
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&app);
    let _ = std::fs::remove_dir_all(&cwd);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
}

/// A file under `app/models` is emitted in place; the anchor holds there too,
/// not only for a library class that relocates.
#[test]
fn an_active_record_model_body_is_anchored_too() {
    let (emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.source_dir = __dir__\n",
        )
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let text = std::fs::read_to_string(emitted.join("app/models/article.rb")).expect("article.rb");
    assert!(text.contains("File.expand_path(\"../../app/models\", __dir__)"), "{text}");
    let cwd = elsewhere("model-cwd");
    let main = emitted.join("main").display().to_string();
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("require {main:?}\nMain.configure_default_adapter!\nputs Article.source_dir"))
        .current_dir(&cwd)
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("spawn ruby");
    let _ = std::fs::remove_dir_all(&cwd);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let printed = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(std::path::Path::new(&printed), emitted.join("app/models"), "{printed}");
}

#[test]
fn emitted_ruby_answers_the_same_from_another_cwd() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/locator.rb", LOCATOR)
        .write("config/application.rb", CONFIG_STUB)
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let cwd = elsewhere("emitted-cwd");
    let main = emitted.join("main").display().to_string();
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("require {main:?}\nMain.configure_default_adapter!\n{REPORT}"))
        .current_dir(&cwd)
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("spawn ruby");
    let _ = std::fs::remove_dir_all(&cwd);
    assert!(
        output.status.success(),
        "emitted program failed in {}:\n{}",
        emitted.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
}
