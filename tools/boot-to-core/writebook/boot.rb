# frozen_string_literal: true

# Only the pinned, disposable app is loaded. The runner supplies a secret-free
# environment and local SQLite database; no application source is replaced.
require File.join(ENV.fetch("WRITEBOOK_ROOT"), "config/environment")

def prepare_writebook
  ActiveRecord::Schema.verbose = false
  load File.join(ENV.fetch("WRITEBOOK_ROOT"), "db/schema.rb")
end

def writebook_roots(lane)
  case lane
  when "attributes" then { Section => %i[body body= markable searchable_content] }
  when "current" then { Current => %i[user user=] }
  when "enum" then { Access => %i[level level= reader? editor?] }
  when "delegate" then { Section => [:title] }
  when "markdown" then { Page => %i[body body= markable] }
  when "position" then { Leaf => [:previous] }
  when "slug" then { Leaf => [:slug] }
  when "qr" then { QrCodeLink => [:url] }
  when "embed" then { EmbedProvider => [:allows?] }
  when "lightbox" then { HtmlScrubber => [:scrub] }
  else raise "unknown lane #{lane}"
  end
end

def generate_writebook_attributes(roots)
  roots.each_key do |owner|
    owner.define_attribute_methods if owner < ActiveRecord::Base
  end
end
