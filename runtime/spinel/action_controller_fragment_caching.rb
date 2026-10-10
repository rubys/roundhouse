# Rails' controller fragment caching (AbstractController::Caching and
# ::Fragments) and the view side that asks it (ActionView::Helpers::
# CacheHelper), for the ruby family and spinel.
#
# A view's `<% cache key do %>` and a `render …, cached: true` collection
# read and write through `ActionView::ViewHelpers.fragment_read` /
# `fragment_write`; this file reopens both to do what Rails' CacheHelper
# does with the controller rendering the page:
#
# - nothing is cached unless `controller.perform_caching`;
# - the key is `controller.combined_fragment_cache_key(name)` —
#   `[:views, ENV["RAILS_CACHE_ID"] || ENV["RAILS_APP_VERSION"], name]`,
#   flattened one level and compacted — expanded as the store files it;
# - the store is `controller.cache_store` (`Rails.cache` by default).
#
# So an app overriding any of the three — campfire's `CachedResponses`
# keys every fragment by its SQLite observer epoch and the viewer, keeps
# them in `FragmentCache.store`, and turns caching off inside a
# transaction or once a foreign commit has invalidated the request's
# snapshot (rubys/roundhouse#698) — has the override honoured, and its
# `super` reaches these.
#
# Rendering with no controller in flight (a broadcast, a job, a test
# rendering a view directly) uses a detached controller. Since it cannot
# recover the app controller's cache policy or cache-key overrides, it
# fails closed and does not read or write shared fragments.
#
# NOT HERE: the `fragment_cache_key` class DSL (the `head` of the
# combined key), template digests (the view's name stands in for its
# digest path, as the key the lowering builds has always carried), and
# `expires_in:` on a store that cannot carry it (`ActiveSupport::Cache::
# MemoryStore` here) — such a fragment renders uncached rather than
# stay forever.
#
# `perform_caching` and `cache_store` are class settings inherited
# through the controller hierarchy, like Rails' class_attribute.
# Base's `perform_caching` defaults to true; the test harness sets it
# false, as a generated `config/environments/test.rb` does.
module ActionController
  class Base
    def self.perform_caching
      return @perform_caching if @perform_caching_assigned
      return true if self == ActionController::Base
      superclass.perform_caching
    end

    def self.perform_caching=(value)
      @perform_caching = value
      @perform_caching_assigned = true
    end

    def self.cache_store
      return @cache_store if @cache_store_assigned
      return Rails.cache if self == ActionController::Base
      superclass.cache_store
    end

    def self.cache_store=(store)
      @cache_store = store
      @cache_store_assigned = true
    end

    def perform_caching
      self.class.perform_caching
    end

    def cache_store
      self.class.cache_store
    end

    def combined_fragment_cache_key(key)
      out = [ :views ]
      prefix = ENV["RAILS_CACHE_ID"] || ENV["RAILS_APP_VERSION"]
      out.push(prefix) unless prefix.nil?
      if key.is_a?(Array)
        key.each { |part| out.push(part) unless part.nil? }
      elsif !key.nil?
        out.push(key)
      end
      out
    end

    # `nil` when caching is off or the fragment is absent.
    def read_fragment(key)
      return nil unless perform_caching
      store = cache_store
      return nil if store.nil?
      name = ActiveSupport::Cache.expanded_key(combined_fragment_cache_key(key))
      return store.read(name) if store.is_a?(ActiveSupport::Cache::MemoryStore)
      store.read_str(name)
    end

    # Answers `content`, stored or not. `ttl` is whole seconds, 0 for
    # none; a store that cannot carry it is not written.
    def write_fragment(key, content, ttl = 0)
      return content unless perform_caching
      store = cache_store
      return content if store.nil?
      name = ActiveSupport::Cache.expanded_key(combined_fragment_cache_key(key))
      if store.is_a?(ActiveSupport::Cache::MemoryStore)
        store.write(name, content) if ttl == 0
        return content
      end
      store.write_str(name, content, ttl)
    end
  end
end

module ActionView
  module ViewHelpers
    class DetachedFragmentController < ActionController::Base
      def perform_caching
        false
      end
    end

    def self.fragment_read(key)
      ActionView::ViewHelpers.fragment_controller.read_fragment(ActionView::ViewHelpers.fragment_name(key))
    end

    def self.fragment_write(key, value, ttl)
      ActionView::ViewHelpers.fragment_controller.write_fragment(ActionView::ViewHelpers.fragment_name(key), value, ttl)
    end

    # The controller rendering this page, or a detached one.
    def self.fragment_controller
      controller = ActionController::Current.controller
      return controller unless controller.nil?
      @detached_controller ||= DetachedFragmentController.new
    end

    # The lowering's key already leads with "views/"; the controller adds
    # its own, as Rails' does.
    def self.fragment_name(key)
      key.start_with?("views/") ? key[6, key.length - 6] : key
    end
  end
end
