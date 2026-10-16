class ParentContract < Dry::Validation::Contract
  params do
    required(:a).filled(:integer)
  end
end

class ChildContract < ParentContract
  params do
    required(:b).filled(:string)
  end
end
