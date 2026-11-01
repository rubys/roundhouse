//! #1687: an `if` whose body is a single modifier `if` (`if a; x = 1 if b;
//! end`) was written on one line, `if a: if b: x = 1`, which Python
//! doesn't parse, so the whole module failed to import. A body that is
//! itself a compound statement takes the block form.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

const METHODS: &str = r#"  def self.level(a, b)
    x = 0
    if a
      x = 1 if b
    end
    x
  end

  def self.flat(a)
    x = 0
    x = 2 if a
    x
  end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", &format!("class Article < ApplicationRecord\n{METHODS}\n"))
        .edit(
            "app/views/articles/index.html.erb",
            "<% content_for :title, \"Articles\" %>\n",
            "<% content_for :title, \"Articles\" %>\n<p id=\"tt\"><%= Article.level(true, true) %></p>\n<p id=\"tf\"><%= Article.level(true, false) %></p>\n<p id=\"flat\"><%= Article.flat(true) %></p>\n",
        )
}

/// The body of `def <name>(cls` in Python's models, up to the next `def`.
fn python_method(name: &str) -> String {
    let (out, _) = app().emit(BuildTarget::Python);
    let source = std::fs::read_to_string(out.join("app/v2/models.py")).unwrap();
    let start = source.find(&format!("def {name}(cls")).unwrap_or_else(|| panic!("no {name}"));
    let rest = &source[start..];
    let end = rest[1..].find("\n    def ").map(|i| i + 1).unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn a_nested_modifier_if_takes_the_block_form_on_python() {
    let level = python_method("level");
    assert!(!level.contains("if a: if"), "{level}");
    assert!(level.contains("        if a:\n            if b: x = 1\n"), "{level}");
    // A one-statement body that isn't compound stays on one line.
    let flat = python_method("flat");
    assert!(flat.contains("        if a: x = 2\n"), "{flat}");
}

/// Ruby's values, on the ruby target: `level(true, true)` is 1,
/// `level(true, false)` is 0, `flat(true)` is 2.
#[test]
fn a_nested_modifier_if_renders_ruby_values() {
    app()
        .write(
            "test/controllers/nested_if_controller_test.rb",
            "require \"test_helper\"\n\nclass NestedIfControllerTest < ActionDispatch::IntegrationTest\n  test \"values\" do\n    get articles_url\n    assert_select \"p#tt\", \"1\"\n    assert_select \"p#tf\", \"0\"\n    assert_select \"p#flat\", \"2\"\n  end\nend\n",
        )
        .run_test("test/controllers/nested_if_controller_test.rb")
        .assert_passes();
}
