//! `lower::defined_ivar_memo`: `return @x if defined?(@x)` becomes a
//! presence-flag guard, and every assignment to `@x` in the class sets
//! the flag after the value lands. campfire's `Opengraph::Location`
//! memoises a nullable `parsed_url` and `resolved_ip` this way.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::apply_defined_ivar_memo_lowering;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

/// The lowered IR of `Location`'s methods, rendered with Debug — the
/// pass is a tree rewrite, and the names it introduces are what the
/// assertions read.
fn location_ir(body: &str) -> String {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/models/location.rb", body),
    ]))
    .expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    apply_defined_ivar_memo_lowering(&mut app);
    let lc = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "Location")
        .expect("Location is a library class");
    lc.methods.iter().map(|m| format!("{}: {:?}\n", m.name.as_str(), m.body)).collect()
}

#[test]
fn the_guard_reads_a_flag_the_assignment_sets() {
    let src = location_ir(
        r#"class Location
  def initialize(url)
    @url = url
  end

  def parsed_url
    return @parsed_url if defined?(@parsed_url)
    @parsed_url = URI.parse(@url) rescue nil
  end
end
"#,
    );
    assert!(src.contains("Ivar { name: Symbol(\"parsed_url_defined\") }"), "{src}");
    assert!(!src.contains("Symbol(\"defined?\")"), "{src}");
    // The flag is set AFTER the value: the Seq is [assign, set flag, read].
    let assign = src.find("Assign { target: Ivar { name: Symbol(\"parsed_url\") }").expect("assignment");
    let flag = src.find("Assign { target: Ivar { name: Symbol(\"parsed_url_defined\") }").expect("flag");
    assert!(assign < flag, "{src}");
}

#[test]
fn a_guard_whose_writer_is_nested_in_an_expression_is_left_alone() {
    // `foo(@x = compute)` — the assignment is a value, not a statement,
    // so the three-statement replacement has nowhere to go. The guard
    // stays `defined?` rather than gaining a flag one writer never sets.
    let src = location_ir(
        r#"class Location
  def value
    return @value if defined?(@value)
    record(@value = compute)
  end

  def compute
    1
  end

  def record(v)
    v
  end
end
"#,
    );
    assert!(src.contains("Symbol(\"defined?\")"), "{src}");
    assert!(!src.contains("value_defined"), "{src}");
}
