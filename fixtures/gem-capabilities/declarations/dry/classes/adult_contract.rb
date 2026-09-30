require_relative "base_contract"

class AdultContract < BaseContract
  # dry-validation composes the superclass schema when the child declares params.
  params do
  end

  rule(:age) do
    key.failure("must be adult") if value < 18
  end
end
