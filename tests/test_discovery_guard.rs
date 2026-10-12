//! Toolchain runners must discover tests. A hand-maintained path list
//! is how `runtime/spinel/test/views/articles_test.rb` and its
//! siblings shipped in every ruby archive while CI ran five other
//! files and stayed green.
//!
//! This gate does not execute those suites (they are `#[ignore]` and
//! need the target SDK). It fails in the default `cargo test` sweep
//! when a harness goes back to naming files, or when the ruby
//! Rakefile stops globs.

#[test]
fn ruby_rake_discovers_every_test_file() {
    for path in [
        "runtime/spinel/scaffold/ruby_overlay/Rakefile",
        "runtime/spinel/scaffold/Rakefile",
    ] {
        let rake = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(
            rake.contains("t.test_files = FileList[\"test/**/*_test.rb\"]"),
            "{path} no longer globs every test file",
        );
    }
    let harness = std::fs::read_to_string("tests/ruby_toolchain.rs").expect("ruby_toolchain.rs");
    assert!(
        harness.contains(".arg(\"rake\")") && harness.contains(".arg(\"test\")"),
        "ruby_toolchain.rs must run rake test",
    );
    assert!(
        !harness.contains("for test in ["),
        "ruby_toolchain.rs grew a hand-maintained test-file list",
    );
}

#[test]
fn other_toolchain_runners_are_not_path_lists() {
    // Each execution leg hands the target's own runner a discovery
    // command (or, for TypeScript, a recursive find). `for test in [`
    // is the shape that skipped files.
    let expectations = [
        ("tests/typescript_toolchain.rs", ".arg(\"test\")"),
        (
            "src/emit/typescript/package.rs",
            "no *.test.ts discovered under test/",
        ),
        ("tests/go_toolchain.rs", "\"./..."),
        ("tests/crystal_toolchain.rs", "\"spec\""),
        ("tests/python_toolchain.rs", "\"discover\""),
        ("tests/elixir_toolchain.rs", "\"test\""),
        ("tests/kotlin_toolchain.rs", "\"test\""),
        ("tests/csharp_toolchain.rs", "\"test\""),
        ("tests/swift_toolchain.rs", "\"test\""),
        ("tests/rust_toolchain.rs", "\"test\""),
        ("tests/spinel_toolchain.rs", "\"spinel-test\""),
    ];
    for (path, needle) in expectations {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(
            src.contains(needle),
            "{path} lost its discovery command ({needle})"
        );
        assert!(
            !src.contains("for test in ["),
            "{path} hardcodes a test-file list",
        );
    }
    // The framework source suite is a glob, not a list of files.
    let unit = std::fs::read_to_string("tests/runtime_ruby_unit.rs").expect("runtime_ruby_unit");
    assert!(unit.contains("*_test.rb"), "{unit}");

    // Roda's harness runs one behavioral oracle, the fixture's spec.
    // It is not an inventory of an emitted suite.
    let roda = std::fs::read_to_string("tests/roda_toolchain.rs").expect("roda");
    assert!(roda.contains("test/blog_test.rb"));
    assert!(roda.contains("test/blog_oracle_test.rb"));
}
