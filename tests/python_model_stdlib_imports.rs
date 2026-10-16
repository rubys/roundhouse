//! A Python overlay module that lowers to the `re` module gets
//! `import re`.
//!
//! `String#sub`/`#gsub` render as `re.sub(...)` and a regexp literal
//! as `re.compile(...)`. Overlay writers pick imports by scanning the
//! emitted body, and `re` was not on that list: the tree transpiled
//! clean and the call raised `NameError: name 're' is not defined`.
//! This overlays such methods onto real-blog, emits Python, checks
//! the import on models and test files, and runs the model probe.
//!
//!     cargo test --test python_model_stdlib_imports

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

use roundhouse::project::BuildTarget;

#[test]
fn model_regexp_methods_import_re() {
    let (emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\
             \n  def self.sub_probe\n    \"hello\".sub(\"l\", \"L\")\n  end\n\
             \n  def self.gsub_probe\n    \"hello\".gsub(/l+/, \"L\")\n  end\n",
        )
        .write(
            "test/models/probe_test.rb",
            "require \"test_helper\"\n\n\
             class ProbeTest < ActiveSupport::TestCase\n  \
               test \"regexp model methods\" do\n    \
                 assert_equal \"heLlo\", Article.sub_probe\n    \
                 assert_equal \"heLo\", Article.gsub_probe\n  \
               end\nend\n",
        )
        .emit(BuildTarget::Python);
    assert!(errors.is_empty(), "errors: {errors:?}");

    let models = std::fs::read_to_string(emitted.join("app/v2/models.py")).expect("models.py");
    assert!(models.contains("\nimport re\n"), "models.py lacks `import re`:\n{models}");

    let output = Command::new("python3")
        .args(["-m", "unittest", "tests.test_probe"])
        .current_dir(&emitted)
        .output()
        .expect("run python3");
    assert!(
        output.status.success(),
        "emitted probe test failed in {}:\n{}\n{}",
        emitted.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Test overlay bodies also reach `re.sub` / `re.compile`; the test
/// writer must emit `import re` (assert lowering is a separate gap —
/// only the import string is pinned here).
#[test]
fn test_file_inline_sub_imports_re() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write(
            "test/models/re_inline_test.rb",
            "require \"test_helper\"\n\n\
             class ReInlineTest < ActiveSupport::TestCase\n  \
               test \"inline sub\" do\n    \
                 x = \"hello\".sub(\"l\", \"L\")\n    \
                 assert_equal \"heLlo\", x\n  \
               end\nend\n",
        )
        .emit(BuildTarget::Python);
    assert!(errors.is_empty(), "errors: {errors:?}");

    let test_py =
        std::fs::read_to_string(emitted.join("tests/test_re_inline.py")).expect("test_re_inline.py");
    assert!(
        test_py.contains("re.sub"),
        "expected re.sub in emitted test:\n{test_py}"
    );
    assert!(
        test_py.contains("\nimport re\n"),
        "test_re_inline.py lacks `import re`:\n{test_py}"
    );
}
