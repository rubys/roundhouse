# CRuby-only ActionView url_for: the polymorphic-record url form.
#
# `form_with url: comment` (lobsters' commentbox) resolves its action
# from the RECORD — /comments for a new record, /comments/:to_param for
# a persisted one. Strings pass through untouched, so the same call
# site serves both spellings. Overlay, not shared runtime: the is_a?
# dispatch on an untyped target is the shape the typed runtime refuses.
module ActionView
  module ViewHelpers
    def self.url_for(target)
      return target if target.is_a?(String)
      base = "/" + target.class.table_name
      target.persisted? ? "#{base}/#{target.to_param}" : base
    end
  end
end
