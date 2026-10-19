//! `I18n.locale_available?` and the i18n gem's `I18n::Locale::Tag`:
//! discourse checks a requested locale with the first and normalizes a
//! browser's language through `Tag::Rfc4646`, mastodon scopes a rule's
//! translations by `Tag.tag(locale).to_a.first`. Each was an error.
//! Expected values are i18n 1.15.2's for the same calls.

use super::emit_and_run;

fn tag_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/application.rb",
            "    # config.time_zone",
            "    config.i18n.available_locales = %i[en pl]\n    config.i18n.default_locale = :en\n    # config.time_zone",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.available\n    [I18n.locale_available?(:en), I18n.locale_available?(\"pl\"), I18n.locale_available?(\"fr\"), I18n.locale_available?(nil)]\n  end\n\n  def self.normalize(language)\n    return if language.nil?\n\n    tag = I18n::Locale::Tag::Rfc4646.tag(language.tr(\"_\", \"-\"))\n    return \"\" if tag&.language.nil?\n\n    primary_language = tag.language.split(\"-\", 2).first\n    [primary_language, tag.script].compact.join(\"-\")\n  end\n\n  def self.tags\n    [I18n::Locale::Tag.tag(\"pt-BR\").to_a.first, I18n::Locale::Tag.tag(:\"zh-Hant\").to_a, I18n::Locale::Tag.tag(\"en\").to_sym, I18n::Locale::Tag::Rfc4646.tag(\"de-de-1996\").to_a, I18n::Locale::Tag::Rfc4646.tag(\"!!\").nil?, I18n::Locale::Tag::Rfc4646.tag(\"en-a-bbb-x-a-ccc\").to_s, I18n::Locale::Tag::Rfc4646.tag(\"/i-klingon/\").grandfathered]\n  end\n",
        )
}

const ASSERTIONS: &str = r##"got = Article.available
raise got.inspect unless got == [true, true, false, false]
got = %w[zh_hant_TW EN-us x-foo zh-yue-HK sr-Latn-RS].map { |l| Article.normalize(l) }
raise got.inspect unless got == ["zh-Hant", "en", "", "zh", "sr-Latn"]
got = Article.tags
raise got.inspect unless got == ["pt", ["zh", "Hant"], :en, ["de", nil, "DE", "1996", nil, nil, nil], true, "en-a-bbb-x-a-ccc", "i-klingon"]
puts "tags passed"
"##;

#[test]
fn locale_available_and_a_locale_s_tag_answer_as_the_gem_does() {
    tag_app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn locale_available_and_a_locale_s_tag_answer_as_the_gem_does_on_spinel() {
    tag_app().run_spinel(ASSERTIONS).assert_passes();
}
