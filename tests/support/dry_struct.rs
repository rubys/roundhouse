//! `Dry::Struct` classes, lowered at ingest (`ingest::dry_struct`). One
//! contract for the interpreted and native lanes.

// Each lane uses only its part.
#![allow(dead_code)]

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write(
            "lib/shop/types.rb",
            "module Shop\n  module Types\n    include Dry.Types()\n\n    LABEL = \"shop\"\n    TITLE = Types::LABEL\n\n    Loud = Types.Constructor(String) do |value|\n      next \"none\" if value.nil?\n\n      value.to_s.upcase\n    end\n  end\nend\n",
        )
        .write(
            "lib/shop/base_response.rb",
            r#"module Shop
  class BaseResponse < Dry::Struct
    transform_keys(&:to_sym)

    attribute? :response, ::Shop::Types::Hash
  end
end
"#,
        )
        .write(
            "lib/shop/refund.rb",
            r#"module Shop
  class Refund < BaseResponse
    attribute :id, ::Shop::Types::Coercible::String
    attribute :amount, ::Shop::Types::Coercible::Integer
    attribute :paid, ::Shop::Types::Strict::Bool
    attribute? :note, ::Shop::Types::Strict::String.optional
    attribute? :status, ::Shop::Types::Coercible::String.default("new")
    attribute? :tags, ::Shop::Types::Array.of(::Shop::Types::Coercible::String)
  end
end
"#,
        )
        .write(
            "lib/shop/money.rb",
            "module Shop\n  class Money\n    def initialize(cents)\n      @cents = cents\n    end\n\n    def cents\n      @cents\n    end\n  end\nend\n",
        )
        .write(
            "lib/shop/order.rb",
            r#"module Shop
  class Order < BaseResponse
    CURRENCY = "RUB"
    CODE = ::Shop::Types::Coercible::String
    Upcased = ::Shop::Types.Constructor(String) { |value| value.to_s.upcase }

    attribute :amount do
      attribute :value, ::Shop::Types::Coercible::String
    end
    attribute :items, ::Shop::Types::Array do
      attribute :sku, ::Shop::Types::Strict::String
    end
    attribute? :refund, Refund
    attribute? :price, ::Shop::Types.Instance(::Shop::Money)
    attribute? :kind, ::Shop::Types::Coercible::Symbol
    attribute? :meta, ::Shop::Types::Hash.default({}.freeze)
    attribute? :state, ::Shop::Types::Coercible::String.enum("open", "closed")
    attribute? :qty, ::Shop::Types::Params::Integer
    attribute? :gift, ::Shop::Types::Params::Bool
    attribute? :currency, ::Shop::Types::Coercible::String.default(CURRENCY)
    attribute? :rush, ::Shop::Types::Strict::Bool.default { false }
    attribute? :rows, ::Shop::Types::Array.of(
      ::Shop::Types::Hash.schema(amount: ::Shop::Types::Coercible::Float, note?: ::Shop::Types::Coercible::String)
    )
    attribute? :source, ::Shop::Types::Coercible::String.default("WEB").enum("WEB", "APP")
    attribute? :at, ::Shop::Types::Strict::Time
    attribute? :extra, ::Shop::Types::Coercible::Hash
    attribute? :lines, ::Shop::Types::Array.of(
      ::Shop::Types::Strict::Hash.schema(sku: ::Shop::Types::Strict::String.meta(omittable: true))
    )
    attribute? :token, ::Shop::Types::Coercible::String.default { Shop::Money.new(7).cents.to_s }
    attribute? :label, ::Shop::Types::String
    attribute? :codes, ::Shop::Types::Array.of(::Shop::Types::String)
    attribute? :anything, ::Shop::Types::Any
    attribute? :loose, ::Shop::Types::Nominal::String
    attribute? :shared, ::Shop::Types::Array.default([], shared: true)
    attribute? :computed, ::Shop::Types::Integer.default(::Shop::Money.new(3).cents)
    attribute? :plan, ::Shop::Types::Hash.schema(tier: ::Shop::Types::String.default("basic"))
    attribute? :payer, ::Shop::Kinds::PAYER
    attribute? :code, CODE
    attribute? :shout, Upcased
    attribute? :list, ::Shop::Types::Array.constructor { |value| Array(value) }

  end
end
"#,
        )
        .write(
            "lib/shop/address.rb",
            "module Shop\n  class Address < Dry::Struct\n    attribute :city, ::Shop::Types::String\n  end\nend\n",
        )
        .write(
            "lib/shop/delivery.rb",
            "module Shop\n  class Delivery < Dry::Struct\n    attribute :to do\n      attributes_from Address\n    end\n    attribute? :raw, ::Shop::Types::JSON::Hash\n    attribute :notes?, ::Shop::Types::Array\n    attribute? :volume, ::Shop::Types::Loud\n  end\n\n  class Parcel < Dry::Struct\n    attributes_from ::Shop::Delivery::To\n  end\nend\n",
        )
        .write(
            "lib/shop/tagged.rb",
            "module Shop\n  class Tagged < Dry::Struct\n    attribute :a, ::Shop::Types.Constructor(String) { |v| \"p#{v}\" }\n    attribute? :n, ::Shop::Types::Strict::Integer.constructor { |v|\n      next \"bad\" if v == :x\n\n      v\n    }\n  end\n\n  class TaggedChild < Tagged\n    attribute :b, ::Shop::Types.Constructor(String) { |v| \"c#{v}\" }\n  end\nend\n",
        )
        .write(
            "lib/shop/stamp.rb",
            "module Shop\n  class Stamp < Dry::Struct\n    attribute? :on, ::Shop::Types::Params::Date\n    attribute? :due, ::Shop::Types::Strict::Date\n    attribute? :price, ::Shop::Types::Coercible::Decimal\n  end\nend\n",
        )
        .write(
            "lib/shop/counted.rb",
            "module Shop\n  module Calls\n    def self.bump\n      @count = count + 1\n    end\n\n    def self.count\n      @count || 0\n    end\n  end\n\n  class Counted < Dry::Struct\n    attribute :pick, ::Shop::Types::String.constructor { |v|\n      ::Shop::Calls.bump\n      v.to_s\n    }.enum(\"a\", \"b\")\n  end\nend\n",
        )
        // An app's own `Types` module, which dry-types never sees.
        .write(
            "lib/billing.rb",
            "module Billing\n  module Types\n    PLAN = \"gold\"\n  end\n\n  module Usage\n    CHOSEN = Types::PLAN\n  end\nend\n",
        )
        .write(
            "lib/shop/kinds.rb",
            "module Shop\n  module Kinds\n    PAYER = ::Shop::Types.Instance(::Shop::Money) | ::Shop::Types.Instance(::Shop::Refund)\n    LABEL = ::Shop::Types::TITLE\n  end\nend\n",
        )
        .write(
            "lib/shop/client.rb",
            r#"module Shop
  class Client
    def refund(payload)
      Refund.new(**payload)
    end

    def qualified_refund(payload)
      ::Shop::Refund.new(**payload)
    end

    def parse(body)
      Refund.new(body)
    end

    def order(body)
      Order.new(body)
    end
  end
end
"#,
        )
}

/// What only the interpreted lane runs. With a full forwarder anywhere in
/// the app, every `X.new(**h)` has to prove its `initialize`: the
/// structs' must be found, `::` spelling included; Spinel refuses `...`
/// itself. And the coercions needing stdlib Spinel's tree lacks.
pub fn ruby_overlay() -> super::emit_and_run::Overlay {
    clock_overlay(overlay()).write(
        "lib/shop/wrapper.rb",
        "module Shop\n  class Wrapper\n    def initialize(...)\n      setup(...)\n    end\n\n    def setup(*args, **kwargs)\n      @args = args\n    end\n  end\nend\n",
    )
}

/// `DateTime`, `Time.parse` and `to_d`'s loose String parsing: CRuby's
/// and JRuby's stdlib only.
pub fn clock_overlay(base: super::emit_and_run::Overlay) -> super::emit_and_run::Overlay {
    base.write(
        "lib/shop/clock.rb",
        r#"module Shop
  class Clock < Dry::Struct
    attribute? :at, ::Shop::Types::JSON::DateTime
    attribute? :seen, ::Shop::Types::Params::Time.optional
    attribute? :fee, ::Shop::Types::Params::Decimal
    attribute? :stamped, ::Shop::Types::Strict::DateTime
  end
end
"#,
    )
}

pub const CLOCK_ASSERTIONS: &str = r#"
c = Shop::Clock.new(at: "2026-01-02T10:00:00+03:00", seen: nil, fee: "2.5")
raise "date time" unless c.at.hour == 10 && c.at.is_a?(DateTime)
raise "optional time" unless c.seen.nil?
raise "params decimal" unless c.fee == BigDecimal("2.5")
raise "strict date time" unless Shop::Clock.new(stamped: c.at).stamped == c.at
[{ at: "nope" }, { fee: "x" }, { stamped: "2026-01-02" }].each do |bad|
  begin
    Shop::Clock.new(bad)
    raise "clock accepted #{bad.inspect}"
  rescue Dry::Struct::Error
  end
end
puts "dry-struct clock contract passed"
"#;

/// Spinel's tree has `Date` and `BigDecimal()`: both lanes run these.
pub const STAMP_ASSERTIONS: &str = r#"
s = Shop::Stamp.new(on: "2026-01-02", price: "1.25")
raise "date" unless s.on == Date.new(2026, 1, 2)
raise "date passes through" unless Shop::Stamp.new(on: Date.new(2020, 5, 6)).on.month == 5
raise "strict date" unless Shop::Stamp.new(due: s.on).due == s.on
raise "coercible decimal" unless s.price == BigDecimal("1.25")
[{ on: "nope" }, { on: 5 }, { due: "2026-01-02" }, { price: "x" }].each do |bad|
  begin
    Shop::Stamp.new(bad)
    raise "stamp accepted #{bad.inspect}"
  rescue Dry::Struct::Error
  end
end
puts "dry-struct stamp contract passed"
"#;

pub const ASSERTIONS: &str = r#"
client = Shop::Client.new
r = client.refund(id: 7, amount: "12", paid: true, tags: [1, :b])
raise "coercible string" unless r.id == "7"
raise "qualified" unless client.qualified_refund(id: 1, amount: 2, paid: false).amount == 2
raise "coercible integer" unless r.amount == 12
raise "strict bool" unless r.paid == true
raise "omitted optional" unless r.note.nil?
raise "default" unless r.status == "new"
raise "array of" unless r.tags == ["1", "b"]
raise "inherited omitted" unless r.response.nil?
s = client.parse({ "id" => "x", "amount" => 3, "paid" => false, "note" => nil, "response" => { "a" => 1 } })
raise "string keys" unless s.id == "x" && s.amount == 3 && s.paid == false
raise "explicit nil" unless s.note.nil?
raise "inherited" unless s.response == { "a" => 1 }
begin
  client.refund(amount: 1, paid: true)
  raise "missing key accepted"
rescue Dry::Struct::Error
end
begin
  client.refund(id: 1, amount: 1, paid: "yes")
  raise "strict bool accepted a string"
rescue Dry::Struct::Error
end
begin
  client.refund(id: 1, amount: "twelve", paid: true)
  raise "coercible integer accepted a word"
rescue Dry::Struct::Error
end
o = client.order({ "amount" => { "value" => 5 }, "items" => [{ "sku" => "a" }, { "sku" => "b" }],
                   "refund" => r, "price" => Shop::Money.new(9), "kind" => "fast" })
raise "nested" unless o.amount.is_a?(Shop::Order::Amount) && o.amount.value == "5"
raise "array of nested" unless o.items.map(&:sku) == ["a", "b"] && o.items.first.is_a?(Shop::Order::Item)
raise "struct instance passes" unless o.refund.equal?(r)
raise "instance" unless o.price.cents == 9
raise "symbol" unless o.kind == :fast
raise "frozen default" unless o.meta == {}
raise "constant default" unless o.currency == "RUB"
raise "block default" unless o.rush == false
raise "default then enum" unless o.source == "WEB"
raise "computed default" unless o.token == "7"
# Under `Dry.Types()` a bare name is strict; `Any` and `Nominal::` are not.
base = { amount: { value: 1 }, items: [] }
raise "bare strict ok" unless client.order(base.merge(label: "x", codes: ["a"])).label == "x"
raise "any" unless client.order(base.merge(anything: 5)).anything == 5
raise "nominal" unless client.order(base.merge(loose: 5)).loose == 5
[{ label: 1 }, { codes: [1] }, { codes: "a" }].each do |bad|
  begin
    client.order(base.merge(bad))
    raise "bare name accepted #{bad.inspect}"
  rescue Dry::Struct::Error
  end
end
now = Time.now
t = client.order({ amount: { value: 1 }, items: [], at: now, extra: nil, lines: [{}, { sku: "s" }] })
raise "strict time" unless t.at == now
raise "coercible hash" unless t.extra == {}
raise "meta omittable" unless t.lines == [{}, { sku: "s" }]
begin
  client.order({ amount: { value: 1 }, items: [], at: "2026-01-01" })
  raise "strict time accepted a string"
rescue Dry::Struct::Error
end
rows = client.order({ amount: { value: 1 }, items: [], rows: [{ amount: "1.5", extra: 1 }, { amount: 2, note: 5 }] }).rows
raise "hash schema #{rows.inspect}" unless rows == [{ amount: 1.5 }, { amount: 2.0, note: "5" }]
begin
  client.order({ amount: { value: 1 }, items: [], rows: [{ "amount" => 2 }] })
  raise "hash schema took a string key"
rescue Dry::Struct::Error
end
p2 = client.order({ amount: { value: 1 }, items: [], state: :open, qty: "042", gift: "yes" })
raise "enum" unless p2.state == "open"
raise "params integer" unless p2.qty == 42
raise "params bool" unless p2.gift == true && client.order({ amount: { value: 1 }, items: [], gift: "0" }).gift == false
begin
  client.order({ amount: { value: 1 }, items: [], state: "lost" })
  raise "enum accepted an outsider"
rescue Dry::Struct::Error
end
begin
  client.order({ amount: { value: 1 }, items: [], gift: "maybe" })
  raise "params bool accepted maybe"
rescue Dry::Struct::Error
end
built = client.order({ amount: { value: 1 }, items: [], refund: { id: 3, amount: 4, paid: true } })
raise "struct from hash" unless built.refund.is_a?(Shop::Refund) && built.refund.amount == 4
begin
  client.order({ amount: 5, items: [] })
  raise "nested accepted a number"
rescue Dry::Struct::Error
end
begin
  client.order({ amount: { value: 1 }, items: [], price: 9 })
  raise "instance accepted a number"
rescue Dry::Struct::Error
end
a1 = client.order(base)
a2 = client.order(base)
raise "shared default" unless a1.shared == [] && a1.shared.equal?(a2.shared)
raise "computed default" unless a1.computed == 3
raise "schema key default" unless client.order(base.merge(plan: {})).plan == { tier: "basic" }
raise "sum left" unless client.order(base.merge(payer: Shop::Money.new(1))).payer.cents == 1
raise "sum right" unless client.order(base.merge(payer: r)).payer.id == "7"
begin
  client.order(base.merge(payer: "nobody"))
  raise "sum accepted a string"
rescue Dry::Struct::Error
end
raise "constant type" unless client.order(base.merge(code: 5)).code == "5"
raise "constructor" unless client.order(base.merge(shout: :hi)).shout == "HI"
raise "array constructor" unless client.order(base.merge(list: "x")).list == ["x"]
d = Shop::Delivery.new(to: { city: "Omsk" }, raw: { "a" => 1 }, volume: :hi)
raise "attributes_from" unless d.to.city == "Omsk" && d.to.is_a?(Shop::Delivery::To)
raise "json hash" unless d.raw == { "a" => 1 }
raise "chained attributes_from" unless Shop::Parcel.new(city: "Kazan").city == "Kazan"
begin
  Shop::Parcel.new({})
  raise "chained attributes_from lost a required key"
rescue Dry::Struct::Error
end
raise "a Types module's value constant was dropped" unless Shop::Kinds::LABEL == "shop"
raise "name? is omittable" unless d.notes.nil? && Shop::Delivery.new(to: { city: "x" }, notes: [1]).notes == [1]
raise "types constant constructor" unless d.volume == "HI"
raise "constructor next" unless Shop::Delivery.new(to: { city: "x" }, volume: nil).volume == "none"
begin
  Shop::Delivery.new(to: { city: "x" }, raw: 1)
  raise "json hash accepted a number"
rescue Dry::Struct::Error
end
begin
  Shop::Delivery.new(to: {})
  raise "copied required attribute missing accepted"
rescue Dry::Struct::Error
end
tc = Shop::TaggedChild.new(a: 1, b: 2)
raise "parent constructor shadowed: #{tc.a}" unless tc.a == "p1" && tc.b == "c2"
raise "constructor value" unless Shop::Tagged.new(a: 1, n: 4).n == 4
begin
  Shop::Tagged.new(a: 1, n: :x)
  raise "next skipped the type check"
rescue Dry::Struct::Error
end
before = Shop::Calls.count
raise "enum" unless Shop::Counted.new(pick: :a).pick == "a"
raise "enum coerced #{Shop::Calls.count - before} times" unless Shop::Calls.count - before == 1
begin
  Shop::Counted.new(pick: "z")
  raise "enum accepted z"
rescue Dry::Struct::Error
end
raise "an app's own Types constant was dropped" unless Billing::Usage::CHOSEN == "gold"
puts "dry-struct contract passed"
"#;
