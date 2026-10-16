# frozen_string_literal: true
# `Rails::HTML5::SafeListSanitizer`'s two class-level lists, for spinel:
# what the `rails-html-sanitizer` gem answers on the ruby family, where
# the gem itself is loaded. campfire reads them at LOAD time since the
# Lexxy merge —
#
#   AUTO_LINK_ALLOWED_TAGS = Rails::HTML5::SafeListSanitizer.allowed_tags +
#     ContentFilters::EDITOR_FORMATTING_TAGS
#
# in `MessagesHelper` — and without the constant the binary answered
# `uninitialized constant` inside `message_presentation`, whose
# `rescue Exception` rendered every message body as "".
#
# The lists are the ported engine's own (`ActionView::ViewHelpers.
# sanitize_default_tags` / `_attributes`, which ARE rails-html-sanitizer
# 1.7.1's HTML5 safe-list, ported), so what the app computes from them is
# what the engine they are handed back to already speaks.
#
# NOT named after the gem, and loaded from spinel's boot.rb only: every
# runtime/spinel/*.rb reaches every ruby-family tree, and on CRuby this
# class is the gem's — defining it first would be a superclass mismatch
# when the gem loads. Same rule as `nokogiri_spinel.rb`.
module Rails
  module HTML5
    class SafeListSanitizer
      def self.allowed_tags
        ActionView::ViewHelpers.sanitize_default_tags
      end

      def self.allowed_attributes
        ActionView::ViewHelpers.sanitize_default_attributes
      end
    end
  end
end
