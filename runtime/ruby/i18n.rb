# `I18n.locale` / `I18n.default_locale` — the locale an app folds into
# its cache keys (campfire's CachedResponses and MessagesHelper key
# every cached page and fragment by it).
#
# Rails' default locale is `:en`, and the current locale is the default
# until something sets it. Both answer `:en` here: setting a locale
# (`I18n.locale =`, `with_locale`, `config.i18n.default_locale`) and
# translation (`I18n.t`) are not modeled, so an app that does either
# is outside what this file supports.
module I18n
  def self.default_locale
    :en
  end

  def self.locale
    default_locale
  end
end
