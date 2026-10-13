# `I18n.locale` / `I18n.default_locale` — the locale an app folds into
# its cache keys (campfire's CachedResponses and MessagesHelper key
# every cached page and fragment by it).
#
# Rails' default locale is `:en`, and the current locale is the default
# until something sets it. Both answer `:en` here; setting a locale at
# run time (`I18n.locale =`, `with_locale`) is not modeled. Translation
# is folded at compile time (`src/lower/i18n_translate.rs`); what reaches
# here is the answer for a key the locale lacks.
module I18n
  def self.default_locale
    :en
  end

  def self.locale
    default_locale
  end

  class MissingTranslationData < ArgumentError
  end

  # Whether this environment raises on a missing translation: `raises`
  # is `config/application.rb`'s setting, and the comma-separated
  # `flipped` environments set the other value in their own file.
  def self.missing_raises(raises, flipped)
    env = Rails.env.to_s
    flipped.split(",").include?(env) ? !raises : raises
  end

  # A run-time key as `I18n.normalize_keys` reads it: dot-separated,
  # empty segments dropped (`"a..b."` is `a.b`).
  def self.normalize_key(key)
    out = []
    key.split(".").each { |part| out << part unless part.empty? }
    out.join(".")
  end

  # A run-time key that names a subtree, which `I18n.t` answers as a
  # Hash; no folded translation carries one.
  def self.subtree_translation(key)
    raise NotImplementedError, "I18n.t(\"#{key}\") answers a Hash of translations, which is not supported"
  end

  # `I18n.t`'s answer for `key` (`en.shop.title`): I18n's default
  # handler returns the message, `raise_on_missing_translations` raises it.
  def self.missing_translation(key, raises)
    message = "Translation missing: " + key
    raise MissingTranslationData, message if raises
    message
  end

  # A template's `t` for `key`: Action View's debug span, titled with the
  # key and the call's options (`details`, each value escaped once), and
  # reading as the key's last segment titleized.
  def self.missing_translation_tag(key, details, raises)
    missing_translation(key, raises)
    last = key.split(".").last.to_s
    title = ActionView::ViewHelpers.html_escape("translation missing: " + key + details)
    text = ActionView::ViewHelpers.html_escape(ActiveSupport.titleize(last))
    "<span class=\"translation_missing\" title=\"" + title + "\">" + text + "</span>"
  end
end
