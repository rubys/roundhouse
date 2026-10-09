//! The Spinel side of the not-found parity checks: a routed action the
//! controller lacks raises `AbstractController::ActionNotFound`, and a
//! finder miss raises `ActiveRecord::RecordNotFound` with Rails' readers
//! and wording. The CRuby side lives in `tests/emit_and_run/`
//! (`action_not_found.rs`, `finder_miss_readers.rs`,
//! `finder_miss_messages.rs`). These need a Spinel compiler, so they are
//! `#[ignore]`d like their sibling suites; CI's `spinel-framework` job
//! runs them with `--ignored` (`scripts/ci-plan.py`'s `SPINEL_TESTS`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const BOOT: &str = "Db.configure(\":memory:\")\nSchema.statements.each { |sql| Db.exec(sql) }\nActiveRecord.adapter = SqliteAdapter\n";

/// `resources :gadgets` routes seven actions; the controller defines
/// only `show`.
fn gadgets() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  resources :gadgets\n")
        .write(
            "app/controllers/gadgets_controller.rb",
            "class GadgetsController < ApplicationController\n  def show\n    render json: { ok: true }\n  end\nend\n",
        )
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_routed_action_the_controller_lacks_raises_action_not_found_on_spinel() {
    let run = gadgets().run_spinel(
        r##"c = GadgetsController.new
begin
  c.process_action(:edit)
  puts "edit fell through"
rescue AbstractController::ActionNotFound => e
  puts "not found: #{e.message}"
end
"##,
    );
    run.assert_passes();
    assert!(
        run.stdout.contains("not found: The action 'edit' could not be found for GadgetsController"),
        "{}",
        run.stdout
    );
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_finder_miss_carries_model_primary_key_and_id_on_spinel() {
    let script = format!(
        r##"{BOOT}begin
  Article.find("999999")
rescue ActiveRecord::RecordNotFound => e
  puts "find model=#{{e.model}} pk=#{{e.primary_key}} id=#{{e.id}}"
end
begin
  Article.where(title: "zz").first!
rescue ActiveRecord::RecordNotFound => e
  puts "first! model=#{{e.model}} pk=#{{e.primary_key}} id_nil=#{{e.id.nil?}}"
end
begin
  Article.find_by!(title: "zz")
rescue ActiveRecord::RecordNotFound => e
  puts "find_by! model=#{{e.model}} pk=#{{e.primary_key}} id_nil=#{{e.id.nil?}}"
end
"##
    );
    let run = emit_and_run::real_blog().run_spinel(&script);
    run.assert_passes();
    for line in [
        "find model=Article pk=id id=999999",
        "first! model=Article pk=id id_nil=true",
        "find_by! model=Article pk=id id_nil=true",
    ] {
        assert!(run.stdout.contains(line), "missing {line:?} in:\n{}", run.stdout);
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_finder_miss_uses_rails_wording_on_spinel() {
    let script = format!(
        r##"{BOOT}article = Article.create!(title: "Present", body: "A sufficiently long body.")
def message(label)
  yield
  raise "#{{label}}: no RecordNotFound"
rescue ActiveRecord::RecordNotFound => e
  e.message
end
{{
  "find string" => [message("find string") {{ Article.find("999") }}, "Couldn't find Article with 'id'=\"999\""],
  "find integer" => [message("find integer") {{ Article.find(999) }}, "Couldn't find Article with 'id'=999"],
  "find nil" => [message("find nil") {{ Article.find(nil) }}, "Couldn't find Article without an ID"],
  "relation find array" => [message("relation find array") {{ ActiveRecord::Relation.new(Article).find([article.id, 999]) }}, "Couldn't find all Articles with 'id': (#{{article.id}}, 999) (found 1 results, but was looking for 2)."],
  "relation find nil" => [message("relation find nil") {{ ActiveRecord::Relation.new(Article).where(title: "Present").find(nil) }}, "Couldn't find Article without an ID"],
  "unscoped relation find" => [message("unscoped relation find") {{ ActiveRecord::Relation.new(Article).find(999) }}, "Couldn't find Article with 'id'=999"],
  "first!" => [message("first!") {{ ActiveRecord::Relation.new(Comment).first! }}, "Couldn't find Comment"],
  "sole" => [message("sole") {{ ActiveRecord::Relation.new(Comment).sole }}, "Couldn't find Comment"],
}}.each do |label, (got, want)|
  raise "#{{label}}: got #{{got.inspect}}, want #{{want.inspect}}" unless got == want
end
puts "messages match"
"##
    );
    let run = emit_and_run::real_blog().run_spinel(&script);
    run.assert_passes();
    assert!(run.stdout.contains("messages match"), "{}", run.stdout);
}

/// An `alias_method` of a public action is an action: no
/// `ActionNotFound` for it, while a name nothing defines still raises.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_alias_of_a_public_action_is_defined_on_spinel() {
    let run = emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  resources :gadgets, only: [:index, :show, :edit]\n",
        )
        .write(
            "app/controllers/gadgets_controller.rb",
            "class GadgetsController < ApplicationController\n  def show\n    render json: { ok: true }\n  end\n  alias_method :index, :show\nend\n",
        )
        .run_spinel(
            r##"[:index, :edit].each do |action|
  begin
    GadgetsController.new.process_action(action)
    puts "#{action}: dispatched"
  rescue AbstractController::ActionNotFound
    puts "#{action}: not found"
  end
end
"##,
        );
    run.assert_passes();
    for line in ["index: dispatched", "edit: not found"] {
        assert!(run.stdout.contains(line), "missing {line:?} in:\n{}", run.stdout);
    }
}
