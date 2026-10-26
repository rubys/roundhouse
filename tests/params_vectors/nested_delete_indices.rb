class ItemsController < ActionController::API
  def create
    head :ok
  end

  private

  def indices
    items = item_params.delete(:items)
    labels = item_params.delete(:labels)
    removed = item_params.delete(:settings)
    index = 0
    string_key = "value"
    symbol_key = :value
    [item_params.delete(:items)[0], items[0], items[-1], items[0..0], items[index],
     labels[0], removed[0], removed[index], removed[nil], removed[false],
     removed[:value], removed["value"], removed[string_key], removed[symbol_key]]
  end

  def unknown_index(key)
    removed = item_params.delete(:settings)
    removed[key]
  end

  def array_index(key)
    items = item_params.delete(:items)
    items[key]
  end

  def scalar_array_index(key)
    labels = item_params.delete(:labels)
    labels[key]
  end

  def hash_integer_index
    removed = item_params.delete(:settings)
    removed[0]
  end

  def item_params
    params.require(:item).permit(items: [[:name]], labels: [], settings: {})
  end
end
