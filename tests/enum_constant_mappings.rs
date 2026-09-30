//! An enum mapping named by a class-body constant, or written as a
//! frozen literal, expands like a literal one. A label that is not a
//! Ruby identifier gets no methods.

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

const ORDER: &str = r#"class Order < ApplicationRecord
  STATUSES = %i[draft confirmed shipped].freeze

  enum :status, STATUSES
  enum :kind, %i[purchase rental].freeze
  enum :arch, %i[x86 32bits 64bits]
end
"#;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do
  create_table "orders", force: :cascade do |t|
    t.integer "status", default: 0
    t.integer "kind", default: 0
    t.integer "arch", default: 0
  end
end
"#;

fn order() -> String {
    let files = spinel(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/order.rb", ORDER),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]);
    assert_parses(&files, "app/models/order.rb");
    file(&files, "app/models/order.rb").to_string()
}

#[test]
fn a_constant_mapping_expands() {
    let order = order();
    for m in ["def draft?", "def shipped!", "def self.confirmed"] {
        assert!(order.contains(m), "missing {m}:\n{order}");
    }
}

#[test]
fn a_frozen_literal_mapping_expands() {
    let order = order();
    assert!(order.contains("def rental?"), "{order}");
    assert!(order.contains("def self.purchase"), "{order}");
}

#[test]
fn a_non_identifier_label_gets_no_methods() {
    let order = order();
    assert!(order.contains("def x86?"), "{order}");
    assert!(!order.contains("def 32bits"), "{order}");
}

fn ingest_enum(model: &str, constants: &[(&str, &str)]) -> Result<roundhouse::App, roundhouse::ingest::IngestError> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = constants.iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
    tree.insert(PathBuf::from("app/models/ticket.rb"), model.as_bytes().to_vec());
    ingest_app_from_tree(tree)
}

fn assert_enum_gap(model: &str, constants: &[(&str, &str)]) {
    let error = ingest_enum(model, constants).err()
        .unwrap_or_else(|| panic!("mapping must remain unsupported: {constants:?}\n{model}"));
    assert!(error.to_string().contains("enum :state mapping"), "unexpected failure: {error}");
}

const TICKET_STATES: &str = "module TicketStates\n  VALUES = %w[draft active archived].freeze\nend\n";

#[test]
fn qualified_cross_file_arrays_keep_indices_or_identity_strings() {
    use roundhouse::expr::Literal;
    use roundhouse::Symbol;

    // Services are ingested AFTER models: expansion cannot depend on
    // directory/file order. A same-named constant elsewhere must not win.
    let constants = [
        ("app/services/ticket_states.rb", TICKET_STATES),
        ("app/services/other.rb", "module Other\n  VALUES = %w[wrong].freeze\nend\n"),
    ];
    for suffix in ["", ".index_with(&:itself)", ".index_by(&:to_s)", ".map { |v| [v, v.to_s] }.to_h"] {
        let source = format!("class Ticket < ApplicationRecord\n  enum :state, TicketStates::VALUES{suffix}\nend\n");
        let app = ingest_enum(&source, &constants).expect("qualified array ingests strictly");
        let expected: Vec<_> = ["draft", "active", "archived"].iter().enumerate()
            .map(|(i, label)| (label.to_string(), if suffix.is_empty() {
                Literal::Int { value: i as i64 }
            } else {
                Literal::Str { value: label.to_string() }
            }))
            .collect();
        assert_eq!(app.models[0].enums.get(&Symbol::from("state")), Some(&expected), "{suffix}");
    }
}

#[test]
fn qualified_arrays_resolve_lexically_and_absolute_paths_bypass_shadowing() {
    use roundhouse::expr::Literal;
    use roundhouse::Symbol;

    let constants = [
        ("app/services/ticket_states.rb", TICKET_STATES),
        ("app/services/admin.rb", "module Admin\n  module TicketStates\n    VALUES = %w[review closed].freeze\n  end\nend\n"),
    ];
    for (path, labels) in [("TicketStates", vec!["review", "closed"]), ("::TicketStates", vec!["draft", "active", "archived"])] {
        let source = format!("module Admin\n  class Ticket < ApplicationRecord\n    enum :state, {path}::VALUES.index_with(&:itself)\n  end\nend\n");
        let app = ingest_enum(&source, &constants).expect("lexical qualified array");
        let expected: Vec<_> = labels.into_iter()
            .map(|label| (label.to_string(), Literal::Str { value: label.to_string() }))
            .collect();
        assert_eq!(app.models[0].enums.get(&Symbol::from("state")), Some(&expected));
    }
    let source = "module Admin\n  class Ticket < ApplicationRecord\n    enum :state, TicketStates::VALUES.index_with(&:itself)\n  end\nend\n";
    let shadow = [("app/services/admin.rb", "module Admin\n  module TicketStates\n  end\nend\n"), constants[0]];
    assert_enum_gap(source, &shadow);
}

#[test]
fn a_compound_namespace_does_not_add_its_prefix_to_lexical_lookup() {
    let constants = [
        ("app/services/ticket_states.rb", TICKET_STATES),
        ("app/services/admin.rb", "module Admin\n  module TicketStates\n    VALUES = %w[wrong].freeze\n  end\nend\n"),
    ];
    let source = "module Admin::Nested\n  class Ticket < ApplicationRecord\n    enum :state, TicketStates::VALUES.index_with(&:itself)\n  end\nend\n";
    let app = ingest_enum(source, &constants).expect("compound namespace");
    let mapping = &app.models[0].enums[&roundhouse::Symbol::from("state")];
    assert_eq!(mapping.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(), vec!["draft", "active", "archived"]);
}

#[test]
fn nonliteral_or_reassigned_cross_file_inputs_stay_unsupported() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, TicketStates::VALUES.index_with(&:itself)\nend\n";
    for value in ["build_states", "OTHER", "%w[draft].freeze(1)", "%i[draft active]", "%w[draft].map(&:upcase)"] {
        let source = format!("module TicketStates\n  VALUES = {value}\nend\n");
        assert_enum_gap(model, &[("app/services/ticket_states.rb", &source)]);
    }
    let reassign = [
        ("app/services/ticket_states.rb", TICKET_STATES),
        ("app/services/reopen.rb", "module TicketStates\n  VALUES = %w[other].freeze\nend\n"),
    ];
    assert_enum_gap(model, &reassign);
    for suffix in [".index_with(&:length)", ".index_with { |v| v.upcase }", ".index_with", ".map { |v| [v, v.length] }.to_h", ".slice(0)"] {
        let source = format!("class Ticket < ApplicationRecord\n  enum :state, TicketStates::VALUES{suffix}\nend\n");
        assert_enum_gap(&source, &[("app/services/ticket_states.rb", TICKET_STATES)]);
    }
    let dynamic_owner = "class Ticket < ApplicationRecord\n  enum :state, choose_namespace::VALUES.index_with(&:itself)\nend\n";
    assert_enum_gap(dynamic_owner, &[("app/services/values.rb", "VALUES = %w[wrong].freeze\n")]);
}

#[test]
fn every_constant_reassignment_form_stays_unsupported() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, TicketStates::VALUES.index_with(&:itself)\nend\n";
    for assignment in [
        "TicketStates::VALUES = %w[closed]",
        "::TicketStates::VALUES = %w[closed]",
        "TicketStates:: VALUES = %w[closed]",
        "TicketStates::VALUES ||= %w[closed]",
        "TicketStates::VALUES &&= %w[closed]",
        "TicketStates::VALUES += %w[closed]",
        "module TicketStates; VALUES ||= %w[closed]; end",
        "module TicketStates; VALUES &&= %w[closed]; end",
        "module TicketStates; VALUES += %w[closed]; end",
        "TicketStates::VALUES, OTHER = [%w[closed], nil]",
        "module TicketStates; VALUES, OTHER = [%w[closed], nil]; end",
        "OTHER = (TicketStates::VALUES = %w[closed])",
        "OTHER ||= (TicketStates::VALUES = %w[closed])",
        "OTHER, ANOTHER = [(TicketStates::VALUES = %w[closed]), nil]",
        "TicketStates = Module.new",
        "if ENV['REDEFINE']; TicketStates::VALUES = %w[closed]; end",
        "module TicketStates; VALUES = %w[closed] if ENV['REDEFINE']; end",
    ] {
        assert_enum_gap(model, &[
            ("app/services/ticket_states.rb", TICKET_STATES),
            ("app/services/reopen.rb", assignment),
        ]);
    }
}

#[test]
fn qualified_reassignments_respect_lexical_shadowing_and_file_order() {
    let constants = [
        ("app/services/ticket_states.rb", TICKET_STATES),
        ("app/services/admin.rb", "module Admin\n  module TicketStates\n    VALUES = %w[review closed].freeze\n  end\nend\n"),
        // This file sorts before the declarations. It changes only Admin's
        // constant; the root mapping must not be rejected or overwritten.
        ("app/services/aaa_reopen.rb", "module Admin; TicketStates::VALUES = %w[other]; end"),
    ];
    let rooted = "class Ticket < ApplicationRecord\n  enum :state, ::TicketStates::VALUES.index_with(&:itself)\nend\n";
    let app = ingest_enum(rooted, &constants).expect("unrelated namespace assignment");
    let mapping = &app.models[0].enums[&roundhouse::Symbol::from("state")];
    assert_eq!(mapping.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(), vec!["draft", "active", "archived"]);
    let lexical = "module Admin\n  class Ticket < ApplicationRecord\n    enum :state, TicketStates::VALUES.index_with(&:itself)\n  end\nend\n";
    assert_enum_gap(lexical, &constants);
}

#[test]
fn class_local_aliases_do_not_expand_cross_file_support() {
    let model = "class Ticket < ApplicationRecord\n  LOCAL = TicketStates::VALUES\n  enum :state, LOCAL.index_with(&:itself)\nend\n";
    assert_enum_gap(model, &[("app/services/ticket_states.rb", TICKET_STATES)]);
}

#[test]
fn ignored_lib_constants_are_not_enum_inputs() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, TicketStates::VALUES.index_with(&:itself)\nend\n";
    let constants = [
        ("config/application.rb", "config.autoload_lib(ignore: %w[assets])\n"),
        ("lib/assets/ticket_states.rb", TICKET_STATES),
    ];
    assert_enum_gap(model, &constants);
}
