//! The runtime's wire contracts with Rails — signed cookies, signed ids,
//! signed GlobalIDs, Active Storage's blob verifier, GlobalID params,
//! the cookie codec — held to Rails' own answers.
//!
//! `tests/rails_compat_vectors.rb` drives the runtime's own signing and
//! verifying code against `tests/rails_compat/rails_compat.json` (minted
//! by Rails inside campfire; once-campfire-rust's generator, vendored
//! beside it) and prints one line per section: `name pass/total`. The
//! gaps are written down in docs/pipeline/runtime.md ("Rails' signed-value
//! contracts, held to Rails' vectors").
//!
//! A RATCHET, not an all-or-nothing gate: each section must match at
//! least as many cases as it does today, and no failing section may be
//! unledgered. A section that improves fails too, asking for its floor
//! to be raised here — so a fix is recorded, and never silently lost.
//!
//!     ruby tests/rails_compat_vectors.rb . -v     # every failing case

use std::path::Path;
use std::process::Command;

/// (section, matching cases today, total cases).
const FLOORS: &[(&str, usize, usize)] = &[
    ("key_generator", 6, 6),
    ("cookie_escaping.parse", 7, 7),
    ("cookie_escaping.write", 3, 5),
    ("signed_cookies.verify", 26, 35),
    ("signed_cookies.generate", 1, 10),
    ("signed_cookies.generate_envelope", 10, 10),
    ("json_string", 20, 20),
    ("signed_ids.generate", 14, 14),
    ("signed_ids.verify", 29, 31),
    ("global_ids", 4, 4),
    ("sgids.generate", 5, 5),
    ("sgids.verify", 19, 24),
    ("app_verifiers.blob_id", 8, 8),
    ("app_verifiers.envelope", 3, 3),
    ("app_verifiers.data", 7, 7),
    ("encrypted_cookies.verify", 20, 20),
    ("encrypted_cookies.generate", 8, 8),
    ("csrf", 0, 208),
    ("passwords", 0, 45),
];

#[test]
fn the_runtime_signs_and_verifies_as_rails_does() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("ruby")
        .arg(root.join("tests/rails_compat_vectors.rb"))
        .arg(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.lines().any(|l| l == "done"),
        "driver did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(!stdout.contains("UNLEDGERED"), "a failing section names no ledger entry\n{stdout}");

    let mut problems = Vec::new();
    for (name, floor, total) in FLOORS {
        let line = stdout
            .lines()
            .find(|l| l.split_whitespace().next() == Some(*name))
            .unwrap_or_else(|| panic!("no `{name}` line\n{stdout}"));
        let counts = line.split_whitespace().nth(1).expect("a pass/total column");
        let (pass, seen) = counts.split_once('/').expect("pass/total");
        let (pass, seen): (usize, usize) = (pass.parse().unwrap(), seen.parse().unwrap());
        if seen != *total {
            problems.push(format!("{name}: {seen} cases, expected {total} — the vectors changed"));
        } else if pass < *floor {
            problems.push(format!("{name}: {pass}/{total}, below its floor of {floor} — a regression"));
        } else if pass > *floor {
            problems.push(format!("{name}: {pass}/{total}, above its floor of {floor} — raise it in FLOORS"));
        }
    }
    assert!(problems.is_empty(), "{}\n\n{stdout}", problems.join("\n"));
}
