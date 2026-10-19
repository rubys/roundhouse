class AddressContract < Dry::Validation::Contract
  params do
    required(:city).filled(:string)
  end
end
