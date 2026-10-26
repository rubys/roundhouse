//! forem's `User` delegates its role questions to `authorizer`, a reader
//! the model defines itself, not an association, and declares
//! `private :has_role?` for a method rolify defines. The delegation
//! expanded only through associations, and the inherited visibility
//! change kept the declaration's own visibility from resolving, so every
//! caller of `user.admin?` was a dispatch failure.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/models/title_check.rb",
            "class TitleCheck\n  def initialize(article)\n    @article = article\n  end\n\n  def shouting?\n    @article.title == @article.title.upcase\n  end\n\n  def short?\n    @article.title.length < 5\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  private :has_role?\n\n  def title_check\n    @title_check ||= TitleCheck.new(self)\n  end\n\n  delegate(\n    :shouting?,\n    :short?,\n    to: :title_check,\n  )\n",
        )
}

const ASSERTIONS: &str = r##"got = [Article.new(title: "LOUD").shouting?, Article.new(title: "quiet one").shouting?, Article.new(title: "abc").short?]
raise got.inspect unless got == [true, false, true]
puts "model delegate to reader passed"
"##;

#[test]
fn a_model_delegate_to_its_own_reader_runs() {
    let errors = app().check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_model_delegate_to_its_own_reader_runs_on_spinel() {
    app().run_spinel(ASSERTIONS).assert_passes();
}
