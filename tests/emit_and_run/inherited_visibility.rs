//! `private :has_role?` names a method the model's body does not define:
//! rolify's, a superclass's, or one Rails generates. The model is
//! carried with it, not refused: a generated method it names is emitted
//! private and a call from outside is an error, and a name roundhouse
//! does not carry (a gem's) declares nothing in the emitted model.
//! forem's `User` was dropped whole for it, and every `current_user`
//! with it.

use super::emit_and_run;

fn app(marker: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        &format!("  validates :title, presence: true\n  {marker}\n\n  def shout\n    title.upcase\n  end\n"),
    )
}

#[test]
fn a_gem_method_s_visibility_carries_the_model() {
    let errors = app("private :has_role?").check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
    app("private :has_role?")
        .run_ruby("raise \"shout\" unless Article.new(title: \"hi\").shout == \"HI\"\nputs \"carried passed\"\n")
        .assert_passes();
}

#[test]
fn a_generated_method_made_private_is_private() {
    let errors = app("private :body").check_errors();
    assert!(errors.iter().any(|e| e.contains("private method `body` called on Article")), "{errors:#?}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_gem_method_s_visibility_carries_the_model_on_spinel() {
    app("private :has_role?")
        .run_spinel("raise \"shout\" unless Article.new(title: \"hi\").shout == \"HI\"\nputs \"carried passed\"\n")
        .assert_passes();
}
