# frozen_string_literal: true
# Rails' `ActionDispatch::DefaultHeaders` for the values
# `load_defaults` 7.0 and later write into
# `config.action_dispatch.default_headers`. An app that replaces that
# hash is not read yet: these five are the framework default, and an
# action that sets one of the names keeps its own value.
#
# RUBY-FAMILY ONLY, like `conditional_get.rb` beside this directory.
# Called once per request, after the action and
# `commit_conditional_get!`, by both dispatchers (the CRuby overlay's
# `main.rb` and the spinel scaffold's `main.rb`), before they copy the
# header store to the wire.
module ActionController
  class Base
    def commit_default_headers!
      write_default_header("X-Frame-Options", "SAMEORIGIN")
      write_default_header("X-XSS-Protection", "0")
      write_default_header("X-Content-Type-Options", "nosniff")
      write_default_header("X-Permitted-Cross-Domain-Policies", "none")
      write_default_header("Referrer-Policy", "strict-origin-when-cross-origin")
      nil
    end

    def write_default_header(key, value)
      current = @headers[key]
      if current.nil?
        @headers[key] = value
      end
      nil
    end
  end
end
