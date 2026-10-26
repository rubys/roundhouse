//! A spin package added without bcrypt still gets a `[dependencies]`
//! header. The manifest's own comment mentions the header, so a
//! substring test found it and the header was never written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";

/// The spinel tree for a small app: the two base classes plus `files`.
#[allow(dead_code)]
fn spinel(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("app/models/application_record.rb"), APPLICATION_RECORD.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/controllers/application_controller.rb"),
        APPLICATION_CONTROLLER.as_bytes().to_vec(),
    );
    for (path, content) in files {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), BuildTarget::Spinel).expect("spinel files")
}

#[allow(dead_code)]
fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{path} not emitted"))
        .1
}

#[allow(dead_code)]
fn assert_parses(files: &[(String, String)], path: &str) {
    let source = file(files, path);
    let result = ruby_prism::parse(source.as_bytes());
    let errors: Vec<String> = result.errors().map(|e| String::from_utf8_lossy(e.message()).into_owned()).collect();
    assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
}

#[test]
fn an_rqrcode_only_manifest_declares_its_dependency_table() {
    let files = spinel(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n  end\nend\n"),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  def qr_svg\n    RQRCode::QRCode.new(\"widget:#{id}\").as_svg(module_size: 3)\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]);
    let manifest = file(&files, "spin.toml");
    let lines: Vec<&str> = manifest.lines().map(str::trim).collect();
    let header = lines.iter().position(|l| *l == "[dependencies]").unwrap_or_else(|| panic!("{manifest}"));
    let rqrcode = lines.iter().position(|l| l.starts_with("rqrcode = ")).unwrap_or_else(|| panic!("{manifest}"));
    assert!(header < rqrcode, "{manifest}");
}
