# frozen_string_literal: true
# Rails 8.1.4 response charset inheritance, shared by Ruby, JRuby and Spinel.
module ActionController
  class Base
    def render(body, status: :ok, content_type: nil, location: nil)
      @_body = body
      @_status = resolve_status(status)
      @_performed = true
      unless content_type.nil?
        incoming = render_content_type_parts(head_option_content_type(content_type))
        previous = render_content_type_parts(@_content_type.to_s)
        charset = incoming[1].to_s
        charset = previous[1].to_s if charset.empty?
        charset = "utf-8" if charset.empty?
        @_content_type = incoming[0].to_s
        unless @_content_type.empty?
          @_content_type = @_content_type + "; charset=" + charset.downcase
        end
      end
      @_location = ActionController.sanitize_location(location) unless location.nil?
      apply_vary_header
      nil
    end

    # ActionDispatch::Response::CONTENT_TYPE_PARSER, with numbered captures
    # for Spinel's MatchData API. Keep Rails' quoting and case semantics.
    def render_content_type_parts(value)
      match = value.match(/\A([^;\s]+\s*(?:;\s*(?:(?!charset)[^;\s])+)*)?(?:;\s*charset=("?)([^;\s]+)\2)?/)
      return ["", ""] if match.nil?
      [match[1].to_s, match[3].to_s]
    end
  end
end
