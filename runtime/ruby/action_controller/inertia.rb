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

    def shared_eager?(key)
      @shared_keys << key unless @shared_keys.include?(key)
      eager?(key)
    end

    def shared_optional?(key)
      @shared_keys << key unless @shared_keys.include?(key)
      optional?(key)
    end

    def shared_deferred?(key, group)
      @shared_keys << key unless @shared_keys.include?(key)
      deferred?(key, group)
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
      out = out + ",\"clearHistory\":false"
      out = out + flash_json(notice, alert)
      out + shared_json + deferred_json + "}"
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
      stored = session["inertia_errors"]
      if !stored.nil?
        session.delete("inertia_errors")
        page.shared_eager?("errors")
        page.prop_json("errors", stored.to_s)
      elsif always_include_errors_hash
        page.shared_eager?("errors")
        page.prop_json("errors", "{}")
      end
      nil
    end

    # JSON text rather than the Hash: the session cookie stores each
    # value's `to_s`, so a Hash would come back as its `inspect` String.
    def inertia_errors_json(json)
      session["inertia_errors"] = json
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
      if !req.nil? && req.get? && inertia_header("HTTP_X_INERTIA_VERSION") != page.version
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
