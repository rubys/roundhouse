# frozen_string_literal: true
# Not inlined in i18n_locale.rb: a strict target writes this module by hand, so the shared runtime reaches the thread, the file and the environment only through it
module I18nHost
  def self.locale_name
    Thread.current[:roundhouse_i18n_locale].to_s
  end

  def self.set_locale_name(name)
    Thread.current[:roundhouse_i18n_locale] = name
    name
  end

  def self.env_name
    Rails.env.to_s
  end

  def self.table_text
    File.read(File.join(Rails.root.to_s, "config", "i18n_translations"))
  end
end
