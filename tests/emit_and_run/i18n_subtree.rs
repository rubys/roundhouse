//! `I18n.t` on a node of Strings answers a nested Hash of them, keyed by
//! Symbols in the files' order, from the first locale of the fallback
//! chain that has the node (forem hands one to its front end). A value
//! that is not a String and no Array of Strings and Symbols, a number or
//! a boolean (an Array holding a Hash here) stays an error; a nil value
//! is a missing one, as I18n finds
//! nothing there; an empty mapping is `{}`, even where a fallback
//! locale has the node. Expected values are i18n 1.15.2's for the same
//! files on CRuby 3.4.

use super::emit_and_run;

const EN_YML: &str = "en:
  probe:
    feed:
      zeta:
        title: \"Z title\"
        action: \"Go\"
      alpha:
        title: \"A title\"
    greet: \"Hello\"
    nothing:
    menu:
      more: {}
      home: \"Home\"
    links: {}
";

fn subtree_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/en.yml", EN_YML)
        .write("config/locales/pl.yml", "pl:\n  probe:\n    greet: \"Cześć\"\n    menu: {}\n")
        .edit(
            "config/application.rb",
            "    # config.time_zone",
            "    config.i18n.available_locales = %i[en pl]\n    config.i18n.default_locale = :en\n    config.i18n.fallbacks = [ :en ]\n    # config.time_zone",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.feed\n    I18n.t(\"probe.feed\")\n  end\n\n  def self.scoped_feed\n    I18n.t(\"feed\", scope: \"probe\")\n  end\n\n  def self.polish_feed\n    I18n.with_locale(:pl) { I18n.t(\"probe.feed\") }\n  end\n\n  def self.first_entry\n    feed.to_a.first.inspect\n  end\n\n  def self.nothing\n    I18n.t(\"probe.nothing\")\n  end\n\n  def self.menus\n    [I18n.t(\"probe.menu\"), I18n.t(\"probe.links\"), I18n.with_locale(:pl) { I18n.t(\"probe.menu\") }, I18n.t(\"probe.menu\").to_a.first.inspect]\n  end\n",
        )
}

const ASSERTIONS: &str = r##"want = { zeta: { title: "Z title", action: "Go" }, alpha: { title: "A title" } }
got = [Article.feed, Article.scoped_feed, Article.polish_feed]
raise got.inspect unless got == [want, want, want]
raise Article.first_entry unless Article.first_entry == [:zeta, { title: "Z title", action: "Go" }].inspect
raise Article.nothing unless Article.nothing == "Translation missing: en.probe.nothing"
menus = Article.menus
raise menus.inspect unless menus == [{ more: {}, home: "Home" }, {}, {}, [:more, {}].inspect]
puts "subtree passed"
"##;

#[test]
fn a_node_of_strings_answers_as_a_hash() {
    subtree_app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_node_of_strings_answers_as_a_hash_on_spinel() {
    subtree_app().run_spinel(ASSERTIONS).assert_passes();
}

#[test]
fn a_value_that_is_not_a_string_stays_an_error() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("config/locales/en.yml", "en:\n  probe:\n    days:\n      - \"Mon\"\n      - tue: \"Tue\"\n    week:\n      days:\n        - \"Mon\"\n      name: \"Week\"\n")
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.days\n    I18n.t(\"probe.days\")\n  end\n\n  def self.week\n    I18n.t(\"probe.week\")\n  end\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("`probe.days` is not a String")), "{errors:#?}");
    assert!(errors.iter().any(|e| e.contains("`probe.week` names a subtree")), "{errors:#?}");
}
