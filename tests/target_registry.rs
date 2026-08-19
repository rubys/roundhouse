//! Cross-artifact target-list drift guard.
//!
//! The target roster lives in Rust (`BuildTarget`) but is re-spelled in
//! `bin/rh` (Ruby, so `doctor`/`fetch`/`fixture` need no Rust
//! toolchain). Nothing structural forces the two lists to agree, and
//! they have drifted before: `csharp` shipped in the compiler and CI
//! while `bin/rh transpile csharp` rejected it as an unknown target.
//! This test pins the relationship, exceptions spelled out explicitly.

use roundhouse::project::BuildTarget;

/// Extract the `TARGETS = %w[...]` list from bin/rh.
fn bin_rh_targets() -> Vec<String> {
    let src = std::fs::read_to_string("bin/rh").expect("read bin/rh");
    let line = src
        .lines()
        .find(|l| l.trim_start().starts_with("TARGETS = %w["))
        .expect("bin/rh TARGETS %w[] literal not found");
    let inner = line
        .split_once("%w[")
        .and_then(|(_, rest)| rest.split_once(']'))
        .expect("malformed TARGETS literal")
        .0;
    inner.split_whitespace().map(str::to_string).collect()
}

#[test]
fn bin_rh_targets_match_build_targets() {
    // `roda` is deliberately absent from bin/rh: it is the
    // experimental Roda+Sequel lane (issue #67), reachable via
    // `roundhouse --target roda`, with no scaffold/fetch/site
    // packaging yet. Everything else in TRANSPILE must be offered by
    // bin/rh, and bin/rh must offer nothing the compiler lacks.
    const NOT_IN_BIN_RH: &[&str] = &["roda"];

    let rh: Vec<String> = bin_rh_targets();
    let transpile: Vec<&str> = BuildTarget::TRANSPILE
        .iter()
        .map(|t| t.as_str())
        .collect();

    for t in &transpile {
        if NOT_IN_BIN_RH.contains(t) {
            assert!(
                !rh.iter().any(|r| r == t),
                "{t} is on the NOT_IN_BIN_RH exception list but bin/rh \
                 now offers it — delete the exception"
            );
        } else {
            assert!(
                rh.iter().any(|r| r == t),
                "BuildTarget::TRANSPILE has `{t}` but bin/rh TARGETS \
                 does not — add it to bin/rh (or to NOT_IN_BIN_RH with \
                 a reason)"
            );
        }
    }
    for r in &rh {
        assert!(
            transpile.contains(&r.as_str()),
            "bin/rh TARGETS has `{r}` but BuildTarget::TRANSPILE does \
             not — bin/rh would accept a target the compiler rejects"
        );
    }
}
