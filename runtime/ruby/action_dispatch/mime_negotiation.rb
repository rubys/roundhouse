# frozen_string_literal: true
# actionpack 8.1's `ActionDispatch::Http::MimeNegotiation#
# should_apply_vary_header?`: whether the response format was
# negotiated from the request's Accept header, which is when Rails'
# `render` answers `Vary: Accept` (ActionController::Rendering
# `_set_vary_header`, Rails 7.1 and later). A cache that keyed only on
# the URL would otherwise hand an HTML page to a JSON client.
#
# RUBY-FAMILY ONLY: both ruby-family dispatchers (the CRuby overlay's
# `main.rb`, the spinel scaffold's `main.rb`) call it once per request
# and hand the answer to the controller (`Base#vary_on_accept`). Plain
# Strings in, so the two dispatchers' different request shapes share
# it. `config.action_dispatch.ignore_accept_header` is not modeled: it
# defaults to false, which is what this answers for.
module ActionDispatch
  module MimeNegotiation
    # `format_param`: the request has a `format` parameter (a path
    # extension, `?format=`, or a route default), which Rails reads
    # instead of the Accept header. The other three are the request's
    # Accept, X-Requested-With and Content-Type headers, "" when absent.
    def self.should_apply_vary_header?(accept, x_requested_with, content_type, format_param)
      return false if format_param
      accept_present = !accept.strip.empty?
      # `xhr?`: an X-Requested-With naming XMLHttpRequest, any case.
      xhr = x_requested_with.downcase.include?("xmlhttprequest")
      # `valid_accept_header`: an XHR with an Accept or a body type, or
      # an Accept that is not a browser's (a list with `*/*` beside
      # another type: Rails' BROWSER_LIKE_ACCEPTS).
      return true if xhr && (accept_present || !content_type.empty?)
      accept_present && !accept.match?(/,\s*\*\/\*|\*\/\*\s*,/)
    end

    # Whether Rails' `request.formats.first` is JSON, which is when
    # `ActionDispatch::PublicExceptions` (the production exceptions app)
    # answers an error status with `{"status":404,"error":"Not Found"}`
    # instead of the static HTML page. `formats` reads, in order: the
    # `format` parameter (a routed path's extension, `?format=`, a
    # route's own format; `format_param`, "" when absent), the Accept
    # header when it is one Rails trusts (see above), and the path's own
    # extension (`extension`, "" when none). `formats.first` of an
    # Accept list is its highest-q type, the earliest on a tie.
    def self.json_format?(format_param, extension, accept, x_requested_with, content_type)
      return format_param == "json" unless format_param.empty?
      if should_apply_vary_header?(accept, x_requested_with, content_type, false)
        preferred = accept.strip.empty? ? content_type : preferred_type(accept)
        return json_type?(preferred)
      end
      extension == "json"
    end

    # The Accept list's highest-q media type (`Mime::Type.parse`, then
    # `first`), without its parameters.
    def self.preferred_type(accept)
      best = ""
      best_q = -1.0
      accept.split(",").each do |entry|
        parts = entry.split(";")
        q = 1.0
        parts.each do |part|
          param = part.strip
          q = param[2, param.length].to_s.to_f if param.start_with?("q=")
        end
        if q > best_q
          best = parts.empty? ? "" : parts[0].to_s.strip
          best_q = q
        end
      end
      best
    end

    # The media types Rails registers for `:json`.
    def self.json_type?(media_type)
      type = media_type.split(";", 2)[0].to_s.strip.downcase
      type == "application/json" || type == "text/x-json" || type == "application/jsonrequest"
    end
  end
end
