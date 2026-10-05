//! The transpile gate's "rerun with --allow-unsupported" hint at the
//! process boundary (#303). The flag downgrades diagnostics, but a
//! project-boundary refusal (`target_files` returning `Err`, as the
//! Date gate does for spinel) fails with or without it, so the hint
//! must only be printed when the flag would actually write the output.

use std::path::{Path, PathBuf};
use std::process::Command;

const HINT: &str = "rerun with --allow-unsupported";

/// A minimal app whose `posts` table is created with `create_table_args`
/// and holds `extra_column`.
fn app(name: &str, create_table_args: &str, extra_column: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "roundhouse_hint_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let files = [
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_string(),
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n".to_string(),
        ),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n".to_string()),
        (
            "db/schema.rb",
            format!(
                "ActiveRecord::Schema.define do\n  create_table \"posts\"{create_table_args}, force: :cascade do |t|\n    t.string \"title\", null: false\n{extra_column}  end\nend\n"
            ),
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n".to_string()),
    ];
    for (path, content) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create app directory");
        std::fs::write(path, content).expect("write app file");
    }
    root
}

fn transpile(target: &str, input: &Path, allow_unsupported: bool) -> (i32, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_roundhouse"));
    command.args(["--target", target]);
    if allow_unsupported {
        command.arg("--allow-unsupported");
    }
    let out = command
        .arg(input)
        .arg("-o")
        .arg(input.join("out"))
        .output()
        .expect("spawn roundhouse");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn a_date_boundary_refusal_does_not_suggest_allow_unsupported() {
    let root = app("date", "", "    t.date \"published_on\"\n");

    let (code, err) = transpile("spinel", &root, false);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("Date not supported (spinel)"), "{err}");
    assert!(err.contains("Date-only values are not supported"), "{err}");
    assert!(!err.contains(HINT), "the flag cannot override this refusal: {err}");

    // The reason the hint is wrong: the flag still fails the run.
    let (code, err) = transpile("spinel", &root, true);
    assert_eq!(code, 1, "{err}");
    assert!(!root.join("out").exists(), "{err}");

    std::fs::remove_dir_all(root).expect("remove temporary app");
}

#[test]
fn a_downgradable_unsupported_error_still_suggests_allow_unsupported() {
    let root = app("uuid_key", ", id: :uuid", "");

    let (code, err) = transpile("go", &root, false);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("non_integer_primary_key not supported (go)"), "{err}");
    assert!(err.contains(HINT), "{err}");

    // And the hint is true: the flag writes the output.
    let (code, err) = transpile("go", &root, true);
    assert_eq!(code, 0, "{err}");
    assert!(root.join("out").is_dir(), "{err}");

    std::fs::remove_dir_all(root).expect("remove temporary app");
}
