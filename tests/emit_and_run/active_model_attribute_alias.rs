//! `alias` of an Attributes-API reader. activemodel 8.1.4: on a tableless
//! `ActiveModel::Model` + `Attributes` form, `attribute :flag, :boolean`
//! defines the reader on that line, so `alias flag? flag` after it copies
//! the casting reader (a later `def` doesn't change what it copied), and
//! an alias before it raises `NameError`. On an ActiveRecord model the
//! attribute's methods are generated lazily, so the same alias raises
//! `NameError` there, as it does for a column.
//!
//! The model path refused every such alias ("not defined").
use super::emit_and_run;

const FORM: &str = r#"class Form::Flags
  include ActiveModel::Model
  include ActiveModel::Attributes

  attribute :flag, :boolean
  alias flag? flag
  attribute :count, :integer
  alias amount count
  attribute :name, :string
  alias label name

  def name
    "override"
  end
end
"#;

const SCRIPT: &str = r##"
require_relative "app/models/form/flags"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
check("alias reads the cast value", Form::Flags.new(flag: "0").flag?, false)
check("alias of a truthy flag", Form::Flags.new(flag: "1").flag?, true)
check("alias of an integer", Form::Flags.new(count: "7").amount, 7)
f = Form::Flags.new
f.count = "8"
check("alias after a write", f.amount, 8)
check("alias copies the reader before a later def", Form::Flags.new(name: "x").label, "x")
check("the later def wins on its own name", Form::Flags.new(name: "x").name, "override")
puts "attribute alias passed"
"##;

fn form() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/form/flags.rb", FORM)
}

#[test]
fn an_alias_of_a_form_attribute_reads_its_reader_on_ruby() {
    let run = form().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("attribute alias passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_alias_of_a_form_attribute_reads_its_reader_on_spinel() {
    let run = form().run_spinel(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("attribute alias passed"), "{}", run.stdout);
}

/// Before the `attribute` line there is no reader yet (`NameError` in
/// Rails), so the alias stays refused.
#[test]
fn an_alias_before_its_attribute_is_refused() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .write(
            "app/models/form/early.rb",
            "class Form::Early\n  include ActiveModel::Model\n  include ActiveModel::Attributes\n\n  alias flag? flag\n  attribute :flag, :boolean\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("alias flag? flag") && e.contains("not defined")), "{errors:#?}");
}

/// An ActiveRecord model's `attribute` is lazy like a column, so the alias
/// is refused with the `alias_attribute` hint.
#[test]
fn an_alias_of_a_records_attribute_names_alias_attribute() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  attribute :flag, :boolean\n  alias flag? flag\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("alias flag? flag") && e.contains("alias_attribute :flag?, :flag")),
        "{errors:#?}"
    );
}
