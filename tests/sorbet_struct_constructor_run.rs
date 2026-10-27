//! A lowered `T::Struct` builds as sorbet's generated constructor does.
//!
//! `T::Struct#initialize` takes the props as keywords or as one
//! positional Hash, and a `T.nilable` prop is optional (nil when
//! omitted). An app that builds a struct from a config Hash
//! whose every prop is nilable; the keyword-only constructor with all
//! keywords required died `given 1, expected 0; required keywords: …`.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const STRUCTS: &str = r#"module Grants
  class Access < T::Struct
    prop :scope, T.nilable(T.any(String, Symbol))
    prop :permission, T.nilable(String)
    prop :flag, T.nilable(String), default: "on"
  end

  class Holder < T::Struct
    prop :attributes_hash, T.nilable(String)
    prop :unknown, T.nilable(String)
  end

  class Named < T::Struct
    const :name, String
    prop :note, T.nilable(String)
  end
end
"#;

const SCRIPT: &str = r#"
a = Grants::Access.new
raise "nilable omitted is nil" unless a.scope.nil? && a.permission.nil?
raise "default kept" unless a.flag == "on"
b = Grants::Access.new(scope: :read, flag: "off")
raise "keywords" unless b.scope == :read && b.flag == "off" && b.permission.nil?
hash = { "scope" => "write", "permission" => "p" }
c = Grants::Access.new(hash)
raise "positional hash" unless c.scope == "write" && c.permission == "p" && c.flag == "on"
d = Grants::Access.new({ scope: "x", flag: nil })
raise "symbol hash" unless d.scope == "x" && d.flag.nil?
begin
  Grants::Access.new({ scpoe: "read" })
  raise "an unknown key must raise"
rescue ArgumentError => e
  raise e.message unless e.message.include?("scpoe")
end
h = Grants::Holder.new({ attributes_hash: "kept" })
raise "a prop named like the Hash parameter" unless h.attributes_hash == "kept"
raise "its keyword" unless Grants::Holder.new(attributes_hash: "kw").attributes_hash == "kw"
raise "a prop named like the key check" unless Grants::Holder.new({}, unknown: "kept").unknown == "kept"
n = Grants::Named.new(name: "n")
raise "required const with nilable omitted" unless n.name == "n" && n.note.nil?
begin
  Grants::Named.new(note: "x")
  raise "missing required const must raise"
rescue ArgumentError
end
puts "struct constructor passed"
"#;

#[test]
fn a_lowered_struct_takes_a_positional_hash_and_optional_nilable_props() {
    emit_and_run::real_blog()
        .write("app/services/grants.rb", STRUCTS)
        .run_ruby(SCRIPT)
        .assert_passes();
}
