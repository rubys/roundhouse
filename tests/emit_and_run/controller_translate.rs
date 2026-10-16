//! A controller's `t` is `AbstractController::Translation#translate`:
//! `I18n.t` for a full key, and for a lazy `.key` the key under
//! `<controller_path>.<action_name>`, falling back to the same key under
//! `<controller_path>` alone. `action_name` is the routed action's, so a
//! private method the action calls reads the action's keys. A key found
//! nowhere answers I18n's message for a `default:` list, which names
//! every key it considered.

use super::emit_and_run;

const EN_YML: &str = "en:
  hello: \"Hello world\"
  plain: \"Plain text\"
  articles:
    shared: \"Shared by every action\"
    index:
      greeting: \"Hi %{name}\"
      label: \"Index label\"
";

fn controller_translate_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/en.yml", EN_YML)
        .edit(
            "app/controllers/articles_controller.rb",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)
    @greeting = t(\".greeting\", name: \"Ann\")
    @shared = t(\".shared\")
    @plain = t(\"plain\")
    @label = index_label
    @missing = t(\".nowhere\")
",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "  private\n",
            "  private\n    def index_label\n      t(\".label\")\n    end\n\n",
        )
        .edit(
            "app/views/articles/index.html.erb",
            "<h1",
            "<p id=\"greeting\"><%= @greeting %></p><p id=\"shared\"><%= @shared %></p><p id=\"plain\"><%= @plain %></p><p id=\"label\"><%= @label %></p><p id=\"missing\"><%= @missing %></p>\n<h1",
        )
        .write(
            "test/controllers/translated_flash_controller_test.rb",
            "require \"test_helper\"

class TranslatedFlashControllerTest < ActionDispatch::IntegrationTest
  test \"the controller's t reads the action's keys\" do
    get articles_url
    assert_includes response.body, \"<p id=\\\"greeting\\\">Hi Ann</p>\"
    assert_includes response.body, \"<p id=\\\"shared\\\">Shared by every action</p>\"
    assert_includes response.body, \"<p id=\\\"plain\\\">Plain text</p>\"
    assert_includes response.body, \"<p id=\\\"label\\\">Index label</p>\"
    assert_includes response.body, \"<p id=\\\"missing\\\">Translation missing. Options considered were:\\n- en.articles.index.nowhere\\n- en.articles.nowhere</p>\"
  end
end
",
        )
}

#[test]
fn a_controller_t_reads_the_action_s_keys() {
    controller_translate_app().run_test("test/controllers/translated_flash_controller_test.rb").assert_passes();
}

/// An `_html` key stays an error in a controller: Rails returns it
/// html-safe, and that mark does not reach the view here. A controller
/// that defines its own `t` keeps it.
#[test]
fn a_controller_html_key_stays_an_error_and_an_own_t_is_kept() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("config/locales/en.yml", "en:\n  articles:\n    index:\n      note_html: \"<b>Note</b>\"\n")
        .edit(
            "app/controllers/articles_controller.rb",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n    @note = t(\".note_html\")\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("`_html` key in a controller")), "{errors:?}");

    let (emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n    @note = t(\"anything\")\n",
        )
        .edit("app/controllers/articles_controller.rb", "  private\n", "  private\n    def t(key)\n      key.upcase\n    end\n\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("I18n")), "{errors:?}");
    let controller = std::fs::read_to_string(emitted.join("app/controllers/articles_controller.rb")).expect("emitted controller");
    assert!(controller.contains("t(\"anything\")"), "{controller}");
}

/// A `t` from a concern the controller includes is the concern's, the
/// same as one the controller defines: typed as the concern's method
/// and not folded into I18n's.
#[test]
fn a_t_from_an_included_concern_is_kept() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write(
            "app/controllers/concerns/shouting.rb",
            "module Shouting\n  extend ActiveSupport::Concern\n\n  def t(key)\n    key.upcase\n  end\nend\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n",
            "class ArticlesController < ApplicationController\n  include Shouting\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n    @note = t(\"anything\")\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("I18n")), "{errors:?}");
    let controller = std::fs::read_to_string(emitted.join("app/controllers/articles_controller.rb")).expect("emitted controller");
    assert!(controller.contains("t(\"anything\")"), "{controller}");
}

/// `I18n.t` with only key defaults, none of them there: a bare Symbol
/// answers the plain missing message, a list names every key considered
/// (i18n 1.15.2's `MissingTranslation#message`), and the environment
/// that raises raises that message.
#[test]
fn a_missing_key_with_key_defaults_answers_as_i18n_does() {
    let app = || {
        emit_and_run::real_blog().write("config/locales/en.yml", EN_YML).edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.missing_texts
    [I18n.t(\"no.such\", default: :also_missing), I18n.t(:nope, scope: :articles, default: [:gone, :lost]), I18n.t(:nope, default: [:plain, :lost])]
  end
",
        )
    };
    app()
        .run_ruby(
            r#"expected = ["Translation missing: en.no.such", "Translation missing. Options considered were:\n- en.articles.nope\n- en.articles.gone\n- en.articles.lost", "Plain text"]
raise Article.missing_texts.inspect unless Article.missing_texts == expected
puts "key defaults passed"
"#,
        )
        .assert_passes();
    app()
        .edit(
            "config/environments/development.rb",
            "  # config.i18n.raise_on_missing_translations = true",
            "  config.i18n.raise_on_missing_translations = true",
        )
        .run_ruby(
            r#"begin
  Article.missing_texts
  raise "no raise"
rescue I18n::MissingTranslationData => e
  raise e.message unless e.message == "Translation missing: en.no.such"
end
puts "raise passed"
"#,
        )
        .assert_passes();
}
