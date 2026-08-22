# Roundhouse Crystal test-support runtime.
#
# Hand-written, shipped alongside generated code (copied in by the
# Crystal emitter as `src/test_support.cr`). Controller specs call
# into `TestClient` for HTTP dispatch (pure in-process — no real
# server) and the returned `TestResponse` for Rails-compatible
# assertions (`assert_ok`, `assert_redirected_to`, `assert_select`).
#
# Mirrors `runtime/typescript/test_support.ts` and
# `runtime/rust/test_support.rs` in intent, shape, and assertion
# semantics — substring-match on the response body, loose-but-
# reliable for the scaffold blog's HTML. A later phase can swap in
# a real HTML parser (Crystal's XML::Node) by touching only this
# file; emitted spec bodies are insulated via method contracts.

require "./http"

module Roundhouse
  # ── Dom primitive surface (the assert_select substrate) ──────────
  #
  # The HTML-query contract assert_select lowers to, shared in shape
  # with the Ruby/TS/Python/Rust/Elixir twins (cross-target contract
  # in runtime/spinel/test/test_helper.rbs). Stub: the substring
  # matcher dressed as a Dom — `select` fabricates one synthetic node
  # (the whole document) per fragment occurrence and `text` returns it
  # verbatim, so presence / minimum / content checks degrade to exactly
  # the pre-contract behavior. The upgrade path is to swap these three
  # methods for an XML::Node-backed engine — real nodes, real CSS
  # selectors — touching only this module; every assert_select call
  # site (RoundhouseTest, TestResponse) stays put. Single home for the
  # selector logic that the two test surfaces previously each copied.
  module Dom
    # Parse an HTML document. Stub: the document *is* its html string.
    def self.parse(html : String) : String
      html
    end

    # Nodes matching `selector` within `root` (a document or node).
    # Stub: one synthetic node (the root's html) per matching element.
    #
    # Anchor-then-verify, kept in step with the Ruby twin in
    # runtime/spinel/test/test_helper.rb, which carries the rationale
    # for every rule below: `fragment_for` enumerates candidate
    # positions and every part of the compound selector is re-checked
    # against the START TAG the candidate sits in.
    def self.select(root : String, selector : String) : Array(String)
      chunk = target_chunk(selector)
      base = chunk.split("[")[0]
      want_tag = selector_tag(base)
      want_id = selector_id(base)
      want_classes = selector_classes(base)
      attrs = selector_attrs(chunk)
      fragment = fragment_for(selector)
      nodes = [] of String
      return nodes if fragment.empty?
      from = 0
      while (i = root.index(fragment, from))
        from = i + fragment.size
        start = tag_start(root, i)
        stop = root.index(">", i)
        tag = root[start, (stop.nil? ? root.size : stop) - start + 1]
        ok = tag_named?(tag, want_tag)
        ok = false if !want_id.empty? && !tag.includes?(%(id="#{want_id}"))
        if ok && !want_classes.empty?
          present = tag_classes(tag)
          ok = false unless want_classes.all? { |c| present.includes?(c) }
        end
        ok = false unless attrs.all? { |a| tag.includes?(a) }
        nodes << root if ok
      end
      nodes
    end

    # Concatenated descendant text of a node. Stub: the node verbatim.
    def self.text(node : String) : String
      node
    end

    # The index of the `<` that opens the tag position `i` sits inside.
    def self.tag_start(root : String, i : Int32) : Int32
      j = i
      while j >= 0
        return j if root[j] == '<'
        j -= 1
      end
      0
    end

    # An empty want matches anything. The boundary check is why this is
    # not a bare `starts_with?`: `<hr` is a prefix of `<hrefish` too.
    def self.tag_named?(tag : String, want : String) : Bool
      return true if want.empty?
      return false unless tag.starts_with?("<" + want)
      after = tag[want.size + 1]?
      after.nil? || after == ' ' || after == '>' || after == '/' || after == '\n'
    end

    # WHOLE tokens of this start tag's `class` attribute — `.message`
    # must not hold on `class="message__body"`, and a class that is not
    # LAST in the attribute must still match.
    def self.tag_classes(tag : String) : Array(String)
      at = tag.index("class=\"")
      return [] of String if at.nil?
      rest = tag[(at + 7)..]
      close = rest.index("\"")
      value = close.nil? ? rest : rest[0, close]
      value.split(" ")
    end

    # The substring `select` scans for: tag, else id, else the first
    # class name. Just a candidate enumerator — every part is re-checked
    # above.
    def self.fragment_for(selector : String) : String
      base = target_chunk(selector).split("[")[0]
      tag = selector_tag(base)
      id = selector_id(base)
      classes = selector_classes(base)
      if !tag.empty?
        "<#{tag}"
      elsif !id.empty?
        %(id="#{id}")
      elsif !classes.empty?
        classes[0]
      else
        ""
      end
    end

    # The chunk the assertion is ABOUT — the LAST one. `assert_select "a
    # b"` names a `b` inside an `a`; this engine cannot scope, so it
    # checks the target and ignores the ancestor. Bracket-aware, because
    # an attribute predicate may hold a space.
    def self.target_chunk(selector : String) : String
      best = ""
      buf = ""
      depth = 0
      selector.each_char do |c|
        if c == '['
          depth += 1
          buf += c
        elsif c == ']'
          depth -= 1 if depth > 0
          buf += c
        elsif depth == 0 && c.whitespace?
          best = buf if element_chunk?(buf)
          buf = ""
        else
          buf += c
        end
      end
      best = buf if element_chunk?(buf)
      best
    end

    def self.element_chunk?(chunk : String) : Bool
      !chunk.empty? && chunk != ">" && chunk != "+" && chunk != "~"
    end

    # `turbo-stream[action='append']` → ['action="append"'], rendered
    # the way an emitted start tag writes it. Both quote styles in,
    # double quotes out; a bare `[connected]` keeps just the name.
    def self.selector_attrs(chunk : String) : Array(String)
      out = [] of String
      parts = chunk.split("[")
      i = 1
      while i < parts.size
        pred = parts[i].split("]")[0]
        eq = pred.index("=")
        if eq.nil?
          out << pred
        else
          name = pred[0, eq]
          value = pred[(eq + 1)..].gsub("'", "").gsub("\"", "")
          out << %(#{name}="#{value}")
        end
        i += 1
      end
      out
    end

    def self.selector_tag(base : String) : String
      without_id(base).split(".")[0]
    end

    def self.selector_id(base : String) : String
      hash = base.index("#")
      return "" if hash.nil?
      rest = base[(hash + 1)..]
      dot = rest.index(".")
      dot.nil? ? rest : rest[0, dot]
    end

    def self.selector_classes(base : String) : Array(String)
      parts = without_id(base).split(".")
      out = [] of String
      i = 1
      while i < parts.size
        out << parts[i] unless parts[i].empty?
        i += 1
      end
      out
    end

    # `hr#x.a` → `hr.a`: the id lifted out so the remainder splits on "."
    # into tag + classes with no special case.
    def self.without_id(base : String) : String
      hash = base.index("#")
      return base if hash.nil?
      head = base[0, hash]
      rest = base[(hash + 1)..]
      dot = rest.index(".")
      dot.nil? ? head : head + rest[dot..]
    end
  end

  module TestSupport
    # Pure-Crystal test client — dispatches through Router.match,
    # calls the resolved handler, wraps the response. No real HTTP,
    # no event-loop glue. Fast + leak-free across specs.
    class TestClient
      def get(path : String) : TestResponse
        dispatch("GET", path, {} of String => String)
      end

      def post(path : String, body : Hash(String, String) = {} of String => String) : TestResponse
        dispatch("POST", path, body)
      end

      def patch(path : String, body : Hash(String, String) = {} of String => String) : TestResponse
        dispatch("PATCH", path, body)
      end

      def delete(path : String) : TestResponse
        dispatch("DELETE", path, {} of String => String)
      end

      private def dispatch(method : String, path : String, body : Hash(String, String)) : TestResponse
        result = Roundhouse::Http::Router.match(method, path)
        raise "no route for #{method} #{path}" if result.nil?
        handler, path_params = result
        merged = path_params.merge(body)
        response = handler.call(Roundhouse::Http::ActionContext.new(merged))
        TestResponse.new(response)
      end
    end

    # Wrapper around `ActionResponse` exposing assertion helpers.
    # Method names mirror Rails' Minitest HTTP assertions; bodies
    # substring-match for `assert_select`-style queries.
    class TestResponse
      getter body : String
      getter status : Int32
      getter location : String

      def initialize(raw : Roundhouse::Http::ActionResponse)
        @body = raw.body
        @status = raw.status
        @location = raw.location
      end

      # `assert_response :success` — status 200 OK.
      def assert_ok : Nil
        raise "expected 200 OK, got #{@status}" unless @status == 200
      end

      # `assert_response :unprocessable_entity` — status 422.
      def assert_unprocessable : Nil
        raise "expected 422 Unprocessable Entity, got #{@status}" unless @status == 422
      end

      # `assert_response <code>`.
      def assert_status(code : Int32) : Nil
        raise "expected status #{code}, got #{@status}" unless @status == code
      end

      # `assert_redirected_to <path>` — status is 3xx and Location
      # substring-matches the expected path. Loose to tolerate
      # absolute-vs-relative URL differences.
      def assert_redirected_to(path : String) : Nil
        raise "expected a redirection, got #{@status}" unless @status >= 300 && @status < 400
        unless @location.includes?(path)
          raise "expected Location to contain #{path.inspect}, got #{@location.inspect}"
        end
      end

      # `assert_select <selector>` — the selector matches at least one
      # node (via the shared `Dom` surface above).
      def assert_select(selector : String) : Nil
        if Dom.select(Dom.parse(@body), selector).empty?
          raise "expected body to match selector #{selector.inspect}"
        end
      end

      # `assert_select <selector>, <text>` — selector check + a matched
      # node's text contains the text.
      def assert_select_text(selector : String, text : String) : Nil
        nodes = Dom.select(Dom.parse(@body), selector)
        if nodes.empty?
          raise "expected body to match selector #{selector.inspect}"
        end
        unless nodes.any? { |n| Dom.text(n).includes?(text) }
          raise "expected text #{text.inspect} under selector #{selector.inspect}"
        end
      end

      # `assert_select <selector>, minimum: N` — at least `n` matched
      # nodes.
      def assert_select_min(selector : String, n : Int32) : Nil
        count = Dom.select(Dom.parse(@body), selector).size
        if count < n
          raise "expected at least #{n} matches for selector #{selector.inspect}, got #{count}"
        end
      end
    end
  end
end
