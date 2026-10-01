class SignupContract < Dry::Validation::Contract
  params do
    required(:age).filled(:integer)
  end

  rule(:age) do
    key.failure("must be adult") if value < 18
  end
end
