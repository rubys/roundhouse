//! A permitted form field is a String, but schema writers take the column type.
//! Both typed construction and PATCH must convert at this shared boundary.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::{
    app::App,
    dialect::LibraryClass,
    expr::{Expr, ExprNode},
    ident::Symbol,
    ty::Ty,
};

fn scalar_app() -> App {
    let files = [
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table :users do |t|
    t.integer :hour, null: false, default: 9
    t.float :ratio
    t.boolean :enabled, null: false, default: true
    t.string :name
  end
end
"#,
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\nend\n",
        ),
        (
            "app/controllers/users_controller.rb",
            r#"class UsersController < ApplicationController
  def create
    User.create!(user_params)
  end
  def update
    User.find(params[:id]).update!(user_params)
  end
  private
  def user_params
    params.require(:user).permit(:hour, :ratio, :enabled, :name)
  end
end
"#,
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, s)| (p.into(), s.as_bytes().to_vec()))
        .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest scalar form")
}

fn lowered_user(app: &App) -> LibraryClass {
    let specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
    roundhouse::lower::lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        vec![],
        &specs,
    )
    .0
    .into_iter()
    .find(|lc| lc.name.0.as_str() == "User")
    .expect("User lowered")
}

fn assignment_value<'a>(e: &'a Expr, setter: &str) -> Option<&'a Expr> {
    if let ExprNode::Send { method, args, .. } = &*e.node {
        if method.as_str() == setter {
            return args.first();
        }
    }
    let mut found = None;
    e.node.for_each_child(&mut |child| {
        if found.is_none() {
            found = assignment_value(child, setter);
        }
    });
    found
}

#[test]
fn schema_scalar_conversions_are_in_shared_lowered_ir() {
    let app = scalar_app();
    let user = lowered_user(&app);
    for name in [
        "from_params",
        "update_from_user_params",
        "update_from_user_params!",
    ] {
        let method = user
            .methods
            .iter()
            .find(|m| m.name.as_str() == name)
            .unwrap();
        for (field, ty) in [
            ("hour", Ty::Int),
            (
                "ratio",
                Ty::Union {
                    variants: vec![Ty::Float, Ty::Nil],
                },
            ),
            ("enabled", Ty::Bool),
        ] {
            let value =
                assignment_value(&method.body, &format!("{field}=")).expect("setter argument");
            assert_eq!(value.ty.as_ref(), Some(&ty), "{name} {field}: {value:?}");
        }
    }
    let specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
    assert_eq!(
        specs
            .canonical(&Symbol::from("user"))
            .unwrap()
            .fields
            .iter()
            .map(Symbol::as_str)
            .collect::<Vec<_>>(),
        ["hour", "ratio", "enabled", "name"]
    );
}

fn scalar_overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"rank\", default: 7\n    t.float \"score\", default: 1.5\n    t.boolean \"visible\", default: true\n    t.integer \"offset\", default: 3, null: false\n    t.boolean \"listed\", default: true, null: false\n    t.integer \"protected_count\", default: 11, null: false")
        .edit("app/controllers/articles_controller.rb", "params.expect(article: [ :title, :body ])",
            "params.expect(article: [ :title, :body, :rank, :score, :visible, :offset, :listed ])")
}

#[test]
fn emitted_typed_factories_and_updates_cast_form_values() {
    scalar_overlay().run_ruby(r#"
def form(values)
  ArticleParams.from_raw({"article" => values})
end
article = Article.from_params(form({"title" => "Typed", "body" => "A sufficiently long body.",
  "rank" => "-12", "score" => "2.75", "offset" => "0", "visible" => "off", "listed" => "0",
  "protected_count" => "99"}))
raise article.rank.inspect unless article.rank == -12
raise article.score.inspect unless article.score == 2.75
raise article.offset.inspect unless article.offset == 0
raise article.visible.inspect unless article.visible == false
raise article.listed.inspect unless article.listed == false
raise article.protected_count.inspect unless article.protected_count == 11
article.save!
article.update_from_article_params!(form({"rank" => "25", "score" => "-0.5", "visible" => "1", "listed" => "yes"}))
raise unless article.rank == 25 && article.score == -0.5 && article.visible == true && article.listed == true
raise unless article.title == "Typed" && article.body == "A sufficiently long body." && article.offset == 0
reloaded = Article.find(article.id)
raise unless reloaded.rank == 25 && reloaded.score == -0.5 && reloaded.visible == true && reloaded.listed == true
%w[0 f F false FALSE off OFF].each do |value|
  article.update_from_article_params!(form({"visible" => value, "listed" => value}))
  raise value unless article.visible == false && article.listed == false
end
%w[1 true TRUE on ON False].each do |value|
  article.update_from_article_params!(form({"visible" => value, "listed" => value}))
  raise value unless article.visible == true && article.listed == true
end
puts "PASS schema-directed form conversion"
"#).assert_passes();
}

#[test]
fn emitted_nullable_blanks_clear_without_clobbering_omitted_or_filtered_fields() {
    scalar_overlay().run_ruby(r#"
def form(values)
  ArticleParams.from_raw({"article" => values})
end
article = Article.from_params(form({"title" => "Defaults", "body" => "A sufficiently long body."}))
raise unless article.rank == 7 && article.score == 1.5 && article.visible == true
raise unless article.offset == 3 && article.listed == true
article.save!
article.update_from_article_params!(form({"rank" => "", "score" => " ", "visible" => ""}))
raise article.rank.inspect unless article.rank.nil?
raise article.score.inspect unless article.score.nil?
raise article.visible.inspect unless article.visible.nil?
raise unless article.offset == 3 && article.listed == true && article.title == "Defaults"
reloaded = Article.find(article.id)
raise unless reloaded.rank.nil? && reloaded.score.nil? && reloaded.visible.nil?
article.update_from_article_params!(form({"rank" => ["999"], "score" => {"amount" => "99"}, "visible" => ["1"], "protected_count" => "99"}))
raise unless article.rank.nil? && article.score.nil? && article.visible.nil? && article.protected_count == 11
blank = Article.from_params(form({"rank" => " ", "score" => "", "visible" => ""}))
raise unless blank.rank.nil? && blank.score.nil? && blank.visible.nil?
puts "PASS nullable, presence and scalar-permit contract"
"#).assert_passes();
}

#[test]
fn emitted_integer_enum_form_labels_are_not_replaced_by_numeric_coercion() {
    scalar_overlay()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0")
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, live: 1 }")
        .edit("app/controllers/articles_controller.rb", "params.expect(article: [ :title, :body, :rank, :score, :visible, :offset, :listed ])",
            "params.expect(article: [ :title, :body, :rank, :score, :visible, :offset, :listed, :state ])")
        .run_ruby(r#"
def form(values)
  ArticleParams.from_raw({"article" => values})
end
article = Article.from_params(form({"title" => "Enum", "body" => "A sufficiently long body.", "state" => "live"}))
raise article.state.inspect unless article.state == "live"
article.save!
article.update_from_article_params!(form({"state" => "draft"}))
raise article.state.inspect unless article.state == "draft"
article.update_from_article_params!(form({"state" => "1"}))
raise article.state.inspect unless article.state == "live"
article.update_from_article_params!(form({"state" => ""}))
raise article.state.inspect unless article.state.nil?
puts "PASS enum labels retain their mapping"
"#).assert_passes();
}

#[test]
fn emitted_nullable_boolean_reads_and_hydration_keep_nil_and_predicates_false() {
    scalar_overlay()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  def nullable_boolean_predicate\n    visible?\n  end")
        .run_ruby(r#"
def assert_null_boolean(article)
  raise article.visible.inspect unless article.visible.nil?
  raise article.visible?.inspect unless article.visible? == false
  raise article[:visible].inspect unless article[:visible].nil?
end
article = Article.new(title: "Nullable", body: "A sufficiently long body.", visible: nil)
assert_null_boolean(article)
article.save!
assert_null_boolean(Article.find(article.id))
assert_null_boolean(Article.instantiate(article.attributes.merge("id" => article.id)))
article.reload
assert_null_boolean(article)
article.visible = false
raise unless article.visible == false && article.visible? == false
article.visible = true
raise unless article.visible == true && article.visible? == true
puts "PASS nullable Boolean getter, predicate and hydration"
"#).assert_passes();
}
