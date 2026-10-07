# Compile the shared reason table without the interpreted socket harness.
require_relative "../runtime/spinel/tep/server"

[200, 201, 409, 422, 404, 500].each do |status|
  puts status.to_s + " " + Tep.reason(status)
end
