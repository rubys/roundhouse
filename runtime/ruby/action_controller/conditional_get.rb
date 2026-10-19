# frozen_string_literal: true
# Conditional GET for the ruby family. Two surfaces:
#
# `commit_conditional_get!` is what Rails' default middleware does to a
# controller's response on its way out: `Rack::ETag` and
# `Rack::ConditionalGet`, plus the `Cache-Control` default
# `ActionDispatch::Response` writes for a response that carries its
# own validator. A Rails 8.1 app's stack runs (innermost first) the
# response's `before_committed`, then `Rack::ETag, "no-cache"`, then
# `Rack::ConditionalGet`; this is the same three steps, in that order,
# on the controller's buffered status, body and header store.
#
# `fresh_when` / `stale?` are the action-side helpers (etag source,
# weak ETag, If-None-Match parse/match, GET/HEAD gate). Strict targets
# keep the empty / always-stale stubs in `base.rb`.
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
      # prefix, does not match (`fresh_when` / `stale?` below do their
      # own weak, list-aware comparison).
      return nil unless request_method == "GET" || request_method == "HEAD"
      return nil unless @status == 200
      return nil if etag.nil? || if_none_match.empty?
      return nil unless etag == if_none_match
      @status = 304
      @body = +""
      @content_type = ""
      nil
    end

    # ---- fresh_when / stale? (Ruby / Spinel) -----------------------
    #
    # Strict-target Base keeps empty / always-stale stubs. This reopen
    # is the real Request-backed matching: etag source, weak ETag,
    # If-None-Match parse/match, and the GET/HEAD gate. `base.rb` stays
    # the thin dispatcher entry so rust/kotlin emit keeps compiling
    # without a Request seat on Base.

    def fresh_when(subject)
      etag_src = fresh_when_etag_source(subject)
      return if etag_src.nil?
      tag = weak_etag_for(etag_src)
      response.headers["ETag"] = tag
      # RFC 9110 §15.4.5: 304 is only for conditional GET/HEAD.
      return unless conditional_get_verb_ok?
      inm = request_if_none_match
      return if inm.nil?
      head(:not_modified) if if_none_match_matches?(inm, tag)
    end

    def stale?(etag: nil)
      return true if etag.nil?
      return true unless etag.is_a?(String)
      tag = weak_etag_for(etag)
      response.headers["ETag"] = tag
      return true unless conditional_get_verb_ok?
      inm = request_if_none_match
      return true if inm.nil?
      # Rails: matching INM both answers 304 and reports fresh (`false`)
      # so `if stale?(etag:)` skips the action body with a performed
      # response — same as `fresh_when`, not a bare boolean.
      if if_none_match_matches?(inm, tag)
        head(:not_modified)
        false
      else
        true
      end
    end

    def conditional_get_verb_ok?
      verb = conditional_get_verb
      verb == "" || verb == "GET" || verb == "HEAD"
    end

    def conditional_get_verb
      req = request
      return req.request_method.to_s unless req.nil?
      # Accessor (not `@request_method`): this reopen's body must type
      # against Base's declared surface; the ivar alone is untyped here.
      request_method.to_s
    end

    def fresh_when_etag_source(subject)
      return nil if subject.nil?
      if subject.is_a?(Hash)
        v = subject[:etag]
        return v.to_s unless v.nil?
        v = subject["etag"]
        return v.to_s unless v.nil?
        return nil
      end
      return subject if subject.is_a?(String)
      nil
    end

    def weak_etag_for(src)
      "W/\"" + src + "\""
    end

    def request_if_none_match
      req = request
      return nil if req.nil?
      value = req.headers["If-None-Match"]
      return nil if value.nil?
      text = value.to_s
      return nil if text.empty?
      text
    end

    def if_none_match_matches?(inm, tag)
      want = normalize_etag(tag)
      return false if want.nil?
      parts = if_none_match_members(inm)
      if_none_match_parts_match?(parts, 0, want)
    end

    # Iterative over members: a long comma-list of valid tags must not
    # recurse once per member (stack blow-up under Spinel's header cap).
    def if_none_match_parts_match?(parts, i, want)
      while i < parts.length
        part = parts[i]
        return true if part == "*"
        got = normalize_etag(part)
        return true if !got.nil? && got == want
        i = i + 1
      end
      false
    end

    def if_none_match_members(inm)
      if_none_match_members_at(inm.to_s, 0, "", false, [])
    end

    # Iterative scan: a long If-None-Match must not recurse once per
    # character (stack blow-up on an accepted header). Quote-aware
    # commas, trim, and empty-member omission match the prior walker.
    def if_none_match_members_at(s, i, buf, in_quote, out)
      while i < s.length
        ch = s[i, 1]
        if ch == "\""
          nxt = false
          nxt = true unless in_quote
          in_quote = nxt
        else
          if ch == "," && !in_quote
            part = buf.strip
            out << part unless part.empty?
            buf = ""
            in_quote = false
            i = i + 1
            next
          end
        end
        buf = buf + ch
        i = i + 1
      end
      part = buf.strip
      out << part unless part.empty?
      out
    end

    def normalize_etag(value)
      s = value.to_s.strip
      return "*" if s == "*"
      if s.length >= 2 && (s[0, 2] == "W/" || s[0, 2] == "w/")
        s = s[2, s.length].to_s.strip
      end
      return nil unless s.length >= 2 && s[0, 1] == "\"" && s[s.length - 1, 1] == "\""
      s[1, s.length - 2].to_s
    end
  end
end
