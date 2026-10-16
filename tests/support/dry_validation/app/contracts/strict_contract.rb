class StrictContract < Dry::Validation::Contract
  schema do
    required(:a).filled(:integer)
    optional(:b).maybe(:string)
  end
end
