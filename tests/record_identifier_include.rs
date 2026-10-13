//! `include ActionView::RecordIdentifier` in a model, a plain library class
//! and a concern: `dom_id` / `dom_class` resolve to the one implementation
//! views use (`ActionView::ViewHelpers`). Expected strings were printed by
//! real Rails 8.1.4 (activerecord + actionview) for the same shapes:
//!
//!   Article#dom_id          article_1 / new_article
//!   dom_id(x, :edit)        edit_article_1 / edit_article  (prefix REPLACES `new`)
//!   dom_class(x) / (x,:row) article / row_article
//!   Admin::Widget (plain `module Admin`, not isolated): admin_widget_1,
//!   new_admin_widget, edit_admin_widget_1, edit_admin_widget
//!
//! Kept out of tests/emit_and_run.rs so concurrent appends there do not
//! conflict. Same harness.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"articles\", force: :cascade do |t|",
            "  create_table \"widgets\", force: :cascade do |t|\n    t.string \"title\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"articles\", force: :cascade do |t|",
        )
        .edit(
            "app/models/application_record.rb",
            "primary_abstract_class\n",
            "primary_abstract_class\n  include ActionView::RecordIdentifier\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def row_id
    dom_id(self)
  end

  def edit_id
    dom_id(self, :edit)
  end

  def row_class
    dom_class(self)
  end

  def prefixed_class
    dom_class(self, :row)
  end
",
        )
        .write(
            "app/models/admin/widget.rb",
            "module Admin\n  class Widget < ApplicationRecord\n    def row_id\n      dom_id(self)\n    end\n\n    def edit_id\n      dom_id(self, :edit)\n    end\n  end\nend\n",
        )
        .write(
            "app/models/row_presenter.rb",
            "class RowPresenter\n  include ActionView::RecordIdentifier\n\n  def row(article)\n    dom_id(article)\n  end\n\n  def details(article)\n    dom_id(article, :details)\n  end\n\n  def klass(article)\n    dom_class(article, :row)\n  end\nend\n",
        )
}

#[test]
fn record_identifier_include_is_admitted_and_dom_id_is_a_string() {
    let (_emitted, app, errors) = overlay().emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "analysis or emission reported errors:\n{}", errors.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("\n"));
    let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").expect("Article");
    for name in ["row_id", "edit_id", "row_class", "prefixed_class"] {
        let m = article.methods().find(|m| m.name.as_str() == name).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(m.body.ty, Some(roundhouse::ty::Ty::Str), "{name} is typed String");
    }
    let presenter = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "RowPresenter")
        .expect("RowPresenter");
    for m in &presenter.methods {
        assert_eq!(m.body.ty, Some(roundhouse::ty::Ty::Str), "{} is typed String", m.name);
    }
}

#[test]
fn record_identifier_dom_id_and_dom_class_match_rails() {
    let run = overlay().run_ruby(
        r#"Article.delete_all
a = Article.create!(title: "One", body: "A sufficiently long body.")
n = Article.new
w = Admin::Widget.create!(title: "w")
wn = Admin::Widget.new
rp = RowPresenter.new
got = [
  a.row_id, a.edit_id, a.row_class, a.prefixed_class,
  n.row_id, n.edit_id,
  w.row_id, w.edit_id, wn.row_id, wn.edit_id,
  rp.row(a), rp.row(n), rp.details(a), rp.details(n), rp.klass(a),
]
want = [
  "article_#{a.id}", "edit_article_#{a.id}", "article", "row_article",
  "new_article", "edit_article",
  "admin_widget_#{w.id}", "edit_admin_widget_#{w.id}", "new_admin_widget", "edit_admin_widget",
  "article_#{a.id}", "new_article", "details_article_#{a.id}", "details_article", "row_article",
]
raise "got #{got.inspect}\nwant #{want.inspect}" unless got == want
puts "record identifier passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("record identifier passed"));
}

/// An app-defined `dom_class` / `dom_id` on a superclass or in an included
/// module is the method a bare call reaches; emission must not redirect it
/// to `ActionView::ViewHelpers`.
#[test]
fn record_identifier_app_method_on_a_superclass_wins_over_the_helper() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/application_record.rb",
            "primary_abstract_class\n",
            "primary_abstract_class\n\n  def dom_class(record)\n    \"custom\"\n  end\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def row_class\n    dom_class(self)\n  end\n",
        )
        .run_ruby(
            r#"a = Article.new
raise "got #{a.row_class.inspect}" unless a.row_class == "custom"
puts "superclass dom_class passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("superclass dom_class passed"));
}

#[test]
fn record_identifier_app_method_in_an_included_module_wins_over_the_helper() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/concerns/labeled.rb",
            "module Labeled\n  def dom_id(record)\n    \"labeled\"\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  include Labeled\n\n  validates :title, presence: true\n\n  def row_id\n    dom_id(self)\n  end\n",
        )
        .write(
            "app/models/row_presenter.rb",
            "class RowPresenter\n  include Labeled\n\n  def row(article)\n    dom_id(article)\n  end\nend\n",
        )
        .run_ruby(
            r#"a = Article.new
raise "model #{a.row_id.inspect}" unless a.row_id == "labeled"
raise "plain #{RowPresenter.new.row(a).inspect}" unless RowPresenter.new.row(a) == "labeled"
puts "module dom_id passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("module dom_id passed"));
}

/// Shadowing is per receiver side: an inherited INSTANCE `dom_class` does
/// not capture a bare call inside a CLASS method (the class object cannot
/// dispatch it), so that call still reaches the shared helper.
#[test]
fn record_identifier_inherited_instance_method_does_not_shadow_a_class_method_call() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/application_record.rb",
            "primary_abstract_class\n",
            "primary_abstract_class\n\n  def dom_class(record)\n    \"custom\"\n  end\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.klass_for(article)\n    dom_class(article)\n  end\n\n  def instance_klass\n    dom_class(self)\n  end\n",
        )
        .run_ruby(
            r#"a = Article.new
raise "class side #{Article.klass_for(a).inspect}" unless Article.klass_for(a) == "article"
raise "instance side #{a.instance_klass.inspect}" unless a.instance_klass == "custom"
puts "class side helper passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("class side helper passed"));
}

/// A class-side app `dom_id` shadows a bare call in a class method, while
/// an instance body with no instance `dom_id` still gets the helper.
#[test]
fn record_identifier_class_method_dom_id_shadows_a_class_method_call() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.dom_id(record)\n    \"classside\"\n  end\n\n  def self.pick(article)\n    dom_id(article)\n  end\n\n  def row_id\n    dom_id(self)\n  end\n",
        )
        .run_ruby(
            r#"a = Article.new
raise "class #{Article.pick(a).inspect}" unless Article.pick(a) == "classside"
raise "instance #{a.row_id.inspect}" unless a.row_id == "new_article"
puts "class dom_id passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("class dom_id passed"));
}
