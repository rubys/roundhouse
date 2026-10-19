# frozen_string_literal: true
# The i18n gem's `I18n::Locale::Tag` (i18n 1.15.2, `lib/i18n/locale/tag*.rb`),
# ported: `Tag.tag` and its Simple and Rfc4646 implementations, as they
# split a locale into subtags. Not `Parents`, which no app reaches, nor
# Simple's `tag` reader: the runtime's signatures key a method by name
# alone, and `Simple.tag` holds it.
module I18n
  module Locale
    module Tag
      # Not the gem's `implementation=`: no app sets one, so its default Simple answers.
      def self.tag(tag)
        Simple.tag(tag)
      end

      class Simple
        def self.tag(tag)
          new(tag)
        end

        def initialize(tag)
          @tag = tag.to_s.to_sym
        end

        def subtags
          @tag.to_s.split("-")
        end

        def to_sym
          @tag
        end

        def to_s
          @tag.to_s
        end

        def to_a
          subtags
        end
      end

      class Rfc4646
        # The gem's `/* … */` around the grandfathered form is no comment under `x`: it reads `\/*(…)*\/`, so only a slash-wrapped tag matches it.
        def self.tag(tag)
          m = /\A(?:([a-z]{2,3}(?:(?:-[a-z]{3}){0,3})?|[a-z]{4}|[a-z]{5,8})(?:-([a-z]{4}))?(?:-([a-z]{2}|\d{3}))?(?:-([0-9a-z]{5,8}|\d[0-9a-z]{3}))*(?:-([0-9a-wyz](?:-[0-9a-z]{2,8})+))*(?:-(x(?:-[0-9a-z]{1,8})+))?|(x(?:-[0-9a-z]{1,8})+)|\/*([a-z]{1,3}(?:-[0-9a-z]{2,8}){1,2})*\/)\z/i.match(tag.to_s)
          return nil if m.nil?
          new(m[1], m[2], m[3], m[4], m[5], m[6].nil? ? m[7] : m[6], m[8])
        end

        attr_reader :extension, :privateuse, :grandfathered

        def initialize(language, script, region, variant, extension, privateuse, grandfathered)
          @language = language
          @script = script
          @region = region
          @variant = variant
          @extension = extension
          @privateuse = privateuse
          @grandfathered = grandfathered
        end

        def language
          @language.nil? ? nil : @language.downcase
        end

        def script
          @script.nil? ? nil : @script.capitalize
        end

        def region
          @region.nil? ? nil : @region.upcase
        end

        def variant
          @variant.nil? ? nil : @variant.downcase
        end

        def to_sym
          to_s.to_sym
        end

        def to_s
          to_a.compact.join("-")
        end

        def to_a
          [language, script, region, variant, extension, privateuse, grandfathered]
        end
      end
    end
  end
end
