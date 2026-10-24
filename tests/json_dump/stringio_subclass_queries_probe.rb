p StringioProbe.call
[JsonNegativeStringIO.new, JsonIncludedNegativeStream.new,
 JsonPrependedNegativeStream.new, JsonAliasedNegativeStream.new,
 JsonAliasMethodNegativeStream.new].each do |stream|
  p StringioProbe.scalar(stream)
  p StringioProbe.hash(stream)
  p StringioProbe.options(stream)
  p StringioProbe.aliased(stream)
  p StringioProbe.nested(stream)
  p stream.string
  p StringioProbe.explicit(stream).equal?(stream)
  p stream.string
end

[JsonOrdinaryStream.new, JsonPositiveStream.new].each do |stream|
  p StringioProbe.explicit(stream).equal?(stream)
  p stream.string
  p StringioProbe.aliased(stream).equal?(stream)
  p stream.string
end
