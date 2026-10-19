module Shop
  module Types
    include Dry.Types()

    Device = String.constrained(included_in: %w[desktop mobile])
    Client = Instance(::Shop::User) | Instance(::Shop::Guest)
  end
end
