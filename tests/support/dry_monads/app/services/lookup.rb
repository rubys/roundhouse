class Lookup
  include Dry::Monads[:result, :try]

  def find(key)
    key == :a ? Success(1) : Failure(:missing)
  end
end
