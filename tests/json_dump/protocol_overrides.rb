require "stringio"
module JsonBaseConversion
  def to_io = StringIO.new
end
module JsonFalseOverride
  include JsonBaseConversion
  def to_io = false
end
module JsonNilOverride
  include JsonBaseConversion
  def to_io = nil
end
module JsonAliasOverride
  include JsonBaseConversion
  def absent = false
  alias to_io absent
end
module JsonAliasMethodOverride
  include JsonBaseConversion
  def absent = nil
  alias_method :to_io, :absent
end
module JsonFalseBase
  def to_io = false
end
module JsonWriterOverride
  include JsonFalseBase
  def to_io = StringIO.new
end
module JsonFalseRelay
  include JsonFalseOverride
  def false_relay_marker = nil
end
module JsonDeferredOverride
  include JsonBaseConversion
  def to_io = absent_destination
  def absent_destination = false
end
class JsonDeferredConflict
  include JsonDeferredOverride
end
module JsonDiamondRelay
  include JsonBaseConversion
  include JsonFalseRelay
  def diamond_relay_marker = nil
end
class JsonNestedFalse
  include JsonFalseOverride
end
class JsonNestedNil
  include JsonNilOverride
end
class JsonNestedAlias
  include JsonAliasOverride
end
class JsonNestedAliasMethod
  include JsonAliasMethodOverride
end
class JsonNestedWriter
  include JsonWriterOverride
end
class JsonRelayedFalse
  include JsonFalseRelay
end
class JsonDiamondFalse
  include JsonDiamondRelay
end
class JsonInheritedConflict < JsonNestedFalse
end
class JsonIncludeOrderFalse
  include JsonBaseConversion
  include JsonFalseBase
end
class JsonIncludeOrderWriter
  include JsonFalseBase
  include JsonBaseConversion
end
class JsonOwnFalse
  include JsonBaseConversion
  def to_io = false
end
class JsonOwnWriter
  include JsonFalseBase
  def to_io = StringIO.new
end
class JsonInheritedOwnWriter < JsonNestedFalse
  def to_io = StringIO.new
end
class JsonExtendedConflict
  extend JsonFalseOverride
end
class JsonInheritedExtendedConflict < JsonExtendedConflict
end
class JsonQueryBase
  def to_io = StringIO.new
end
module JsonRejectQuery
  def respond_to?(name, include_private = false) = false
end
module JsonQueryRelay
  include JsonRejectQuery
  def query_relay_marker = nil
end
class JsonOwnQuery < JsonQueryBase
  def respond_to?(name, include_private = false) = false
end
class JsonInheritedQuery < JsonOwnQuery
end
class JsonIncludedQuery < JsonQueryBase
  include JsonRejectQuery
end
class JsonRelayedQuery < JsonQueryBase
  include JsonQueryRelay
end
class JsonAliasQuery < JsonQueryBase
  def rejects(name, include_private = false) = false
  alias respond_to? rejects
end
class JsonAliasMethodQuery < JsonQueryBase
  def rejects(name, include_private = false) = false
  alias_method :respond_to?, :rejects
end
class JsonAcceptQuery < JsonQueryBase
  def respond_to?(name, include_private = false) = true
end
class JsonQueryWriter
  def write(text) = text.bytesize
  def flush = self
  def respond_to?(name, include_private = false) = false
end
class JsonClassQuery
  def self.to_io = StringIO.new
  def self.respond_to?(name, include_private = false) = false
end
class JsonInheritedClassQuery < JsonClassQuery
end
class JsonExtendedQuery
  extend JsonBaseConversion
  extend JsonRejectQuery
end
class JsonRelayedExtendedQuery
  extend JsonBaseConversion
  extend JsonQueryRelay
end
class JsonOrdinaryConversion
  include JsonBaseConversion
end
module JsonBaseWriter
  def write(text) = false
end
module JsonOverrideWriter
  include JsonBaseWriter
  def write(text) = text.bytesize
end
class JsonConflictingWriter
  include JsonOverrideWriter
  def flush = self
end
class JsonProtocolProbe
  DESTINATION = JsonNestedFalse.new
  def initialize
    @destination = JsonNestedFalse.new
  end
  def ivar = JSON.dump([1], @destination)
  def self.destination = JsonNestedFalse.new
  def self.nested_false = JSON.dump([1], JsonNestedFalse.new)
  def self.deferred_conflict = JSON.dump([1], JsonDeferredConflict.new)
  def self.nested_nil = JSON.dump([1], JsonNestedNil.new)
  def self.nested_alias = JSON.dump([1], JsonNestedAlias.new)
  def self.nested_alias_method = JSON.dump([1], JsonNestedAliasMethod.new)
  def self.nested_writer = JSON.dump([1], JsonNestedWriter.new)
  def self.relayed_false = JSON.dump([1], JsonRelayedFalse.new)
  def self.diamond_false = JSON.dump([1], JsonDiamondFalse.new)
  def self.inherited_conflict = JSON.dump([1], JsonInheritedConflict.new)
  def self.include_order_false = JSON.dump([1], JsonIncludeOrderFalse.new)
  def self.include_order_writer = JSON.dump([1], JsonIncludeOrderWriter.new)
  def self.extended_conflict = JSON.dump([1], JsonExtendedConflict)
  def self.inherited_extended_conflict = JSON.dump([1], JsonInheritedExtendedConflict)
  def self.own_query = JSON.dump([1], JsonOwnQuery.new)
  def self.inherited_query = JSON.dump([1], JsonInheritedQuery.new)
  def self.included_query = JSON.dump([1], JsonIncludedQuery.new)
  def self.relayed_query = JSON.dump([1], JsonRelayedQuery.new)
  def self.alias_query = JSON.dump([1], JsonAliasQuery.new)
  def self.alias_method_query = JSON.dump([1], JsonAliasMethodQuery.new)
  def self.accept_query = JSON.dump([1], JsonAcceptQuery.new)
  def self.query_writer = JSON.dump([1], JsonQueryWriter.new)
  def self.class_query = JSON.dump([1], JsonClassQuery)
  def self.inherited_class_query = JSON.dump([1], JsonInheritedClassQuery)
  def self.extended_query = JSON.dump([1], JsonExtendedQuery)
  def self.relayed_extended_query = JSON.dump([1], JsonRelayedExtendedQuery)
  def self.constant = JSON.dump([1], DESTINATION)
  def self.returned = JSON.dump([1], destination)
  def self.local_alias
    destination = JsonNestedFalse.new
    encoder = JSON
    encoder.dump({"a" => [1]}, destination, 3, {strict: true})
  end
  def self.local_query_alias
    destination = JsonInheritedQuery.new
    JSON.dump({"a" => [1]}, destination)
  end
  def self.absolute = ::JSON.dump(1, JsonNestedFalse.new, 2)
  def self.explicit_limit = JSON.dump([1], JsonOwnQuery.new, 2)
  def self.options = JSON.dump([1], JsonOwnQuery.new, {strict: true})
  def self.nested_dump = JSON.dump(JSON.dump([1], JsonNestedFalse.new))
  def self.own_false = JSON.dump([1], JsonOwnFalse.new)
  def self.own_writer = JSON.dump([1], JsonOwnWriter.new)
  def self.inherited_own_writer = JSON.dump([1], JsonInheritedOwnWriter.new)
  def self.ordinary = JSON.dump([1], JsonOrdinaryConversion.new)
  def self.disagreeing_writer = JSON.dump([1], JsonConflictingWriter.new)
  def self.union(flag) = JSON.dump([1], flag ? JsonNestedFalse.new : StringIO.new, 2)
  def self.query_union(flag) = JSON.dump([1], flag ? JsonInheritedQuery.new : StringIO.new, 2)
end
