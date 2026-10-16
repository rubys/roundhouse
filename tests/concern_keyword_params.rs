//! A concern's keyword-parameter helper, spliced into a controller,
//! keeps its keywords, as a controller's own helper does since #137.

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
    let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
    assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
}

const CONCERN: &str = r#"module Filterable
  extend ActiveSupport::Concern

  def query_flag(name:, cast: :string, fallback: nil, **options)
    value = params[name]
    return fallback if value.nil?

    cast == :boolean ? value.to_s == "true" : value
  end
end
"#;

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  include Filterable

  def index
    archived = query_flag(name: :archived, cast: :boolean, fallback: false)
    render json: { archived: archived }
  end
end
"#;

#[test]
fn a_spliced_concern_helper_keeps_its_keywords() {
    let files = spinel(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/controllers/concerns/filterable.rb", CONCERN),
        ("app/controllers/widgets_controller.rb", CONTROLLER),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n"),
    ]);
    assert_parses(&files, "app/controllers/widgets_controller.rb");
    let emitted = file(&files, "app/controllers/widgets_controller.rb");
    assert!(
        emitted.contains("def query_flag(name:, cast: :string, fallback: nil, **options)"),
        "{emitted}"
    );
}

/// Only OPTIONAL keywords: the library ingest flattens the group to
/// positionals (`from_keyword`), and the splice must put them back, or
/// the includer's `gadget_call("w", url: "/g")` binds the whole keyword
/// Hash to `url`.
#[test]
fn a_spliced_concern_helper_keeps_optional_only_keywords() {
    let concern = r##"module GadgetCalls
  extend ActiveSupport::Concern

  def gadget_call(label, url: nil, verb: :get, body: "")
    return "none" if url.nil?
    "#{label} #{verb} #{url}"
  end
end
"##;
    let controller = r#"class WidgetsController < ApplicationController
  include GadgetCalls

  def index
    render plain: gadget_call("w", url: "/gadgets", verb: :put)
  end
end
"#;
    let files = spinel(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/controllers/concerns/gadget_calls.rb", concern),
        ("app/controllers/widgets_controller.rb", controller),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n"),
    ]);
    assert_parses(&files, "app/controllers/widgets_controller.rb");
    let emitted = file(&files, "app/controllers/widgets_controller.rb");
    assert!(emitted.contains("def gadget_call(label, url: nil, verb: :get, body: \"\")"), "{emitted}");
    let rbs = file(&files, "app/controllers/widgets_controller.rbs");
    assert!(rbs.contains("?url: "), "{rbs}");
}
