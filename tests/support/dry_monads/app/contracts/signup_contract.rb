class SignupContract < Dry::Validation::Contract
  Dry::Validation.load_extensions(:monads)

  params do
    required(:name).filled(:string)
  end
end
