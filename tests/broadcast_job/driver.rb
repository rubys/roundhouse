# Writes the ActionBroadcastJob arguments for the oracle's two
# broadcasts through `ActiveJob::Arguments.turbo_broadcast_options`,
# compares them as JSON text (key order included) with what turbo-rails
# enqueued, and reads them back with the option readers the worker uses.
require "json"
module ActiveJob
  class DeserializationError < StandardError; end
end
require_relative "active_job_serialization"

A = ActiveJob::Arguments
oracle = JSON.parse(File.read(ARGV[0]))
gid = "gid://caboose/Car/1"
ours = {
  "create" => A.list([A.str("cars"), A.turbo_broadcast_options("prepend", "cars", "cars/car", "car", gid)]),
  "update" => A.list([A.str("cars"), A.turbo_broadcast_options("replace", "car_1", "cars/car", "car", gid)])
}
ours.each do |name, text|
  want = JSON.generate(oracle[name])
  puts(text == want ? "match #{name}" : "differ #{name}\n  ours:  #{text}\n  rails: #{want}")
  args = JSON.parse(text)
  puts [
    "read #{name}", A.str_at(args, 0), A.option_sym_name_at(args, 1, "action"),
    A.option_str_at(args, 1, "target"), A.option_str_at(args, 1, "partial"),
    A.key_of(A.option_local_at(args, 1, "car"), "_aj_globalid")
  ].join(" ")
end
puts "done"
