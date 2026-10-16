class ChainContract < Dry::Validation::Contract
  Context = Dry::Schema.Params do
    optional(:n).value(:integer)
  end

  schema do
    required(:v)
    optional(:c).value(:hash).schema(Context)
    optional(:f).filled(:hash).hash do
      required(:k).filled(:string)
    end
  end
end
