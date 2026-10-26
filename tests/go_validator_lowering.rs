//! #1165: on the go target, the lowered `validate` for `format:`,
//! `numericality:` and `inclusion:` did not compile. A nullable column is a
//! Go pointer (`*string`, `*int64`), and the checks
//!
//! - asked `x.is_a?(String)` as `x.IsAPred(String)`, a method no Go type has;
//! - read the pointer as a value after the nil test
//!   (`regexp….MatchString(self.Code)`, `slices.Contains(list, self.Kind)`);
//! - compared an `int64` against a Float bound (`self.Score <= 1.0`).
//!
//! Now `is_a?` on a typed field folds to its nil test (rust's
//! `static_is_a`), the value is dereferenced where the check reads it, and
//! an Integer is widened to `float64` against a Float bound. `format`
//! matches nil as `""` (Rails matches `value.to_s`), and `inclusion`
//! keeps nil "not included", as Rails does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const MODEL: &str = r#"class Widget < ApplicationRecord
  validates :code, format: { with: /\A[A-Z0-9]+\z/ }
  validates :score, numericality: { greater_than: 1 }
  validates :kind, inclusion: { in: %w[a b] }
end
"#;

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"code\"\n    t.integer \"score\"\n    t.string \"kind\"\n  end\nend\n";

fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/widget.rb", MODEL),
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
}

fn widget_go() -> String {
    let tree: HashMap<PathBuf, Vec<u8>> =
        files().into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::emit::go::emit(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("widget.go"))
        .map(|f| f.content)
        .expect("widget.go")
}

#[test]
fn validations_read_nullable_columns_as_values() {
    let go = widget_go();
    let validate = &go[go.find("func (self *Widget) Validate()").expect("Validate")..];
    let validate = &validate[..validate[1..].find("\nfunc ").map(|i| i + 1).unwrap_or(validate.len())];
    for line in [
        r#"if !(self.Code == nil && regexp.MustCompile("\\A[A-Z0-9]+\\z").MatchString("") || !(self.Code == nil) && regexp.MustCompile("\\A[A-Z0-9]+\\z").MatchString(*self.Code)) {"#,
        "if self.Score == nil || !(self.Score != nil) {",
        "if float64(DerefInt64(self.Score)) <= 1.0 {",
        r#"if !((self.Kind != nil && slices.Contains([]string{"a", "b"}, DerefString(self.Kind)))) {"#,
    ] {
        assert!(validate.contains(line), "{line:?} in:\n{validate}");
    }
    assert!(!validate.contains("IsAPred"), "{validate}");
}

/// `go vet` on the emitted project. Needs a Go toolchain, so it is
/// `#[ignore]`d like `go_toolchain.rs`; run with
/// `cargo test --test go_validator_lowering -- --ignored`.
#[test]
#[ignore = "requires the Go toolchain"]
fn the_emitted_go_vets_clean() {
    let root = std::env::temp_dir().join(format!("roundhouse-go-validators-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, body) in files() {
        let file = root.join("app_src").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    let out = root.join("out");
    let emit = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "go", "-o"])
        .arg(&out)
        .arg(root.join("app_src"))
        .output()
        .expect("run roundhouse");
    assert!(emit.status.success(), "{}", String::from_utf8_lossy(&emit.stderr));
    let vet = Command::new("go").arg("vet").arg("./...").current_dir(&out).env("GOFLAGS", "-mod=mod").output().expect("run go vet");
    let stderr = String::from_utf8_lossy(&vet.stderr).into_owned();
    let _ = std::fs::remove_dir_all(Path::new(&root));
    assert!(vet.status.success(), "go vet:\n{stderr}");
}
