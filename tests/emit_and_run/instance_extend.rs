//! `obj.extend(Mod)` on one instance, the shape of campfire's
//! `web_push/connections_test` TLS-failure case: an anonymous module
//! whose only body is `private def begin_transport(*)`, mixed into one
//! object a helper built as `Class.new(...).tap { ... }`.
//!
//! The module is a real module and the receiver is one `.new` local.
//! The `extend` is a statement, which is where Spinel flips that
//! object's class; the value the caller uses is the local. A second
//! `.new` of the same class keeps the original method.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "lib/box.rb",
            r#"class Box
  attr_accessor :note
  def begin_transport
    :open
  end
  def ping
    begin_transport
  end
end
"#,
        )
        .write(
            "test/test_helpers/open_helper.rb",
            r#"module OpenHelper
  def opened(tag)
    Box.new.tap do |box|
      box.note = tag
    end
  end
end
"#,
        )
        .write(
            "test/lib/box_test.rb",
            r#"require "test_helper"

class BoxTest < ActiveSupport::TestCase
  include OpenHelper

  test "one object" do
    failing = Module.new { private def begin_transport(*) = :closed }
    assert_equal :closed, opened("a").extend(failing).ping
    assert_equal :open, opened("b").ping
  end

  test "sibling" do
    assert_equal :open, Box.new.ping
  end

  test "direct" do
    failing = Module.new { def begin_transport = :closed }
    one = Box.new
    assert_equal :closed, one.extend(failing).ping
    assert_equal :open, Box.new.ping
  end
end
"#,
        )
}

#[test]
fn one_extended_object_answers_the_module_and_its_sibling_does_not() {
    app().run_test("test/models/box_test.rb").assert_passes();
}

/// The emitted module and the `extend` statements, compiled by Spinel
/// on their own. A full test-file compile also loads the blog runtime;
/// this is the file's own extend, which is what Spinel refuses when the
/// receiver is not one `.new`.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn one_extended_object_answers_the_module_on_spinel() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(
        errors.is_empty(),
        "analysis or emit reports errors:\n{}",
        errors.join("\n")
    );
    let box_rb =
        std::fs::read_to_string(walk_for(&emitted, "box.rb").expect("emitted box.rb")).unwrap();
    let test_rb =
        std::fs::read_to_string(walk_for(&emitted, "box_test.rb").expect("emitted box_test.rb"))
            .unwrap();
    let tests = test_rb.split("# Spinel AOT autorun").next().unwrap();
    let tests: String = tests
        .lines()
        .filter(|line| !line.starts_with("require"))
        .collect::<Vec<_>>()
        .join("\n");
    let program = format!(
        "class TestBase; end\n{box_rb}\n{tests}\nBoxTest.new.test_one_object\nBoxTest.new.test_sibling\nBoxTest.new.test_direct\nputs \"3 tests passed\"\n"
    );
    let source = emitted.join("build/box_extend.rb");
    std::fs::create_dir_all(source.parent().unwrap()).expect("mkdir build");
    std::fs::write(&source, &program).expect("write spinel program");
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    let binary = emitted.join("build/box_extend");
    let compiled = std::process::Command::new(&compiler)
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .current_dir(&emitted)
        .output()
        .expect("spawn spinel");
    assert!(
        compiled.status.success(),
        "spinel refused the emitted extend\n=== stdout ===\n{}\n=== stderr ===\n{}\n=== program ===\n{program}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .current_dir(&emitted)
        .output()
        .expect("run box_extend");
    assert!(
        ran.status.success(),
        "spinel binary failed\n=== stdout ===\n{}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    let stdout = String::from_utf8_lossy(&ran.stdout);
    assert!(stdout.contains("3 tests passed"), "spinel run:\n{stdout}");
}

fn walk_for(root: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                return Some(path);
            }
        }
    }
    None
}
