# View helpers — module functions invoked from Views::* render methods.
#
# Surface tracks what real-blog actually uses (cf. fixtures/real-blog/
# app/views/**/*.html.erb): link_to, button_to, dom_id, the content_for
# slot store, turbo_stream_from, truncate, pluralize (delegated to
# Inflector). FormBuilder is a small class — enough for label /
# text_field / text_area / submit, which is all real-blog's _form.html.erb
# uses.
#
# Polymorphic dispatch (e.g., link_to "Edit", @article → article_path)
# is the lowerer's job, not the runtime's. Call sites pass explicit
# paths — `link_to "Edit", RouteHelpers.article_path(article.id)`.
# This keeps the runtime small and free of class-name-keyed dispatch.
module ViewHelpers
  module_function

  # ── slot store (content_for / yield) ─────────────────────────────
  #
  # Module-level state. In CRuby this is a single shared hash; in a
  # multi-request server a real implementation would scope this per
  # request. For the spinel-blog specimen (single-threaded by spinel
  # constraint anyway), module state is fine.
  @slots = {}

  def reset_slots!
    @slots = {}
  end

  def content_for_set(slot, value)
    @slots[slot] = value
    nil
  end

  def content_for_get(slot)
    @slots[slot]
  end

  def get_slot(slot)
    @slots[slot] || ""
  end

  def get_yield
    @slots[:__body__] || ""
  end

  def set_yield(content)
    @slots[:__body__] = content
    nil
  end

  # ── escaping / formatting ────────────────────────────────────────

  # Hand-rolled to drop the `cgi` stdlib dependency (which spinel
  # doesn't ship). Matches CGI.escapeHTML semantics: replaces `&`,
  # `<`, `>`, `"`, and `'`. The `'` mapping uses `&#39;` (numeric)
  # rather than `&apos;` (named) — same convention as CGI.escapeHTML
  # in CRuby, so test assertions written against the prior behavior
  # keep passing.
  HTML_ESCAPES = {
    "&" => "&amp;",
    "<" => "&lt;",
    ">" => "&gt;",
    '"' => "&quot;",
    "'" => "&#39;",
  }.freeze

  HTML_ESCAPE_PATTERN = /[&<>"']/.freeze

  def html_escape(s)
    return "" if s.nil?
    s.to_s.gsub(HTML_ESCAPE_PATTERN, HTML_ESCAPES)
  end

  def truncate(s, length: 30, omission: "...")
    return "" if s.nil?
    str = s.to_s
    return str if str.length <= length
    cutoff = length - omission.length
    cutoff = 0 if cutoff < 0
    "#{str[0, cutoff]}#{omission}"
  end

  # ── DOM helpers ──────────────────────────────────────────────────

  def dom_id(prefix, id_or_suffix = nil)
    if id_or_suffix.nil?
      # `dom_id(article)` — pass a record
      "#{record_dom_prefix(prefix)}_#{prefix.id}"
    elsif id_or_suffix.is_a?(Symbol) || id_or_suffix.is_a?(String)
      # `dom_id(article, :comments_count)` — record + suffix
      "#{record_dom_prefix(prefix)}_#{prefix.id}_#{id_or_suffix}"
    else
      # `dom_id("article", 42)` — explicit prefix + integer id
      "#{prefix}_#{id_or_suffix}"
    end
  end

  def record_dom_prefix(record)
    # Singularized class name; for the blog we just lowercase.
    record.class.name.downcase
  end

  # ── HTML element helpers ─────────────────────────────────────────

  def link_to(text, href, opts = {})
    attrs = render_attrs({ "href" => href }.merge(stringify_keys(opts)))
    "<a#{attrs}>#{html_escape(text)}</a>"
  end

  def button_to(text, href, opts = {})
    method = opts[:method]
    form_class = opts[:form_class]
    inner_opts = opts.dup
    inner_opts.delete(:method)
    inner_opts.delete(:form_class)
    form_attrs = { "action" => href, "method" => "post" }
    form_attrs["class"] = form_class if form_class
    button_attrs = render_attrs({ "type" => "submit" }.merge(stringify_keys(inner_opts)))
    method_input = if !method.nil? && method.to_s != "post"
                     %(<input type="hidden" name="_method" value="#{method}">)
                   else
                     ""
                   end
    %(<form#{render_attrs(form_attrs)}>#{method_input}<button#{button_attrs}>#{html_escape(text)}</button></form>)
  end

  # ── Asset / meta tag helpers (stubs for now) ─────────────────────
  # Full implementations require an asset manifest + importmap config.
  # The shapes here match what the layout consumes; iteration ≥3 will
  # fill them in when the Phase-1 lowerer surfaces the asset metadata.

  def csrf_meta_tags
    %(<meta name="csrf-param" content="authenticity_token"><meta name="csrf-token" content="">)
  end

  def csp_meta_tag
    %(<meta name="csp-nonce" content="">)
  end

  def stylesheet_link_tag(name, opts = {})
    href = "/assets/#{name}.css"
    attrs = render_attrs({ "rel" => "stylesheet", "href" => href }.merge(stringify_keys(opts)))
    "<link#{attrs}>"
  end

  def javascript_importmap_tags(_pins = nil, _entry = "application")
    # Stub: emits an empty importmap script. Iteration ≥3 will read
    # config/importmap.rb and emit real <script type="importmap"> +
    # <script type="module"> tags.
    %(<script type="importmap"></script>)
  end

  def turbo_stream_from(stream)
    %(<turbo-cable-stream-source channel="Turbo::StreamsChannel" signed-stream-name="#{html_escape(stream)}"></turbo-cable-stream-source>)
  end

  # ── form builder (used as `form_with` block-yielded value) ────────

  class FormBuilder
    def initialize(model, model_name, action, method)
      @model = model
      @model_name = model_name
      @action = action
      @method = method
    end

    attr_reader :model, :model_name, :action

    def label(field, opts = {})
      attrs = ViewHelpers.render_attrs({ "for" => "#{@model_name}_#{field}" }.merge(ViewHelpers.stringify_keys(opts)))
      "<label#{attrs}>#{ViewHelpers.html_escape(field.to_s.capitalize)}</label>"
    end

    def text_field(field, opts = {})
      value = @model[field]
      attrs = ViewHelpers.render_attrs(
        {
          "type" => "text",
          "name" => "#{@model_name}[#{field}]",
          "id" => "#{@model_name}_#{field}",
          "value" => value.nil? ? "" : value.to_s,
        }.merge(ViewHelpers.stringify_keys(opts))
      )
      "<input#{attrs}>"
    end

    def text_area(field, opts = {})
      value = @model[field]
      attrs = ViewHelpers.render_attrs(
        {
          "name" => "#{@model_name}[#{field}]",
          "id" => "#{@model_name}_#{field}",
        }.merge(ViewHelpers.stringify_keys(opts))
      )
      "<textarea#{attrs}>#{ViewHelpers.html_escape(value)}</textarea>"
    end

    def submit(label = nil, opts = {})
      text = label || (@method == :patch ? "Update #{@model_name.capitalize}" : "Create #{@model_name.capitalize}")
      attrs = ViewHelpers.render_attrs({ "type" => "submit", "name" => "commit", "value" => text }.merge(ViewHelpers.stringify_keys(opts)))
      "<input#{attrs}>"
    end
  end

  # `form_with(model:, model_name:, action:, method:) { |f| ... }` —
  # yields a FormBuilder whose body the block builds; wraps that body
  # in a <form> element with the right action + method.
  def form_with(model:, model_name:, action:, method: :post, opts: {})
    builder = FormBuilder.new(model, model_name, action, method)
    body = yield(builder)
    method_str = method.to_s
    method_input = if method_str != "get" && method_str != "post"
                     %(<input type="hidden" name="_method" value="#{method_str}">)
                   else
                     ""
                   end
    form_method = method_str == "get" ? "get" : "post"
    attrs = render_attrs({ "action" => action, "method" => form_method }.merge(stringify_keys(opts)))
    "<form#{attrs}>#{method_input}#{body}</form>"
  end

  # ── attribute rendering ──────────────────────────────────────────
  # Public so FormBuilder can call them; not the user-facing surface.

  def render_attrs(attrs)
    return "" if attrs.empty?
    pairs = []
    attrs.each do |k, v|
      next if v.nil?
      pairs << " #{k}=\"#{html_escape(v)}\""
    end
    pairs.join
  end

  def stringify_keys(h)
    out = {}
    h.each { |k, v| out[k.to_s] = v }
    out
  end
end
