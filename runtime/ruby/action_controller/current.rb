# Per-request context reachable from module-function helpers. Rails
# helpers run in the view context, which delegates `request` to the
# controller; the emitted helpers are module functions with no such
# context, so the dispatcher parks the request/controller here (the
# ActiveSupport::CurrentAttributes pattern) and the Ruby emit path
# rewrites bare `request` reads in helper/view module bodies to
# `ActionController::Current.request`. Single-threaded dispatch —
# plain module state, reset by assignment each request.
#
# Module-ivar statics rather than `class << self` (the overlay's
# shape): explicit `def self.` accessors are the statically-resolvable
# form every ingest/AOT path handles. The controller is parked (not
# its session) because `reset_session` swaps the controller's @session
# mid-action — a parked session reference would go stale.
module ActionController
  class Base
    attr_accessor :request

    def request_host_for_redirect
      r = @request
      return "" if r.nil?
      r.host.to_s
    end

    def request_protocol_for_url
      r = @request
      return "http://" if r.nil?
      r.protocol
    end

    def request_for_csrf
      @request
    end

    def csrf_header_token
      req = @request
      return "" if req.nil?
      req.env.fetch("HTTP_X_CSRF_TOKEN", "").to_s
    end

    # ---- conditional GET (Ruby / Spinel) ----------------------------
    #
    # Strict-target Base keeps empty / always-stale stubs; this reopen
    # parks the real Request-backed matching next to `attr_accessor
    # :request` so emit_and_run and Campfire get 304s without asking
    # rust/kotlin to host a Request object on Base.

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
      req = @request
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
      req = @request
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

  module Current
    def self.request
      @request
    end

    def self.request=(value)
      @request = value
    end

    def self.controller
      @controller
    end

    def self.controller=(value)
      @controller = value
    end

    # The current request's session, or nil outside a dispatch (unit
    # tests construct view helpers without a controller; they get the
    # shared runtime's empty-token behavior).
    def self.session
      c = @controller
      return nil if c.nil?
      c.session
    end
  end
end
