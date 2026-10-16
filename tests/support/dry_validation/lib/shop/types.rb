module Shop
  module Types
    include Dry.Types()

    Device = String.constrained(included_in: %w[desktop mobile])
  end
end
