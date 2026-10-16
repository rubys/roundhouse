pub const DECLARATIONS: &str = r#"module FactoryExamples
  class First
    Result = Data.define(:name, :score, :enabled)
    Alias = Result
    ChainedAlias = Alias

    def self.build
      Result.new(name: "first", score: 0.8, enabled: false)
    end

    def self.aliased
      ChainedAlias.new(name: nil, score: 0.0, enabled: true)
    end

    def self.qualified
      FactoryExamples::First::Result.new("qualified", 1.0, false)
    end
  end

  class Second
    Result = Data.define(:name)

    def self.build
      Result.new(name: "second")
    end
  end

  class Empty
    Result = Data.define

    def self.build
      Result.new
    end
  end
end
"#;

pub const CUSTOM_DECLARATIONS: &str = r##"module FactoryExamples
  class Stateful
    PREFIX = "order"

    private

    def outer_private
      "hidden"
    end

    State = Data.define(:quantity, :enabled) do
      def initialize(quantity: 3, enabled: false)
        super(quantity: quantity + 1, enabled: enabled)
      end

      def label
        "#{PREFIX}:#{quantity}"
      end

      private

      def secret
        quantity
      end
    end

    NameFactory = Data.define(:name) do
      def self.name
        "factory class name"
      end
    end

    Alias = State
    FIRST = State.new

    public

    def self.build
      State.new(quantity: 6, enabled: true)
    end
  end

  class OtherStateful
    State = Data.define(:quantity) do
      def label
        quantity.to_s
      end
    end
  end

  Payment = ::Data.define(:authorized, :expired) do
    def initialize(authorized: false, expired: false) = super

    def release?
      authorized && expired
    end
  end
end
"##;

pub const CUSTOM_ASSERTIONS: &str = r#"
first = FactoryExamples::Stateful::FIRST
raise "default initializer changed" unless first.quantity == 4 && first.enabled == false
raise "lexical constant or method scope changed" unless first.label == "order:4"
raise "outer visibility leaked" unless first.respond_to?(:label)
raise "factory visibility leaked" if FactoryExamples::Stateful.new.respond_to?(:outer_private)
raise "private method became public" if first.respond_to?(:secret)
raise "private method disappeared" unless first.send(:secret) == 4
raise "alias changed identity" unless FactoryExamples::Stateful::Alias == first.class
raise "Data is mutable" unless first.frozen? && !first.respond_to?(:quantity=)
positional = FactoryExamples::Stateful::State.new(10, true)
raise "positional arguments bypassed keyword initialize" unless positional.quantity == 11 && positional.enabled == true
built = FactoryExamples::Stateful.build
raise "factory call lost keywords" unless built.quantity == 7 && built.enabled == true
changed = first.with(quantity: 8)
raise "with bypassed custom initialize" unless changed.quantity == 9 && changed.enabled == false
raise "with changed original or class" unless first.quantity == 4 && changed.class == first.class
other = FactoryExamples::OtherStateful::State.new(quantity: 17)
raise "same-named factories collided" unless other.label == "17" && other.class != first.class
payment = FactoryExamples::Payment.new
raise "bare super lost defaults" unless payment.authorized == false && payment.expired == false
raise "predicate lost state" if payment.with(authorized: true).release?
raise "predicate lost both flags" unless payment.with(authorized: true, expired: true).release?
puts "custom Data factory contract passed"
"#;
