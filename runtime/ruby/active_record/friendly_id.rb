# friendly_id's finder rule that is not a query: whether a String that
# matched no slug could still be a primary key.
#
# `Model.friendly.find(x)` (ActiveRecord::Relation#friendly_find) is
# friendly_id 5.x's FinderMethods#find, which for a String does, in
# order: look the slug up (and, under `:history`, the slug history);
# else, if `potential_primary_key?(x)`, fall back to the plain primary
# key find; else raise RecordNotFound. `potential_primary_key?` for an
# integer key is `Integer(id, 10)` succeeding, which is STRICTER than
# the `to_i` prefix parse the primary-key cast applies afterwards:
# "12abc" is not a candidate (a not-found, with friendly_id's message),
# "12" and " 12 " and "1_2" are. It is spelled here once, rather than
# inline in the Relation, so the rule has a name and a home.
#
# Not modeled: `Integer`'s radix prefix ("0d12"), which no URL carries.
module ActiveRecord
  module FriendlyIdFinder
    # `Integer(text, 10)` succeeds: optional surrounding whitespace, an
    # optional sign, decimal digits with single underscores between them.
    def self.integer_text?(text)
      text.match?(/\A\s*[+-]?\d+(?:_\d+)*\s*\z/)
    end

    # friendly_id's `potential_primary_key?` for a model whose key is an
    # integer or, for any other key type this runtime has, a string
    # (`else true`). A uuid key is declined at lowering.
    def self.potential_primary_key?(text, string_key)
      return true if string_key
      integer_text?(text)
    end
  end
end
