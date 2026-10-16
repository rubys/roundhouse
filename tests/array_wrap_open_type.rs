//! `Array.wrap(x)` where `x` has no static shape (a json column, an
//! untyped value).
//!
//! The fold treated every type that is not nil, an Array or a union as
//! one closed value and wrapped it: `[x]`. A json column holding an
//! Array was wrapped a second time, so
//! `self.history = Array.wrap(history) << { "step" => s }` stored
//! `[[{"step":"a"}],{"step":"b"}]` on the second call where Rails
//! stores `[{"step":"a"},{"step":"b"}]`. An open type now takes Rails'
//! three answers as a branch: nil is `[]`, an Array is itself, anything
//! else is one element.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.json "history"
  end
end
"#;

const MODEL: &str = r#"class Widget < ApplicationRecord
  def logged(step)
    Array.wrap(history) << { "step" => step }
  end

  def named
    Array.wrap(name.to_s)
  end
end
"#;

fn model(target: BuildTarget) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/widget.rb"), MODEL.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit");
    files
        .into_iter()
        .find(|(p, _)| p.ends_with("models/widget.rb"))
        .map(|(_, c)| c)
        .expect("the widget model is emitted")
}

/// The body of `def <name>` … its `end`, as emitted.
fn method<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src.find(&format!("  def {name}")).unwrap_or_else(|| panic!("no def {name}:\n{src}"));
    let end = src[start..].find("\n  end\n").map(|i| start + i).unwrap();
    &src[start..end]
}

#[test]
fn an_open_value_is_not_wrapped_twice() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let src = model(target);
        let body = method(&src, "logged");
        assert!(body.contains("is_a?(Array)"), "{target:?}:\n{body}");
        // A String is one closed value: still one element, no branch.
        let body = method(&src, "named");
        assert!(!body.contains("is_a?"), "{target:?}:\n{body}");
    }
}

/// The emitted `logged` body, run on CRuby with `history` holding each
/// shape Rails' `Array.wrap` distinguishes.
#[test]
fn the_branch_answers_what_rails_answers() {
    let src = model(BuildTarget::Ruby);
    let body = method(&src, "logged");
    let script = format!(
        "class Probe\n  attr_accessor :history\n{body}\n  end\nend\n\
         p [nil, [{{\"step\" => \"a\"}}], {{\"step\" => \"a\"}}].map {{ |h| w = Probe.new; w.history = h; w.logged(\"b\") }}\n"
    );
    let output = Command::new("ruby").args(["-e", &script]).output().unwrap();
    assert!(
        output.status.success(),
        "{script}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        r#"[[{"step" => "b"}], [{"step" => "a"}, {"step" => "b"}], [{"step" => "a"}, {"step" => "b"}]]"#
    );
}
