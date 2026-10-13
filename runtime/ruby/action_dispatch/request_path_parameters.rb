module ActionDispatch
  # Route captures exposed with String/Symbol-indifferent reads. Query
  # parameters are kept separately by the request and never enter this map.
  class RequestPathParameters
    def initialize(values)
      @values = {}
      values.each { |key, value| @values[key.to_s] = value }
    end

    def [](key)
      @values[key.to_s]
    end

    def fetch(key, *defaults)
      name = key.to_s
      return @values[name] if @values.key?(name)
      raise ArgumentError, "wrong number of arguments" if defaults.length > 1
      return defaults[0] unless defaults.empty?
      raise KeyError, "key not found: #{key}"
    end

    def key?(key)
      @values.key?(key.to_s)
    end
  end
end
