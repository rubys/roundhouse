# `ActiveSupport::Cache.expand_cache_key` and `ActiveSupport::Cache::
# MemoryStore` — ported from activesupport 8.1 (cache.rb,
# cache/memory_store.rb) for the ruby family and spinel, where campfire's
# `FragmentCache` keeps its bounded store (rubys/roundhouse#698):
#
#     STORE = ActiveSupport::Cache::MemoryStore.new(size: 64.megabytes)
#
# `RecordCache` reads and writes record snapshots in it; `MessagesHelper`
# `fetch`es rendered fragments; `CachedResponses` hands it to Rails as the
# controller's `cache_store`.
#
# THE STORE IS ACTIVESUPPORT'S, BOUND FOR BOUND. Entries are kept in
# least-recently-used order (a read moves its key to the end); an entry
# costs its key's bytes + its value's bytes + 240, as `cached_size` counts
# it; and a write that takes the total past `size` prunes from the
# least-recently-used end down to three quarters of it. A String value is
# copied on the way in and on the way out (`DupCoder`), so a caller
# mutating what it wrote or read does not change the cache. Keys are
# normalized as `Store#expanded_key` does: a record by its `cache_key`, an
# Array by its elements joined with "/", a Hash as sorted `k=v` pairs.
#
# NOT HERE: entry options (`expires_in:`, `version:`, `race_condition_ttl:`,
# `namespace:`) — a call passing one raises ArgumentError rather than
# being cached with the option ignored; `increment`/`decrement`;
# instrumentation (`cache_read.active_support`). Non-String values are
# limited to recursively copied Arrays/Hashes and immutable scalars; other
# object types fail closed rather than being retained by reference. Rails
# counts a Marshal dump for non-Strings; this port counts the copied value's
# `to_s` bytes.
#
# A SUBCLASS OF `Rails::Cache`, so one stands wherever the runtime's own
# store does: campfire's caching tests assign a fresh MemoryStore to
# `Rails.cache` and restore the old one after. The runtime's fragment
# lowering reaches `Rails.cache` through its typed String protocol
# (`read_str`/`write_str`/`fetch_str`), which this class inherits whole;
# the Rails API below is the bounded store.
module ActiveSupport
  module Cache
    # `ActiveSupport::Cache.expand_cache_key(key, namespace = nil)`.
    def self.expand_cache_key(key, namespace = nil)
      out = namespace.nil? ? "" : "#{namespace}/"
      prefix = ENV["RAILS_CACHE_ID"] || ENV["RAILS_APP_VERSION"]
      out += "#{prefix}/" unless prefix.nil?
      out + ActiveSupport::Cache.retrieve_cache_key(key)
    end

    # Rails' `retrieve_cache_key`: a record's `cache_key_with_version`,
    # an Array's elements joined with "/", a Hash as its pairs, nil as
    # "" (its `to_a` is empty), anything else as its `to_param`.
    def self.retrieve_cache_key(key)
      return key.cache_key_with_version.to_s if key.respond_to?(:cache_key_with_version)
      return key.cache_key.to_s if key.respond_to?(:cache_key)
      return key.map { |element| ActiveSupport::Cache.retrieve_cache_key(element) }.join("/") if key.is_a?(Array)
      return ActiveSupport::Cache.retrieve_cache_key(key.to_a) if key.is_a?(Hash)
      return "" if key.nil?
      key.to_s
    end

    # `Store#expanded_key`: the key a store files an entry under.
    def self.expanded_key(key)
      return key.cache_key.to_s if key.respond_to?(:cache_key)
      if key.is_a?(Array)
        return ActiveSupport::Cache.expanded_key(key.first) if key.length == 1
        return key.map { |element| ActiveSupport::Cache.expanded_key(element) }.join("/")
      end
      return key.map { |k, v| "#{k}=#{v}" }.sort.join("/") if key.is_a?(Hash)
      return "" if key.nil?
      key.to_s
    end

    class MemoryStore < Rails::Cache
      PER_ENTRY_OVERHEAD = 240

      def initialize(size: 32 * 1024 * 1024)
        super()
        @data = {}
        @max_size = size
        @cache_size = 0
        @lock = Mutex.new
      end

      def read(name)
        key = ActiveSupport::Cache.expanded_key(name)
        _, value = @lock.synchronize { read_locked(key) }
        MemoryStore.copy(value)
      end

      def exist?(name)
        key = ActiveSupport::Cache.expanded_key(name)
        @lock.synchronize { @data.key?(key) }
      end

      def write(name, value)
        key = ActiveSupport::Cache.expanded_key(name)
        payload = MemoryStore.copy(value)
        @lock.synchronize do
          if @data.key?(key)
            @cache_size -= MemoryStore.bytes(@data[key]) - MemoryStore.bytes(payload)
            @data.delete(key)
          else
            @cache_size += key.bytesize + MemoryStore.bytes(payload) + PER_ENTRY_OVERHEAD
          end
          @data[key] = payload
          prune_locked(@max_size * 3 / 4) if @cache_size > @max_size
        end
        true
      end

      # The cached value, or the block's — which is then written.
      def fetch(name)
        key = ActiveSupport::Cache.expanded_key(name)
        found, cached = @lock.synchronize { read_locked(key) }
        # Keep one result path: Spinel inlines yield-bearing methods at
        # call sites, where an early return is mis-typed as the caller's.
        value = if found
          MemoryStore.copy(cached)
        else
          computed = yield
          write(key, computed)
          computed
        end
        value
      end

      def delete(name)
        key = ActiveSupport::Cache.expanded_key(name)
        @lock.synchronize { delete_locked(key) }
      end

      def read_multi(*names)
        out = {}
        names.each do |name|
          key = ActiveSupport::Cache.expanded_key(name)
          found, value = @lock.synchronize { read_locked(key) }
          out[name] = MemoryStore.copy(value) if found
        end
        out
      end

      def write_multi(hash)
        hash.each { |name, value| write(name, value) }
        true
      end

      def clear
        @lock.synchronize do
          @data.clear
          @cache_size = 0
        end
        super
      end

      # Entries expire only by option, and options are not carried here.
      def cleanup
        nil
      end

      def prune(target_size)
        @lock.synchronize { prune_locked(target_size) }
        nil
      end

      def inspect
        "#<ActiveSupport::Cache::MemoryStore entries=#{@data.size}, size=#{@cache_size}>"
      end

      def self.copy(value, ancestors = [])
        # Any String, a SafeBuffer included: `dup` keeps the class, so the
        # html-safe body a helper renders (campfire's text presentation,
        # through `auto_link`) comes back html-safe, as Rails' store
        # returns it.
        return value.dup if value.is_a?(String)
        if value.instance_of?(Array)
          raise ArgumentError, "MemoryStore cannot safely copy a recursive value" if ancestors.any? { |ancestor| ancestor.equal?(value) }
          ancestors << value
          out = value.map { |item| MemoryStore.copy(item, ancestors) }
          ancestors.pop
          return out
        end
        if value.instance_of?(Hash)
          raise ArgumentError, "MemoryStore cannot safely copy a recursive value" if ancestors.any? { |ancestor| ancestor.equal?(value) }
          raise ArgumentError, "MemoryStore cannot safely copy an identity Hash" if value.compare_by_identity?
          raise ArgumentError, "MemoryStore cannot safely copy a Hash with a default proc" if value.default_proc
          ancestors << value
          out = Hash.new(MemoryStore.copy(value.default, ancestors))
          value.each do |key, item|
            out[MemoryStore.copy(key, ancestors)] = MemoryStore.copy(item, ancestors)
          end
          ancestors.pop
          return out
        end
        return value if value.nil? || value.equal?(true) || value.equal?(false)
        return value if value.instance_of?(Integer) || value.instance_of?(Float) || value.instance_of?(Symbol)
        raise ArgumentError, "MemoryStore cannot safely copy this value type"
      end

      def self.bytes(value)
        return 0 if value.nil?
        value.is_a?(String) ? value.bytesize : value.to_s.bytesize
      end

      private

      # Return presence separately because a cached nil is still a hit.
      # Call only while holding @lock.
      def read_locked(key)
        return [false, nil] unless @data.key?(key)
        value = @data.delete(key)
        @data[key] = value
        [true, value]
      end

      def delete_locked(key)
        return false unless @data.key?(key)
        payload = @data.delete(key)
        @cache_size -= key.bytesize + MemoryStore.bytes(payload) + PER_ENTRY_OVERHEAD
        true
      end

      def prune_locked(target_size)
        @data.keys.each do |key|
          break if @cache_size <= target_size
          delete_locked(key)
        end
      end
    end
  end
end
