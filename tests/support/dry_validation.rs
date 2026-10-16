//! `Dry::Validation::Contract` classes and `Dry::Schema` constants,
//! lowered at ingest (`ingest::dry_validation`). The expectations are the
//! gems' own answers (dry-validation 1.11.1, dry-schema 1.14.1) to the
//! cases in `dry_validation/cases.txt`, written by `dry_validation/gen.rb`.

// Each lane uses only its part.
#![allow(dead_code)]

const FILES: &[(&str, &str)] = &[
    ("app/contracts/user_contract.rb", include_str!("dry_validation/app/contracts/user_contract.rb")),
    ("app/contracts/event_contract.rb", include_str!("dry_validation/app/contracts/event_contract.rb")),
    ("app/contracts/strict_contract.rb", include_str!("dry_validation/app/contracts/strict_contract.rb")),
    ("app/contracts/parent_contract.rb", include_str!("dry_validation/app/contracts/parent_contract.rb")),
    ("app/contracts/localized_contract.rb", include_str!("dry_validation/app/contracts/localized_contract.rb")),
    ("app/contracts/standalone.rb", include_str!("dry_validation/app/contracts/standalone.rb")),
    ("config/locales/contracts.ru.yml", include_str!("dry_validation/config/locales/contracts.ru.yml")),
    ("app/contracts/typed_contract.rb", include_str!("dry_validation/app/contracts/typed_contract.rb")),
    ("lib/shop/types.rb", include_str!("dry_validation/lib/shop/types.rb")),
    ("app/contracts/chain_contract.rb", include_str!("dry_validation/app/contracts/chain_contract.rb")),
    ("app/contracts/rules_contract.rb", include_str!("dry_validation/app/contracts/rules_contract.rb")),
];

pub fn overlay() -> super::emit_and_run::Overlay {
    FILES.iter().fold(super::emit_and_run::real_blog(), |o, (path, source)| o.write(path, source))
}

pub fn assertions() -> String {
    format!(
        r##"
def dv_check(index, callee, input, success, to_h, errors)
  result = callee.call(input)
  raise "case #{{index}}: success? #{{result.success?}}" unless result.success? == success
  raise "case #{{index}}: to_h #{{result.to_h.inspect}}" unless result.to_h == to_h
  raise "case #{{index}}: errors #{{result.errors.to_h.inspect}}" unless result.errors.to_h == errors
end

def dv_raises(index, callee, input, error)
  callee.call(input)
  raise "case #{{index}}: no #{{error}}"
rescue error
end

{}
messages = UserContract.new.call("addr" => {{}}, "ids" => ["x", 1, "y"]).errors
raise "messages" unless messages.messages.map {{ |m| [m.path, m.text] }} == [[[:name], "is missing"], [[:age], "is missing"], [[:ids, 0], "must be an integer"], [[:ids, 2], "must be an integer"], [[:addr, :city], "is missing"]]
raise "map" unless messages.map(&:to_s).size == 5 && messages.count == 5
puts "dry-validation contract passed"
"##,
        include_str!("dry_validation/expected.rb")
    )
}
