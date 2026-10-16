//! With rails-i18n in the bundle, or a ruby-cldr `config/locales/plurals.rb`,
//! a count picks its form by the locale's own rule (Polish `few`/`many`,
//! Russian, Japanese `other` only, French `one` below two), with I18n's
//! Pluralization backend taking `zero`, `0` and `1` first and `other` for
//! a form an entry lacks, and a locale without a rule taking the one its
//! fallback has. Expected values are i18n 1.15.2's with the same rule
//! files and translations on CRuby 3.4.

use super::emit_and_run;

const LOCALES_YML: &str = "en:
  probe:
    items:
      zero: \"no items\"
      one: \"%{count} item\"
      other: \"%{count} items\"
    files:
      one: \"%{count} file\"
      other: \"%{count} files\"
pl:
  probe:
    items:
      \"0\": \"brak\"
      one: \"%{count} rzecz\"
      few: \"%{count} rzeczy\"
      many: \"%{count} rzeczy (wiele)\"
      other: \"%{count} rzeczy (ułamek)\"
ru:
  probe:
    items:
      one: \"%{count} вещь\"
      few: \"%{count} вещи\"
      many: \"%{count} вещей\"
      other: \"%{count} вещи (дробь)\"
ja:
  probe:
    items:
      one: \"%{count} 個 (one)\"
      other: \"%{count} 個\"
fr:
  probe:
    items:
      one: \"%{count} objet\"
      other: \"%{count} objets\"
";

const PROBE: &str = "  validates :title, presence: true

  def self.items(locale, counts)
    I18n.with_locale(locale) do
      counts.map { |n| I18n.t(\"probe.items\", count: n) }
    end
  end

  def self.files(locale, count)
    I18n.with_locale(locale) do
      I18n.t(\"probe.files\", count: count)
    end
  end
";

fn plural_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/probe.yml", LOCALES_YML)
        .edit("Gemfile.lock", "    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n", "    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n    rails-i18n (8.1.0)\n      i18n (>= 0.7, < 2)\n")
        .edit(
            "config/application.rb",
            "    # config.time_zone",
            "    config.i18n.available_locales = %i[en pl ru ja fr]\n    config.i18n.default_locale = :en\n    config.i18n.fallbacks = [ :en ]\n    # config.time_zone",
        )
        .edit("app/models/article.rb", "  validates :title, presence: true\n", PROBE)
}

const PLURAL_ASSERTIONS: &str = r##"expect = lambda { |what, got, want| raise "#{what}: #{got.inspect}" unless got == want }
expect.call("en", Article.items(:en, [0, 1, 2]), ["no items", "1 item", "2 items"])
expect.call("pl", Article.items(:pl, [0, 1, 2, 5, 12, 22, 1.5]), ["brak", "1 rzecz", "2 rzeczy", "5 rzeczy (wiele)", "12 rzeczy (wiele)", "22 rzeczy", "1.5 rzeczy (ułamek)"])
expect.call("ru", Article.items(:ru, [1, 3, 5, 11, 21, 1.5]), ["1 вещь", "3 вещи", "5 вещей", "11 вещей", "21 вещь", "1.5 вещи (дробь)"])
expect.call("ja", Article.items(:ja, [1, 2]), ["1 個", "2 個"])
expect.call("fr", Article.items(:fr, [0, 1, 1.5, 2]), ["0 objet", "1 objet", "1.5 objet", "2 objets"])
expect.call("fallback", [Article.files(:pl, 1), Article.files(:pl, 5)], ["1 file", "5 files"])
puts "plural rules passed"
"##;

#[test]
fn a_count_takes_the_form_its_locale_s_rule_picks() {
    plural_app().run_ruby(PLURAL_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_count_takes_the_form_its_locale_s_rule_picks_on_spinel() {
    plural_app().run_spinel(PLURAL_ASSERTIONS).assert_passes();
}

/// ruby-cldr's output for `en`, `fr`, `ja` and `pl`, as an app checks it in.
const PLURALS_RB: &str = "{
  en: {
    i18n: {
      plural: {
        keys: %i[one other],
        rule:
          lambda do |n|
            n = n.respond_to?(:abs) ? n.abs : ((m = n.to_s)[0] == \"-\" ? m[1, m.length] : m)
            (n.to_i == 1 && ((v = n.to_s.split(\".\")[1]) ? v.length : 0) == 0) ? :one : :other
          end,
      },
    },
  },
  fr: {
    i18n: {
      plural: {
        keys: %i[one many other],
        rule:
          lambda do |n|
            n = n.respond_to?(:abs) ? n.abs : ((m = n.to_s)[0] == \"-\" ? m[1, m.length] : m)
            [0, 1].include?(n.to_i) ?
              :one :
              if (
                     (e = 0) == 0 && n.to_i != 0 && n.to_i % 1_000_000 == 0 &&
                       ((v = n.to_s.split(\".\")[1]) ? v.length : 0) == 0
                   ) || !(0..5).include?(e = 0)
                :many
              else
                :other
              end
          end,
      },
    },
  },
  ja: {
    i18n: {
      plural: {
        keys: [:other],
        rule:
          lambda do |n|
            n = n.respond_to?(:abs) ? n.abs : ((m = n.to_s)[0] == \"-\" ? m[1, m.length] : m)
            :other
          end,
      },
    },
  },
  pl: {
    i18n: {
      plural: {
        keys: %i[one few many other],
        rule:
          lambda do |n|
            n = n.respond_to?(:abs) ? n.abs : ((m = n.to_s)[0] == \"-\" ? m[1, m.length] : m)
            (n.to_i == 1 && ((v = n.to_s.split(\".\")[1]) ? v.length : 0) == 0) ?
              :one :
              (
                ((v = n.to_s.split(\".\")[1]) ? v.length : 0) == 0 && (2..4).include?(n.to_i % 10) &&
                  !(12..14).include?(n.to_i % 100)
              ) ?
                :few :
                if (
                       ((v = n.to_s.split(\".\")[1]) ? v.length : 0) == 0 && n.to_i != 1 &&
                         (0..1).include?(n.to_i % 10)
                     ) ||
                       (
                         ((v = n.to_s.split(\".\")[1]) ? v.length : 0) == 0 &&
                           (5..9).include?(n.to_i % 10)
                       ) ||
                       (
                         ((v = n.to_s.split(\".\")[1]) ? v.length : 0) == 0 &&
                           (12..14).include?(n.to_i % 100)
                       )
                  :many
                else
                  :other
                end
          end,
      },
    },
  },
}
";

const CLDR_YML: &str = "en:
  probe:
    items:
      zero: \"no items\"
      one: \"%{count} item\"
      other: \"%{count} items\"
    files:
      one: \"%{count} file\"
      other: \"%{count} files\"
pl:
  probe:
    items:
      \"0\": \"brak\"
      one: \"%{count} rzecz\"
      few: \"%{count} rzeczy\"
      many: \"%{count} rzeczy (wiele)\"
      other: \"%{count} rzeczy (ułamek)\"
ja:
  probe:
    items:
      one: \"%{count} 個 (one)\"
      other: \"%{count} 個\"
fr:
  probe:
    items:
      many: \"%{count} d’objets\"
      one: \"%{count} objet\"
      other: \"%{count} objets\"
de:
  probe:
    items:
      one: \"%{count} Ding\"
      other: \"%{count} Dinge\"
";

fn cldr_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/plurals.rb", PLURALS_RB)
        .write("config/locales/probe.yml", CLDR_YML)
        .edit(
            "config/application.rb",
            "    # config.time_zone",
            "    config.i18n.available_locales = %i[en pl ja fr de]\n    config.i18n.default_locale = :en\n    config.i18n.fallbacks = [ :en ]\n    # config.time_zone",
        )
        .edit("app/models/article.rb", "  validates :title, presence: true\n", PROBE)
}

const CLDR_ASSERTIONS: &str = r##"expect = lambda { |what, got, want| raise "#{what}: #{got.inspect}" unless got == want }
expect.call("en", Article.items(:en, [0, 1, 2, 1.0]), ["no items", "1 item", "2 items", "1.0 items"])
expect.call("pl", Article.items(:pl, [0, 1, 2, 5, 12, 22, 1.5]), ["brak", "1 rzecz", "2 rzeczy", "5 rzeczy (wiele)", "12 rzeczy (wiele)", "22 rzeczy", "1.5 rzeczy (ułamek)"])
expect.call("ja", Article.items(:ja, [1, 2]), ["1 個", "2 個"])
expect.call("fr", Article.items(:fr, [0, 1, 1.5, 2, 1_000_000]), ["0 objet", "1 objet", "1.5 objet", "2 objets", "1000000 d’objets"])
expect.call("de takes en's rule", Article.items(:de, [1, 1.0, 2]), ["1 Ding", "1.0 Dinge", "2 Dinge"])
puts "cldr rules passed"
"##;

#[test]
fn a_count_takes_the_form_an_app_s_plurals_rb_picks() {
    cldr_app().run_ruby(CLDR_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_count_takes_the_form_an_app_s_plurals_rb_picks_on_spinel() {
    cldr_app().run_spinel(CLDR_ASSERTIONS).assert_passes();
}
