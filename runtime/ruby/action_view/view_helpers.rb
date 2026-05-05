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
      # `dom_id(article, :comments_count)` — record + suffix.
      # Rails puts the suffix BEFORE the model_name in the resulting
      # id (e.g. `comments_count_article_3`), not after — match that
      # order so cross-target compare passes.
      "#{id_or_suffix}_#{record_dom_prefix(prefix)}_#{prefix.id}"
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
    # Rails' `button_to` defaults the form class to `button_to` when
    # the caller doesn't pass one — match that so the cross-target
    # compare sees the same `class` attribute set.
    # `.to_s` narrows the `opts[k]` union (Hash/Symbol/String/...)
    # to String for strict-typed targets. Ruby `String#to_s` is a no-op;
    # `||` short-circuits before `.to_s` runs on a real String value.
    form_attrs["class"] = (form_class || "button_to").to_s
    button_attrs = render_attrs({ "type" => "submit" }.merge(stringify_keys(inner_opts)))
    method_input = if !method.nil? && method.to_s != "post"
                     %(<input type="hidden" name="_method" value="#{method}">)
                   else
                     ""
                   end
    # Rails appends a CSRF authenticity_token hidden input AFTER the
    # button. The compare harness blanks the value via an existing
    # AttributeRule, so emitting an empty value here is sufficient
    # for parity. Keeps the element in the DOM tree at the same
    # position Rails puts it.
    auth_token_input = %(<input type="hidden" name="authenticity_token" value="">)
    %(<form#{render_attrs(form_attrs)}>#{method_input}<button#{button_attrs}>#{html_escape(text)}</button>#{auth_token_input}</form>)
  end

  # ── Asset / meta tag helpers (stubs for now) ─────────────────────
  # Full implementations require an asset manifest + importmap config.
  # The shapes here match what the layout consumes; iteration ≥3 will
  # fill them in when the Phase-1 lowerer surfaces the asset metadata.

  # Two `<meta>` tags joined by `\n`, matching Rails' tag-helper output
  # shape — Rails renders them on separate lines with the second tag's
  # leading indent stripped. Compare drops both metas via ignore rule;
  # the inter-element newline survives the drop and contributes to the
  # merged whitespace text content. (Without the newline, head-content
  # diff appears against Rails for purely formatting reasons.) The
  # `authenticity_token` value is the form-field name; the token value
  # is empty here because spinel-blog doesn't sign sessions.
  def csrf_meta_tags
    %(<meta name="csrf-param" content="authenticity_token" />\n<meta name="csrf-token" content="" />)
  end

  # Empty in dev mode without a CSP nonce configured, mirroring Rails'
  # behavior and the other targets' runtimes (Rust / Python / Elixir
  # all return "" here). Production deployment with CSP wired would
  # plug a real nonce in.
  def csp_meta_tag
    ""
  end

  def stylesheet_link_tag(name, opts = {})
    href = "/assets/#{name}.css"
    attrs = render_attrs({ "rel" => "stylesheet", "href" => href }.merge(stringify_keys(opts)))
    "<link#{attrs}>"
  end

  # Emit the importmap script + per-pin modulepreload hints + a
  # module-script that imports the entry point. Mirrors Rails'
  # `javascript_importmap_tags` shape so the cross-target comparison
  # harness sees equivalent head structure.
  #
  # `pins` is the frozen array Roundhouse emits to `config/importmap.rb`
  # as `Importmap::PINS` (one `{ name:, path: }` hash per pin in the
  # source `config/importmap.rb`). When pins is nil/empty — the case
  # for the hand-written standalone specimen, before Roundhouse ingests
  # an importmap — fall back to a Turbo-only shape so the fixture stays
  # runnable on its own.
  def javascript_importmap_tags(pins = nil, entry = "application")
    # Lines joined by `\n` only (no indent) — matches Rails' helper
    # output where each preload link / bootstrap script is flush left
    # in the source. The first line lands at the layout's source-indent
    # column; subsequent lines start at column 0.
    #
    # The importmap-script's JSON is pretty-printed with 2-space indent
    # to match Rails' `:pretty` JSON output. Both sides are semantically
    # identical; the text comparison checks character-for-character.
    # Build the JSON via string concatenation (rather than `%(...)` with
    # `\n` escapes + `#{var}` interpolation): the TS transpiler renders
    # the latter as a template literal that bakes the Ruby source-line
    # indent into the output, breaking byte-for-byte parity with Rails'
    # pretty-printed importmap. Concat-form transpiles to a flat string
    # with `\n` escapes and matches Rails exactly.
    if pins.nil? || pins.empty?
      json = "{\n  \"imports\": {\n    \"@hotwired/turbo\": \"/assets/turbo.min.js\"\n  }\n}"
      return %(<script type="importmap" data-turbo-track="reload">) + json + %(</script>) +
        "\n" +
        %(<link rel="modulepreload" href="/assets/turbo.min.js">) +
        "\n" +
        %(<script type="module">import "@hotwired/turbo"</script>)
    end
    import_lines = pins.map { |p| "    \"#{p[:name]}\": \"#{p[:path]}\"" }.join(",\n")
    json = "{\n  \"imports\": {\n" + import_lines + "\n  }\n}"
    parts = []
    parts << %(<script type="importmap" data-turbo-track="reload">) + json + %(</script>)
    pins.each do |p|
      parts << %(<link rel="modulepreload" href="#{p[:path]}">)
    end
    parts << %(<script type="module">import "#{entry}"</script>)
    parts.join("\n")
  end

  # Matches Rails' `turbo_stream_from` byte-output: the channel
  # name travels base64-encoded-JSON through `signed-stream-name`
  # so the Action Cable client can decode it server-side. Rails
  # additionally HMAC-signs the value with a `--<sig>` suffix; we
  # emit `--unsigned` (matches the other targets' runtimes), and
  # the compare harness's existing ignore rule strips the suffix
  # so the unsigned base64 value matches Rails' signed value.
  def turbo_stream_from(stream)
    require "base64"
    require "json"
    encoded = Base64.strict_encode64(JSON.generate(stream))
    %(<turbo-cable-stream-source channel="Turbo::StreamsChannel" signed-stream-name="#{encoded}--unsigned"></turbo-cable-stream-source>)
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
      base = {
        "type" => "text",
        "name" => "#{@model_name}[#{field}]",
        "id" => "#{@model_name}_#{field}",
      }
      # Rails omits the `value` attribute entirely when the field is
      # nil/empty; only render it when there's a non-empty value.
      base["value"] = value.to_s unless value.nil? || value.to_s.empty?
      attrs = ViewHelpers.render_attrs(base.merge(ViewHelpers.stringify_keys(opts)))
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
      # Rails appends `data-disable-with="<value>"` to submit inputs so
      # turbo prevents a double-submit while the request is in flight.
      attrs = ViewHelpers.render_attrs(
        {
          "type" => "submit",
          "name" => "commit",
          "value" => text,
          "data-disable-with" => text,
        }.merge(ViewHelpers.stringify_keys(opts))
      )
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
    # Rails' `form_with` injects a CSRF authenticity_token hidden
    # input as the first child of the form (after `_method` for
    # PATCH/DELETE forms). The compare harness blanks the value via
    # an existing AttributeRule.
    auth_token_input = %(<input type="hidden" name="authenticity_token" value="">)
    form_method = method_str == "get" ? "get" : "post"
    # Rails' default `accept-charset="UTF-8"` lands on every
    # form_with output; mirror it so cross-target compare sees the
    # same attribute set.
    attrs = render_attrs(
      { "action" => action, "accept-charset" => "UTF-8", "method" => form_method }
        .merge(stringify_keys(opts))
    )
    "<form#{attrs}>#{method_input}#{auth_token_input}#{body}</form>"
  end

  # ── attribute rendering ──────────────────────────────────────────
  # Public so FormBuilder can call them; not the user-facing surface.

  # Render an HTML attribute list. Hash-valued attrs (`data: { turbo_confirm:
  # "..." }`, `aria: { labelledby: "..." }`) flatten with a kebab-prefixed
  # key — so `data: { turbo_confirm: "x" }` emits `data-turbo-confirm="x"`,
  # matching Rails ActionView's tag helper. Underscores in the inner key
  # become hyphens (turbo_confirm → turbo-confirm). Non-hash values render
  # as-is via html_escape.
  def render_attrs(attrs)
    return "" if attrs.empty?
    pairs = []
    attrs.each do |k, v|
      next if v.nil?
      if v.is_a?(Hash)
        v.each do |inner_k, inner_v|
          next if inner_v.nil?
          inner_name = inner_k.to_s.tr("_", "-")
          pairs << " #{k}-#{inner_name}=\"#{html_escape(inner_v)}\""
        end
      else
        pairs << " #{k}=\"#{html_escape(v)}\""
      end
    end
    pairs.join
  end

  def stringify_keys(h)
    out = {}
    h.each { |k, v| out[k.to_s] = v }
    out
  end
end
