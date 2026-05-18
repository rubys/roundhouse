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
module ActionView
  module ViewHelpers
    # ── slot store (content_for / yield) ─────────────────────────────
    #
    # Module-level state. In CRuby this is a single shared hash; in a
    # multi-request server a real implementation would scope this per
    # request. For the spinel-blog specimen (single-threaded by spinel
    # constraint anyway), module state is fine.
    @slots = {}
  
    def self.reset_slots!
      @slots = {}
    end
  
    def self.content_for_set(slot, value)
      @slots[slot] = value
      nil
    end
  
    def self.content_for_get(slot)
      # `fetch(slot, nil)` (which the Crystal emit lowers to
      # `@@slots[slot]?`) — Ruby Hash#[] returns nil for missing
      # keys, but Crystal's strict Hash#[] raises KeyError. Same
      # cross-target nil-safe pattern used elsewhere.
      @slots.fetch(slot, nil)
    end

    def self.get_slot(slot)
      @slots[slot] || ""
    end

    def self.get_yield
      @slots[:__body__] || ""
    end
  
    def self.set_yield(content)
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
  
    # Monomorphic: param typed String. Callers handle nil/non-String
    # coercion explicitly. Contracts the dispatch surface so every
    # backend compiler sees a stable input shape.
    def self.html_escape(s)
      s.gsub(HTML_ESCAPE_PATTERN, HTML_ESCAPES)
    end

    def self.truncate(s, length: 30, omission: "...")
      return s if s.length <= length
      cutoff = length - omission.length
      cutoff = 0 if cutoff < 0
      "#{s[0, cutoff]}#{omission}"
    end
  
    # ── DOM helpers ──────────────────────────────────────────────────

    # Monomorphic: param typed `ActiveRecord::Base`. Was previously a
    # String|Base union dispatched via `prefix.is_a?(String)`; real-blog
    # only ever calls it with a record, so the String branch is
    # contracted away. Callers needing the explicit-prefix form spell it
    # out directly: `"article_#{id}"`.
    #
    # The per-model class name (`"article"`, `"comment"`) reaches dom_id
    # via `record.dom_prefix` — an instance method synthesized per-model
    # by the lowerer. Replaces the previous `record.class.name.downcase`
    # runtime introspection; the synthesizer knows the model name at
    # transpile time so no compiler has to chase the reflection chain.
    def self.dom_id(record, suffix = nil)
      # Explicit parens on `record.dom_prefix()` — TS emit collapses
      # parens-less zero-arg sends to attr-reader-shaped property access
      # (`record.dom_prefix` returns the function reference, not the
      # string). The synthesized method's AccessorKind doesn't yet
      # thread to the Send-emit; parens are the cheap forcing function.
      if suffix.nil?
        # `dom_id(article)` -> "article_3"
        "#{record.dom_prefix()}_#{record.id}"
      else
        # `dom_id(article, :comments_count)` -> "comments_count_article_3"
        # (Rails order: suffix BEFORE model name in the resulting id.)
        "#{suffix}_#{record.dom_prefix()}_#{record.id}"
      end
    end
  
    # ── HTML element helpers ─────────────────────────────────────────
  
    def self.link_to(text, href, opts = {})
      # `opts.to_h` is a no-op on Ruby Hash and a NamedTuple→Hash
      # conversion under Crystal. Call sites that use kwargs syntax
      # (`link_to "Show", "/x", class: "btn"`) lift to NamedTuple
      # in Crystal, but the receiver builds a Hash via `merge` —
      # NamedTuple#merge can't take a Hash, and Hash#merge can't
      # take a NamedTuple. Same `.to_h` pattern applies to every
      # helper below that merges user opts into a default Hash.
      attrs = render_attrs({ href: href }.merge(opts.to_h))
      "<a#{attrs}>#{html_escape(text)}</a>"
    end
  
    def self.button_to(text, href, opts = {})
      # Use `.fetch(k, nil)` instead of bare `opts[:k]`: Ruby's Hash#[]
      # returns nil for missing keys, but Crystal's strict Hash#[]
      # raises KeyError. fetch-with-default produces nil-on-missing in
      # both. Same Ruby semantics, target-portable shape.
      method = opts.fetch(:method, nil)
      form_class = opts.fetch(:form_class, nil)
      # `opts.to_h.dup` rather than `opts.dup`: kwargs call sites
      # (`button_to "X", "/y", method: :delete`) lift to NamedTuple
      # in Crystal; NamedTuple has `dup` (no-op since immutable) but
      # no `delete`. Convert to Hash first.
      inner_opts = opts.to_h.dup
      inner_opts.delete(:method)
      inner_opts.delete(:form_class)
      # `.to_h` makes form_attrs a Hash (Ruby no-op; Crystal converts
       # the NamedTuple literal). Subsequent `[:class] = ...` mutation
      # would fail on Crystal's immutable NamedTuple.
      form_attrs = { action: href, method: "post" }.to_h
      # Rails' `button_to` defaults the form class to `button_to` when
      # the caller doesn't pass one — match that so the cross-target
      # compare sees the same `class` attribute set.
      # `.to_s` narrows the `opts[k]` union (Hash/Symbol/String/...)
      # to String for strict-typed targets. Ruby `String#to_s` is a no-op;
      # `||` short-circuits before `.to_s` runs on a real String value.
      form_attrs[:class] = (form_class || "button_to").to_s
      button_attrs = render_attrs({ type: "submit" }.merge(inner_opts))
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
    def self.csrf_meta_tags
      %(<meta name="csrf-param" content="authenticity_token" />\n<meta name="csrf-token" content="" />)
    end
  
    # Empty in dev mode without a CSP nonce configured, mirroring Rails'
    # behavior and the other targets' runtimes (Rust / Python / Elixir
    # all return "" here). Production deployment with CSP wired would
    # plug a real nonce in.
    def self.csp_meta_tag
      ""
    end
  
    def self.stylesheet_link_tag(name, opts = {})
      href = "/assets/#{name}.css"
      attrs = render_attrs({ rel: "stylesheet", href: href }.merge(opts.to_h))
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
    def self.javascript_importmap_tags(pins = nil, entry = "application")
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
    def self.turbo_stream_from(stream)
      encoded = Base64.strict_encode64(JSON.generate(stream))
      %(<turbo-cable-stream-source channel="Turbo::StreamsChannel" signed-stream-name="#{encoded}--unsigned"></turbo-cable-stream-source>)
    end
  
    # ── form_with primitives ─────────────────────────────────────────
    # Small typed-scalar helpers callable from macro-inlined form_with
    # expansions. Centralizes semantics that may evolve (real signed
    # tokens, CSP nonces) so they live in one runtime file rather than
    # being baked into every form_with call site at lower time.

    # Rails injects an authenticity_token hidden input as the first
    # child of every form_with-rendered form (after the optional
    # _method override). The compare harness blanks the value via an
    # existing AttributeRule, so emitting an empty value is sufficient
    # for parity. When real signing arrives, this is the one place to
    # hook the signer.
    def self.csrf_token_hidden_input
      %(<input type="hidden" name="authenticity_token" value="">)
    end

    # Rails emits `<input type="hidden" name="_method" value="patch">`
    # for forms whose semantic method is PATCH/PUT/DELETE (form's HTML
    # `method` attribute stays "post"; the server routes off _method).
    # Returns the empty string for get/post — the inline macro emits
    # this unconditionally and relies on the empty-string case being a
    # no-op concat.
    def self.method_override_input(method)
      method_str = method.to_s
      if method_str == "get" || method_str == "post"
        ""
      else
        %(<input type="hidden" name="_method" value="#{method_str}">)
      end
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
        attrs = ViewHelpers.render_attrs({ for: "#{@model_name}_#{field}" }.merge(opts.to_h))
        "<label#{attrs}>#{ViewHelpers.html_escape(field.to_s.capitalize)}</label>"
      end
  
      def text_field(field, opts = {})
        value = @model[field]
        # `.to_h` makes base a Hash (Ruby no-op; Crystal converts).
        # Subsequent `base[:value] = ...` mutation requires Hash;
        # NamedTuple is immutable.
        base = {
          type: "text",
          name: "#{@model_name}[#{field}]",
          id: "#{@model_name}_#{field}",
        }.to_h
        # Rails omits the `value` attribute entirely when the field is
        # nil/empty; only render it when there's a non-empty value.
        base[:value] = value.to_s unless value.nil? || value.to_s.empty?
        attrs = ViewHelpers.render_attrs(base.merge(opts.to_h))
        "<input#{attrs}>"
      end
  
      def text_area(field, opts = {})
        value = @model[field]
        attrs = ViewHelpers.render_attrs(
          {
            name: "#{@model_name}[#{field}]",
            id: "#{@model_name}_#{field}",
          }.merge(opts.to_h)
        )
        # `@model[field]` is untyped per Base#[]; coerce to String at
        # the boundary so html_escape sees its String-typed contract.
        # Explicit nil-check rather than `value.to_s` — JS `String(null)`
        # returns the literal `"null"` (4 chars) whereas Ruby's
        # `nil.to_s` returns `""`. The Ruby-shape-on-every-target
        # invariant demands the explicit guard at the source.
        body_str = value.nil? ? "" : ViewHelpers.html_escape(value.to_s)
        "<textarea#{attrs}>#{body_str}</textarea>"
      end
  
      def submit(label = nil, opts = {})
        text = label || (@method == :patch ? "Update #{@model_name.capitalize}" : "Create #{@model_name.capitalize}")
        # Rails appends `data-disable-with="<value>"` to submit inputs so
        # turbo prevents a double-submit while the request is in flight.
        # Quoted-Symbol form preserves the `data-disable-with` hyphenation
        # — Symbol-keyed bases keep merge type-compatible with Symbol-
        # keyed `opts` while letting render_attrs see hyphenated names
        # through `k.to_s`.
        attrs = ViewHelpers.render_attrs(
          {
            type: "submit",
            name: "commit",
            value: text,
            :"data-disable-with" => text,
          }.merge(opts.to_h)
        )
        "<input#{attrs}>"
      end
    end
  
    # `form_with(model:, model_name:, action:, method:) { |f| ... }` —
    # yields a FormBuilder whose body the block builds; wraps that body
    # in a <form> element with the right action + method.
    def self.form_with(model:, model_name:, action:, method: :post, opts: {})
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
        { action: action, :"accept-charset" => "UTF-8", method: form_method }
          .merge(opts.to_h)
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
    # Render an HTML attribute hash as ` name="val"` pairs. Accepts
    # Symbol-or-String keys uniformly via `k.to_s` at the iteration
    # boundary — callers pass Symbol-keyed `opts` straight through
    # without an upfront stringify pass. Nested hashes (`data: {
    # turbo_confirm: ... }`) render as `data-turbo-confirm="…"`;
    # underscores in the inner key map to hyphens to match Rails'
    # tag-helper convention.
    def self.render_attrs(attrs)
      return "" if attrs.empty?
      pairs = []
      attrs.each do |k, v|
        next if v.nil?
        name = k.to_s
        if v.is_a?(Hash)
          v.each do |inner_k, inner_v|
            next if inner_v.nil?
            inner_name = inner_k.to_s.tr("_", "-")
            # Coerce untyped Hash values to String before html_escape;
            # html_escape's contract is `(String) -> String` and the
            # untyped values flowing through Hash[String, untyped]
            # need explicit stringification.
            pairs << " #{name}-#{inner_name}=\"#{html_escape(inner_v.to_s)}\""
          end
        else
          pairs << " #{name}=\"#{html_escape(v.to_s)}\""
        end
      end
      pairs.join
    end
  end
end
