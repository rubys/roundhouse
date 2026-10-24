[:array, :hash, :string].each do |name|
  begin
    if name == :array
      p StrictProbe.array
    elsif name == :hash
      p StrictProbe.hash
    else
      p StrictProbe.string
    end
  rescue JSON::GeneratorError => error
    puts error.class.to_s + ":" + error.message
  end
end
array = ReviewArray.new
array << 1
hash = ReviewHash.new
hash[:a] = 1
string = ReviewString.new("s")
[ReviewArray.new, ReviewHash.new, ReviewString.new, array, hash, string,
 ReviewChildArray.new, StrictProbe::ArrayAlias.new, ReviewNumber.new,
 ReviewTaggedArray.new, ReviewTaggedHash.new, ReviewTaggedString.new,
 [array], {a: hash}, [{a: string}]].each do |value|
  stream = StringIO.new
  begin
    p StrictProbe.check(value, stream)
  rescue JSON::GeneratorError => error
    puts error.class.to_s + ":" + error.message
  end
  p stream.string
end
[nil, true, false, "s", 1, 1.5, [], {}, [1], {a: "s"}].each do |value|
  stream = StringIO.new
  result = StrictProbe.check(value, stream)
  p result.equal?(stream)
  p stream.string
end
# Strict validation traverses values; JSON stringifies keys separately.
p JSON.dump({ReviewString.new("key") => 1}, strict: true)
p JSON.dump(ReviewArray.new, strict: false)
p JSON.dump(ReviewHash.new, strict: false)
p JSON.dump(ReviewString.new, strict: false)
