# frozen_string_literal: true
# CRuby-only Rails::Application#routes surface.
#
# Lobsters' config/application.rb composes absolute URLs via
# `Rails.application.routes.url_helpers.root_url(host:, protocol:)`
# (Story#short_id_url and friends). The shared runtime's Application is
# deliberately empty (NameError over silent stubs); this overlay adds
# the real thing for CRuby: url_helpers resolves against the emitted
# RouteHelpers, and root_url composes protocol://host + root_path.
# Overlay, not shared runtime — the kwarg signature is exactly the
# forwarding shape strict targets refuse (see the kwarg-forwarding gap).
#
# `#routes` returns `RouteSet`, which is Rails' name for this object
# (`ActionDispatch::Routing::RouteSet`). The name must not be
# `RouteTable`: a nested `RouteTable` hides the emitted top-level
# `RouteTable` that `recognize_path` reads.
require "uri"

module ActionController
  # Raised by `recognize_path` for a verb that Rails does not accept.
  # It lives in this overlay and not in runtime/ruby/, so `check` still
  # reports app code that names it. The Rails superclass is
  # `ActionControllerError`, which the runtime does not define.
  class UnknownHttpMethod < StandardError
  end
end

module Rails
  class Application
    def routes
      RouteSet
    end

    module RouteSet
      # `ActionDispatch::Request::HTTP_METHODS`, in the Rails order.
      HTTP_METHODS = %w[
        OPTIONS GET HEAD POST PUT DELETE TRACE CONNECT
        PROPFIND PROPPATCH MKCOL COPY MOVE LOCK UNLOCK
        VERSION-CONTROL REPORT CHECKOUT CHECKIN UNCHECKOUT MKWORKSPACE UPDATE LABEL MERGE BASELINE-CONTROL MKACTIVITY
        ORDERPATCH ACL SEARCH MKCALENDAR PATCH
      ].freeze

      # The Rails paths of the Active Storage controllers that
      # `ActiveStorage::Routes.table` names. The emitted
      # `RouteTable::CONTROLLER_PATHS` holds the app's namespaced ones.
      CONTROLLER_PATHS = {
        active_storage_blobs_redirect: "active_storage/blobs/redirect",
        active_storage_blobs_proxy: "active_storage/blobs/proxy",
        active_storage_representations_redirect: "active_storage/representations/redirect",
        active_storage_disk: "active_storage/disk",
        active_storage_direct_uploads: "active_storage/direct_uploads"
      }.freeze

      def self.url_helpers
        UrlHelpers
      end

      # Rails' `recognize_path` for one path. The result has Symbol
      # keys and String values: `:controller`, `:action`, each dynamic
      # segment, and `:format` last. `:controller` is the Rails path of
      # a namespaced or Active Storage controller (`"admin/posts"`), and
      # the router symbol of any other controller.
      # `environment[:method]` is the verb, a Symbol or a String in any
      # case, and GET is the default. Other keys have no effect.
      def self.recognize_path(path, environment = {})
        # Rails normalizes a path, but not a full URL. The error message
        # shows the result.
        path = normalize_path(path) unless path.to_s.include?("://")
        # Rack's `MockRequest.env_for` reads the request path with this
        # parser. The query and the fragment have no effect. The router
        # ignores host and subdomain constraints, so the host has no
        # effect either.
        begin
          uri = URI::Parser.new.parse(path)
        rescue URI::InvalidURIError => e
          raise ActionController::RoutingError, e.message
        end
        request_path = uri.path.to_s
        request_path = "/#{request_path}" unless request_path.start_with?("/")
        verb = (environment[:method] || "GET").to_s.upcase
        # Rails checks the verb after it parses the path and before it
        # matches. The check also keeps out "ANY", which the router uses
        # for a `via: :all` route.
        unless HTTP_METHODS.include?(verb)
          accepted = "#{HTTP_METHODS[0...-1].join(", ")}, and #{HTTP_METHODS.last}"
          raise ActionController::UnknownHttpMethod, "#{verb}, accepted HTTP methods are #{accepted}"
        end
        matched = ActionDispatch::Router.match(verb, request_path, table)
        # The router ignores a trailing slash. Rails does not normalize
        # the path of a full URL, so there a trailing slash matches no
        # route without a glob. On a glob route, Rails keeps the slash in
        # the glob value. The match result does not show a glob, so this
        # method gives no match for that case too.
        matched = nil if request_path != "/" && request_path.end_with?("/")
        raise ActionController::RoutingError, "No route matches #{path.inspect}" if matched.nil?

        params = matched.path_params
        controller = CONTROLLER_PATHS[matched.controller] || RouteTable::CONTROLLER_PATHS[matched.controller]
        recognized = { controller: controller || matched.controller.to_s, action: matched.action.to_s }
        params.each { |name, value| recognized[name.to_sym] = value unless name == "format" }
        recognized[:format] = params["format"] if params.key?("format")
        recognized
      end

      # `ActionDispatch::Journey::Router::Utils.normalize_path`: one
      # leading slash, no doubled slashes, no trailing slash, and an
      # uppercase percent-encoded octet.
      def self.normalize_path(path)
        normalized = "/#{path}".squeeze("/")
        return normalized if normalized == "/"

        normalized.delete_suffix("/").gsub(/(%[a-f0-9]{2})/) { $1.upcase }
      end
      private_class_method :normalize_path

      # The table that the dispatcher composes, built once.
      # `RouteTable.table` makes new Route objects on each call. The
      # routes emit defines `RouteTable.root` only for an app with a
      # root route. Rails `recognize_path` skips a redirect route and
      # tries the later routes, so the table leaves the redirect routes
      # out.
      def self.table
        @table ||= ((RouteTable.respond_to?(:root) ? [RouteTable.root] : []) +
                    RouteTable.table + ActiveStorage::Routes.table)
                   .reject { |route| route.controller == :roundhouse_redirects }
      end
    end

    module UrlHelpers
      def self.root_url(host: "localhost", protocol: "http")
        "#{protocol}://#{host}#{RouteHelpers.root_path}"
      end
    end
  end
end
