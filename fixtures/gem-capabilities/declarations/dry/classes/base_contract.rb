class BaseContract < Dry::Validation::Contract
  params do
    required(:age).filled(:integer)
    required(:active).value(:bool)
    optional(:note).maybe(:string)
  end
end
