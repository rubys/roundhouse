# CRuby/JRuby's fast implementation of the shared header policy. Keep regex
# matching in this overlay: strict-target String runtimes cannot emit match?.
module ActionController
  HEADER_KEY_ILLEGAL = /[\x00-\x1f\x7f": ]/.freeze
  HEADER_VALUE_ILLEGAL = /[\x00-\x08\x0a-\x1f\x7f]/.freeze

  def self.header_key_ok?(k)
    return false if k.nil?
    return false if k.length == 0
    !k.b.match?(HEADER_KEY_ILLEGAL)
  end

  def self.header_value_ok?(v)
    return false if v.nil?
    !v.b.match?(HEADER_VALUE_ILLEGAL)
  end
end
