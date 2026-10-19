class TagsBaseContract < Dry::Validation::Contract
  TagSchema = Dry::Schema.Params do
    required(:title).filled(:string)
  end
end

class ShapesContract < TagsBaseContract
  MIN = 2

  params do
    optional(:tags).array(TagSchema)
    optional(:code).value(::Shop::Types::Params::String)
    optional(:nick).value(:string) { min_size?(MIN) }
    optional(:owner).value(::Shop::Types::Client)
    optional(:address).filled(AddressContract.schema)
  end
end
