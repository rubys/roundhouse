module ActiveSupport
  # Shared implementation used by the typed Hash#deep_merge call lowering.
  # Recursively combine only Hash/Hash value pairs; all other right-hand
  # values replace the left, as Rails 8.1.4 does.
  def self.deep_merge(left, other_hash)
    merged = left.dup
    other_hash.each do |key, other_value|
      if left.key?(key) && left[key].is_a?(Hash) && other_value.is_a?(Hash)
        current_value = left[key]
        merged[key] = ActiveSupport.deep_merge(current_value, other_value)
      else
        merged[key] = other_value
      end
    end
    merged
  end
end
