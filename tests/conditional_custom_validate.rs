//! Emitted-program regression for this fix (kept out of tests/emit_and_run.rs
//! so concurrent appends there do not conflict). Same harness.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// `validate :m, if: :pred` / `unless: :pred` (e.g. `validate
/// :no_overlap, if: :validate_overlap?`). Only the bare-Symbol form was
/// ingested; with an `if:` the whole declaration fell to the
/// unsupported-DSL ledger as a warning, and a record the validation
/// should have rejected saved.
#[test]
fn a_conditional_custom_validation_runs_when_its_predicate_holds() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true",
            "  validate :no_shouting, if: :strict?\n  validate :no_whisper, unless: :strict?\n  validate :no_tilde, if: :strict?, unless: :lenient?\n\n  def strict?\n    body.to_s.start_with?(\"!\")\n  end\n\n  def lenient?\n    body.to_s.end_with?(\"~\")\n  end\n\n  def no_tilde\n    errors.add(:title, \"has a tilde\") if title.include?(\"~\")\n  end\n\n  def no_shouting\n    errors.add(:title, \"is shouting\") if title == title.upcase\n  end\n\n  def no_whisper\n    errors.add(:title, \"is whispering\") if title == title.downcase\n  end\n\n  validates :title, presence: true",
        )
        .run_ruby(r#"
loud = Article.new(title: "LOUD", body: "!Long enough body")
raise "if: should run: #{loud.errors.inspect}" if loud.valid?
raise "if: off should skip" unless Article.new(title: "LOUD", body: "Long enough body").valid?
raise "unless: should run" if Article.new(title: "quiet", body: "Long enough body").valid?
raise "unless: on should skip" unless Article.new(title: "quiet", body: "!Long enough body").valid?
raise "if: and unless: both hold should skip" unless Article.new(title: "Mid~dle", body: "!Long enough body~").valid?
raise "if: holds, unless: does not should run" if Article.new(title: "Mid~dle", body: "!Long enough body").valid?
puts "conditional validate"
"#)
        .assert_passes();
}
