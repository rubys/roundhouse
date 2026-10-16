module Standalone
  Item = Dry::Schema.Params do
    required(:sku).filled(:string)
    optional(:qty).value(:integer, gt?: 0)
  end
end
