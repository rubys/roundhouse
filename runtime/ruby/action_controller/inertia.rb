# frozen_string_literal: true
# Props are held as JSON text, not as a Hash of values, and a lazy prop
# is a condition around its expression, not a Proc: a strict target can
# compile neither a mixed-value Hash nor a Proc.
module ActionController
  class InertiaPage
    attr_reader :version

    def initialize
      @component = +""
      @version = +""
      @encrypt_history = false
      @partial = false
      @only = []
      @except = []
      @keys = []
      @values = []
      @shared_keys = []
      @deferred_groups = []
      @deferred_keys = []
      @clear_history = false
      @preserve_fragment = false
    end

    def start_page(component, version, encrypt_history, partial_component, partial_data, partial_except)
      @component = component
      @version = version
      @encrypt_history = encrypt_history
      @partial = partial_component == component
      @only = InertiaPage.header_list(partial_data)
      @except = InertiaPage.header_list(partial_except)
      @keys = []
      @values = []
      @shared_keys = []
      @deferred_groups = []
      @deferred_keys = []
      @clear_history = false
      @preserve_fragment = false
      nil
    end

    def self.header_list(value)
      out = []
      parts = value.split(",")
      i = 0
      while i < parts.length
        part = parts[i].strip
        out << part unless part.empty?
        i += 1
      end
      out
    end

    def requested?(key)
      (@only.empty? || @only.include?(key)) && !@except.include?(key)
    end

    def eager?(key)
      !@partial || requested?(key)
    end

    def optional?(key)
      @partial && requested?(key)
    end

    def deferred?(key, group)
      unless @partial
        @deferred_groups << group
        @deferred_keys << key
        return false
      end
      requested?(key)
    end

    # Shared props are registered separately from the evaluate? guards so
    # share synthesis and page props share one eager?/optional?/deferred?
    # surface (predicates stay non-mutating aside from deferred bookkeeping).
    def mark_clear_history
      @clear_history = true
      nil
    end

    def mark_preserve_fragment
      @preserve_fragment = true
      nil
    end

    def mark_shared(key)
      @shared_keys << key unless @shared_keys.include?(key)
      nil
    end

    def prop_json(key, json)
      i = @keys.index(key)
      if i.nil?
        @keys << key
        @values << json
      else
        @values[i] = json
      end
      nil
    end

    def page_json(url, notice, alert)
      out = "{\"component\":" + JsonBuilder.encode_value(@component)
      out = out + ",\"props\":{"
      i = 0
      while i < @keys.length
        out = out + "," if i > 0
        out = out + JsonBuilder.encode_value(@keys[i]) + ":" + @values[i]
        i += 1
      end
      out = out + "},\"url\":" + JsonBuilder.encode_value(url)
      out = out + ",\"version\":" + (@version.empty? ? "null" : JsonBuilder.encode_value(@version))
      out = out + ",\"encryptHistory\":" + (@encrypt_history ? "true" : "false")
      out = out + ",\"clearHistory\":" + (@clear_history ? "true" : "false")
      out = out + flash_json(notice, alert)
      out = out + preserve_fragment_json
      out + shared_json + deferred_json + "}"
    end

    # Omitted when false, matching inertia-rails: the key is present only
    # after a redirect that asked to keep the URL fragment.
    def preserve_fragment_json
      return "" unless @preserve_fragment
      ",\"preserveFragment\":true"
    end

    def flash_json(notice, alert)
      parts = []
      parts << "\"notice\":" + JsonBuilder.encode_value(notice) unless notice.nil?
      parts << "\"alert\":" + JsonBuilder.encode_value(alert) unless alert.nil?
      return "" if parts.empty?
      ",\"flash\":{" + parts.join(",") + "}"
    end

    def shared_json
      return "" if @shared_keys.empty?
      ",\"sharedProps\":" + InertiaPage.json_string_array(@shared_keys)
    end

    def deferred_json
      return "" if @deferred_keys.empty?
      groups = []
      i = 0
      while i < @deferred_groups.length
        groups << @deferred_groups[i] unless groups.include?(@deferred_groups[i])
        i += 1
      end
      entries = []
      g = 0
      while g < groups.length
        keys = []
        i = 0
        while i < @deferred_keys.length
          keys << @deferred_keys[i] if @deferred_groups[i] == groups[g]
          i += 1
        end
        entries << JsonBuilder.encode_value(groups[g]) + ":" + InertiaPage.json_string_array(keys)
        g += 1
      end
      ",\"deferredProps\":{" + entries.join(",") + "}"
    end

    def self.json_string_array(values)
      parts = []
      i = 0
      while i < values.length
        parts << JsonBuilder.encode_value(values[i])
        i += 1
      end
      "[" + parts.join(",") + "]"
    end
  end

  class Base
    def inertia_page
      page = @inertia_page_object
      return page unless page.nil?
      fresh = ActionController::InertiaPage.new
      @inertia_page_object = fresh
      fresh
    end

    def inertia_request?
      req = request
      return false if req.nil?
      req.inertia?
    end

    def inertia_header(name)
      req = request
      return "" if req.nil?
      req.env.fetch(name, "").to_s
    end

    def inertia_begin(component, version, encrypt_history, always_include_errors_hash)
      page = inertia_page
      page.start_page(component, version, encrypt_history,
                 inertia_header("HTTP_X_INERTIA_PARTIAL_COMPONENT"),
                 inertia_header("HTTP_X_INERTIA_PARTIAL_DATA"),
                 inertia_header("HTTP_X_INERTIA_PARTIAL_EXCEPT"))
      return nil if inertia_stale_version?(version)
      stored = session["inertia_errors"]
      if !stored.nil?
        session.delete("inertia_errors")
        page.mark_shared("errors")
        page.prop_json("errors", stored.to_s)
      elsif always_include_errors_hash
        page.mark_shared("errors")
        page.prop_json("errors", "{}")
      end
      # Same once-only session slot as errors. A stale-version 409 returns
      # above, so the flags stay for the full-page reload that follows.
      # The session stores each value's `to_s`, so the flag is `"true"`.
      if session["inertia_clear_history"] == "true"
        session.delete("inertia_clear_history")
        page.mark_clear_history
      end
      if session["inertia_preserve_fragment"] == "true"
        session.delete("inertia_preserve_fragment")
        page.mark_preserve_fragment
      end
      nil
    end

    def inertia_stale_version?(version)
      req = request
      return false if req.nil?
      req.get? && req.inertia? && inertia_header("HTTP_X_INERTIA_VERSION") != version
    end

    # JSON text rather than the Hash: the session cookie stores each
    # value's `to_s`, so a Hash would come back as its `inspect` String.
    def inertia_errors_json(json)
      session["inertia_errors"] = json
      nil
    end

    # A false or nil option stores nothing, so it does not delete a flag
    # an earlier redirect already stored.
    def inertia_clear_history(flag)
      session["inertia_clear_history"] = "true" if flag
      nil
    end

    def inertia_preserve_fragment(flag)
      session["inertia_preserve_fragment"] = "true" if flag
      nil
    end

    # An Inertia visit cannot follow an external redirect. The protocol
    # answer is 409 with X-Inertia-Location. A non-Inertia request is a
    # normal 302. `redirect_to` refuses another host, and this method
    # exists to send one, so the location is set directly.
    #
    # One sanitized value for both answers. HeaderStore and the CGI writer
    # drop a value that still contains CR or LF, which would make the 409
    # unreadable to the client. `head` does not take `location:`.
    def inertia_location(url)
      target = ActionController.sanitize_location(url)
      if inertia_request?
        headers["X-Inertia-Location"] = target
        head(:conflict)
      else
        head(:found)
        @location = target
      end
      nil
    end

    def inertia_url
      req = request
      return "/" if req.nil?
      req.original_fullpath
    end

    def render_inertia_json(status: :ok)
      page = inertia_page
      headers["Vary"] = "X-Inertia"
      req = request
      if !req.nil? && inertia_stale_version?(page.version)
        headers["X-Inertia-Location"] = req.original_url
        head(:conflict)
        return nil
      end
      headers["X-Inertia"] = "true"
      render(page.page_json(inertia_url, flash[:notice], flash[:alert]),
             status: status, content_type: "application/json")
      nil
    end

    def inertia_redirect_see_other
      req = request
      return nil if req.nil?
      loc = location
      return nil if loc.nil?
      if status == 302 && req.inertia? && ["PUT", "PATCH", "DELETE"].include?(req.request_method)
        redirect_to(loc, status: :see_other)
      end
      nil
    end

    def merge_inertia_props(hash)
      raise TypeError, "inertia props must be a Hash" unless hash.is_a?(Hash)
      page = inertia_page
      hash.keys.each do |key|
        apply_inertia_prop(page, key.to_s, hash[key])
      end
      nil
    end

    def apply_inertia_prop(page, name, raw)
      if raw.is_a?(InertiaRails::Prop)
        kind = raw.kind
        if kind == "defer"
          if page.deferred?(name, raw.group)
            page.prop_json(name, raw.call(self))
          end
        elsif kind == "optional"
          if page.optional?(name)
            page.prop_json(name, raw.call(self))
          end
        elsif kind == "always"
          page.prop_json(name, raw.call(self))
        end
      elsif raw.is_a?(Proc)
        if page.eager?(name)
          page.prop_json(name, inertia_exec(raw))
        end
      elsif page.eager?(name)
        # Same encoder a literal prop is lowered to. It walks a Hash or
        # Array and calls `as_json` on a record. The method lives in the
        # JSON runtime, so this file does not define it.
        page.prop_json(name, ActionController::JsonRender.encode(raw))
      end
      nil
    end

    # A computed prop proc is built in a controller method, so its
    # `self` is already that controller (`-> { @stamp }`). `call` runs
    # it and encodes the value. `instance_exec` would rebind a proc
    # that was built on some other object; Spinel has no compile path
    # for `instance_exec` of a proc that is not a literal at the call,
    # and this method is the one body every target runs.
    def inertia_exec(work)
      ActionController::JsonRender.encode(work.call)
    end

    def inertia_root_html(use_script_element)
      headers["Vary"] = "X-Inertia"
      json = JsonBuilder.escape_html_entities(inertia_page.page_json(inertia_url, flash[:notice], flash[:alert]))
      if use_script_element
        "<script data-page=\"app\" type=\"application/json\">" + json + "</script>\n<div id=\"app\"></div>"
      else
        "<div id=\"app\" data-page=\"" + ActionView::ViewHelpers.html_escape(json) + "\"></div>"
      end
    end
  end
end

# The objects a computed props hash can hold. A literal hash never
# builds these: the lowerer sees `-> { }`, `InertiaRails.optional`,
# `.defer`, and `.always` and emits the gate itself. A helper that
# returns the same wrappers builds them here, and `merge_inertia_props`
# reads `kind`.
module InertiaRails
  class Prop
    def initialize(kind, group)
      @kind = kind
      @group = group
      @block = nil
    end

    def kind
      @kind
    end

    def group
      @group
    end

    def store(work)
      @block = work
      nil
    end

    def call(controller)
      controller.inertia_exec(@block)
    end
  end

  def self.optional(&block)
    prop = Prop.new("optional", "")
    prop.store(block)
    prop
  end

  def self.defer(group: "default", &block)
    prop = Prop.new("defer", group)
    prop.store(block)
    prop
  end

  def self.always(&block)
    prop = Prop.new("always", "")
    prop.store(block)
    prop
  end
end
