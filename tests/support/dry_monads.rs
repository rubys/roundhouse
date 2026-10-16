//! dry-monads' Result and do notation, lowered at ingest
//! (`ingest::dry_monads`). The expectations are the gem's own answers
//! (dry-monads 1.9.0) to the cases in `dry_monads/cases.txt`, written by
//! `dry_monads/gen.rb`.

// Each lane uses only its part.
#![allow(dead_code)]

const FILES: &[(&str, &str)] = &[
    ("app/services/base_service.rb", include_str!("dry_monads/app/services/base_service.rb")),
    ("app/services/order_service.rb", include_str!("dry_monads/app/services/order_service.rb")),
    ("app/services/lookup.rb", include_str!("dry_monads/app/services/lookup.rb")),
    ("app/services/worker.rb", include_str!("dry_monads/app/services/worker.rb")),
    ("app/contracts/signup_contract.rb", include_str!("dry_monads/app/contracts/signup_contract.rb")),
];

pub fn overlay() -> super::emit_and_run::Overlay {
    FILES.iter().fold(super::emit_and_run::real_blog(), |o, (path, source)| o.write(path, source))
}

pub fn assertions() -> String {
    format!(
        r##"
def dm_check(index, actual, expected)
  raise "case #{{index}}: #{{actual}} != #{{expected}}" unless actual == expected
end

def dm_raises(index, block, error, message)
  block.call
  raise "case #{{index}}: no #{{error}}"
rescue error => e
  raise "case #{{index}}: #{{e.message}}" unless e.message == message
end

{}
puts "dry-monads contract passed"
"##,
        include_str!("dry_monads/expected.rb")
    )
}
