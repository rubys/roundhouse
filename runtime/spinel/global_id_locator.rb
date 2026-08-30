# `GlobalID::Locator.locate` — the READ side of the `gid://<app>/<Model>/<id>`
# identifier `GlobalID.param` mints in runtime/ruby/rails.rb.
#
# WHY IT IS HERE AND THE MINT IS THERE. The mint prices every target: a
# page renders `<turbo-cable-stream-source signed-stream-name="…">`
# whatever the runtime underneath it, so `GlobalID.param` transpiles
# eight ways. Locating prices only the lanes that have a subscribe path
# to run it on — a channel turning a stream name back into a record —
# and that is the spinel/ruby pair. Splitting them costs a drift risk
# (an encoder and a decoder in different files), which is why
# `tests/overlay_cable_dispatch.rb` round-trips `GlobalID.param`
# THROUGH this file rather than asserting either half against a literal.
#
# `only:` IS REQUIRED, AND IT IS THE FINDER. globalid 1.3.0 treats it as
# a filter and reflects the model name into a constant:
#
#   def locate(gid, options = {})
#     gid = GlobalID.parse(gid)
#     ... find_allowed?(gid.model_class, options[:only]) ... gid.find
#   end
#
# `gid.model_class` is `model_name.constantize`, and a constant computed
# from a wire string is the shape this pipeline will not emit — it is
# also the shape that lets a crafted name name any class in the process.
# So the caller's `only:` is what the record is found ON, and the model
# name in the URI is checked AGAINST it rather than resolved. campfire's
# one call site already passes it (`GlobalID::Locator.locate gid_param,
# only: Room`), so this is a narrowing of an API nobody used the wide
# half of, not a reinterpretation of the call.
#
# NIL for anything that is not one of OUR names — a truncated param, a
# different app's gid, a gid naming another model. `RecordNotFound` for
# a well-formed name whose record is gone, because that is `find`'s
# contract and campfire's `room_from` rescues exactly it.
require_relative "base64"

module GlobalID
  module Locator
    # `gid_param` is the urlsafe-base64 form (what a stream name carries);
    # `only` is the model class the caller will accept.
    #
    # THE GENERIC FORM, AND THE ONE A STRICT TARGET CANNOT RUN. `only`
    # arrives as a class OBJECT, so `only.name` and `only.find` need a
    # singleton to dispatch through; spinel has none and emits a call to
    # a class method `ActiveRecord::Base` never defines
    # (matz/spinel#4217). `lower::global_id_locate` rewrites every call
    # site whose `only:` is a literal class — which is all of them in
    # the corpus — to one of the generated `locate_<model>` entry points
    # below, so on a strict target nothing reaches this body and it is
    # not emitted. It stays for the Ruby lanes, where a class object
    # dispatches, and as the honest answer for a COMPUTED `only:`: that
    # call is left alone and refused at compile time rather than
    # rewritten to find on a class the caller did not name.
    def self.locate(gid_param, only:)
      parts = parts_from(gid_param)
      return nil if parts.nil?
      return nil unless parts[1] == only.name

      only.find(cast_id(parts[2]))
    end

    # One entry point per model class an `only:` names, with the finder
    # spelled as a literal constant. GENERATED —
    # `project::apply_global_id_locate` rewrites the span between the
    # markers from `App::global_id_locate_models`, the same
    # eager-arm shape `apply_cable_connection` uses for the connection
    # class. Empty for an app with no `locate` call site.
    # >>> generated: global-id-locate
    # <<< generated: global-id-locate

    # The `[app, model, id]` triple a well-formed gid carries, or nil
    # for anything that is not a name this app minted — a truncated
    # param, a different app's gid, a shape with the wrong arity.
    #
    # SHARED BY THE GENERIC FORM AND EVERY GENERATED ONE, so the two
    # cannot drift: a specialization differs from `locate` only in how
    # the finder is named, and that is the only line the generator
    # writes differently.
    def self.parts_from(gid_param)
      uri = decode(gid_param)
      return nil if uri.nil?

      # `gid://<app>/<Model>/<id>` — split on "/" after the scheme so an
      # id containing no slash is the last segment. Anything with a
      # different shape is not a name this app minted.
      rest = uri.start_with?("gid://") ? uri[6..] : nil
      return nil if rest.nil?

      parts = rest.split("/")
      return nil unless parts.length == 3
      return nil unless parts[0] == Rails.application.global_id_app

      parts
    end

    # A malformed param is a nil, not a raise: the caller is deciding
    # whether to authorize a subscription, and "this is not a name I
    # minted" is an ordinary answer to that question.
    def self.decode(gid_param)
      return nil if gid_param.nil?
      value = Base64.urlsafe_decode64(gid_param.to_s)
      value.empty? ? nil : value
    rescue ArgumentError
      nil
    end

    # The id travels as text and the column is typically an integer.
    # Rails hands `find` the string and lets the attribute type cast it;
    # there is no such cast here, so an all-digit id becomes an Integer
    # and anything else (a uuid pk) is passed through unchanged.
    def self.cast_id(text)
      return text if text.empty?
      i = 0
      while i < text.length
        c = text[i]
        return text if c < "0" || c > "9"
        i = i + 1
      end
      text.to_i
    end
  end
end
