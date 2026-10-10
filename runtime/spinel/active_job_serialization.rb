# ActiveJob payloads on the Ruby-family lanes: job arguments written in
# ActiveJob's wire format, and read back when the drain runs the job.
#
# THE FORMAT IS RAILS', key for key. `ActiveJob::Arguments.serialize`
# writes a record as `{"_aj_globalid":"gid://app/Model/1"}`, a Symbol
# or a Time as `{"_aj_serialized":"<serializer>","value":...}`, and the
# JSON primitives as themselves; `Payload.build` writes the hash
# `ActiveJob::Core#serialize` returns (activejob 8.1.3). That is the
# shape Rails hands Sidekiq, so a payload built here is one a Rails
# worker could run. `tests/active_job_rails_conformance.rs` holds both
# halves to Rails' own output.
#
# WRITERS ANSWER JSON TEXT, NOT VALUES. A job's arguments are
# heterogeneous and a strict target has one element type per Array, so
# the enqueue side never builds an Array of arguments: the lowering
# (`lower::job_payload`) writes one helper call per parameter, each
# answering that argument's JSON, and joins them. The read side gets the
# parsed `arguments` array and one typed `*_at(args, i)` per parameter.
# Parsed JSON crosses method boundaries on spinel, both ways (probed
# before this was written; Switchyard spikes/t0).
#
# WHY spinel/ AND NOT ruby/. The readers take `JSON.parse` output, which
# is untyped by nature, and runtime/ruby/ requires every body to be
# fully typed (invariant 3). Only the Ruby-family lanes have a drain to
# run a payload on, so this is where it belongs anyway — the same split
# as GlobalID's mint (ruby/) and locator (spinel/).
module ActiveJob
  module Arguments
    GLOBALID_KEY = "_aj_globalid"
    SERIALIZER_KEY = "_aj_serialized"
    SYMBOL_SERIALIZER = "ActiveJob::Serializers::SymbolSerializer"
    TIME_SERIALIZER = "ActiveJob::Serializers::TimeSerializer"

    # ---- Write side: one argument as JSON text ------------------------

    def self.null
      "null"
    end

    def self.bool(value)
      value ? "true" : "false"
    end

    def self.int(value)
      value.to_s
    end

    # JSON has no NaN or Infinity; Sidekiq's `JSON.generate` refuses
    # them at enqueue, and so does this, rather than queue text the
    # drain cannot parse.
    def self.float(value)
      raise ArgumentError, "#{value} is not valid JSON" unless value.finite?
      value.to_s
    end

    # A String as Ruby's `JSON.generate` writes it: `"` and `\` escaped,
    # control characters as `\b \f \n \r \t` or `\u00xx`, everything
    # else as its UTF-8 bytes. Not `MessageVerifier.json_string`: that
    # one also escapes `<`, `>` and `&`, as ActiveSupport's JSON does,
    # and a job payload is written by the adapter's plain generator.
    # Byte-wise for the reason json_string gives: CRuby never meets an
    # encoding clash and spinel sees the same bytes.
    def self.str(value)
      s = value.to_s
      out = +"\""
      i = 0
      n = s.bytesize
      while i < n
        b = s.getbyte(i)
        if b == 34
          out << "\\\""
        elsif b == 92
          out << "\\\\"
        elsif b == 10
          out << "\\n"
        elsif b == 13
          out << "\\r"
        elsif b == 9
          out << "\\t"
        elsif b == 8
          out << "\\b"
        elsif b == 12
          out << "\\f"
        elsif b < 32
          out << "\\u" + format("%04x", b)
        else
          out << b.chr
        end
        i += 1
      end
      out << "\""
      out.force_encoding("UTF-8")
    end

    def self.sym(value)
      "{\"" + SERIALIZER_KEY + "\":\"" + SYMBOL_SERIALIZER + "\",\"value\":" + str(value.to_s) + "}"
    end

    # `gid_uri` is the record's unencoded `gid://<app>/<Model>/<id>`,
    # from the per-model `to_gid_uri` the lowering adds beside
    # `to_gid_param`, so an STI subclass writes its own name, as in Rails.
    def self.record(gid_uri)
      "{\"" + GLOBALID_KEY + "\":" + str(gid_uri) + "}"
    end

    # Rails' TimeSerializer: `time.iso8601(9)`, in the time's own offset.
    def self.time(value)
      "{\"" + SERIALIZER_KEY + "\":\"" + TIME_SERIALIZER + "\",\"value\":" + str(value.iso8601(9)) + "}"
    end

    # An Array argument, or the whole argument list: the element JSON
    # each helper above wrote, joined.
    def self.list(parts)
      "[" + parts.join(",") + "]"
    end

    # ---- Read side: one argument out of the parsed array --------------

    def self.count(args)
      args.length
    end

    def self.null_at(args, i)
      args[i].nil?
    end

    def self.bool_at(args, i)
      args[i] == true
    end

    def self.int_at(args, i)
      args[i].to_i
    end

    def self.float_at(args, i)
      args[i].to_f
    end

    def self.str_at(args, i)
      args[i].to_s
    end

    def self.sym_at(args, i)
      serialized_value(args[i], SYMBOL_SERIALIZER).to_sym
    end

    def self.time_at(args, i)
      parse_time(serialized_value(args[i], TIME_SERIALIZER))
    end

    # Element readers for an Array argument. Each builds its result from
    # a typed empty Array (`[0]` then `clear`), the form spinel infers an
    # element type from.
    def self.int_array_at(args, i)
      out = [0]
      out.clear
      args[i].each { |v| out << v.to_i }
      out
    end

    def self.float_array_at(args, i)
      out = [0.0]
      out.clear
      args[i].each { |v| out << v.to_f }
      out
    end

    def self.str_array_at(args, i)
      out = [""]
      out.clear
      args[i].each { |v| out << v.to_s }
      out
    end

    def self.bool_array_at(args, i)
      out = [true]
      out.clear
      args[i].each { |v| out << (v == true) }
      out
    end

    def self.sym_array_at(args, i)
      out = [:a]
      out.clear
      args[i].each { |v| out << serialized_value(v, SYMBOL_SERIALIZER).to_sym }
      out
    end

    # `[app, model, id]` from one `{"_aj_globalid": ...}` argument.
    # Anything else (a missing key, another app's gid, a malformed URI)
    # is a DeserializationError, which is what Rails raises when
    # `GlobalID::Locator.locate` fails.
    def self.gid_parts(value)
      uri = key_of(value, GLOBALID_KEY)
      raise ActiveJob::DeserializationError, deserialize_message("not a GlobalID argument") if uri.nil?
      parts = GlobalID::Locator.parts_from_uri(uri.to_s)
      raise ActiveJob::DeserializationError, deserialize_message("not a GlobalID for this app: " + uri.to_s) if parts.nil?
      parts
    end

    # The message Rails' DeserializationError carries for a record that
    # is gone: `Error while trying to deserialize arguments: ` and then
    # `find`'s RecordNotFound text.
    def self.missing_record_message(model_name, id)
      deserialize_message("Couldn't find " + model_name + " with 'id'=" + id)
    end

    def self.deserialize_message(detail)
      "Error while trying to deserialize arguments: " + detail
    end

    # One record locator per model a job parameter names, GENERATED —
    # `project::apply_job_registry` rewrites the span between the
    # markers from `App::job_plans`. Each takes the declared model or
    # any of its STI subclasses (their names are literals here, never
    # resolved from the wire), finds on the declared model, and raises
    # DeserializationError when the row is gone:
    #
    #   def self.locate_room_at(args, i)
    #     parts = gid_parts(args[i])
    #     name = parts[1]
    #     unless name == "Room" || name == "Rooms::Open"
    #       raise ActiveJob::DeserializationError, deserialize_message("unexpected model " + name)
    #     end
    #     record = Room.find_by(id: GlobalID::Locator.cast_id(parts[2]))
    #     raise ActiveJob::DeserializationError, missing_record_message(name, parts[2]) if record.nil?
    #     record
    #   end
    #
    # Empty for an app with no payload jobs.
    # >>> generated: job-locators
    # <<< generated: job-locators

    # `{"_aj_serialized": serializer, "value": v}` -> v, checking the
    # serializer is the one the parameter's type expects.
    def self.serialized_value(value, serializer)
      raise ActiveJob::DeserializationError, deserialize_message("expected " + serializer) unless key_of(value, SERIALIZER_KEY) == serializer
      value["value"].to_s
    end

    # `value[key]` for a parsed JSON object, nil for any other JSON
    # value: on CRuby, `7["_aj_globalid"]` is a TypeError, and a payload
    # with a number where a record goes should read as a bad argument.
    def self.key_of(value, key)
      value[key]
    rescue StandardError
      nil
    end

    # `YYYY-MM-DDTHH:MM:SS[.fffffffff](Z|+HH:MM|-HH:MM)` -> Time, in UTC.
    # Hand-written because spinel has no `Time.iso8601` (it compiles and
    # raises NoMethodError). The instant is exact to the nanosecond: whole
    # seconds come from `Time.utc`, and nanoseconds go in through
    # `Time.at(..., :nsec)`, since adding a Rational loses one on spinel.
    # Rails answers the time in the offset it was written with; this one
    # answers the same instant in UTC.
    def self.parse_time(text)
      raise ActiveJob::DeserializationError, deserialize_message("bad time " + text) if text.length < 20
      year = text[0, 4].to_i
      mon = text[5, 2].to_i
      day = text[8, 2].to_i
      hour = text[11, 2].to_i
      min = text[14, 2].to_i
      sec = text[17, 2].to_i
      pos = 19
      nsec = 0
      if text[pos] == "."
        pos += 1
        digits = +""
        while pos < text.length && text[pos] >= "0" && text[pos] <= "9"
          digits << text[pos]
          pos += 1
        end
        nsec = (digits + "000000000")[0, 9].to_i
      end
      offset = 0
      zone = text[pos, text.length - pos]
      if zone != "Z"
        raise ActiveJob::DeserializationError, deserialize_message("bad time " + text) unless zone.length == 6
        offset = zone[1, 2].to_i * 3600 + zone[4, 2].to_i * 60
        offset = -offset if zone[0] == "-"
      end
      Time.at(Time.utc(year, mon, day, hour, min, sec).to_i - offset, nsec, :nsec).utc
    end
  end

  module Payload
    # The `job.serialize` hash, as JSON, for one enqueue. Keys in Rails'
    # order. `arguments_json` is `Arguments.list` of the job's arguments.
    #
    # The fixed values are the ones a stock Rails app writes: no
    # provider id until an adapter assigns one, no priority, a first
    # execution, locale `en` and time zone `UTC` (Rails' defaults; an
    # app that changes either is a known gap). `job_id` is a fresh v4
    # UUID, as `SecureRandom.uuid` gives Rails.
    def self.build(job_class, queue_name, arguments_json)
      "{\"job_class\":" + Arguments.str(job_class) +
        ",\"job_id\":" + Arguments.str(SecureRandom.uuid) +
        ",\"provider_job_id\":null" +
        ",\"queue_name\":" + Arguments.str(queue_name) +
        ",\"priority\":null" +
        ",\"arguments\":" + arguments_json +
        ",\"executions\":0" +
        ",\"exception_executions\":{}" +
        ",\"locale\":\"en\"" +
        ",\"timezone\":\"UTC\"" +
        ",\"enqueued_at\":" + Arguments.str(Time.now.utc.iso8601(9)) +
        ",\"scheduled_at\":null}"
    end
  end
end
