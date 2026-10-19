//! Rails' signed-value vectors under the SPINEL-compiled runtime — the
//! verifier and signed ids the binary runs, over sp_crypto's HMAC and
//! PBKDF2 rather than CRuby's OpenSSL.
//!
//! `tests/rails_compat_vectors.rb --cases` flattens the vectors (only
//! the clock-stable ones: the binary reads the real clock) and adds
//! `nul_key` cases — secrets whose derived keys hold a zero byte, with
//! OpenSSL's MAC as the answer. None of Rails' vectors has such a key and
//! about one real SECRET_KEY_BASE in five does; the binary's PBKDF2
//! decode used to cut the key at the zero, signing with a key a few bytes
//! long.
//!
//! The SAME driver (`tests/rails_compat/spinel_driver.rb`) runs twice:
//! compiled by spinel, and under CRuby over OpenSSL. The two tables must
//! be identical — a difference is a spinel-lane bug whatever Rails says —
//! and each section must meet its floor (the gaps both lanes share are
//! ledgered in docs/pipeline/runtime.md, "Rails' signed-value contracts,
//! held to Rails' vectors").
//!
//! Marked `#[ignore]` — needs `spinel` on PATH (CI's spinel job runs it):
//!
//!     PATH=$HOME/git/spinel/bin:$PATH cargo test --test rails_compat_vectors_spinel -- --ignored

use std::path::{Path, PathBuf};
use std::process::Command;

const FLOORS: &[(&str, usize, usize)] = &[
    ("kg", 6, 6),
    ("unescape", 7, 7),
    ("sc_verify", 23, 31),
    ("sc_envelope", 10, 10),
    ("json_encode", 10, 10),
    ("json_decode", 10, 10),
    ("sid_generate", 9, 9),
    ("sid_verify", 23, 25),
    ("sgid_generate", 5, 5),
    ("sgid_verify", 16, 21),
    ("as_generate", 6, 6),
    ("blob_verify", 5, 5),
    ("as_data", 4, 4),
    ("ec_verify", 18, 18),
    ("ec_cipher", 8, 8),
    ("nul_key", 6, 6),
];

fn scratch(name: &str) -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir scratch");
    dir
}

/// The driver's `op pass/total` lines, in order.
fn table(stdout: &str, lane: &str) -> Vec<String> {
    assert!(stdout.lines().any(|l| l == "done"), "the {lane} driver did not finish\n{stdout}");
    stdout
        .lines()
        .filter(|l| !l.starts_with("FAIL") && !l.starts_with(' ') && l.contains('/'))
        .map(String::from)
        .collect()
}

#[test]
#[ignore]
fn the_binary_signs_and_verifies_as_rails_and_cruby_do() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spinel_dir = scratch("roundhouse-rails-compat-spinel");
    let cruby_dir = scratch("roundhouse-rails-compat-cruby");
    for dir in [&spinel_dir, &cruby_dir] {
        for (from, to) in [
            ("runtime/spinel/base64.rb", "base64.rb"),
            ("runtime/ruby/action_controller/message_verifier.rb", "message_verifier.rb"),
            ("runtime/ruby/action_controller/message_encryptor.rb", "message_encryptor.rb"),
            ("runtime/ruby/active_record/signed_id.rb", "signed_id.rb"),
            ("runtime/spinel/tep/url.rb", "url.rb"),
            ("tests/rails_compat/spinel_driver.rb", "driver.rb"),
        ] {
            std::fs::copy(root.join(from), dir.join(to)).expect("copy");
        }
    }
    std::fs::copy(root.join("runtime/spinel/message_digest.rb"), spinel_dir.join("message_digest.rb")).unwrap();
    std::fs::copy(root.join("runtime/spinel/message_digest_cruby.rb"), cruby_dir.join("message_digest.rb")).unwrap();

    let cases = Command::new("ruby")
        .arg(root.join("tests/rails_compat_vectors.rb"))
        .arg(root)
        .arg("--cases")
        .arg(spinel_dir.join("cases.tsv"))
        .output()
        .expect("ruby is on PATH");
    assert!(cases.status.success(), "writing the case file failed: {}", String::from_utf8_lossy(&cases.stderr));
    std::fs::copy(spinel_dir.join("cases.tsv"), cruby_dir.join("cases.tsv")).unwrap();

    let build = Command::new("spinel")
        .args(["driver.rb", "-o", "driver"])
        .current_dir(&spinel_dir)
        .output()
        .expect("spinel is on PATH");
    assert!(
        build.status.success(),
        "spinel failed to compile the driver\n=== stdout ===\n{}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let binary = Command::new(spinel_dir.join("driver")).arg("cases.tsv").current_dir(&spinel_dir).output().unwrap();
    let interpreted = Command::new("ruby").args(["driver.rb", "cases.tsv"]).current_dir(&cruby_dir).output().unwrap();
    let binary_out = String::from_utf8_lossy(&binary.stdout).to_string();
    let cruby_out = String::from_utf8_lossy(&interpreted.stdout).to_string();
    let spinel_table = table(&binary_out, "spinel");
    let cruby_table = table(&cruby_out, "CRuby");
    assert_eq!(
        spinel_table, cruby_table,
        "the binary and CRuby disagree — a spinel-lane bug\n=== spinel ===\n{binary_out}\n=== CRuby ===\n{cruby_out}"
    );

    let mut problems = Vec::new();
    for (op, floor, total) in FLOORS {
        let line = spinel_table
            .iter()
            .find(|l| l.split_whitespace().next() == Some(*op))
            .unwrap_or_else(|| panic!("no `{op}` line\n{binary_out}"));
        let (pass, seen) = line.split_whitespace().nth(1).unwrap().split_once('/').unwrap();
        let (pass, seen): (usize, usize) = (pass.parse().unwrap(), seen.parse().unwrap());
        if seen != *total {
            problems.push(format!("{op}: {seen} cases, expected {total}"));
        } else if pass < *floor {
            problems.push(format!("{op}: {pass}/{total}, below its floor of {floor}"));
        } else if pass > *floor {
            problems.push(format!("{op}: {pass}/{total}, above its floor of {floor} — raise it"));
        }
    }
    assert!(problems.is_empty(), "{}\n\n{binary_out}", problems.join("\n"));
}
