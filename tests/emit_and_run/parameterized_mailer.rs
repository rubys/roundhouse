//! `TitleMailer.with(to: …).titled(self)` is a parameterized mailer: the
//! action reads `params`, and `with` hands its argument on to `titled`
//! as `TitleMailer.titled(self)` does. The emitted runtime had neither
//! `with` nor `params`, so `check` was clean while the program raised
//! NoMethodError, and the untyped `with` left `@article` unknown in the
//! template (mastodon's admin mailers).

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/mailers/title_mailer.rb", "class TitleMailer < ApplicationMailer\n  def titled(article)\n    @article = article\n    mail(to: params[:to], subject: \"Titled\")\n  end\nend\n")
        .write("app/views/title_mailer/titled.text.erb", "Title: <%= @article.title.upcase %>\n")
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def announce\n    m = TitleMailer.with(to: \"reader@example.com\").titled(self).deliver_now\n    [m.to, m.body.to_s]\n  end\n",
        )
}

const ASSERTIONS: &str = r##"got = Article.new(title: "x").announce
raise got.inspect unless got == [["reader@example.com"], "Title: X\n\n"]
puts "parameterized mailer passed"
"##;

#[test]
fn a_parameterized_action_types_its_argument() {
    let errors = app().check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_parameterized_action_types_its_argument_on_spinel() {
    app().run_spinel(ASSERTIONS).assert_passes();
}

/// Rails' `params` is an empty hash until `with` gives it one.
#[test]
fn an_unparameterized_action_reads_empty_params() {
    let app = emit_and_run::real_blog()
        .write("app/mailers/title_mailer.rb", "class TitleMailer < ApplicationMailer\n  def titled(article)\n    @article = article\n    mail(to: params.fetch(:to, \"default@example.com\"), subject: \"Titled\")\n  end\nend\n")
        .write("app/views/title_mailer/titled.text.erb", "Title: <%= @article.title %>\n")
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def announce\n    TitleMailer.titled(self).deliver_now.to\n  end\n",
        );
    app.run_ruby("got = Article.new(title: \"x\").announce\nraise got.inspect unless got == [\"default@example.com\"]\nputs \"empty params passed\"\n")
        .assert_passes();
}
