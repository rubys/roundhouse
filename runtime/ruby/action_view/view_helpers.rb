# View helpers — module functions invoked from Views::* render methods.
#
# Surface tracks what real-blog actually uses (cf. fixtures/real-blog/
# app/views/**/*.html.erb): link_to, button_to, dom_id, the content_for
# slot store, turbo_stream_from, truncate, pluralize (delegated to
# Inflector), plus four form_with macro-inline primitives
# (csrf_token_hidden_input, method_override_input, optional_value_attr,
# escape_or_empty). form_with itself + the FormBuilder class are
# retired: the lowerer macro-expands `<%= form_with ... do |form| ... %>`
# and `form.label`/`form.text_field`/`form.text_area`/`form.submit`
# at lower time into direct HTML accumulation, so no runtime
# FormBuilder dispatch survives in lowered output.
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
    #
    # `ActionView::Slots` (see action_view/slots.rb) is the value
    # object the lowerer migration will thread per-request; the
    # module-level store here remains the call surface during
    # migration so existing transpiled call sites in
    # TS/Crystal/Rust/Go/Spinel keep working.
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

    # Rails' `content_for?(:slot)` — has the slot been populated?
    # (Layout conditionals: `<% if content_for? :subnav %>`.)
    # Composed from `get_slot` (missing slot → ""), NOT a local
    # `fetch(slot, nil)` + nil-check: the nilable-local shape broke two
    # strict transpiles (Swift didn't narrow `String?` across `||`;
    # go2 mangled the slots read into an undefined identifier), while
    # get_slot's `|| ""` idiom is already proven on every target.
    def self.content_for?(slot)
      !get_slot(slot).empty?
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

    # `link_to raw("Page 2 &gt;&gt;"), url` — the html_safe-text form.
    # There's no safe-buffer type in the transpiled runtime, so the
    # Ruby emit path rewrites `link_to(raw(x), ...)` to this variant,
    # which skips the label escape Rails would skip for a safe buffer.
    def self.link_to_raw(text, href, opts = {})
      attrs = render_attrs({ href: href }.merge(opts.to_h))
      "<a#{attrs}>#{text}</a>"
    end

    # Rails' `raw` marks a string html_safe; with no safe-buffer type
    # the value passes through (escape-exemption is decided at the
    # call-site rewrite layer — see `link_to_raw` and the emit-path
    # html_escape unwrap). `to_s` matches Rails: `raw(nil)` renders "".
    def self.raw(value)
      value.to_s
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
      # button; value via form_authenticity_token like the other csrf
      # emitters. Keeps the element in the DOM tree at the same
      # position Rails puts it.
      auth_token_input = %(<input type="hidden" name="authenticity_token" value="#{form_authenticity_token}">)
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
      %(<meta name="csrf-param" content="authenticity_token" />\n<meta name="csrf-token" content="#{form_authenticity_token}" />)
    end

    # The per-request CSRF token every csrf-emitting helper
    # (csrf_meta_tags / csrf_token_hidden_input / button_to) reads.
    # Empty in the shared runtime — targets without session-backed
    # token generation render the same empty value they always have,
    # and the compare harness blanks the attribute either way. The
    # CRuby overlay overrides this with a session-backed lazy
    # generator (runtime/action_controller_session.rb), which is how
    # real tokens reach lobsters' login form without the shared
    # runtime needing SecureRandom or a session on every target.
    def self.form_authenticity_token
      ""
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

    # `<script src>` include for a JS source. The source resolves through
    # the same undigested `/assets/<name>.js` convention as
    # `javascript_path` (see the `image_path` note on why no digests);
    # absolute paths and URLs pass verbatim.
    def self.javascript_include_tag(source, opts = {})
      name = source.include?(".") ? source : "#{source}.js"
      src = name.start_with?("/") || name.include?("://") ? name : "/assets/#{name}"
      attrs = render_attrs({ src: src }.merge(opts.to_h))
      "<script#{attrs}></script>"
    end

    # Asset path for an image source. `skip_pipeline: true` (and any
    # already-absolute or protocol-relative source) returns the source
    # verbatim — Rails bypasses the pipeline for those, and the lobsters
    # benchmark (production, `config.assets.compile = false`, no manifest)
    # has no digests to apply anyway, so the undigested `/assets/<name>`
    # prefix below matches its Rails output. Fingerprinting belongs to an
    # app that actually precompiles assets, not here.
    def self.image_path(source, skip_pipeline: false)
      return source if skip_pipeline
      return source if source.start_with?("/")
      return source if source.include?("://")
      "/assets/#{source}"
    end

    # `image_url` — Rails' absolute-URL variant of `image_path`. With no
    # asset host configured (the benchmark shape) Rails emits the same
    # path `image_path` produces, so this mirrors it. Inlined rather
    # than delegating: forwarding `skip_pipeline:` would transpile to a
    # positional Map on strict targets (the kwarg-forwarding trap).
    def self.image_url(source, skip_pipeline: false)
      return source if skip_pipeline
      return source if source.start_with?("/")
      return source if source.include?("://")
      "/assets/#{source}"
    end

    # `path_to_javascript "application"` → the asset path for a JS source.
    # Rails appends the `.js` extension when the source carries none, then
    # prefixes the asset path (undigested here, per the `image_path` note —
    # lobsters has no manifest). Absolute paths and URLs pass verbatim.
    # `javascript_path` is the same helper under its non-`path_to_` name.
    def self.path_to_javascript(source, skip_pipeline: false)
      return source if skip_pipeline
      return source if source.start_with?("/")
      return source if source.include?("://")
      name = source.include?(".") ? source : "#{source}.js"
      "/assets/#{name}"
    end

    # Alias of `path_to_javascript`. Inlined rather than delegating, so no
    # keyword argument is forwarded — a `skip_pipeline: skip_pipeline` call
    # transpiles to a positional `Map` on strict targets (kotlin/swift),
    # which mismatches the `Boolean` parameter.
    def self.javascript_path(source, skip_pipeline: false)
      return source if skip_pipeline
      return source if source.start_with?("/")
      return source if source.include?("://")
      name = source.include?(".") ? source : "#{source}.js"
      "/assets/#{name}"
    end

    # `<img>` tag for a source path + attribute opts. The source flows
    # through `image_path` (verbatim for absolute/skip-pipeline avatars),
    # then merges the caller's attrs (srcset/class/size/alt/...).
    def self.image_tag(source, opts = {})
      attrs = render_attrs({ src: image_path(source) }.merge(opts.to_h))
      "<img#{attrs}>"
    end

    # `content_tag :span, text, title: "..."` → `<span title="...">text</span>`.
    # Content is escaped (Rails escapes unless the caller passes an
    # html_safe buffer; lowered call sites pass plain strings). Attrs
    # flow through `render_attrs` like the other tag helpers. The
    # content default is nil, NOT "" — `content` is untyped, which
    # lands as C# `object`, and C# rejects any non-null default on a
    # reference-typed parameter (CS1763); `to_s` maps nil → "".
    def self.content_tag(name, content = nil, opts = {})
      n = name.to_s
      "<#{n}#{render_attrs(opts.to_h)}>#{html_escape(content.to_s)}</#{n}>"
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
    # _method override). The value rides `form_authenticity_token` —
    # empty on targets without a token generator (the compare harness
    # blanks the attribute anyway), real on CRuby where the overlay
    # supplies a session-backed token (the lobsters benchmark scrapes
    # it off GET /login and POSTs it back).
    def self.csrf_token_hidden_input
      %(<input type="hidden" name="authenticity_token" value="#{form_authenticity_token}">)
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

    # Rails' `text_field` (and field helpers) omits the `value`
    # attribute entirely when the record's value is nil or an empty
    # string — only emits ` value="<escaped>"` when there's content.
    # Wraps the nil-or-empty check + html_escape so the macro-inline
    # form.text_field expansion calls one typed helper instead of
    # reconstructing the conditional at each call site.
    def self.optional_value_attr(value)
      if value.nil? || value.to_s.empty?
        ""
      else
        %( value="#{html_escape(value.to_s)}")
      end
    end

    # Inverse of `html_escape`'s nil-discipline: returns html_escape
    # on the value when present, empty string when nil. Used by the
    # macro-inline `form.text_area` expansion for the textarea body
    # — Rails renders an empty body when the attribute is nil rather
    # than the literal "null" / "nil". Keeping the conditional in
    # one runtime function avoids re-emitting the nil-check at every
    # call site.
    def self.escape_or_empty(value)
      if value.nil?
        ""
      else
        html_escape(value.to_s)
      end
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
