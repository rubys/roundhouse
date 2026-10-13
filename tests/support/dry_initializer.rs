//! `Dry::Initializer` classes, lowered at ingest (`ingest::dry_initializer`).
//! One contract for the interpreted and native lanes, checked against
//! dry-initializer 3.2 with dry-types 1.8.

// Each lane uses only its part.
#![allow(dead_code)]

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write(
            "lib/svc/types.rb",
            r##"module Svc
  module Types
    include Dry.Types()

    DEVICE = String.constrained(included_in: %w[desktop mobile])
  end
end
"##,
        )
        .write(
            "lib/svc/charge.rb",
            r##"module Svc
  class Base
    extend Dry::Initializer

    option :currency, ::Svc::Types::Strict::String, default: -> { default_currency }

    def self.call(**options)
      new(**options).call
    end

    private

    def default_currency
      "RUB"
    end
  end

  class Charge < Base
    param :order_id, ::Svc::Types::Coercible::Integer
    option :amount, ::Svc::Types::Coercible::Integer
    option :note, ::Svc::Types::Coercible::String.optional, optional: true
    option :paid, ::Svc::Types::Strict::Bool, default: -> { false }
    option :tags, ::Svc::Types::Array.of(::Svc::Types::Coercible::Symbol), default: -> { [] }
    option :title, type: proc { |value| value.to_s.upcase }, optional: true
    option :secret, ::Svc::Types::Strict::String, reader: :private, optional: true
    option :kind, as: :category, optional: true

    def call
      "#{order_id}:#{amount}:#{currency}"
    end

    def revealed
      secret
    end

    def note_given?
      @note != Dry::Initializer::UNDEFINED
    end
  end

  class Quote < Base
    option :amount, ::Svc::Types::Coercible::Integer

    def call
      "#{amount} #{currency}"
    end
  end

  class Refund < Charge
    option :reason, ::Svc::Types::Strict::String

    def initialize(order_id, **options)
      super
      @reason = "#{reason}!"
    end

    def call
      "refund #{reason}"
    end
  end
end
"##,
        )
        .write(
            "lib/svc/limits.rb",
            r##"module Svc
  STATES = { fresh: "new", done: "done" }.freeze

  class Limits < Base
    option :qty, ::Svc::Types::Params::Integer.constrained(gteq: 0, lteq: 10)
    option :code, ::Svc::Types::Coercible::String.constrained(format: /\A[a-z]+\z/, max_size: 5), optional: true
    option :name, ::Svc::Types::Coercible::String.constrained(filled: true), optional: true
    option :ratio, ::Svc::Types::Params::Float, optional: true
    option :tries, ::Svc::Types::Coercible::Integer.fallback(3), optional: true
    option :size, ::Svc::Types::Coercible::Integer.optional.fallback { |input| input.to_s.size }, optional: true
    option :ids, ::Svc::Types.Array(::Svc::Types::Coercible::Integer), optional: true
    option :span, ::Svc::Types::Range.optional, optional: true
    option :state, ::Svc::Types::Strict::String.enum(*STATES.values), optional: true
    option :flag, ::Svc::Types::Strict::Bool.optional.default(false), optional: true
    option :articles, ::Svc::Types::Array.of(::Article), default: -> { [] }
    option :device, ::Svc::Types::DEVICE, optional: true
  end

  class Paged < Base
    option :page, ::Svc::Types::Coercible::Integer
  end

  class FirstPage < Paged
    option :page, default: -> { 1 }
    option :currency, default: -> { "EUR" }
  end

  class Must < Base
    option :must, required: true, default: -> { 1 }, note: "ignored"
  end
end
"##,
        )
}

/// With a full forwarder anywhere in the app, `new(**options)` has to
/// prove the lowered `initialize`; Spinel refuses `...` itself. And
/// `Params::Time`, whose `Time.parse` Spinel's tree does not have.
pub fn ruby_overlay() -> super::emit_and_run::Overlay {
    overlay()
        .write(
            "lib/svc/wrapper.rb",
            "module Svc\n  class Wrapper\n    def initialize(...)\n      setup(...)\n    end\n\n    def setup(*args, **kwargs)\n      @args = args\n    end\n  end\nend\n",
        )
        .write(
            "lib/svc/clock.rb",
            "module Svc\n  class Clock < Base\n    option :since, ::Svc::Types::Params::Time.optional.fallback { |time| Time.at(time.to_i) }, optional: true\n  end\nend\n",
        )
}

pub const CLOCK_ASSERTIONS: &str = r##"
raise "time parsed" unless Svc::Clock.new(since: "2026-01-02 03:04:05").since == Time.parse("2026-01-02 03:04:05")
raise "time fallback" unless Svc::Clock.new(since: 1_700_000_000).since == Time.at(1_700_000_000)
puts "dry-initializer clock contract passed"
"##;

pub const ASSERTIONS: &str = r##"
c = Svc::Charge.new("7", amount: "12", note: 5, tags: ["a", :b], title: "x", secret: "s", kind: "k", stray: 1)
raise "param" unless c.order_id == 7
raise "coercible integer" unless c.amount == 12
raise "coercible optional" unless c.note == "5"
raise "default" unless c.paid == false && c.currency == "RUB"
raise "array of" unless c.tags == [:a, :b]
raise "proc type" unless c.title == "X"
raise "private reader" if c.respond_to?(:secret)
raise "private reader value" unless c.revealed == "s"
raise "as" unless c.category == "k"
d = Svc::Charge.new(1, amount: 2, currency: "USD", paid: true)
raise "optional nil" unless d.note.nil? && d.title.nil? && d.category.nil?
raise "left out" if d.note_given?
raise "given nil" unless Svc::Charge.new(1, amount: 1, note: nil).note_given?
raise "given over default" unless d.currency == "USD" && d.paid == true
raise "class call" unless Svc::Quote.call(amount: "3") == "3 RUB"
raise "subclass" unless Svc::Refund.new(1, amount: 1, reason: "r").call == "refund r!"
begin
  Svc::Charge.new(1)
  raise "missing option accepted"
rescue KeyError => e
  raise "message #{e.message}" unless e.message == "Svc::Charge: option 'amount' is required"
end
begin
  Svc::Charge.new(1, amount: "twelve")
  raise "coercible integer accepted a word"
rescue Dry::Types::CoercionError
end
begin
  Svc::Charge.new(1, amount: 1, paid: "yes")
  raise "strict bool accepted a string"
rescue Dry::Types::ConstraintError
end
begin
  Svc::Refund.new(1, amount: 1)
  raise "subclass option missing"
rescue KeyError
end

l = Svc::Limits.new(qty: "4", code: "abc", name: "n", ratio: "1.5", tries: "x", size: "abcd", ids: ["1", 2], span: (1..2), state: "done", flag: nil)
raise "constrained" unless l.qty == 4 && l.code == "abc" && l.name == "n"
raise "params float" unless l.ratio == 1.5
raise "fallback value" unless l.tries == 3
raise "fallback block" unless l.size == 4
raise "fallback skips nil" unless Svc::Limits.new(qty: 1, size: nil).size.nil?
raise "array()" unless l.ids == [1, 2]
raise "range" unless l.span == (1..2)
raise "enum splat" unless l.state == "done"
raise "bare name in Types" unless Svc::Limits.new(qty: 1, device: "mobile").device == "mobile"
raise "type default ignored" unless l.flag.nil?
raise "array of a class, empty" unless l.articles == []
begin
  Svc::Limits.new(qty: 1, articles: [Article.new])
  raise "array of a class took a member"
rescue NoMethodError
end
[
  [{ qty: 11 }, Dry::Types::ConstraintError],
  [{ qty: "-1" }, Dry::Types::ConstraintError],
  [{ qty: 1, code: "ABC" }, Dry::Types::ConstraintError],
  [{ qty: 1, code: "abcdef" }, Dry::Types::ConstraintError],
  [{ qty: 1, name: "" }, Dry::Types::ConstraintError],
  [{ qty: 1, ratio: "x" }, Dry::Types::CoercionError],
  [{ qty: 1, ids: "1" }, Dry::Types::ConstraintError],
  [{ qty: 1, span: 1 }, Dry::Types::ConstraintError],
  [{ qty: 1, state: "old" }, Dry::Types::ConstraintError],
  [{ qty: 1, device: "tv" }, Dry::Types::ConstraintError]
].each do |options, error|
  begin
    Svc::Limits.new(**options)
    raise "accepted #{options}"
  rescue error
  end
end
raise "redeclared default" unless Svc::FirstPage.new.page == 1 && Svc::FirstPage.new.currency == "EUR"
raise "redeclared drops the type" unless Svc::FirstPage.new(page: "3").page == "3"
begin
  Svc::Paged.new
  raise "parent changed by a redeclaring child"
rescue KeyError
end
begin
  Svc::Must.new
  raise "required: true kept the default"
rescue KeyError
end
raise "required given" unless Svc::Must.new(must: 2).must == 2

puts "dry-initializer contract passed"
"##;
