class RulesContract < Dry::Validation::Contract
  LIMIT = 3

  params do
    required(:name).filled(:string)
    optional(:nick).maybe(:string)
    optional(:a).value(:integer)
    optional(:b).value(:integer)
    optional(:addr).hash do
      optional(:city).filled(:string)
    end
  end

  rule(:name) do
    key.failure("too short") if value.length < 2
    key.failure("bad char") unless value.match?(/\A[a-z]+\z/)
  end

  rule(:nick) do
    next if value.nil?

    key.failure("nick long") if value.size > LIMIT
    key.failure('nick #@x #$y #{z}') if value == "hsh"
  end

  rule(:a, :b) do
    key(:b).failure("b must exceed a") if key?(:b) && values[:a] && values[:b] <= values[:a]
    base.failure("both zero") if values[:a] == 0 && values[:b] == 0
  end

  rule(addr: :city) do
    key.failure("no Moscow") if key? && forbidden?(value)
  end

  rule(:a) do
    key.failure("a schema err") if schema_error?(:name)
  end

  def forbidden?(city)
    city == "Moscow"
  end
end
