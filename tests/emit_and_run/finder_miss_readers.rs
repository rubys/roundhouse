//! `ActiveRecord::RecordNotFound` carries Rails' `model`, `primary_key`
//! and `id` readers. `find` sets all three, with `id` as passed;
//! `find_by!`, `first!` and `sole` set the model and key and leave `id`
//! nil (activerecord 8.1.4, `errors.rb` and `finder_methods.rb`).

use super::emit_and_run;

/// The shape a JSON API's global handler takes: the 404 body is built
/// from `e.model` and `e.id`.
fn gadgets() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  resources :gadgets, only: :show\n")
        .write(
            "app/controllers/gadgets_controller.rb",
            r#"class GadgetsController < ApplicationController
  rescue_from ActiveRecord::RecordNotFound, with: :not_found

  def show
    article = Article.find(params[:id])
    render json: { title: article.title }
  end

  private

  def not_found(e)
    render json: { model: e.model, id: e.id }, status: :not_found
  end
end
"#,
        )
}

#[test]
fn a_finder_miss_carries_model_primary_key_and_id() {
    gadgets()
        .run_ruby(
            r##"status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/gadgets/999999", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
text = body.join
raise "GET /gadgets/999999 answered #{status} #{text}" unless status == 404 && JSON.parse(text) == { "model" => "Article", "id" => "999999" }

article = Article.create!(title: "Present", body: "A sufficiently long body.")
def miss(label)
  yield
  raise "#{label}: no RecordNotFound"
rescue ActiveRecord::RecordNotFound => e
  [e.model, e.primary_key, e.id]
end
{
  "find string" => [miss("find string") { Article.find("999") }, ["Article", "id", "999"]],
  "find integer" => [miss("find integer") { Article.find(999) }, ["Article", "id", 999]],
  "find nil" => [miss("find nil") { Article.find(nil) }, ["Article", "id", nil]],
  "find array" => [miss("find array") { Article.find([article.id, 999]) }, ["Article", "id", [article.id, 999]]],
  "where find" => [miss("where find") { Article.where(title: "Present").find(999) }, ["Article", "id", 999]],
  "relation find nil" => [miss("relation find nil") { Article.where(title: "Present").find(nil) }, ["Article", "id", nil]],
  "find_by!" => [miss("find_by!") { Article.find_by!(title: "zz") }, ["Article", "id", nil]],
  "relation find_by!" => [miss("relation find_by!") { Article.where(title: "Present").find_by!(body: "zz") }, ["Article", "id", nil]],
  "first!" => [miss("first!") { Article.where(title: "zz").first! }, ["Article", "id", nil]],
  "sole" => [miss("sole") { Article.where(title: "zz").sole }, ["Article", "id", nil]],
  "find_sole_by" => [miss("find_sole_by") { Article.find_sole_by(title: "zz") }, ["Article", "id", nil]],
}.each do |label, (got, want)|
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
"##,
        )
        .assert_passes();
}

/// The strict targets that transpile `errors.rb` declare `id` nilable:
/// a bare `RecordNotFound.new(message)` leaves it nil. `untyped` gave
/// Crystal `property id : String = ""`, which a nil does not fit, and a
/// nested `(A | B)?` gave Kotlin `Any??`.
#[test]
fn strict_targets_declare_the_finder_miss_id_nilable() {
    use roundhouse::project::BuildTarget;
    for (target, path, want) in [
        (BuildTarget::Crystal, "src/errors.cr", "property id : String | Int64 | Float64 | Array(String | Int64 | Float64) | Nil\n"),
        (BuildTarget::Kotlin, "src/main/kotlin/Errors.kt", "primaryKey: String? = null, id: Any? = null)"),
        (BuildTarget::Python, "app/errors.py", "    id: str | int | float | list[str | int | float] | None\n"),
        (BuildTarget::CSharp, "app/runtime/Errors.cs", "string? primaryKey = null, object? id = null)"),
        (BuildTarget::Swift, "Sources/App/Errors.swift", "_ primaryKey: String? = nil, _ id: Any? = nil)"),
    ] {
        let (emitted, _) = emit_and_run::real_blog().emit(target);
        let src = std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{target:?} {path}: {e}"));
        assert!(src.contains(want), "{target:?} {path} lacks {want:?}:\n{src}");
    }
}
