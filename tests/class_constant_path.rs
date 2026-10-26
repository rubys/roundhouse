//! A constant read through a class value (`self.class::KINDS`,
//! `other.class::KINDS`). Ruby looks the name up through that class's
//! ancestors, so a subclass's own constant wins, an inherited one is found,
//! and so is one from an included module (Ruby 4.0.7). With none, it raises
//! `NameError: uninitialized constant <class>::KINDS`. Ingest dropped the
//! parent and read a lexical `KINDS`, which no target could resolve; the
//! path now reaches the analysis as `const_get(:KINDS)` and the ruby family
//! writes it back as the path, a chained one (`self.class::NAMES::LABEL`)
//! included.

use roundhouse::project::BuildTarget;

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const CLASSES: &str = r#"module Tagged
  LABELS = %w(m).freeze
end

class Shape
  include Tagged
  LIMIT = 10

  def kinds
    self.class::KINDS
  end

  def limit
    self.class::LIMIT
  end

  def labels
    self.class::LABELS
  end

  def peer_kinds(other)
    other.class::KINDS
  end

  def label
    self.class::NAMES::LABEL
  end
end

class Polygon < Shape
  KINDS = %w(a b).freeze

  module NAMES
    LABEL = "poly"
  end
end

class Square < Polygon
  LIMIT = 3
end

class Circle < Shape
  KINDS = %w(z).freeze
end
"#;

const MODEL: &str = r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  STATES = %w(draft published).freeze

  def states
    self.class::STATES
  end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/shape.rb", CLASSES).edit(
        "app/models/article.rb",
        "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
        MODEL,
    )
}

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
check("own", Polygon.new.kinds, %w(a b))
check("inherited", Square.new.kinds, %w(a b))
check("other subclass", Circle.new.kinds, %w(z))
check("own overrides", Square.new.limit, 3)
check("parent's", Polygon.new.limit, 10)
check("included module", Square.new.labels, %w(m))
check("other.class", Shape.new.peer_kinds(Circle.new), %w(z))
check("model", Article.new.states, %w(draft published))
check("chained", Polygon.new.label, "poly")
check("chained, inherited", Square.new.label, "poly")
message = begin
  Shape.new.kinds
  "no error"
rescue NameError => e
  e.message
end
check("none", message, "uninitialized constant Shape::KINDS")
puts "class constant paths passed"
"##;

#[test]
fn a_constant_through_a_class_value_resolves_as_ruby_does_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("class constant paths passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_constant_through_a_class_value_resolves_as_ruby_does_on_spinel() {
    let run = app().run_spinel(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("class constant paths passed"), "{}", run.stdout);
}

/// The strict targets have no class object to ask; the read is refused
/// where it is written instead of resolving a lexical constant.
#[test]
fn a_strict_target_refuses_the_read() {
    let (_emitted, errors) = app().emit(BuildTarget::Rust);
    assert!(errors.iter().any(|e| e.contains("const_get")), "{errors:#?}");
}

/// A view's read keeps the path too: the view lowering rebuilds the send
/// (ivars become locals), and the path survives it.
#[test]
fn a_view_reads_the_constant_through_the_class() {
    let (emitted, errors) = app()
        .edit("app/views/articles/show.html.erb", "<%= @article.title %>", "<%= @article.title %> <%= @article.class::STATES.join(\",\") %>")
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let view = std::fs::read_to_string(emitted.join("app/views/articles/show.rb")).expect("show.rb");
    assert!(view.contains("article.class::STATES"), "{view}");
}
