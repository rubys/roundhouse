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

      # `assert_select <selector>` — body contains a match for the
      # selector. Substring-matches on the opening tag or
      # `id=`/`class=` fragment. Covers the scaffold blog shapes:
      #   "h1"        → contains "<h1"
      #   "#articles" → contains `id="articles"`
      #   ".p-4"      → contains `p-4"`
      #   "form"      → contains "<form"
      def assert_select(selector : String) : Nil
        fragment = TestSupport.selector_fragment(selector)
        unless @body.includes?(fragment)
          raise "expected body to match selector #{selector.inspect} (looked for #{fragment.inspect})"
        end
      end

      # `assert_select <selector>, <text>` — selector check + body
      # also contains the text.
      def assert_select_text(selector : String, text : String) : Nil
        assert_select(selector)
        unless @body.includes?(text)
          raise "expected body to contain text #{text.inspect} under selector #{selector.inspect}"
        end
      end

      # `assert_select <selector>, minimum: N` — at least `n`
      # occurrences of the selector fragment.
      def assert_select_min(selector : String, n : Int32) : Nil
        fragment = TestSupport.selector_fragment(selector)
        count = 0
        from = 0
        while (i = @body.index(fragment, from))
          count += 1
          from = i + fragment.size
        end
        if count < n
          raise "expected at least #{n} matches for selector #{selector.inspect}, got #{count}"
        end
      end
    end

    # Loose selector → substring fragment. Same rules as the Rust
    # and TS twins.
    def self.selector_fragment(selector : String) : String
      first = selector.split.first? || ""
      case first[0]?
      when '#'
        %(id="#{first[1..]}")
      when '.'
        %(#{first[1..]}")
      else
        "<#{first}"
      end
    end
  end
end
