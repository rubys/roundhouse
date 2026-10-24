require "json"
require "stringio"
module JsonPrependedAbsent
  def to_io = false
end
module JsonPrependedNil
  def to_io = nil
end
module JsonPrependedConversion
  def to_io = StringIO.new
end
module JsonPrependedAlias
  def absent = false
  alias to_io absent
end
module JsonPrependedAliasMethod
  def absent = nil
  alias_method :to_io, :absent
end
module JsonPrependedWriter
  def write(text) = text.bytesize
  def flush = self
end
module JsonPrependedQuery
  def respond_to?(name, include_private = false) = false
end
class PrependedDestination
  prepend JsonPrependedAbsent
  def write(text) = text.bytesize
  def flush = self
end
class JsonPrependedOwnConversion < PrependedDestination
  prepend JsonPrependedNil
  def to_io = StringIO.new
end
class JsonPrependedPresent
  prepend JsonPrependedConversion
  def to_io = false
end
class JsonPrependedInherited < PrependedDestination
end
module JsonPrependedRelay
  prepend JsonPrependedAbsent
  def write(text) = text.bytesize
  def flush = nil
end
class JsonPrependedIncluded
  include JsonPrependedRelay
end
class JsonPrependedExtended
  extend JsonPrependedRelay
end
class JsonPrependedAliasDestination < PrependedDestination
  prepend JsonPrependedAlias
end
class JsonPrependedAliasMethodDestination < PrependedDestination
  prepend JsonPrependedAliasMethod
end
class JsonPrependedWriteDestination
  prepend JsonPrependedWriter
end
class JsonPrependedQueryDestination < PrependedDestination
  prepend JsonPrependedQuery
end
class JsonPrependedConvertedWriter
  def to_io = JsonPrependedWriteDestination.new
end
module JsonPrependedPrivateWriter
  private
  def write(text) = text.bytesize
end
module JsonPrependedProtectedWriter
  protected
  def write(text) = text.bytesize
end
class JsonPrependedPrivateDestination
  prepend JsonPrependedPrivateWriter
  def write(text) = text.bytesize
  def flush = self
end
class JsonPrependedProtectedDestination
  def write(text) = text.bytesize
  def flush = self
  prepend JsonPrependedProtectedWriter
end
class JsonPrependedLateDestination
  def to_io = StringIO.new
  prepend JsonPrependedAbsent
end
class JsonPrependedOrderAbsent
  prepend JsonPrependedConversion
  prepend JsonPrependedAbsent
end
class JsonPrependedOrderPresent
  prepend JsonPrependedAbsent
  prepend JsonPrependedConversion
end
class JsonPrependedMultiple
  prepend JsonPrependedAbsent, JsonPrependedConversion
end
class JsonPrependedModuleAliasHost
  ModuleAlias = JsonPrependedAbsent
  prepend ModuleAlias
  def write(text) = text.bytesize
  def flush = self
end
class JsonPrependedModuleAliasExtended
  ModuleAlias = JsonPrependedRelay
  extend ModuleAlias
end
module JsonPrependedNamespace
  module Absent
    def to_io = false
  end
  class Destination
    prepend Absent
    def write(text) = text.bytesize
    def flush = self
  end
end
class PrependProbe
  JsonEncoder = JSON
  DESTINATION = PrependedDestination.new
  def initialize
    @destination = PrependedDestination.new
  end
  def ivar = JSON.dump([1], @destination)
  def self.call = JSON.dump([1], PrependedDestination.new)
  def self.own_conversion = JSON.dump([1], JsonPrependedOwnConversion.new)
  def self.present = JSON.dump([1], JsonPrependedPresent.new)
  def self.inherited = JSON.dump([1], JsonPrependedInherited.new)
  def self.included = JSON.dump([1], JsonPrependedIncluded.new)
  def self.extended = JSON.dump([1], JsonPrependedExtended)
  def self.aliased = JSON.dump([1], JsonPrependedAliasDestination.new)
  def self.alias_method = JSON.dump([1], JsonPrependedAliasMethodDestination.new)
  def self.writer = JSON.dump([1], JsonPrependedWriteDestination.new)
  def self.converted_writer = JSON.dump([1], JsonPrependedConvertedWriter.new)
  def self.query = JSON.dump([1], JsonPrependedQueryDestination.new)
  def self.constant = JSON.dump([1], DESTINATION)
  def self.factory = PrependedDestination.new
  def self.returned = JSON.dump([1], factory)
  def self.local_alias
    destination = PrependedDestination.new
    ::JSON.dump([1], destination)
  end
  def self.receiver_alias = JsonEncoder.dump([1], PrependedDestination.new)
  def self.scalar = JSON.dump(1, PrependedDestination.new)
  def self.hash = JSON.dump({a: [1]}, PrependedDestination.new)
  def self.limit = JSON.dump([1], PrependedDestination.new, 2)
  def self.options = JSON.dump([1], PrependedDestination.new, strict: true)
  def self.nested = JSON.dump(JSON.dump([1], PrependedDestination.new))
  def self.private_writer = JSON.dump([1], JsonPrependedPrivateDestination.new)
  def self.protected_writer = JSON.dump([1], JsonPrependedProtectedDestination.new)
  def self.late = JSON.dump([1], JsonPrependedLateDestination.new)
  def self.order_absent = JSON.dump([1], JsonPrependedOrderAbsent.new)
  def self.order_present = JSON.dump([1], JsonPrependedOrderPresent.new)
  def self.multiple = JSON.dump([1], JsonPrependedMultiple.new)
  def self.module_alias = JSON.dump([1], JsonPrependedModuleAliasHost.new)
  def self.module_alias_extended = JSON.dump([1], JsonPrependedModuleAliasExtended)
  def self.namespaced = JSON.dump([1], JsonPrependedNamespace::Destination.new)
  def self.union(flag)
    JSON.dump([1], flag ? PrependedDestination.new : StringIO.new)
  end
end
