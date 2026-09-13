# `Hash#to_query`'s nested grammar and ordering, for the ruby family: a
# Hash value renders as `outer[inner]=…`, an Array as repeated
# `key[]=…`, an empty container as nothing, and the strings at each
# Hash level are SORTED (Rails sorts them, except under an Array
# namespace) — exactly what `CgiIo.parse_form_into` reads back on this
# family's router, byte for byte what activesupport 8.1 writes. The
# shared `ActionView::ViewHelpers.to_query` (runtime/ruby) renders every
# value as a scalar in insertion order and says why the rest lives
# here: a poly walk over untyped values and an `Array#sort` are not
# shapes every strict target's emit answers, and the other families'
# routers do not read brackets anyway.
#
# Two methods reopened, the pair the shared file splits `to_query` into.
# Each value's rendering is ONE string, and `to_query_pairs` orders the
# strings at its level — so `{a: [2, 1]}` keeps its array order inside
# the one string and `{b: …, a: …}` puts `a` first, as Rails does.
#
# Required by BOTH boots (the spinel scaffold's and the CRuby overlay's)
# after the chain that defines the shared methods, and `walk_dir_flat`
# copies it into every tree under runtime/hash_to_query.rb. campfire's
# tests write the shape twice: `rooms_closed_url(room, params: { room: {
# name: … }, user_ids: [ … ] })` and `user_push_subscriptions_url(params:
# { push_subscription: { … } })`.
module ActionView
  module ViewHelpers
    def self.to_query_pairs(params, namespace)
      pairs = []
      params.each do |key, value|
        name = namespace.empty? ? key.to_s : "#{namespace}[#{key.to_s}]"
        pair = to_query_value(name, value)
        pairs << pair unless pair.empty?
      end
      pairs = pairs.sort unless namespace.include?("[]")
      pairs.join("&")
    end

    def self.to_query_value(name, value)
      case value
      when Hash
        to_query_pairs(value, name)
      when Array
        parts = []
        value.each { |v| parts << to_query_value("#{name}[]", v) }
        parts.join("&")
      when nil
        url_encode(name)
      else
        "#{url_encode(name)}=#{url_encode(value.to_s)}"
      end
    end
  end
end
