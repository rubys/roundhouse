//! `include?` is Module's ancestry query and the membership test of
//! Array, String, Hash, Set and Range. It is refused as Module protocol
//! only where the receiver is known not to be a class object. A value
//! typed `Class` answers the query (`klass.include?(Mod)`). A receiver of
//! unknown or gradual type gets the diagnostics any other send on it
//! gets, not a claim that a Module was required. A known instance of an
//! app class is still refused.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn app(lib: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/services/tags.rb", lib)
}

fn errors(lib: &str) -> Vec<String> {
    app(lib).emit(BuildTarget::Ruby).1
}

const TAGS: &str = r#"class Tags
  def tagged?(record)
    record.class.include?(Tagged)
  end
end
"#;

const ANCESTRY_QUERY: &str = r#"
raise "tagged" unless Tags.new.tagged?(Note.new)
raise "plain" if Tags.new.tagged?(Plain.new)
"#;

#[test]
fn a_class_value_answers_include() {
    app(TAGS)
        .write("app/services/tagged.rb", "module Tagged\n  def tag\n    \"tagged\"\n  end\nend\n")
        .write("app/services/note.rb", "class Note\n  include Tagged\nend\n")
        .write("app/services/plain.rb", "class Plain\nend\n")
        .run_ruby(ANCESTRY_QUERY)
        .assert_passes();
}

#[test]
fn an_unknown_or_gradual_receiver_is_not_a_missing_module() {
    for body in ["value.include?(1)", "T.unsafe(value).include?(1)"] {
        let errors = errors(&format!("class Tags\n  def has?(value)\n    {body}\n  end\nend\n"));
        assert!(!errors.iter().any(|e| e.contains("Module protocol")), "{body}: {errors:#?}");
    }
}

#[test]
fn a_known_instance_is_still_refused() {
    let errors = errors("class Tags\n  def has?\n    Tags.new.include?(1)\n  end\nend\n");
    assert!(errors.iter().any(|e| e.contains("Module protocol")), "{errors:#?}");
}

#[test]
fn a_receiver_typed_class_or_module_is_not_a_missing_module() {
    for ty in ["Class", "Module"] {
        let errors = errors(&format!(
            "class Tags\n  #: ({ty}) -> bool\n  def tagged?(klass)\n    klass.include?(Comparable)\n  end\nend\n"
        ));
        assert!(!errors.iter().any(|e| e.contains("Module protocol")), "{ty}: {errors:#?}");
    }
}
