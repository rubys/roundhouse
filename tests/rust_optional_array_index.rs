//! Rust array indexing preserves Ruby's nil-on-miss result when typed as optional.

use roundhouse::emit::rust;
use roundhouse::ingest::ingest_app_from_tree;
use std::collections::HashMap;
use std::path::PathBuf;

#[test]
fn optional_array_index_uses_a_checked_lookup() {
    let files: HashMap<PathBuf, Vec<u8>> = [(
        "app/models/index_probe.rb",
        r#"class IndexProbe
  def item_at
    ["first", "second"][5]
  end

  def last_item
    ["first", "second"][-1]
  end

  def last_or_nil
    rows = ["first", "second"]
    rows.length == 0 ? nil : rows[-1]
  end

  def missing_or_nil
    rows = ["first", "second"]
    rows.length == 0 ? nil : rows[8]
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
        .find(|file| {
            file.path
                .to_string_lossy()
                .ends_with("index_probe_class.rs")
        })
        .expect("Rust class")
        .content;

    assert!(
        source.contains("pub fn item_at(&self) -> Option<String>"),
        "Ruby array lookup should retain its nilable result type:\n{source}"
    );
    assert!(
        source.contains(".get((5_i64) as usize).cloned()"),
        "positive indexes must return None when out of bounds:\n{source}"
    );
    assert!(
        source.contains("checked_sub(1_usize).and_then(|__index| __recv.get(__index)).cloned()"),
        "negative indexes must also preserve Ruby's nil-on-miss result:\n{source}"
    );
    for method in ["last_or_nil", "missing_or_nil"] {
        let body = source
            .split(&format!("fn __rh_static_{method}("))
            .nth(1)
            .or_else(|| source.split(&format!("pub fn {method}(")).nth(1))
            .expect("conditional index method")
            .split("\n    pub fn ")
            .next()
            .unwrap();
        assert!(body.contains("-> Option<String>"), "{body}");
        assert!(body.contains(".get("), "{body}");
        assert!(
            !body.contains("Some("),
            "the branch lookup already returns Option; wrapping it nests Option and breaks nil-on-miss:\n{body}"
        );
    }
}
