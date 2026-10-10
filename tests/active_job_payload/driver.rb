# ActiveJob payloads, written and read by the runtime the drain runs:
# runtime/spinel/active_job_serialization.rb over the shared queue in
# runtime/ruby/active_job.rb.
#
# tests/active_job_payload.rs copies this file beside the runtime
# sources it requires, then runs it under CRuby and, compiled by spinel,
# as a binary. It prints `args <json>` and `job <json>` for the harness
# to hold against Rails' own output (rails.json, from oracle.rb), and one
# `ok`/`FAIL` line per read-side check. Both lanes must print the same
# thing. The values written are oracle.rb's, in the same order.
require "json"
require "securerandom"
require "time"
require_relative "base64"

# What the runtime reads off the app.
module Rails
  class App
    def global_id_app
      "campfire"
    end

    def secret_key_base
      ""
    end
  end

  def self.application
    App.new
  end
end

# global_id_locator.rb's signed reader names the verifier; nothing here
# calls it.
module ActionController
  class MessageVerifier
    def self.verified_data_json(secret, salt, data, purpose, rotate)
      ""
    end

    def self.json_value(json)
      ""
    end
  end
end

require_relative "active_job"
require_relative "global_id_locator"
require_relative "active_job_serialization"

# Two rows and an STI subclass, found the way a model's `find_by` is.
class Room
  def initialize(id)
    @id = id
  end

  def id
    @id
  end

  def kind
    "Room"
  end

  def self.find_by(id:)
    ROOMS[id]
  end
end

module Rooms
  class Open < Room
    def kind
      "Rooms::Open"
    end
  end
end

ROOMS = { 7 => Room.new(7), 8 => Rooms::Open.new(8) }

# The locator `project::apply_job_registry` generates for a `Room`
# parameter, written out by hand: the shape is the contract.
module ActiveJob
  module Arguments
    def self.locate_room_at(args, i)
      parts = gid_parts(args[i])
      name = parts[1]
      unless name == "Room" || name == "Rooms::Open"
        raise ActiveJob::DeserializationError, deserialize_message("unexpected model " + name)
      end
      record = Room.find_by(id: GlobalID::Locator.cast_id(parts[2]))
      raise ActiveJob::DeserializationError, missing_record_message(name, parts[2]) if record.nil?
      record
    end
  end
end

# A record's unencoded GlobalID, as `to_gid_uri` writes it
# (`GlobalID.uri` in runtime/ruby/rails.rb).
def gid(model_name, id)
  "gid://campfire/" + model_name + "/" + id.to_s
end

def check(name, ok, got)
  if ok
    puts "ok " + name
  else
    puts "FAIL " + name + ": " + got
  end
end

A = ActiveJob::Arguments
t = Time.at(1_791_548_096, 123_456_789, :nsec).utc
t2 = Time.at(1_791_548_096, 5, :nsec).utc

# ---- Write: oracle.rb's values, in its order ------------------------
parts = [
  A.null, A.bool(true), A.bool(false),
  A.int(42), A.int(-7), A.int(1_099_511_627_776),
  A.float(1.5), A.float(0.1), A.float(-2.5e-08), A.float(1.0e+20),
  A.str("plain"), A.str("quote\" backslash\\ nl\n tab\t ctrl\u0001 <&> é ✓  "), A.str(""),
  A.sym(:open),
  A.record(gid("Room", 7)), A.record(gid("Rooms::Open", 8)),
  A.time(t), A.time(t2),
  A.list([A.int(1), A.int(2), A.int(3)]), A.list([A.str("a"), A.str("b")]), A.list([]),
  A.list([A.bool(true), A.bool(false)]), A.list([A.sym(:x), A.sym(:y)])
]
args_json = A.list(parts)
puts "args " + args_json

payload = ActiveJob::Payload.build("PushMessageJob", "default", args_json)
job = JSON.parse(payload)
id = job["job_id"].to_s
check("job_id is a v4 uuid", id.length == 36 && id[14] == "4" && id[8] == "-", id)
enqueued = A.parse_time(job["enqueued_at"].to_s)
check("enqueued_at is now", (Time.now.utc - enqueued).abs < 60, job["enqueued_at"].to_s)
# The two fields that differ per enqueue, masked so both lanes print
# the same line.
at = payload.index("\"job_id\":\"").to_i + 10
payload = payload[0, at] + "<job_id>" + payload[at + 36, payload.length - at - 36]
at = payload.index("\"enqueued_at\":\"").to_i + 15
payload = payload[0, at] + "<enqueued_at>" + payload[at + 30, payload.length - at - 30]
puts "job " + payload

# ---- Read: every value back through its typed reader ----------------
args = JSON.parse(args_json)
check("count", A.count(args) == 23, A.count(args).to_s)
check("null", A.null_at(args, 0), "")
check("bool true", A.bool_at(args, 1) == true, "")
check("bool false", A.bool_at(args, 2) == false, "")
check("int", A.int_at(args, 3) == 42, A.int_at(args, 3).to_s)
check("negative int", A.int_at(args, 4) == -7, A.int_at(args, 4).to_s)
check("big int", A.int_at(args, 5) == 1_099_511_627_776, A.int_at(args, 5).to_s)
check("float", A.float_at(args, 6) == 1.5, A.float_at(args, 6).to_s)
check("float 0.1", A.float_at(args, 7) == 0.1, A.float_at(args, 7).to_s)
check("small float", A.float_at(args, 8) == -2.5e-08, A.float_at(args, 8).to_s)
check("large float", A.float_at(args, 9) == 1.0e+20, A.float_at(args, 9).to_s)
check("string", A.str_at(args, 10) == "plain", A.str_at(args, 10))
check("escaped string", A.str_at(args, 11) == "quote\" backslash\\ nl\n tab\t ctrl\u0001 <&> é ✓  ", A.str_at(args, 11))
check("empty string", A.str_at(args, 12) == "", A.str_at(args, 12))
check("symbol", A.sym_at(args, 13) == :open, A.sym_at(args, 13).to_s)
room = A.locate_room_at(args, 14)
check("record", room.id == 7 && room.kind == "Room", room.kind)
open = A.locate_room_at(args, 15)
check("STI record comes back as its subclass", open.id == 8 && open.kind == "Rooms::Open", open.kind)
check("time to the nanosecond", A.time_at(args, 16) == t, A.time_at(args, 16).iso8601(9))
check("time with 5ns", A.time_at(args, 17) == t2, A.time_at(args, 17).iso8601(9))
check("int array", A.int_array_at(args, 18) == [1, 2, 3], "")
check("string array", A.str_array_at(args, 19) == ["a", "b"], "")
check("empty array", A.int_array_at(args, 20).length == 0, "")
check("bool array", A.bool_array_at(args, 21) == [true, false], "")
check("symbol array", A.sym_array_at(args, 22) == [:x, :y], "")

# An offset time is the same instant, read back in UTC.
check("time with an offset", A.parse_time("2026-10-09T14:14:56.123456789+02:00") == t, "")
check("time without fractions", A.parse_time("2026-10-09T12:14:56Z") == Time.at(1_791_548_096, 0, :nsec).utc, "")

# ---- Read failures are DeserializationErrors ------------------------
def message_of(args, i)
  ActiveJob::Arguments.locate_room_at(args, i)
  "no error"
rescue ActiveJob::DeserializationError => e
  e.message
end

gone = JSON.parse(A.list([A.record(gid("Room", 404))]))
check("a deleted record raises",
  message_of(gone, 0) == "Error while trying to deserialize arguments: Couldn't find Room with 'id'=404",
  message_of(gone, 0))
other_app = JSON.parse("[{\"_aj_globalid\":\"gid://lobsters/Room/7\"}]")
check("another app's gid raises", message_of(other_app, 0).start_with?("Error while trying to deserialize arguments: not a GlobalID for this app"), message_of(other_app, 0))
other_model = JSON.parse(A.list([A.record(gid("User", 7))]))
check("another model raises", message_of(other_model, 0) == "Error while trying to deserialize arguments: unexpected model User", message_of(other_model, 0))
not_gid = JSON.parse("[7]")
check("a plain value where a record goes raises", message_of(not_gid, 0) != "no error", message_of(not_gid, 0))
sym_as_time = JSON.parse(A.list([A.sym(:open)]))
time_error = begin
  A.time_at(sym_as_time, 0)
  "no error"
rescue ActiveJob::DeserializationError => e
  e.message
end
check("the wrong serializer raises", time_error == "Error while trying to deserialize arguments: expected ActiveJob::Serializers::TimeSerializer", time_error)

nan_error = begin
  A.float(0.0 / 0.0)
  "no error"
rescue ArgumentError => e
  "raised"
end
check("NaN is refused at enqueue", nan_error == "raised", nan_error)

# ---- The queue: Procs and payloads in one FIFO ----------------------
TRACE = [""]
TRACE.clear

module ActiveJob
  # What the job registry does, cut down: run by class name, raise
  # DeserializationError for a gone record, answer false for a class it
  # does not know.
  def self.perform_payload(json)
    name = JSON.parse(json)["job_class"].to_s
    TRACE << "j:" + name
    raise ActiveJob::DeserializationError, "gone" if name == "GoneJob"
    name != "UnknownJob"
  end
end

ActiveJob.enqueue(-> { TRACE << "p1"; nil })
ActiveJob.enqueue_payload(ActiveJob::Payload.build("FirstJob", "default", "[]"))
ActiveJob.enqueue(-> { TRACE << "p2"; nil })
ActiveJob.enqueue_payload(ActiveJob::Payload.build("GoneJob", "default", "[]"))
ActiveJob.enqueue_payload(ActiveJob::Payload.build("UnknownJob", "default", "[]"))
ActiveJob.enqueue(-> { TRACE << "p3"; nil })
check("pending counts both kinds", ActiveJob.pending_count == 6, ActiveJob.pending_count.to_s)
ran = ActiveJob.drain
check("FIFO across both kinds", TRACE.join(",") == "p1,j:FirstJob,p2,j:GoneJob,j:UnknownJob,p3", TRACE.join(","))
# A payload is a Proc on the queue: one that raises is not a run, and
# one whose class the registry does not know still ran its Proc.
check("a raise is not a run", ran == 5, ran.to_s)
check("the queue is empty", ActiveJob.pending_count == 0, ActiveJob.pending_count.to_s)

puts "done"
