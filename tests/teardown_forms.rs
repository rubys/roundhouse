//! Only `teardown do … end` is modeled in a test class. Any other
//! `teardown` call is refused by name at ingest, since it would otherwise
//! fall to the unrecognized-statement drop and its cleanup would silently
//! never run.

fn ingest(body: &str) -> Result<(), String> {
    let source = format!("require \"test_helper\"\n\nclass WidgetTest < ActiveSupport::TestCase\n{body}\nend\n");
    roundhouse::ingest::ingest_test_file(source.as_bytes(), "widget_test.rb")
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[test]
fn a_teardown_block_is_modeled() {
    ingest("  teardown do\n    cleanup\n  end\n\n  test \"a\" do\n    assert true\n  end").unwrap();
}

#[test]
fn a_teardown_symbol_is_refused_by_name() {
    let err = ingest("  teardown :close_files\n\n  test \"a\" do\n    assert true\n  end").unwrap_err();
    assert!(err.contains("test teardown not modeled"), "{err}");
}

#[test]
fn a_teardown_block_argument_is_refused_by_name() {
    let err = ingest("  teardown(&:close_files)\n\n  test \"a\" do\n    assert true\n  end").unwrap_err();
    assert!(err.contains("test teardown not modeled"), "{err}");
}
