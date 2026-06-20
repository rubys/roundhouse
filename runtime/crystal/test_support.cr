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
    # Stub: one synthetic node (the root's html) per substring-fragment
    # occurrence.
    def self.select(root : String, selector : String) : Array(String)
      fragment = fragment_for(selector)
      nodes = [] of String
      from = 0
      while (i = root.index(fragment, from))
        nodes << root
        from = i + fragment.size
      end
      nodes
    end

    # Concatenated descendant text of a node. Stub: the node verbatim.
    def self.text(node : String) : String
      node
    end

    # Loose selector → substring fragment (the stub's rule, replaced by
    # a real CSS engine on upgrade): "#id" → id="id", ".cls" → cls",
    # "tag" → <tag. Compound selectors take the first chunk.
    def self.fragment_for(selector : String) : String
      first = selector.split(/\s+/).first? || ""
      case first
      when .starts_with?("#") then %(id="#{first[1..]}")
      when .starts_with?(".") then %(#{first[1..]}")
      else                         "<#{first}"
      end
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
