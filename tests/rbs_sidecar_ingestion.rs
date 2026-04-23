//! Ingest-side: Rails app containing `sig/**/*.rbs` gets its user-
//! declared method signatures populated on `App.rbs_signatures`.
//! Covers the file-system scan + RBS parse + namespace-preserving
//! merge path end-to-end.

use std::fs;
use std::path::Path;

use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app;
use roundhouse::ty::Ty;

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("roundhouse-rbs-sidecar-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, content).expect("write file");
}

#[test]
fn ingest_scans_sig_dir_and_populates_rbs_signatures() {
    let dir = scratch_dir("basic");

    // Minimal Rails app shape: just enough for ingest_app to succeed.
    write(
        &dir.join("db/schema.rb"),
        "ActiveRecord::Schema.define(version: 1) do\nend\n",
    );

    // User-authored RBS sidecars under sig/.
    write(
        &dir.join("sig/article.rbs"),
        "\
class Article
  def full_name: () -> String
  def word_count: () -> Integer
end
",
    );
    write(
        &dir.join("sig/helpers/application_helper.rbs"),
        "\
module ApplicationHelper
  def format_date: (String) -> String
end
",
    );

    let app = ingest_app(&dir).expect("ingest");

    // Both classes/modules appear, with their declared methods.
    let article = ClassId(Symbol::from("Article"));
    let helper = ClassId(Symbol::from("ApplicationHelper"));
    assert!(
        app.rbs_signatures.contains_key(&article),
        "Article should be in rbs_signatures; got: {:?}",
        app.rbs_signatures.keys().collect::<Vec<_>>()
    );
    assert!(app.rbs_signatures.contains_key(&helper));

    let article_methods = &app.rbs_signatures[&article];
    assert_eq!(article_methods.len(), 2);
    assert!(article_methods.contains_key(&Symbol::from("full_name")));
    assert!(article_methods.contains_key(&Symbol::from("word_count")));

    // Method types round-trip through ingest unchanged.
    let full_name = &article_methods[&Symbol::from("full_name")];
    let Ty::Fn { ret, .. } = full_name else {
        panic!("expected Ty::Fn");
    };
    assert_eq!(**ret, Ty::Str);
}

#[test]
fn ingest_nested_namespace_in_rbs() {
    let dir = scratch_dir("nested");

    write(
        &dir.join("db/schema.rb"),
        "ActiveRecord::Schema.define(version: 1) do\nend\n",
    );
    write(
        &dir.join("sig/api/v1/post.rbs"),
        "\
module Api
  class V1
    class Post
      def title: () -> String
    end
  end
end
",
    );

    let app = ingest_app(&dir).expect("ingest");

    let nested = ClassId(Symbol::from("Api::V1::Post"));
    assert!(
        app.rbs_signatures.contains_key(&nested),
        "nested class key should be namespace-joined; got: {:?}",
        app.rbs_signatures.keys().collect::<Vec<_>>()
    );
}

#[test]
fn ingest_empty_when_no_sig_dir() {
    let dir = scratch_dir("empty");
    write(
        &dir.join("db/schema.rb"),
        "ActiveRecord::Schema.define(version: 1) do\nend\n",
    );

    let app = ingest_app(&dir).expect("ingest");
    assert!(app.rbs_signatures.is_empty());
}
