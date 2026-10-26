# frozen_string_literal: true
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
      name = ActiveSupport::Cache.retrieve_cache_key(combined_fragment_cache_key(key))
      return store.read(name) if store.is_a?(ActiveSupport::Cache::MemoryStore)
      store.read_str(name)
    end

    # Answers `content`, stored or not. `ttl` is whole seconds, 0 for
    # none; a store that cannot carry it is not written.
    # Rails' `AbstractController::Caching::Fragments#
    # instrument_fragment_cache`: the block's value. What Rails reports
    # around it (`read_fragment.action_controller`) is not reported.
    def instrument_fragment_cache(_name, _key)
      yield
    end

    def write_fragment(key, content, ttl = 0)
      return content unless perform_caching
      store = cache_store
      return content if store.nil?
      name = ActiveSupport::Cache.retrieve_cache_key(combined_fragment_cache_key(key))
      if store.is_a?(ActiveSupport::Cache::MemoryStore)
        store.write(name, content) if ttl == 0
        return content
      end
      store.write_str(name, content, ttl)
    end
  end
end

module ActionView
  # Rails' `ActionView::PartialRenderer.collection_cache`: the store a
  # cached collection render reads and writes, `Rails.cache` until
  # assigned. An app assigns it from an initializer (written at the end
  # of boot.rb, `project::apply_module_mixins`), a test around a block.
  class PartialRenderer
    def self.collection_cache
      return @collection_cache if @collection_cache_assigned
      Rails.cache
    end

    def self.collection_cache=(store)
      @collection_cache = store
      @collection_cache_assigned = true
    end
  end

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

    # `<% cache record do %>`: Rails' CacheHelper hands the controller
    # `cache_fragment_name(record, digest_path:)`, which is `[template,
    # record]`, and the controller combines it. So an app's
    # `combined_fragment_cache_key` sees the record itself (campfire
    # shares a message's fragment across viewers only when it does), and
    # the key is the one a `cached: true` collection of the same partial
    # files the fragment under (`collection_fragment_key`).
    def self.fragment_read_record(template, record)
      ActionView::ViewHelpers.fragment_controller.read_fragment([ template, record ])
    end

    def self.fragment_write_record(template, record, value, ttl)
      ActionView::ViewHelpers.fragment_controller.write_fragment([ template, record ], value, ttl)
    end

    # Rails' `CollectionCaching#will_cache?`: the rendering controller's
    # `perform_caching`.
    def self.collection_caching?
      ActionView::ViewHelpers.fragment_controller.perform_caching
    end

    # One element's key, as Rails' `expanded_cache_key` builds it:
    # `combined_fragment_cache_key(cache_fragment_name(key, digest_path:))`.
    # The template path stands in for the digest path (templates carry
    # no digest here), so an app's override of
    # `combined_fragment_cache_key` sees the same `[path, key]` shape.
    # Expanded once, for the read and the write to share.
    #
    # Expanded as Rails' `expand_cache_key` does, by `retrieve_cache_key`:
    # a record contributes its `cache_key_with_version`. `cached: true`
    # hands the element itself through here, and a store's own
    # `expanded_key` (`cache_key`, no version) would never let it expire.
    def self.collection_fragment_key(partial, key)
      controller = ActionView::ViewHelpers.fragment_controller
      ActiveSupport::Cache.retrieve_cache_key(controller.combined_fragment_cache_key([ partial, key ]))
    end

    # Rails reads and writes every element through `ActionView::
    # PartialRenderer.collection_cache` — `Rails.cache` unless the app
    # points it elsewhere (campfire's `fragment_cache.rb` initializer
    # points it at its bounded `FragmentCache.store`), NOT the
    # controller's `cache_store`.
    def self.collection_fragment_read(name)
      store = ActionView::PartialRenderer.collection_cache
      return nil if store.nil?
      return store.read(name) if store.is_a?(ActiveSupport::Cache::MemoryStore)
      store.read_str(name)
    end

    # The whole collection's keys in one `read_multi`, as Rails asks.
    def self.collection_fragment_read_multi(names)
      store = ActionView::PartialRenderer.collection_cache
      return names.map { |_name| nil } if store.nil?
      if store.is_a?(ActiveSupport::Cache::MemoryStore)
        found = store.read_multi(*names)
        return names.map { |name| found[name] }
      end
      names.map { |name| store.read_str(name) }
    end

    def self.collection_fragment_write(name, value)
      store = ActionView::PartialRenderer.collection_cache
      return value if store.nil?
      if store.is_a?(ActiveSupport::Cache::MemoryStore)
        store.write(name, value)
        return value
      end
      store.write_str(name, value, 0)
    end

    # Jbuilder's `cache!`: on when the controller caches.
    def self.jbuilder_caching?
      ActionView::ViewHelpers.fragment_controller.perform_caching
    end

    # Jbuilder's `_cache_key`: the template and the key as
    # `cache_fragment_name` pairs them (the template's path standing in
    # for its digest path), through the controller's
    # `combined_fragment_cache_key`, expanded under the `jbuilder`
    # namespace.
    def self.jbuilder_cache_key(template, key)
      controller = ActionView::ViewHelpers.fragment_controller
      ActiveSupport::Cache.expand_cache_key(controller.combined_fragment_cache_key([ template, key ]), :jbuilder)
    end

    def self.jbuilder_fragment_read(key)
      JbuilderTemplate.new(ActionView::ViewHelpers.fragment_controller).read_fragment(key)
    end

    def self.jbuilder_fragment_write(key, value)
      JbuilderTemplate.new(ActionView::ViewHelpers.fragment_controller).write_fragment(key, value)
    end

    # The controller rendering this page, or a detached one.
    def self.fragment_controller
      controller = ActionController::Current.controller
      return controller unless controller.nil?
      ActionView::ViewHelpers.broadcast_controller
    end

    # The detached controller: what a broadcast renders through
    # (runtime/thread_state.rb), and what any render with no controller
    # in flight reads.
    def self.broadcast_controller
      @detached_controller ||= DetachedFragmentController.new
    end

    # The lowering's key already leads with "views/"; the controller adds
    # its own, as Rails' does.
    def self.fragment_name(key)
      key.start_with?("views/") ? key[6, key.length - 6] : key
    end
  end
end

# Jbuilder's template, as far as its fragment cache goes: the two
# methods Jbuilder 2.14's `cache!` reads and writes through, with
# Jbuilder's bodies (`Rails.cache`, under the controller's
# `instrument_fragment_cache`). An app overrides them by prepending onto
# this class from an initializer (campfire's `BoundedJbuilderFragments`
# reads `controller.cache_store` instead); the prepend is written at the
# end of boot.rb (`project::apply_module_mixins`). `@context` is the
# view, which answers `controller`.
class JbuilderTemplate
  class Context
    def initialize(controller)
      @controller = controller
    end

    def controller
      @controller
    end
  end

  def initialize(controller)
    @context = JbuilderTemplate::Context.new(controller)
  end

  def read_fragment(key)
    _read_fragment_cache(key, nil)
  end

  def write_fragment(key, value)
    _write_fragment_cache(key, nil) { value }
  end

  def _read_fragment_cache(key, options = nil)
    @context.controller.instrument_fragment_cache(:read_fragment, key) { Rails.cache.read_str(key) }
  end

  def _write_fragment_cache(key, options = nil)
    @context.controller.instrument_fragment_cache(:write_fragment, key) do
      value = yield
      Rails.cache.write_str(key, value, 0)
      value
    end
  end
end
