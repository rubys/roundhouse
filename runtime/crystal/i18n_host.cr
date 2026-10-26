module I18nHost
  # Not one class variable: HTTP::Server runs each connection in its own fiber, and a fiber that waits lets another request set the locale
  @@locales = {} of UInt64 => String
  @@table : String? = nil

  def self.locale_name : String
    @@locales[Fiber.current.object_id]? || ""
  end

  def self.set_locale_name(name : String) : String
    @@locales[Fiber.current.object_id] = name
    name
  end

  def self.reset_request : Nil
    @@locales.delete(Fiber.current.object_id)
    nil
  end

  def self.env_name : String
    ENV["RAILS_ENV"]? || ENV["RACK_ENV"]? || "development"
  end

  def self.table_text : String
    @@table ||= File.read(File.join(Dir.current, "config", "i18n_translations"))
  end
end
