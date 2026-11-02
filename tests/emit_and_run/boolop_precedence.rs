//! #1686: an `||` under an `&&` keeps its grouping. Crystal, Python and
//! Go read `a && b || c` as `(a && b) || c`, so `a && (b || c)` emitted
//! bare is a different value when `a` is false and `c` is true. Python's
//! conditional expression binds looser than `and`, so a ternary operand
//! needs its parentheses too. Same-operator chains and `&&` under `||`
//! stay bare.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

const METHODS: &str = r#"  def self.p1(a, b, c)
    a && (b || c)
  end

  def self.p2(a, b)
    !(a && b)
  end

  def self.p3(a, b, c)
    a || (b && c)
  end

  def self.p4(a, b, c)
    (a ? b : c) && a
  end

  def self.p5(a, b, c)
    (a || b) && c
  end

  def self.p6(a, b, c)
    a and (b or c)
  end

  def self.p7(a, b)
    !a || b
  end

  def self.g1(a, b, c)
    (a && b) || c
  end

  def self.g2(a, b, c)
    a || (b || c)
  end
"#;

/// Each shape on inputs where the two groupings differ, with Ruby 4.0.7's
/// value.
const CALLS: &[(&str, &str, &str)] = &[
    ("p1-ftt", "p1(false, false, true)", "false"),
    ("p1-tft", "p1(true, false, true)", "true"),
    ("p2-tf", "p2(true, false)", "true"),
    ("p3-tff", "p3(true, false, false)", "true"),
    ("p4-txy", "p4(true, \"x\", \"y\")", "true"),
    ("p4-fxy", "p4(false, \"x\", \"y\")", "false"),
    ("p5-tff", "p5(true, false, false)", "false"),
    ("p5-ftt", "p5(false, true, true)", "true"),
    ("p6-fft", "p6(false, false, true)", "false"),
    ("p7-tf", "p7(true, false)", "false"),
    ("g1-fft", "g1(false, false, true)", "true"),
    ("g1-ttf", "g1(true, true, false)", "true"),
    ("g2-fft", "g2(false, false, true)", "true"),
];

/// real-blog with the shapes on `Article`, each called from the index.
fn app() -> emit_and_run::Overlay {
    let view: String = CALLS.iter().map(|(id, call, _)| format!("<p id=\"{id}\"><%= Article.{call} %></p>\n")).collect();
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", &format!("class Article < ApplicationRecord\n{METHODS}\n"))
        .edit("app/views/articles/index.html.erb", "<% content_for :title, \"Articles\" %>\n", &format!("<% content_for :title, \"Articles\" %>\n{view}"))
}

/// The line after each method's opening line in `file`.
fn bodies(target: BuildTarget, file: &str, opener: impl Fn(&str) -> String) -> Vec<(String, String)> {
    let (out, _) = app().emit(target);
    let source = std::fs::read_to_string(out.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
    let lines: Vec<&str> = source.lines().collect();
    ["p1", "p2", "p3", "p5", "p6", "p7", "g1", "g2", "p4"]
        .iter()
        .map(|name| {
            let at = lines.iter().position(|l| l.contains(&opener(name))).unwrap_or_else(|| panic!("{file}: no {name}"));
            (name.to_string(), lines[at + 1].trim().to_string())
        })
        .collect()
}

fn assert_bodies(target: BuildTarget, got: Vec<(String, String)>, want: &[(&str, &str)]) {
    for (name, line) in want {
        let found = &got.iter().find(|(n, _)| n == name).unwrap().1;
        assert_eq!(found, line, "{target:?} {name}: {got:#?}");
    }
}

#[test]
fn crystal_keeps_an_or_under_an_and_grouped() {
    let got = bodies(BuildTarget::Crystal, "src/models/article.cr", |n| format!("def self.{n}("));
    assert_bodies(
        BuildTarget::Crystal,
        got,
        &[
            ("p1", "a && (b || c)"),
            ("p5", "(a || b) && c"),
            ("p6", "a && (b || c)"),
            ("p2", "!(a && b)"),
            ("p3", "a || b && c"),
            ("p7", "!(a) || b"),
            ("g1", "a && b || c"),
            ("g2", "a || b || c"),
        ],
    );
}

#[test]
fn python_keeps_an_or_under_an_and_and_a_conditional_operand_grouped() {
    let got = bodies(BuildTarget::Python, "app/v2/models.py", |n| format!("def {n}(cls"));
    assert_bodies(
        BuildTarget::Python,
        got,
        &[
            ("p1", "return a and (b or c)"),
            ("p5", "return (a or b) and c"),
            ("p6", "return a and (b or c)"),
            ("p4", "return (b if a else c) and a"),
            ("p2", "return not (a and b)"),
            ("p3", "return a or b and c"),
            ("p7", "return not (a) or b"),
            ("g1", "return a and b or c"),
            ("g2", "return a or b or c"),
        ],
    );
}

#[test]
fn go_keeps_an_or_under_an_and_grouped() {
    let got = bodies(BuildTarget::Go, "app/v2/article.go", |n| format!("func Article_{n}("));
    assert_bodies(
        BuildTarget::Go,
        got,
        &[
            ("p1", "return a && (b || c)"),
            ("p5", "return (a || b) && c"),
            ("p6", "return a && (b || c)"),
            ("p2", "return !(a && b)"),
            ("p3", "return a || b && c"),
            ("p7", "return !(a) || b"),
            ("g1", "return a && b || c"),
            ("g2", "return a || b || c"),
        ],
    );
}

/// The ruby target renders Ruby's value for every shape.
#[test]
fn boolean_shapes_render_ruby_values() {
    let asserts: String = CALLS.iter().map(|(id, _, v)| format!("    assert_select \"p#{id}\", \"{v}\"\n")).collect();
    let test = format!("require \"test_helper\"\n\nclass BoolopControllerTest < ActionDispatch::IntegrationTest\n  test \"each shape answers Ruby's value\" do\n    get articles_url\n    assert_response :success\n{asserts}  end\nend\n");
    app().write("test/controllers/boolop_controller_test.rb", &test).run_test("test/controllers/boolop_controller_test.rb").assert_passes();
}

/// The runtime router's verb test, `(verb == m || (verb == "ANY" && …)) &&
/// !(unpeeled && format_held_param_tail(route))`: emitted bare, a matching
/// verb skipped the format guard, and Crystal served `/members/1.csv` from
/// a route held to `format: /json|xml/` (`200 member:1.csv`; Rails 404s).
#[test]
fn the_router_keeps_its_verb_test_grouped() {
    for (target, file, line) in [
        (BuildTarget::Crystal, "src/router.cr", "if (verb == method_upcase || verb == \"ANY\" && method_upcase != \"HEAD\") && !(unpeeled && format_held_param_tail(route))"),
        (BuildTarget::Python, "app/router.py", "if (verb == method_upcase or verb == \"ANY\" and method_upcase != \"HEAD\") and not (unpeeled and cls.format_held_param_tail(route)):"),
        (BuildTarget::Go, "app/v2/router.go", "if (verb == method_upcase || verb == \"ANY\" && method_upcase != \"HEAD\") && !(unpeeled && ActionDispatchRouter_format_held_param_tail(route)) {"),
    ] {
        let (out, _) = emit_and_run::real_blog().emit(target);
        let source = std::fs::read_to_string(out.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(source.lines().any(|l| l.trim() == line), "{target:?}: {}", source.lines().filter(|l| l.contains("format_held_param_tail(route)")).collect::<Vec<_>>().join("\n"));
    }
}

/// A model's timestamp fill, `creating && (raw.nil? || raw.empty?)`:
/// emitted bare, an update with an empty `created_at` overwrote it.
#[test]
fn the_timestamp_fill_keeps_its_creating_test_grouped() {
    for (target, file, needle) in [
        (BuildTarget::Crystal, "src/models/article.cr", "if creating && (@created_at_raw.nil? || @created_at_raw.not_nil!.empty?)"),
        (BuildTarget::Python, "app/v2/models.py", "if creating and (self.created_at_raw is None or len(self.created_at_raw) == 0):"),
        (BuildTarget::Go, "app/v2/article.go", "if creating && (false || len(self.CreatedAtRaw) == 0) {"),
    ] {
        let (out, _) = emit_and_run::real_blog().emit(target);
        let source = std::fs::read_to_string(out.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(source.contains(needle), "{target:?}: {}", source.lines().filter(|l| l.contains("creating")).collect::<Vec<_>>().join("\n"));
    }
}
