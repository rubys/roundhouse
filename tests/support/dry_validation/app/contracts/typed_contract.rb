class TypedContract < Dry::Validation::Contract
  Line = Dry::Schema.Params do
    required(:sku).filled(:string)
  end

  Order = Dry::Schema.Params do
    required(:lines).array(Line)
    optional(:note).maybe(:string)
  end

  params do
    optional(:n) { number? }
    optional(:i) { int? & gt?(0) }
    optional(:on).value(::Shop::Types::Params::Date)
    optional(:qty).filled(::Shop::Types::Coercible::Integer)
    optional(:ratio).maybe(::Shop::Types::Coercible::Float)
    optional(:label).value(::Shop::Types::Coercible::String)
    optional(:device).filled(::Shop::Types::Device)
    optional(:state).value(::Shop::Types::Strict::String.enum("new", "done"))
    required(:flag).filled(Dry::Types["strict.bool"].default(false))
    optional(:at).value(::Shop::Types::Strict::String.optional)
    optional(:items).value(:array, min_size?: 1).each do
      hash do
        required(:id).filled(:integer)
      end
    end
    optional(:pairs).value(:array).each do
      value(:array, size?: 2)
    end
    optional(:codes).value(:array).each(:string)
    optional(:order).hash(Order)
  end
end
