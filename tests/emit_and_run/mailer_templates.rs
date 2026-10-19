//! A mailer action renders its templates into the message, as
//! ActionMailer does: a text and an html template make a
//! multipart/alternative (each part in the `layout "mailer"` of its
//! format, the message's own body empty), one template makes the body.
//! The class's `default from:` fills `from`, a nearer class's wins, and
//! address headers answer bare addresses. Expected values are
//! ActionMailer 8.1.4's for the same templates and layouts on CRuby 3.4.

use super::emit_and_run;

fn mailer_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/mailers/title_mailer.rb", "class TitleMailer < ApplicationMailer\n  def titled(article)\n    @article = article\n    mail(to: \"Reader <reader@example.com>\", subject: \"Titled\")\n  end\nend\n")
        .write("app/mailers/note_mailer.rb", "class NoteMailer < ApplicationMailer\n  default from: \"notes@example.com\"\n\n  def noted(article)\n    @article = article\n    mail(to: \"reader@example.com\", subject: \"Noted\")\n  end\nend\n")
        .write("app/views/title_mailer/titled.text.erb", "Title: <%= @article.title.upcase %> & more\n")
        .write("app/views/title_mailer/titled.html.erb", "<p>Title: <%= @article.title %></p>\n")
        .write("app/views/note_mailer/noted.text.erb", "Note: <%= @article.title %>\n")
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def announce\n    t = TitleMailer.titled(self).deliver_now\n    n = NoteMailer.noted(self).deliver_now\n    [[t.to, t.from, t.subject, t.multipart?, t.body.to_s, t.text_part&.body.to_s, t.html_part&.body.to_s], [n.to, n.from, n.multipart?, n.body.to_s, n.text_part, n.html_part]]\n  end\n",
        )
}

const ASSERTIONS: &str = r##"t, n = Article.new(title: "<b>x</b>").announce
html = "<!DOCTYPE html>\n<html>\n  <head>\n    <meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\">\n    <style>\n      /* Email styles need to be inline */\n    </style>\n  </head>\n\n  <body>\n    <p>Title: &lt;b&gt;x&lt;/b&gt;</p>\n\n  </body>\n</html>\n"
got = t
raise "multipart: #{got.inspect}" unless got == [["reader@example.com"], ["from@example.com"], "Titled", true, "", "Title: <B>X</B> & more\n\n", html]
got = n
raise "text only: #{got.inspect}" unless got == [["reader@example.com"], ["notes@example.com"], false, "Note: <b>x</b>\n\n", nil, nil]
puts "mailer templates passed"
"##;

#[test]
fn a_mailer_renders_its_templates_into_the_message() {
    mailer_app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_mailer_renders_its_templates_into_the_message_on_spinel() {
    mailer_app().run_spinel(ASSERTIONS).assert_passes();
}

/// What is not modeled is reported, not sent empty.
#[test]
fn an_unmodeled_mail_is_an_error() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .write(
            "app/mailers/odd_mailer.rb",
            "class OddMailer < ApplicationMailer\n  default reply_to: -> { \"r@example.com\" }\n\n  def blocked\n    mail(to: \"a@example.com\") do |format|\n      format.text\n    end\n  end\n\n  def missing\n    mail(to: \"a@example.com\")\n  end\n\n  def renamed\n    mail(to: \"a@example.com\", template_name: \"other\")\n  end\n\n  def given\n    mail(to: \"a@example.com\", body: \"plain\")\n  end\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    for needle in ["`mail { |format| … }`", "no `odd_mailer/missing` template", "`template_name:`", "a `default` given a Proc"] {
        assert!(errors.iter().any(|e| e.contains(needle)), "{needle}: {errors:#?}");
    }
    assert_eq!(errors.len(), 4, "{errors:#?}");
}

/// No strict target carries the ActionMailer runtime: each mailer is
/// reported there, and real-blog's `ApplicationMailer` with it.
#[test]
fn a_mailer_is_unsupported_on_strict_targets() {
    let (_emitted, errors) = mailer_app().emit(roundhouse::project::BuildTarget::Rust);
    for name in ["TitleMailer", "NoteMailer"] {
        assert!(errors.iter().any(|e| e.contains(&format!("`{name}` delivers through the Ruby-family ActionMailer runtime"))), "{name}: {errors:#?}");
    }
}

