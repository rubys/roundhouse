require "json"
require "stringio"
class JsonNegativeStringIO < StringIO
  def respond_to?(name, include_private = false) = false
end
module JsonNegativeStreamQuery
  def respond_to?(name, include_private = false) = false
end
class JsonIncludedNegativeStream < StringIO
  include JsonNegativeStreamQuery
end
class JsonPrependedNegativeStream < StringIO
  prepend JsonNegativeStreamQuery
end
class JsonAliasedNegativeStream < StringIO
  def query(name, include_private = false) = false
  alias respond_to? query
end
class JsonAliasMethodNegativeStream < StringIO
  def query(name, include_private = false) = false
  alias_method :respond_to?, :query
end
class StringioProbe
  JsonEncoder = JSON
  def self.call
    stream = StringIO.new
    result = JSON.dump([1], stream)
    [result.class.to_s, stream.string]
  end
  def self.scalar(stream) = JSON.dump(1, stream)
  def self.hash(stream) = JSON.dump({a: [1]}, stream)
  def self.options(stream) = JSON.dump([1], stream, strict: true)
  def self.aliased(stream) = JsonEncoder.dump([1], stream)
  def self.nested(stream) = JSON.dump(JSON.dump([1], stream), stream)
  def self.explicit(stream) = JSON.dump([1], stream, 2)
end

class JsonOrdinaryStream < StringIO
end
class JsonPositiveStream < StringIO
  def respond_to?(name, include_private = false) = name == :write
end
