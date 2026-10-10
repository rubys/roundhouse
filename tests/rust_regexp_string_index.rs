//! Ruby String regexp indexing returns a nullable capture in Rust.

use roundhouse::emit::rust;
use roundhouse::ingest::ingest_app_from_tree;
use std::collections::HashMap;
use std::path::PathBuf;

#[test]
fn regexp_index_uses_captures_and_preserves_a_missing_match_as_none() {
    let files: HashMap<PathBuf, Vec<u8>> = [(
        "app/models/regexp_matcher.rb",
        r#"class RegexpMatcher
  def capture
    "Firefox123"[/([A-Za-z]+)([0-9]+)/, 1]
  end

  def capture_string
    "Firefox123"[/([A-Za-z]+)([0-9]+)/, 1].to_s
  end
end
"#,
    )]
    .into_iter()
    .map(|(path, content)| (PathBuf::from(path), content.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);

    let source = rust::emit(&app)
        .into_iter()
        .find(|file| file.path.to_string_lossy().ends_with("regexp_matcher_class.rs"))
        .expect("Rust matcher class")
        .content;

    assert!(
        source.contains("pub fn capture(&self) -> Option<String>"),
        "regexp indexing must retain Ruby's nil-on-miss result:\n{source}"
    );
    assert!(
        source.contains(".captures(&__source)") && source.contains("__captures.and_then"),
        "capture lookup should use regex::Captures and return None on a miss:\n{source}"
    );
    assert!(
        !source.contains("(regex::Regex::new") && !source.contains("MATCHER) as i64"),
        "a Regexp must not be treated as a numeric String index:\n{source}"
    );
    assert!(
        source.contains(".map(|v| v.to_string()).unwrap_or_default()"),
        "Ruby's nil.to_s behavior should convert an optional capture to an empty String:\n{source}"
    );
}
