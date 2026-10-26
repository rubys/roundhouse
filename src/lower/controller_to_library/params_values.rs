use crate::dialect::{MethodDef, MethodReceiver};
use crate::ident::Symbol;

use super::params::ParamsSpec;

pub(super) fn methods(spec: &ParamsSpec) -> Vec<MethodDef> {
    let owner = spec.class_id.0.as_str();
    let mut source = format!(r#"class {owner}
  def initialize
    @__rh_permitted_values = {{}}
  end

  attr_accessor :__rh_permitted_values

  def __rh_values
    @__rh_permitted_values || {{}}
  end

  def self.__rh_copy(values)
    instance = {owner}.new
    instance.__rh_permitted_values = values
    instance
  end

  def [](key)
    {owner}.__rh_param_value(self.__rh_values, key.to_s)
  end

  def to_h
    self.__rh_values.dup
  end
"#);
    let mut signatures = format!(r#"class {owner}
  def initialize: () -> void
  @__rh_permitted_values: Hash[String, Roundhouse::ParamValue]
  def __rh_permitted_values: () -> Hash[String, Roundhouse::ParamValue]
  def __rh_permitted_values=: (Hash[String, Roundhouse::ParamValue]) -> Hash[String, Roundhouse::ParamValue]
  def __rh_values: () -> Hash[String, Roundhouse::ParamValue]
  def self.__rh_copy: (Hash[String, Roundhouse::ParamValue]) -> {owner}
  def []: (String | Symbol) -> Roundhouse::ParamValue
  def to_h: () -> Hash[String, Roundhouse::ParamValue]
"#);
    for method in ["key?", "include?"] {
        if spec.lookup_methods.contains(&Symbol::from(method)) {
            source.push_str(&format!(r#"
  def {method}(key)
    self.__rh_values.key?(key.to_s)
  end
"#));
            signatures.push_str(&format!("  def {method}: (String | Symbol) -> bool\n"));
        }
    }
    if spec.lookup_methods.contains(&Symbol::from("delete")) {
        source.push_str(r#"
  def delete(key)
    values = self.__rh_values
    removed = values.delete(key.to_s)
    self.__rh_permitted_values = values
    removed
  end
"#);
        signatures.push_str("  def delete: (String | Symbol) -> Roundhouse::ParamValue\n");
    }
    if spec.wants_except {
        source.push_str(&format!(r#"
  def except(*keys)
    values = self.__rh_values.dup
    i = 0
    while i < keys.length
      values.delete(keys[i].to_s)
      i = i + 1
    end
    {owner}.__rh_copy(values)
  end
"#));
        signatures.push_str(&format!("  def except: (*(String | Symbol)) -> {owner}\n"));
    }
    if spec.lookup_methods.contains(&Symbol::from("compact")) {
        source.push_str(&format!(r#"
  def compact
    values = {{}}
    self.__rh_values.each do |key, value|
      values[key] = value unless value.nil?
    end
    {owner}.__rh_copy(values)
  end
"#));
        signatures.push_str(&format!("  def compact: () -> {owner}\n"));
    }
    if spec.wants_to_attrs {
        source.push_str(r#"
  def to_attrs
    values = {}
    self.__rh_values.each do |key, value|
      values[key.to_sym] = value
    end
    values
  end
"#);
        signatures.push_str("  def to_attrs: () -> Hash[Symbol, Roundhouse::ParamValue]\n");
    }
    if spec.wants_as_json {
        source.push_str(r#"
  def as_json
    self.to_h
  end
"#);
        signatures.push_str("  def as_json: () -> Hash[String, Roundhouse::ParamValue]\n");
    }
    source.push_str(r#"
  def self.__rh_param_value(values, key)
    values.fetch(key, nil)
  end
end
"#);
    signatures.push_str("  def self.__rh_param_value: (Hash[String, Roundhouse::ParamValue], String) -> Roundhouse::ParamValue\n");
    signatures.push_str("end\n");
    crate::runtime_src::parse_library_with_rbs(source.as_bytes(), &signatures, "<params_values>")
        .expect("typed permitted values template")
        .into_iter().flat_map(|class| class.methods)
        .map(|mut method| {
            method.enclosing_class = Some(spec.class_id.0.clone());
            if method.name.as_str() == "delete" {
                method.mutates_self = true;
            }
            if method.receiver == MethodReceiver::Instance && method.name.as_str() == "initialize" {
                method.visibility = crate::dialect::MethodVisibility::Private;
            }
            method
        }).collect()
}
