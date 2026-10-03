# Rails' HTTP Token and Basic authentication helpers
# (`ActionController::HttpAuthentication::Token::ControllerMethods` and
# `::Basic::ControllerMethods`), which actionpack mixes into every
# controller: `authenticate_with_http_token`,
# `authenticate_or_request_with_http_token`, `authenticate_with_http_basic`,
# `authenticate_or_request_with_http_basic` and the two `request_http_*`
# challenges they fall back to.
#
# The `authenticate_with_*` forms yield what the client sent and answer
# the block's value (nil when the request carries no credentials of that
# scheme, and then the block does not run). The `or_request` forms answer
# 401 with the scheme's `WWW-Authenticate` challenge when the block
# answers nil or false, which marks the response performed, so the
# before_action preamble halts the chain.
#
# Parsing follows actionpack's:
#
# * Token: `Authorization: Token <token>` or `Bearer <token>`, optionally
#   followed by `key=value` pairs separated by `,`, `;` or a tab. The
#   first pair may be spelled `token="<token>"`; surrounding quotes are
#   stripped from every value. The options Hash is String-keyed (Rails
#   answers it with indifferent access); a pair with no value maps to "",
#   where Rails maps it to nil, so the Hash stays one concrete type.
# * Basic: `Authorization: Basic base64(user:password)`, the scheme
#   compared case-insensitively, the password split at the FIRST colon.
#
# Ruby family only, like `request_forgery_protection.rb` and for its
# reasons: it reads the parked request, which only the ruby family
# carries, and a method on the shared `runtime/ruby` Base would be
# transpiled to every target. Required from BOTH ruby-family boots, after
# the controller runtime it reopens. The `Authorization` header reaches
# `request.env` through the Rack env on CRuby and through the env copy in
# the spinel dispatcher (`HTTP_AUTHORIZATION`).
module ActionController
  class Base
    def authenticate_with_http_token
      token = HttpAuthentication.token_from(HttpAuthentication.authorization(self))
      return nil if token.empty?
      yield(token, HttpAuthentication.token_options(HttpAuthentication.authorization(self)))
    end

    # The block is yielded to directly rather than forwarded to
    # `authenticate_with_http_token`: an explicit `&block` parameter is
    # a Proc value, which the AOT path types far less precisely than a
    # `yield` it can see the caller's block for.
    def authenticate_or_request_with_http_token(realm = "Application", message = nil)
      header = HttpAuthentication.authorization(self)
      token = HttpAuthentication.token_from(header)
      result = token.empty? ? nil : yield(token, HttpAuthentication.token_options(header))
      request_http_token_authentication(realm, message) unless result
      result
    end

    def request_http_token_authentication(realm = "Application", message = nil)
      headers["WWW-Authenticate"] = "Token realm=\"#{realm.delete("\"")}\""
      render(message.nil? ? "HTTP Token: Access denied.\n" : message,
        status: :unauthorized, content_type: "text/plain; charset=utf-8")
    end

    def authenticate_with_http_basic
      header = HttpAuthentication.authorization(self)
      return nil unless HttpAuthentication.basic?(header)
      pair = HttpAuthentication.basic_credentials(header)
      yield(pair[0], pair[1])
    end

    def authenticate_or_request_with_http_basic(realm = "Application", message = nil)
      header = HttpAuthentication.authorization(self)
      result = nil
      if HttpAuthentication.basic?(header)
        pair = HttpAuthentication.basic_credentials(header)
        result = yield(pair[0], pair[1])
      end
      request_http_basic_authentication(realm, message) unless result
      result
    end

    def request_http_basic_authentication(realm = "Application", message = nil)
      headers["WWW-Authenticate"] = "Basic realm=\"#{realm.delete("\"")}\""
      render(message.nil? ? "HTTP Basic: Access denied.\n" : message,
        status: :unauthorized, content_type: "text/plain; charset=utf-8")
    end
  end

  module HttpAuthentication
    # `request.authorization`, or "" outside a dispatch (a controller
    # built by hand in a unit test has no request).
    def self.authorization(controller)
      req = controller.request
      return "" if req.nil?
      req.env.fetch("HTTP_AUTHORIZATION", "").to_s
    end

    # The `key=value` pairs after the scheme, the first one keyed
    # `token` whether or not the client wrote `token=`; empty when the
    # scheme is neither Token nor Bearer.
    def self.token_pairs(header)
      pairs = []
      rest = nil
      if header.start_with?("Token ")
        rest = header[6, header.length].to_s
      elsif header.start_with?("Bearer ")
        rest = header[7, header.length].to_s
      end
      return pairs if rest.nil?
      rest.tr(";\t", ",,").split(",").each do |raw|
        param = raw.strip
        next if param.empty?
        param = "token=#{param}" if pairs.empty? && !param.start_with?("token=")
        at = param.index("=")
        key = at.nil? ? param : param[0, at].to_s
        value = at.nil? ? "" : param[at + 1, param.length].to_s
        pairs << [key, unquote(value)]
      end
      pairs
    end

    def self.token_from(header)
      pairs = token_pairs(header)
      return "" if pairs.empty?
      pairs[0][1].to_s
    end

    # Every pair after the token, as Rails' `options`.
    def self.token_options(header)
      options = {}
      token_pairs(header).each_with_index do |pair, i|
        options[pair[0].to_s] = pair[1].to_s if i > 0
      end
      options
    end

    # Rails' `gsub(/^"|"$/, "")`: one quote off each end, independently.
    def self.unquote(value)
      v = value
      v = v[1, v.length].to_s if v.start_with?("\"")
      v = v[0, v.length - 1].to_s if v.end_with?("\"")
      v
    end

    def self.basic?(header)
      header.split(" ", 2)[0].to_s.downcase == "basic"
    end

    # `[user, password]`, both "" when the payload is missing; a
    # password may itself hold colons.
    def self.basic_credentials(header)
      decoded = Base64.decode64(header.split(" ", 2)[1].to_s)
      at = decoded.index(":")
      return [decoded, ""] if at.nil?
      [decoded[0, at].to_s, decoded[at + 1, decoded.length].to_s]
    end
  end
end
