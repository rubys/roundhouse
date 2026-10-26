# frozen_string_literal: true
require "erb"
require "yaml"

# `ActiveSupport::ConfigurationFile.parse(path)` — read a YAML file
# and run it through ERB when it contains a tag. Rails loads with
# `unsafe_load` so a symbol or a date in the document is kept.
# An empty document is `{}`. Symbolizing keys is the caller's.
module ActiveSupport
  class ConfigurationFile
    def self.parse(path)
      text = File.read(path)
      text = ERB.new(text).result if text.include?("<%")
      loaded = YAML.respond_to?(:unsafe_load) ? YAML.unsafe_load(text) : YAML.load(text)
      return {} if loaded.nil?
      loaded
    end
  end
end
