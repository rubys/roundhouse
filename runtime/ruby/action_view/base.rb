# frozen_string_literal: true

# The view context Rails instantiates for a template. Roundhouse still
# macro-expands `form_with` into the accumulator (the block binds a
# typed builder the runtime does not reconstruct), but each expanded
# site begins by calling this instance method. A fragment-cache hit
# never walks that site, so it never enters `form_with`.
#
# That is the entry `ActionView::Base.any_instance.expects(:form_with)`
# observes. The method returns nil; the markup that follows in the
# expansion is the form. A body that also emitted the tags would
# double them.
module ActionView
  class Base
    def form_with
      nil
    end

    # ActionView::Helpers::AssetUrlHelper#asset_path for a view context a
    # plain object asks (`ApplicationController.new.view_context`): the source as the asset pipeline names it
    # (`ViewHelpers.digested_asset_path`; `skip_pipeline:` leaves it
    # under public/), then the `host:` option as the asset host. Rails'
    # `compute_asset_host` reads a request and `config.asset_host` as
    # well; neither exists for a context built outside a request.
    def asset_path(source, options = {})
      raise ArgumentError, "nil is not a valid asset source" if source.nil?
      source = source.to_s
      return "" if source.strip.empty?
      return source if source.match?(/\A(?:[a-z][a-z\d+.-]*:|\/\/)/i)
      tail = source[/([?#].+)\z/]
      source = source.sub(/([?#].+)\z/, "")
      unless source.start_with?("/")
        source = options[:skip_pipeline] ? "/#{source}" : ActionView::ViewHelpers.digested_asset_path(source)
      end
      host = options[:host]
      source = "#{host.match?(/\A(?:[a-z][a-z\d+.-]*:|\/\/)/i) ? host : "//#{host}"}/#{source.delete_prefix("/")}" if host
      "#{source}#{tail}"
    end

    # `asset_path` with `protocol: :request`: with no request the host
    # stays protocol-relative, as `asset_path` leaves it.
    def asset_url(source, options = {})
      asset_path(source, options)
    end
  end
end
