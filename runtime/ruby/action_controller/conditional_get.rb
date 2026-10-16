# frozen_string_literal: true
# What Rails' default middleware does to a controller's response on
# its way out: `Rack::ETag` and `Rack::ConditionalGet`, plus the
# `Cache-Control` default `ActionDispatch::Response` writes for a
# response that carries its own validator. A Rails 8.1 app's stack
# runs (innermost first) the response's `before_committed`, then
# `Rack::ETag, "no-cache"`, then `Rack::ConditionalGet`; this is the
# same three steps, in that order, on the controller's buffered
# status, body and header store.
#
# RUBY-FAMILY ONLY, like `cache_control.rb` beside this file: the
# strict targets serve through their own dispatchers. Called once per
# request, after `commit_cache_control!` and before the response is
# copied to the wire, by both dispatchers that carry this file (the
# CRuby overlay's `main.rb` and the spinel scaffold's `main.rb`).
#
# Not modeled: `Rack::ConditionalGet`'s `If-Modified-Since` branch.
# It compares HTTP dates (`Time.rfc2822`), which the spinel runtime
# cannot parse, and the runtime writes `Last-Modified` only when an
# action sets the header by hand. Such a response still gets the
# default `Cache-Control` and no body ETag, as in Rails.
module ActionController
  # `Rack::ETag::DEFAULT_CACHE_CONTROL` and actionpack's
  # `ActionDispatch::Http::Cache::Response::DEFAULT_CACHE_CONTROL`: the
  # same string.
  DEFAULT_CACHE_CONTROL = "max-age=0, private, must-revalidate"

  class Base
    # The hex SHA-256 Rack::ETag takes of a body. One method so the CRuby
    # overlay can take it through OpenSSL (message_verifier_cruby.rb):
    # the same digest, 7x faster on a campfire room page.
    def self.body_sha256_hex(body)
      Digest::SHA256.hexdigest(body)
    end

    # `request_method` is the request's own verb (Rack::ConditionalGet
    # reads REQUEST_METHOD); `if_none_match` is the request's
    # If-None-Match header, "" when it has none.
    def commit_conditional_get!(request_method, if_none_match)
      etag = @headers["ETag"]
      last_modified = @headers["Last-Modified"]

      # ActionDispatch::Response#handle_conditional_get!: an action
      # that set its own validator (`fresh_when`, `stale?`, or the
      # header itself) and no Cache-Control gets the default, which
      # Rack::ETag below would otherwise answer with "no-cache".
      if (!etag.nil? || !last_modified.nil?) && @headers["Cache-Control"].nil?
        @headers["Cache-Control"] = ActionController::DEFAULT_CACHE_CONTROL
      end

      # Rack::ETag: a 200 or 201 with a non-empty body and no validator
      # of its own gets a weak ETag, the first 32 hex digits of the
      # body's SHA-256. Rails mounts it as `Rack::ETag, "no-cache"`, so
      # a response without a body digest (a redirect, a `head`, an empty
      # body, an error status) gets `Cache-Control: no-cache` instead
      # of the default, unless it already has one.
      digested = false
      if (@status == 200 || @status == 201) && etag.nil? && last_modified.nil? && !@body.empty?
        etag = "W/\"" + ActionController::Base.body_sha256_hex(@body)[0, 32].to_s + "\""
        @headers["ETag"] = etag
        digested = true
      end
      if @headers["Cache-Control"].nil?
        @headers["Cache-Control"] = digested ? ActionController::DEFAULT_CACHE_CONTROL : "no-cache"
      end

      # Rack::ConditionalGet: a GET or HEAD answered 200 whose ETag is
      # exactly the request's If-None-Match becomes a 304 with no body,
      # no Content-Type and no Content-Length. Exact string equality,
      # as the middleware compares: a list, or the tag without its W/
      # prefix, does not match (`fresh_when` / `stale?` in base.rb do
      # their own weak, list-aware comparison).
      return nil unless request_method == "GET" || request_method == "HEAD"
      return nil unless @status == 200
      return nil if etag.nil? || if_none_match.empty?
      return nil unless etag == if_none_match
      @status = 304
      @body = +""
      @content_type = ""
      nil
    end
  end
end
