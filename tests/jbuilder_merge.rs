//! `json.merge!(hash)` merges the hash's pairs into the object.
//!
//! Jbuilder's `merge!` adds each pair of its argument to the object
//! being built. The lowering took `merge!` for a key, so the template
//! wrote `"merge!":"{kind: \"widget\", version: 2}"`. A literal Hash
//! whose keys the object sets nowhere else now writes its pairs; a
//! value typed Hash is walked when the template runs. Any other shape
//! (an Array, an untyped value, a key the object may already set) is
//! refused: Jbuilder's answer there is not one pair per entry.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn jbuilder_merge_bang_merges_pairs() {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/gizmos_controller.rb",
            r#"class GizmosController < ApplicationController
  def index; end

  def show; end

  def create
    @extra = { "region" => "north" }
  end
end
"#,
        )
        .write(
            "app/views/gizmos/index.json.jbuilder",
            "json.merge!({ kind: \"widget\", \"version\" => 2 })\njson.done true\n",
        )
        .write("app/views/gizmos/show.json.jbuilder", "json.merge!({})\njson.id 1\n")
        .write("app/views/gizmos/create.json.jbuilder", "json.merge!(@extra)\njson.id 1\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :gizmos, only: %i[index show create]\nend\n")
        .run_ruby(
            r##"
def probe(path, query = "", verb = "GET")
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => verb, "PATH_INFO" => path, "QUERY_STRING" => query, "rack.input" => StringIO.new(""))
  text = +""
  body.each { |part| text << part }
  [status, text]
end
got = probe("/gizmos.json")
raise "index #{got.inspect}" unless got == [200, %({"kind":"widget","version":2,"done":true})]
got = probe("/gizmos/1.json")
raise "show #{got.inspect}" unless got == [200, %({"id":1})]
# An ivar's type is not known to the template: refused, not guessed.
refused = begin
  probe("/gizmos.json", "", "POST")
  nil
rescue RuntimeError => e
  e.message
end
raise "create #{refused.inspect}" unless refused.to_s.include?("merge! not supported")
puts "ALL OK"
"##,
        )
        .assert_passes();
}

/// A literal `merge!` repeating a key the object already set is not
/// written as a second pair. Rails 8.1.4 + jbuilder 2.15.1 deep-merges a
/// String key (`{"profile":{"name":"a","age":2}}`) and answers 500 for a
/// Symbol one (`detected duplicate key "profile"`), so the lowering
/// refuses the statement (a raise naming it) rather than write
/// `"profile"` twice.
#[test]
fn jbuilder_merge_bang_over_a_set_key_is_refused() {
    let (tree, errors) = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("app/controllers/gizmos_controller.rb", "class GizmosController < ApplicationController\n  def index; end\nend\n")
        .write(
            "app/views/gizmos/index.json.jbuilder",
            "json.profile do\n  json.name \"a\"\nend\njson.merge!(\"profile\" => { \"age\" => 2 })\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :gizmos, only: %i[index]\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let view = std::fs::read_to_string(tree.join("app/views/gizmos/index_json.rb")).expect("view emitted");
    assert!(
        view.contains("merge! not supported") && view.matches("profile").count() == 1,
        "an overlapping merge! must be refused, not written as a second pair:\n{view}"
    );
}

/// `json.merge!([1, 2])` is an Array merge (jbuilder 2.15.1: `[1,2]`
/// alone, `Jbuilder::MergeError` after a pair), not a Hash to iterate:
/// refused rather than lowered as an object.
#[test]
fn jbuilder_merge_bang_of_an_array_is_refused() {
    let (tree, errors) = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("app/controllers/gizmos_controller.rb", "class GizmosController < ApplicationController\n  def index; end\nend\n")
        .write("app/views/gizmos/index.json.jbuilder", "json.merge!([1, 2])\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :gizmos, only: %i[index]\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let view = std::fs::read_to_string(tree.join("app/views/gizmos/index_json.rb")).expect("view emitted");
    assert!(
        view.contains("merge! not supported") && !view.contains(".each"),
        "an Array merge! must be refused, not iterated as a Hash:\n{view}"
    );
}
