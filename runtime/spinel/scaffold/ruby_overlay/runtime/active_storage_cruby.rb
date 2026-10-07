# frozen_string_literal: true

# Active Storage's per-load string work, on the CRuby/JRuby trees.
#
# `Blob.columns(alias)` built the same projection with a chain of String#+
# on every attachment load, and `BlobMetadata.int_field` walked a blob's
# metadata JSON one character at a time (a String per digit). Both run on
# every avatar and attachment the page touches. Shared runtime/ruby keeps
# the portable versions for the strict targets and Spinel.
module ActiveStorage
  class Blob
    COLUMNS_BY_ALIAS = {} # rubocop:disable Style/MutableConstant -- a memo, filled once per alias

    class << self
      alias_method :columns_uncached, :columns

      # The projection depends only on the alias (`"b"`, `"blob"`): built
      # once per alias, and frozen.
      def columns(alias_prefix)
        COLUMNS_BY_ALIAS[alias_prefix] ||= columns_uncached(alias_prefix).freeze
      end
    end
  end

  class BlobMetadata
    INT_FIELD_PATTERNS = {} # rubocop:disable Style/MutableConstant -- a memo, filled once per name

    # The digits right after `"name":`, as the shared walker reads them
    # (0 when the field is absent or not a number).
    def self.int_field(json, name)
      pattern = (INT_FIELD_PATTERNS[name] ||= /"#{Regexp.escape(name)}":(\d+)/)
      m = pattern.match(json)
      m.nil? ? 0 : m[1].to_i
    end
  end
end
