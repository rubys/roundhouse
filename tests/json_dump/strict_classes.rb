require "json"
require "stringio"
class ReviewArray < Array; end
class ReviewHash < Hash; end
class ReviewString < String; end
class ReviewChildArray < ReviewArray; end
class ReviewNumber < Numeric; end
class ReviewTaggedArray < Array
  def to_json(state = nil) = '"custom"'
end
class ReviewTaggedHash < Hash
  def to_json(state = nil) = '"custom"'
end
class ReviewTaggedString < String
  def to_json(state = nil) = '"custom"'
end
class StrictProbe
  ArrayAlias = ReviewArray
  def self.array = JSON.dump(ReviewArray.new, strict: true)
  def self.hash = JSON.dump(ReviewHash.new, strict: true)
  def self.string = JSON.dump(ReviewString.new, strict: true)
  def self.check(value, stream)
    JSON.dump(value, stream, nil, {strict: true})
  end
end
