class UserContract < Dry::Validation::Contract
  KINDS = %w[admin guest].freeze

  params do
    required(:name).filled(:string)
    required(:age).value(:integer, gt?: 0)
    optional(:nick).maybe(:string, min_size?: 2, max_size?: 5)
    optional(:ok).value(:bool)
    optional(:ids).array(:integer)
    optional(:meta).value(type?: Hash)
    optional(:addr).hash do
      required(:city).filled(:string)
      optional(:zip).maybe(:integer)
    end
    optional(:tags).array(:hash) do
      required(:t).filled(:string)
    end
    optional(:score).maybe(:float)
    optional(:price).filled(:decimal)
    optional(:born).maybe(:date)
    optional(:kind).value(:string, included_in?: KINDS)
    optional(:email).maybe(:string, format?: /@/)
    optional(:count).maybe(:number?)
    optional(:pair).value(:array, size?: 2)
    optional(:level).filled(:integer, gteq?: 1, lt?: 10)
    optional(:range).value(:integer, included_in?: 1..3)
  end
end
