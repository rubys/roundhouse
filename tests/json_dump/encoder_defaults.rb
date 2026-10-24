require "json"
require "stringio"
JsonEncoderDefaults = JSON
[Float::NAN, Float::INFINITY, -Float::INFINITY].each do |value|
  puts JSON.dump(value)
  puts JSON.dump([value, {"value" => value}])
  puts ::JSON.dump(value, false, 4, {strict: true})
  stream = StringIO.new
  puts JsonEncoderDefaults.dump(value, stream, nil, {strict: false}).string
  puts JSON.dump(JSON.dump(value))
end
# arrays exceed generate's default depth; dump has no such limit.
value = [[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]
puts JSON.dump(value).bytesize
puts JSON.dump(value, nil, 250).bytesize
stream = StringIO.new
puts JSON.dump(value, stream, 250, {strict: true}).string.bytesize
stream = StringIO.new
begin
  JSON.dump(value, stream, 100)
rescue ArgumentError => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
p stream.string
# hashes exceed generate's default depth; dump has no such limit.
value = {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => {"x" => 1}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}}
puts JSON.dump(value).bytesize
puts JSON.dump(value, nil, 250).bytesize
stream = StringIO.new
puts JSON.dump(value, stream, 250, {strict: true}).string.bytesize
stream = StringIO.new
begin
  JSON.dump(value, stream, 100)
rescue ArgumentError => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
p stream.string
# mixed exceed generate's default depth; dump has no such limit.
value = [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => [{"x" => 1}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]}]
puts JSON.dump(value).bytesize
puts JSON.dump(value, nil, 250).bytesize
stream = StringIO.new
puts JSON.dump(value, stream, 250, {strict: true}).string.bytesize
stream = StringIO.new
begin
  JSON.dump(value, stream, 100)
rescue ArgumentError => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
p stream.string
