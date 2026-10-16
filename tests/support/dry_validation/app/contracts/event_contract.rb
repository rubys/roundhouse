class EventContract < Dry::Validation::Contract
  IndexSchema = Dry::Schema.Params do
    optional(:fields).array(:string)
    optional(:object_ids).array(:integer)
  end

  json do
    required(:id).filled(:integer)
    optional(:on).value(:date)
    optional(:ratio).maybe(:float)
    optional(:amount).maybe(:decimal)
    required(:index).hash(IndexSchema)
  end
end
