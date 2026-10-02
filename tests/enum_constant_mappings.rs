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
        // Sorbet alias refusal must not invalidate existing literal arrays.
        ("app/services/alias.rb", "AliasTicketStates = TicketStates\nAliasValues = TicketStates::VALUES\n"),
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

const RATING: &str = r#"class Rating < T::Enum
  enums do
    High = new("severe")
    Low = new("mild")
  end
end
"#;

#[test]
fn sorbet_serialized_members_expand_before_their_file_is_ingested() {
    use roundhouse::expr::Literal;
    for suffix in ["", ".freeze", ".freeze.freeze"] {
        let source = format!("class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h {{ |v| [v.serialize, v.serialize] }}{suffix}\nend\n");
        let app = ingest_enum(&source, &[("app/services/z_rating.rb", RATING)])
            .expect("literal Sorbet enum identity mapping");
        assert_eq!(app.models[0].enums[&roundhouse::Symbol::from("state")], vec![
            ("severe".to_string(), Literal::Str { value: "severe".to_string() }),
            ("mild".to_string(), Literal::Str { value: "mild".to_string() }),
        ]);
    }
}

#[test]
fn sorbet_mapping_resolves_lexically_without_decoys_or_prefix_fallback() {
    let local = format!("module Admin\n{RATING}end\n");
    let decoy = RATING.replace("severe", "urgent").replace("mild", "calm");
    let constants = [
        ("app/services/z_admin.rb", local.as_str()),
        ("app/services/a_rating.rb", decoy.as_str()),
    ];
    for (owner, expected) in [("Rating", vec!["severe", "mild"]), ("::Rating", vec!["urgent", "calm"])] {
        let source = format!("module Admin\n  class Ticket < ApplicationRecord\n    enum :state, {owner}.values.to_h {{ |v| [v.serialize, v.serialize] }}\n  end\nend\n");
        let app = ingest_enum(&source, &constants).expect("lexical enum");
        assert_eq!(app.models[0].enums[&roundhouse::Symbol::from("state")]
            .iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(), expected);
    }
    // Compound namespace syntax does not put Admin in lexical nesting.
    let source = "module Admin::Nested\n  class Ticket < ApplicationRecord\n    enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\n  end\nend\n";
    let app = ingest_enum(source, &constants).expect("compound nesting");
    assert_eq!(app.models[0].enums[&roundhouse::Symbol::from("state")][0].0, "urgent");
    let qualified = "class Ticket < ApplicationRecord\n  enum :state, Admin::Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let app = ingest_enum(qualified, &constants).expect("qualified enum");
    assert_eq!(app.models[0].enums[&roundhouse::Symbol::from("state")][0].0, "severe");
    for shadow in ["Rating = Object.new", "module Rating; end", "class Rating; end"] {
        let source = "module Admin\n  class Ticket < ApplicationRecord\n    enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\n  end\nend\n";
        let shadow = format!("module Admin\n  {shadow}\nend\n");
        assert_enum_gap(source, &[("app/services/rating.rb", RATING), ("app/services/shadow.rb", &shadow)]);
    }
}

#[test]
fn sorbet_mapping_refuses_dynamic_duplicate_and_reassigned_members() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for body in [
        "High = new(\"severe\"); High = new(\"mild\")",
        "High = new(\"severe\"); Low = new(\"severe\")",
        "High = new(build_value)", "High = new", "High = new(1)",
        "High = new(:severe)", "High = new(\"severe\", \"extra\")",
        "High = new(\"sev#{suffix}\")", "High = new(\"severe\") if enabled?",
        "High = new(\"severe\"); mutate_members", "High = Other.new(\"severe\")",
    ] {
        let declaration = format!("class Rating < T::Enum\n  enums do\n    {body}\n  end\nend\n");
        assert_enum_gap(model, &[("app/services/rating.rb", &declaration)]);
    }
    for override_source in [
        "Rating = Class.new", "::Rating = Class.new", "Rating ||= Class.new",
        "Rating::High = nil", "::Rating::Low ||= nil", "Rating::High &&= nil",
        "Rating::High += 1", "Rating::High, OTHER = [nil, nil]",
        "OTHER = (Rating::High = nil)", "Rating::Low = nil if enabled?",
        "class Rating; enums do; Extra = new(\"other\"); end; end",
        "class Rating; def self.values; []; end; end",
        "def Rating.values; []; end", "class << Rating; def values; []; end; end",
        "class << Rating::High; def serialize; \"wrong\"; end; end",
        "Rating.class_eval { def serialize; \"wrong\"; end }",
        "Rating.define_singleton_method(:values) { [] }",
        "Rating.const_set(:High, nil)", "Rating.remove_const(:Low)",
    ] {
        assert!(ruby_prism::parse(override_source.as_bytes()).errors().next().is_none(), "invalid repro: {override_source}");
        // Both file orders must refuse rather than retain an earlier snapshot.
        for (enum_file, override_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
            assert_enum_gap(model, &[(enum_file, RATING), (override_file, override_source)]);
        }
    }
}

#[test]
fn sorbet_mapping_refuses_nonidentity_or_dynamic_enumeration() {
    for expression in [
        "Rating.values.to_h { |v| [v.serialize, v.to_s] }",
        "Rating.values.to_h { |v| [v.to_s, v.serialize] }",
        "Rating.values.to_h { |v| [v.serialize, other.serialize] }",
        "Rating.values.to_h { |v| [v.serialize, v.serialize, v.serialize] }",
        "Rating.values.to_h { |v| log(v); [v.serialize, v.serialize] }",
        "Rating.values.to_h { |v, extra = mutate| [v.serialize, v.serialize] }",
        "Rating.values.to_h { |v, *rest| [v.serialize, v.serialize] }",
        "Rating.values.to_h { |v; extra| [v.serialize, v.serialize] }",
        "Rating.values.to_h { |v| [v.serialize(1), v.serialize] }",
        "Rating.values.to_h { |v| [v.serialize { mutate }, v.serialize] }",
        "Rating.values(1).to_h { |v| [v.serialize, v.serialize] }",
        "Rating.values { mutate }.to_h { |v| [v.serialize, v.serialize] }",
        "choose_enum.values.to_h { |v| [v.serialize, v.serialize] }",
        "Rating.values.map { |v| [v.serialize, v.serialize] }.to_h",
        "VALUES.to_h { |v| [v.serialize, v.serialize] }",
        "Rating.values.to_h { |v| [v.serialize, v.serialize] }.freeze { mutate }",
        "Rating.values.to_h { |v| [v.serialize, v.serialize] }.index_with(&:itself)",
        "Rating.values.to_h { |v| [v.serialize, v.serialize] }.index_by(&:to_s)",
        "Rating.values.to_h { |v| [v.serialize, v.serialize] }.map { |v| [v, v.to_s] }.to_h",
        "Rating.values.to_h { |v| [v.serialize, v.serialize] }.freeze.index_with(&:itself)",
    ] {
        let model = format!("class Ticket < ApplicationRecord\n  VALUES = %w[decoy]\n  enum :state, {expression}\nend\n");
        assert_enum_gap(&model, &[("app/services/rating.rb", RATING)]);
    }
}

#[test]
fn nested_sorbet_enum_is_independent_of_model_body_declaration_order() {
    for body in [
        format!("{RATING}\n  enum :state, Rating.values.to_h {{ |v| [v.serialize, v.serialize] }}"),
        format!("enum :state, Rating.values.to_h {{ |v| [v.serialize, v.serialize] }}\n{RATING}"),
    ] {
        let source = format!("class Ticket < ApplicationRecord\n{body}\nend\n");
        let app = ingest_enum(&source, &[]).expect("nested enum");
        let model = roundhouse::ingest::ingest_model(source.as_bytes(), "ticket.rb",
            &roundhouse::schema::Schema::default(), &Default::default())
            .expect("single-file ingest").expect("model");
        assert_eq!(model.enums, app.models[0].enums);
        assert_eq!(model.enums[&roundhouse::Symbol::from("state")][0].0, "severe");
    }
}

#[test]
fn application_defined_sorbet_base_is_not_assumed_to_be_the_gem() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Admin::Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let declaration = format!("module Admin\n{RATING}end\n");
    for shadow in ["module Admin; T = Object.new; end", "Admin::T = Object.new", "::T = Object.new"] {
        assert_enum_gap(model, &[("app/services/a_enum.rb", &declaration), ("app/services/z_shadow.rb", shadow)]);
    }
    let rooted = declaration.replace("< T::Enum", "< ::T::Enum");
    let app = ingest_enum(model, &[("app/services/a_enum.rb", &rooted), ("app/services/z_shadow.rb", "module Admin; T = Object.new; end")])
        .expect("explicit root base bypasses a local shadow");
    assert_eq!(app.models[0].enums[&roundhouse::Symbol::from("state")][0].0, "severe");
}

#[test]
fn normalized_sorbet_assertions_do_not_turn_computed_members_into_literals() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let declaration = RATING.replace("new(\"severe\")", "new(T.let(\"severe\", raise(\"must not disappear\")))");
    assert_enum_gap(model, &[("app/services/rating.rb", &declaration)]);
}

#[test]
fn singleton_class_mutations_do_not_leave_stale_sorbet_values() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for source in [
        "Rating.singleton_class.define_method(:values) { [] }",
        "Rating.singleton_class.class_eval { def values; []; end }",
    ] {
        for (enum_file, override_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
            assert_enum_gap(model, &[(enum_file, RATING), (override_file, source)]);
        }
    }
}

#[test]
fn qualified_reopens_resolve_lexically_before_invalidating_sorbet_values() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Catalog::Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let declaration = format!("module Catalog\n{RATING}end\n");
    let reopen = "module Wrapper\n  class Catalog::Rating\n    def self.values; [Low]; end\n  end\nend\n";
    for (enum_file, override_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
        assert_enum_gap(model, &[(enum_file, &declaration), (override_file, reopen)]);
        let declaration = format!("module Catalog\n  module Types\n{RATING}end\nend\n");
        let model = model.replace("Catalog::Rating", "Catalog::Types::Rating");
        let reopen = "module Wrapper\n  module Catalog::Types\n    class Rating\n      def self.values; [Low]; end\n    end\n  end\nend\n";
        assert_enum_gap(&model, &[(enum_file, &declaration), (override_file, reopen)]);
    }
}

#[test]
fn replacing_the_external_sorbet_enum_base_refuses_dependent_mappings() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for (enum_file, override_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
        for source in ["::T::Enum = Class.new", "T::Enum = Class.new", "T::Enum.class_eval { def serialize; \"wrong\"; end }"] {
            assert_enum_gap(model, &[(enum_file, RATING), (override_file, source)]);
        }
    }
}

#[test]
fn qualified_external_base_reopens_refuse_dependent_mappings() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let reopen = "module Wrapper\n  class T::Enum\n    def serialize; \"wrong\"; end\n  end\nend\n";
    for declaration in [RATING.to_string(), RATING.replace("< T::Enum", "< ::T::Enum")] {
        for (enum_file, override_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
            assert_enum_gap(model, &[(enum_file, &declaration), (override_file, reopen)]);
        }
    }
}

#[test]
fn external_mixins_cannot_change_folded_sorbet_serialization_or_values() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for mutation in [
        "Rating.prepend(WrongSerialization)", "Rating.include(WrongSerialization)",
        "Rating.singleton_class.prepend(WrongValues)", "Rating.extend(WrongValues)",
        "T::Enum.prepend(WrongSerialization)", "T::Enum.include(WrongSerialization)",
    ] {
        let source = format!("module WrongSerialization\n  def serialize; \"wrong\"; end\nend\nmodule WrongValues\n  def values; []; end\nend\n{mutation}\n");
        for (enum_file, override_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
            assert_enum_gap(model, &[(enum_file, RATING), (override_file, &source)]);
        }
    }
}

#[test]
fn constant_aliases_cannot_retain_stale_sorbet_metadata() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for source in [
        "AliasRating = Rating\ndef AliasRating.values; [Rating::Low]; end\n",
        "AliasHigh = Rating::High\ndef AliasHigh.serialize; \"wrong\"; end\n",
        "AliasBase = ::T::Enum\nAliasBase.class_eval { def serialize; \"wrong\"; end }\n",
        "module Holder; end\nHolder::AliasRating = ::Rating\ndef (Holder::AliasRating).values; [Rating::Low]; end\n",
        // Even a read-only direct alias stays outside the supported subset;
        // there is no second vocabulary that promises to track its uses.
        "AliasRating = Rating\n",
    ] {
        assert!(ruby_prism::parse(source.as_bytes()).errors().next().is_none(), "{source}");
        for (enum_file, alias_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
            assert_enum_gap(model, &[(enum_file, RATING), (alias_file, source)]);
        }
    }
}

#[test]
fn namespace_aliases_cannot_expose_sorbet_objects_for_mutation() {
    let namespaced = format!("module Catalog\n{RATING}end\n");
    for (receiver, declaration, source) in [
        ("Rating", RATING, "AliasSorbet = ::T\nAliasSorbet::Enum.class_eval { def serialize; \"wrong\"; end }\n"),
        ("Catalog::Rating", namespaced.as_str(), "AliasCatalog = Catalog\nAliasCatalog::Rating.define_singleton_method(:values) { [] }\n"),
    ] {
        assert!(ruby_prism::parse(source.as_bytes()).errors().next().is_none(), "{source}");
        let model = format!("class Ticket < ApplicationRecord\n  enum :state, {receiver}.values.to_h {{ |v| [v.serialize, v.serialize] }}\nend\n");
        for (enum_file, alias_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
            assert_enum_gap(&model, &[(enum_file, declaration), (alias_file, source)]);
        }
    }
}

#[test]
fn compound_and_parenthesized_aliases_cannot_retain_sorbet_metadata() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for (alias, receiver) in [
        ("AliasRating ||= Rating", "AliasRating"),
        ("AliasRating = (Rating)", "AliasRating"),
        ("AliasRating = nil\nAliasRating ||= Rating", "AliasRating"),
        ("AliasRating = Object.new\nAliasRating &&= Rating", "AliasRating"),
        ("module Holder; end\nHolder::AliasRating ||= ::Rating", "(Holder::AliasRating)"),
    ] {
        let source = format!("{alias}\ndef {receiver}.values; [Rating::Low]; end\n");
        assert!(ruby_prism::parse(source.as_bytes()).errors().next().is_none(), "{source}");
        for (enum_file, alias_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
            assert_enum_gap(model, &[(enum_file, RATING), (alias_file, &source)]);
        }
    }
}

#[test]
fn receiver_rebinding_blocks_cannot_replace_folded_sorbet_surfaces() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for method in ["instance_eval", "instance_exec", "class_exec", "module_exec"] {
        let source = format!("Rating.{method} do\n  def self.values; [Low]; end\nend\n");
        assert!(ruby_prism::parse(source.as_bytes()).errors().next().is_none(), "{source}");
        for (enum_file, override_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
            assert_enum_gap(model, &[(enum_file, RATING), (override_file, &source)]);
        }
    }
}

#[test]
fn reflective_calls_and_compound_alias_expressions_stay_gaps() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for source in [
        "Rating.singleton_class.attr_reader :values",
        "Rating.send(:define_singleton_method, :values) { [Rating::Low] }",
        "Rating.public_send(:define_singleton_method, :values) { [Rating::Low] }",
        "(Rating.singleton_class).define_method(:values) { [Rating::Low] }",
        "AliasRating, Unused = Rating, nil\ndef AliasRating.values; [Rating::Low]; end",
        "AliasRating = T.let(Rating, T.class_of(Rating))\ndef AliasRating.values; [Rating::Low]; end",
        "Aliases = [Rating]\nAliases.first.define_singleton_method(:values) { [Rating::Low] }",
        "AliasSorbet = T.itself\nAliasSorbet::Enum.class_eval { def serialize; \"wrong\"; end }",
        "rating = Rating\nrating.define_singleton_method(:values) { [Rating::Low] }",
        "def mutate(enum); enum.define_singleton_method(:values) { [enum::Low] }; end\nmutate(Rating)",
    ] {
        assert!(ruby_prism::parse(source.as_bytes()).errors().next().is_none(), "{source}");
        for (enum_file, override_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
            assert_enum_gap(model, &[(enum_file, RATING), (override_file, source)]);
        }
    }
}

#[test]
fn canonical_reads_and_unrelated_namespace_calls_do_not_change_mappings() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let source = "module Catalog\n  LABEL = T.let(\"label\", String)\nend\nCatalog.name\nRating.values\nRating.deserialize(\"severe\")\nRating.try_deserialize(\"mild\")\nRating::High.serialize\nRating::Low.to_s\ntext = Rating::High.serialize\nRating::High.equal?(Rating::Low)\n";
    let app = ingest_enum(model, &[("app/services/rating.rb", RATING), ("app/services/reads.rb", source)])
        .expect("canonical reads do not escape or mutate the enum");
    assert_eq!(app.models[0].enums[&roundhouse::Symbol::from("state")], vec![
        ("severe".into(), roundhouse::expr::Literal::Str { value: "severe".into() }),
        ("mild".into(), roundhouse::expr::Literal::Str { value: "mild".into() }),
    ]);
}

#[test]
fn type_argument_references_remain_a_conservative_folding_gap() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    assert_enum_gap(model, &[("app/services/rating.rb", RATING),
        ("app/services/consumer.rb", "class Consumer\n  sig { returns(Rating) }\n  def rating; Rating::High; end\nend\n")]);
}

#[test]
fn canonical_read_results_cannot_expose_enum_objects_for_mutation() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for source in [
        "Rating.values.first.class.define_method(:serialize) { \"critical\" }",
        "Rating.values.each { |value| value.class.define_method(:serialize) { \"critical\" } }",
        "rating = Rating.deserialize(\"severe\")\nrating.class.define_method(:serialize) { \"critical\" }",
        "ratings = (Rating.values)\nratings.first.class.define_method(:serialize) { \"critical\" }",
        "def mutate(values); values.first.class.define_method(:serialize) { \"critical\" }; end\nmutate(Rating.values)",
        "def ratings; Rating.values; end\nratings.first.class.define_method(:serialize) { \"critical\" }",
        "def rating; return Rating::High; end\nrating.class.define_method(:serialize) { \"critical\" }",
        "ratings = begin; Rating.values; end\nratings.first.class.define_method(:serialize) { \"critical\" }",
        "def ratings; enabled? ? Rating.values : []; end\nratings.first.class.define_method(:serialize) { \"critical\" }",
    ] {
        assert!(ruby_prism::parse(source.as_bytes()).errors().next().is_none(), "{source}");
        for (enum_file, use_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
            assert_enum_gap(model, &[(enum_file, RATING), (use_file, source)]);
        }
    }
}

#[test]
fn invalidated_consumers_cannot_keep_order_dependent_argument_exemptions() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let other = RATING.replace("Rating", "OtherRating");
    let usage = "class Consumer\n  def self.change; OtherRating::High.equal?(Rating); end\nend\n";
    let mutation = "def (OtherRating::High).equal?(enum)\n  enum.define_singleton_method(:values) { [] }\nend\nConsumer.change\n";
    for (use_file, mutation_file) in [("app/services/a_use.rb", "app/services/z_override.rb"), ("app/services/z_use.rb", "app/services/a_override.rb")] {
        assert_enum_gap(model, &[
            ("app/services/rating.rb", RATING),
            ("app/services/other_rating.rb", &other),
            (use_file, usage), (mutation_file, mutation),
        ]);
    }
}

#[test]
fn consumed_read_invalidations_propagate_through_multiple_consumers() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let other = RATING.replace("Rating", "OtherRating");
    let third = RATING.replace("Rating", "ThirdRating");
    for sources in [
        ["OtherRating::High.equal?(Rating)", "ThirdRating::High.equal?(OtherRating)",
            "def (ThirdRating::High).equal?(enum); enum.define_singleton_method(:values) { [] }; end"],
        ["def (ThirdRating::High).equal?(enum); enum.define_singleton_method(:values) { [] }; end",
            "ThirdRating::High.equal?(OtherRating)", "OtherRating::High.equal?(Rating)"],
    ] {
        assert_enum_gap(model, &[
            ("app/services/rating.rb", RATING),
            ("app/services/other_rating.rb", &other),
            ("app/services/third_rating.rb", &third),
            ("app/services/a.rb", sources[0]),
            ("app/services/b.rb", sources[1]),
            ("app/services/z.rb", sources[2]),
        ]);
    }
}

#[test]
fn consumed_nonlibrary_sources_cannot_mutate_folded_enum_inputs() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let mutation = "module WrongSerialization\n  def serialize; \"critical\"; end\nend\nRating.prepend(WrongSerialization)\n";
    for path in ["config/initializers/rating.rb", "app/controllers/ratings_controller.rb", "app/helpers/ratings_helper.rb"] {
        assert_enum_gap(model, &[("app/services/rating.rb", RATING), (path, mutation)]);
    }
}

#[test]
fn mutation_observation_does_not_admit_inputs_from_nonlibrary_sources() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let array_model = "class Ticket < ApplicationRecord\n  enum :state, TicketStates::VALUES.index_with(&:itself)\nend\n";
    for path in ["config/initializers/rating.rb", "app/controllers/ratings_controller.rb", "app/helpers/ratings_helper.rb"] {
        assert_enum_gap(model, &[(path, RATING)]);
        assert_enum_gap(array_model, &[(path, TICKET_STATES)]);
    }
}

#[test]
fn inert_observed_sources_preserve_literal_mapping_and_lexical_mutations() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let inert = "module Observer\n  VALUES = %w[urgent quiet]\n  def label; Rating::High.serialize; end\nend\n";
    for path in ["config/initializers/rating.rb", "app/controllers/ratings_controller.rb", "app/helpers/ratings_helper.rb"] {
        let app = ingest_enum(model, &[("app/services/rating.rb", RATING), (path, inert)])
            .expect("an unrelated excluded constant and scalar read are inert");
        assert_eq!(app.models[0].enums[&roundhouse::Symbol::from("state")][0].0, "severe");
    }
    let decoy = format!("module Admin\n{}end\n", RATING.replace("severe", "urgent").replace("mild", "quiet"));
    let mutation = "module WrongSerialization; def serialize; \"critical\"; end; end\nmodule Admin; Rating.prepend(WrongSerialization); end\n";
    for declaration in ["app/services/a_rating.rb", "app/services/z_rating.rb"] {
        let constants = [(declaration, RATING), ("app/services/admin.rb", decoy.as_str()),
            ("config/initializers/rating.rb", mutation)];
        let app = ingest_enum(model, &constants).expect("only the lexically resolved decoy was mutated");
        assert_eq!(app.models[0].enums[&roundhouse::Symbol::from("state")][0].0, "severe");
        assert_enum_gap(&model.replace("Rating.values", "Admin::Rating.values"), &constants);
    }
}

#[test]
fn an_explicit_input_root_is_not_replayed_as_an_observed_source() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let app = ingest_enum(model, &[
        ("config/application.rb", "config.autoload_paths += %w[app/helpers]\n"),
        ("app/helpers/rating.rb", RATING),
    ]).expect("existing explicit declaration-root admission is unchanged");
    assert_eq!(app.models[0].enums[&roundhouse::Symbol::from("state")][0].0, "severe");
}

#[test]
fn identity_namespace_results_cannot_escape_the_mutation_guard() {
    let namespaced = format!("module Catalog\n{RATING}end\n");
    for (receiver, declaration, mutation) in [
        ("Rating", RATING, "base = T.itself\nbase::Enum.class_eval { def serialize; \"critical\"; end }"),
        ("Rating", RATING, "base = T.freeze\nbase::Enum.class_eval { def serialize; \"critical\"; end }"),
        ("Rating", RATING, "def mutate(base); base::Enum.class_eval { def serialize; \"critical\"; end }; end\nmutate(T.itself)"),
        ("Catalog::Rating", namespaced.as_str(), "catalog = Catalog.itself\ncatalog::Rating.define_singleton_method(:values) { [] }"),
        ("Catalog::Rating", namespaced.as_str(), "catalog = Catalog.freeze\ncatalog::Rating.define_singleton_method(:values) { [] }"),
    ] {
        let model = format!("class Ticket < ApplicationRecord\n  enum :state, {receiver}.values.to_h {{ |v| [v.serialize, v.serialize] }}\nend\n");
        assert!(ruby_prism::parse(mutation.as_bytes()).errors().next().is_none(), "{mutation}");
        for (enum_file, use_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
            assert_enum_gap(&model, &[(enum_file, declaration), (use_file, mutation)]);
        }
    }
}

#[test]
fn yielding_and_copied_namespaces_cannot_hide_enum_mutations() {
    let namespaced = format!("module Catalog\n{RATING}end\n");
    for (receiver, declaration, namespace, child) in [
        ("Rating", RATING, "T", "Enum"),
        ("Catalog::Rating", namespaced.as_str(), "Catalog", "Rating"),
    ] {
        let model = format!("class Ticket < ApplicationRecord\n  enum :state, {receiver}.values.to_h {{ |v| [v.serialize, v.serialize] }}\nend\n");
        for method in ["tap", "then", "yield_self", "dup", "clone"] {
            // Discarding the result does not make yielding safe; copied
            // modules also share the original child constant objects.
            let mutation = if matches!(method, "dup" | "clone") {
                format!("namespace = {namespace}.{method}\nnamespace::{child}.class_eval {{ def serialize; \"critical\"; end }}")
            } else {
                format!("{namespace}.{method} {{ |namespace| namespace::{child}.class_eval {{ def serialize; \"critical\"; end }} }}")
            };
            assert!(ruby_prism::parse(mutation.as_bytes()).errors().next().is_none(), "{mutation}");
            for (enum_file, use_file) in [
                ("app/services/a.rb", "app/services/z.rb"),
                ("app/services/z.rb", "app/services/a.rb"),
                ("app/services/rating.rb", "config/initializers/rating.rb"),
            ] {
                assert_enum_gap(&model, &[(enum_file, declaration), (use_file, &mutation)]);
            }
        }
    }
}

#[test]
fn namespace_reflection_and_enumerators_cannot_expose_mutable_enum_children() {
    let namespaced = format!("module Catalog\n{RATING}end\n");
    for (receiver, declaration, namespace, child) in [
        ("Rating", RATING, "T", "Enum"),
        ("Catalog::Rating", namespaced.as_str(), "Catalog", "Rating"),
    ] {
        let model = format!("class Ticket < ApplicationRecord\n  enum :state, {receiver}.values.to_h {{ |v| [v.serialize, v.serialize] }}\nend\n");
        for exposure in ["ancestors.first", "method(:itself).receiver", "public_method(:itself).receiver"] {
            let mutation = format!("namespace = {namespace}.{exposure}\nnamespace::{child}.class_eval {{ def serialize; \"critical\"; end }}");
            assert!(ruby_prism::parse(mutation.as_bytes()).errors().next().is_none(), "{mutation}");
            for (enum_file, use_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
                assert_enum_gap(&model, &[(enum_file, declaration), (use_file, &mutation)]);
            }
        }
        for method in ["to_enum", "enum_for"] {
            let mutation = format!("{namespace}.{method}(:tap).each {{ |namespace| namespace::{child}.class_eval {{ def serialize; \"critical\"; end }} }}");
            assert!(ruby_prism::parse(mutation.as_bytes()).errors().next().is_none(), "{mutation}");
            for path in ["app/services/use.rb", "config/initializers/rating.rb"] {
                assert_enum_gap(&model, &[("app/services/rating.rb", declaration), (path, &mutation)]);
            }
        }
    }
}

#[test]
fn namespace_body_self_and_implicit_calls_cannot_hide_mutations() {
    let declaration = format!("module Catalog\n{RATING}end\n");
    let model = "class Ticket < ApplicationRecord\n  enum :state, Catalog::Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for body in [
        "self.tap { |ns| ns::Rating.class_eval { def serialize; \"critical\"; end } }",
        "tap { |ns| ns::Rating.class_eval { def serialize; \"critical\"; end } }",
        "ns = self; ns::Rating.class_eval { def serialize; \"critical\"; end }",
        "[1].each { ns = self; ns::Rating.class_eval { def serialize; \"critical\"; end } }",
        "def self.identity; self; end; ns = identity; ns::Rating.class_eval { def serialize; \"critical\"; end }",
        "ns = (module ::Catalog; self; end); ns::Rating.class_eval { def serialize; \"critical\"; end }",
        "ns = (module ::Catalog; self.freeze; end); ns::Rating.class_eval { def serialize; \"critical\"; end }",
    ] {
        let mutation = format!("module Catalog\n{body}\nend\n");
        assert!(ruby_prism::parse(mutation.as_bytes()).errors().next().is_none(), "{mutation}");
        for path in ["app/services/a.rb", "app/services/z.rb", "config/initializers/rating.rb"] {
            assert_enum_gap(model, &[("app/services/m_rating.rb", &declaration), (path, &mutation)]);
        }
    }
}

#[test]
fn inherited_namespace_constants_expose_the_original_enum() {
    let declaration = format!("class Catalog\n{RATING}end\n");
    let model = "class Ticket < ApplicationRecord\n  enum :state, Catalog::Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let mutation = "class AliasCatalog < Catalog; end\nAliasCatalog::Rating.class_eval { def serialize; \"critical\"; end }";
    for (enum_file, use_file) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
        assert_enum_gap(model, &[(enum_file, &declaration), (use_file, mutation)]);
    }
}

#[test]
fn unproven_namespace_calls_remain_a_conservative_mapping_gap() {
    let declaration = format!("module Catalog\n{RATING}end\n");
    let model = "class Ticket < ApplicationRecord\n  enum :state, Catalog::Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for call in ["Catalog.autoload(:Rating, \"./replacement\")", "Catalog.unknown_operation"] {
        for path in ["app/services/a.rb", "app/services/z.rb", "config/initializers/rating.rb"] {
            assert_enum_gap(model, &[("app/services/m_rating.rb", &declaration), (path, call)]);
        }
    }
}

#[test]
fn proven_namespace_scalar_reads_and_non_escaping_freeze_remain_inert() {
    let declaration = format!("module Catalog\n{RATING}end\n");
    let model = "class Ticket < ApplicationRecord\n  enum :state, Catalog::Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let reads = "label = Catalog.name\nCatalog.freeze\nmodule Catalog\n  self.name\n  name\n  self.freeze\nend\n";
    for path in ["app/services/a.rb", "app/services/z.rb", "config/initializers/rating.rb"] {
        let app = ingest_enum(model, &[("app/services/m_rating.rb", &declaration), (path, reads)])
            .expect("scalar reads and shallow freeze do not expose child objects");
        assert_eq!(app.models[0].enums[&roundhouse::Symbol::from("state")][0],
            ("severe".into(), roundhouse::expr::Literal::Str { value: "severe".into() }));
    }
}

#[test]
fn registered_consumed_sources_cannot_escape_enum_validation() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for (path, before, after) in [
        ("db/seeds.rb", "", "\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n", "\n"),
        ("config/application.rb", "module Synthetic\nclass Application < Rails::Application\ndef self.enum_probe\n", "\nend\nend\nend\n"),
        ("test/models/enum_probe_test.rb", "class EnumProbeTest < ActiveSupport::TestCase\ntest \"probe\" do\n", "\nend\nend\n"),
    ] {
        let inert = format!("{before}Rating::High.serialize{after}");
        let mutation = format!("{before}Rating.define_singleton_method(:values) {{ [] }}{after}");
        for enum_file in ["app/services/a_rating.rb", "app/services/z_rating.rb"] {
            let app = ingest_enum(model, &[(enum_file, RATING), (path, &inert)])
                .expect("the unaffected mapping still ingests");
            assert!(app.sources.iter().any(|source| source.path == path && source.text == inert),
                "{path} must actually enter the original consumed-source registry");
            assert_eq!(app.models[0].enums[&roundhouse::Symbol::from("state")][0],
                ("severe".into(), roundhouse::expr::Literal::Str { value: "severe".into() }));
            assert_enum_gap(model, &[(enum_file, RATING), (path, &mutation)]);
        }
    }
}

#[test]
fn late_invalidations_revisit_original_consumer_facts() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let other = RATING.replace("Rating", "OtherRating");
    for (declaration, usage) in [("app/services/a.rb", "app/services/z.rb"), ("app/services/z.rb", "app/services/a.rb")] {
        assert_enum_gap(model, &[
            ("app/services/rating.rb", RATING),
            (declaration, &other),
            (usage, "OtherRating::High.equal?(Rating)"),
            ("db/seeds.rb", "def (OtherRating::High).equal?(enum); enum.define_singleton_method(:values) { [] }; end"),
        ]);
    }
}

#[test]
fn late_validation_reports_only_invalid_enum_declarations_under_survey() {
    use roundhouse::ingest::survey;

    let mut unrelated_baseline = None;
    for declaration in [
        "enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }",
        "enum state: Rating.values.to_h { |v| [v.serialize, v.serialize] }, _prefix: true",
    ] {
        let model = format!("module Tickets\nclass Ticket < ApplicationRecord\n{declaration}\nend\nend\n");
        for (observation, expected_enum_gaps) in [
            ("Rating::High.serialize", 0),
            ("Rating.define_singleton_method(:values) { [] }", 1),
        ] {
            survey::activate();
            let result = ingest_enum(&model, &[
                ("app/services/rating.rb", RATING),
                ("db/seeds.rb", observation),
                ("config/routes.rb", "Rails.application.routes.draw do\n  devise_for :users\nend\n"),
            ]);
            let gaps = survey::drain();
            result.expect("survey continues while reporting unsupported mappings");
            let (enum_gaps, mut unrelated): (Vec<_>, Vec<_>) = gaps.iter()
                .map(ToString::to_string).partition(|gap| gap.contains("enum :state mapping"));
            assert_eq!(enum_gaps.len(), expected_enum_gaps, "{gaps:?}");
            assert!(unrelated.iter().any(|gap| gap.contains("devise_for")), "{gaps:?}");
            unrelated.sort();
            if let Some(baseline) = &unrelated_baseline {
                assert_eq!(&unrelated, baseline, "enum validation must not repeat unrelated diagnostics");
            } else {
                unrelated_baseline = Some(unrelated);
            }
        }
    }
}

#[test]
fn replaced_consumer_namespaces_revoke_read_exemptions() {
    let model = "class Ticket < ApplicationRecord\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    let other = format!("module Catalog\n{}end\n", RATING.replace("Rating", "OtherRating"));
    let consumer = "class Consumer\n  def self.change; Catalog::OtherRating::High.equal?(Rating); end\nend\n";
    let replacement = "module Replacement\n  module OtherRating\n    module High\n      def self.equal?(enum); enum.define_singleton_method(:values) { [] }; end\n    end\n  end\nend\n";
    for path in ["app/services/a_replace.rb", "app/services/z_replace.rb", "config/initializers/replacement.rb"] {
        assert_enum_gap(model, &[
            ("app/services/rating.rb", RATING), ("app/services/catalog.rb", &other),
            ("app/services/consumer.rb", consumer), ("app/services/replacement.rb", replacement),
            (path, "Catalog = Replacement\nConsumer.change\n"),
        ]);
    }
}

#[test]
fn inherited_enum_shadow_cannot_fall_back_to_an_unrelated_root() {
    let base = format!("class BaseTicket < ApplicationRecord\n  self.abstract_class = true\n{}end\n",
        RATING.replace("severe", "urgent").replace("mild", "calm"));
    let model = "class Ticket < BaseTicket\n  enum :state, Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for path in ["app/models/a_base.rb", "app/models/z_base.rb"] {
        let constants = [(path, base.as_str()), ("app/services/rating.rb", RATING)];
        let explicit = model.replace("Rating.values", "::Rating.values");
        let app = ingest_enum(&explicit, &constants).expect("root qualification bypasses inherited shadow");
        let ticket = app.models.iter().find(|model| model.name.0.as_str() == "Ticket").unwrap();
        assert_eq!(ticket.enums[&roundhouse::Symbol::from("state")][0].0, "severe");
        assert_enum_gap(model, &constants);
    }
}

#[test]
fn inherited_sorbet_base_shadows_are_not_the_external_gem_base() {
    let holder = format!("class Holder < Parent\n{RATING}end\n");
    let model = "class Ticket < ApplicationRecord\n  enum :state, Holder::Rating.values.to_h { |v| [v.serialize, v.serialize] }\nend\n";
    for (parent, declaration) in [("app/services/a_parent.rb", "app/services/z_holder.rb"), ("app/services/z_parent.rb", "app/services/a_holder.rb")] {
        for mutation in ["", "::T::Enum.class_eval { def serialize; \"critical\"; end }"] {
            assert_enum_gap(model, &[
                (parent, "class Parent\n  T = ::T\nend\n"),
                (declaration, &holder), ("config/initializers/mutation.rb", mutation),
            ]);
        }
    }
}
