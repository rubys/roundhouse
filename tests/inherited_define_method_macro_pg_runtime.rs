//! PostgreSQL: inherited define method macro, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

use roundhouse::analyze::{diagnose, DiagnosticKind};

const STRUCTURE: &str = r#"CREATE TABLE changes (
    id bigserial NOT NULL,
    handle character varying,
    CONSTRAINT changes_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_inherited_macro
"#;

const BASE: &str = r##"module Staged
  class Base
    class << self
      #: (*Symbol keys) -> void
      def data_accessor(*keys)
        data_attributes.concat(keys)
        keys.each do |key|
          define_method("#{key}=") do |value|
            data[key] = value
          end
          define_method(key) do
            data[key]
          end
        end
      end

      #: (*Symbol keys, default: bool?) -> void
      def boolean_data_accessor(*keys, default:)
        keys.each do |key|
          define_method("#{key}=") do |value|
            data[key] = value
          end
          define_method("#{key}?") do
            value = data[key]
            value.nil? ? default : value
          end
        end
      end

      #: (*Symbol keys) -> void
      def money_data_accessor(*keys)
        keys.each do |key|
          define_method("#{key}=") do |money|
            data[key] ||= {}
            data[key][:value] = money
            public_send(key)
          end
          define_method(key) do
            data[key]
          end
        end
      end

      #: -> Array[Symbol]
      def data_attributes
        @data_attributes ||= []
      end
    end

    #: (?Hash[untyped, untyped] data) -> void
    def initialize(data = {})
      @data = data
    end

    private

    #: untyped
    attr_reader :data
  end
end
"##;

const ADD_ITEM: &str = r##"module Staged
  class AddItem < Base
    data_accessor(:sku, :quantity)
    money_data_accessor(:price)

    #: -> String
    def label
      "#{sku} x#{quantity}"
    end
  end
end
"##;

const REVERSE: &str = r#"module Staged
  class Reverse < Base
    data_accessor(:write_off_id)

    #: -> String
    def create_reference_id
      write_off_id.to_s
    end
  end
end
"#;

const DISCOUNT: &str = r#"module Staged
  class Discount < Base
    data_accessor(:fixed_amount, :reason)
    money_data_accessor(:fixed_amount)
    boolean_data_accessor(:taxable, default: true)
  end
end
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const CHANGE: &str = r#"class Change < ApplicationRecord
  HELPERS = {
    "add" => Staged::AddItem,
    "reverse" => Staged::Reverse,
  }.freeze #: Hash[untyped, untyped]

  delegate :sku, to: :staged_change_helper
  delegate :price=, to: :staged_change_helper

  #: -> (Staged::AddItem | Staged::Reverse)
  def staged_change_helper
    @staged_change_helper ||= HELPERS.fetch(handle).new({})
  end
end
"#;

const PROBE: &str = r##"class InheritedMacroProbe
  def self.lines
    out = []
    item = Staged::AddItem.new({})
    item.sku = "S1"
    item.quantity = 2
    out << "price=: #{item.public_send(:price=, 5).inspect}"
    out << "label: #{item.label}"
    out << "attributes: #{Staged::AddItem.data_attributes.inspect} #{Staged::Reverse.data_attributes.inspect}"
    out << "defined: #{Staged::AddItem.method_defined?(:sku)} #{Staged::Reverse.method_defined?(:sku)} #{Staged::Reverse.method_defined?(:write_off_id)}"
    discount = Staged::Discount.new({})
    out << "fixed_amount=: #{discount.public_send(:fixed_amount=, 7).inspect}"
    out << "taxable?: #{discount.taxable?}"
    discount.taxable = false
    out << "taxable? after: #{discount.taxable?}"
    change = Change.create!(handle: "add")
    change.staged_change_helper.sku = "Z9"
    out << "delegated: #{change.sku}"
    change.price = 3
    out << "delegated price: #{change.staged_change_helper.price.inspect}"
    begin
      Change.create!(handle: "reverse").sku
      out << "reverse sku: answered"
    rescue NoMethodError
      out << "reverse sku: NoMethodError"
    end
    out
  end
end
"##;

const EXPECTED: [&str; 10] = [
    "price=: {value: 5}",
    "label: S1 x2",
    "attributes: [:sku, :quantity] [:write_off_id]",
    "defined: true false true",
    "fixed_amount=: {value: 7}",
    "taxable?: true",
    "taxable? after: false",
    "delegated: Z9",
    "delegated price: {value: 3}",
    "reverse sku: NoMethodError",
];

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/staged/base.rb", BASE),
    ("app/models/staged/add_item.rb", ADD_ITEM),
    ("app/models/staged/reverse.rb", REVERSE),
    ("app/models/staged/discount.rb", DISCOUNT),
    ("app/models/change.rb", CHANGE),
    ("app/models/inherited_macro_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn inherited_define_method_macros_match_native_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "inherited-macro",
        &FILES,
        &emitted,
        "rh_spec_inherited_macro",
        "InheritedMacroProbe",
        &[
            "app/models/application_record",
            "app/models/staged/base",
            "app/models/staged/add_item",
            "app/models/staged/reverse",
            "app/models/staged/discount",
            "app/models/change",
            "app/models/inherited_macro_probe",
        ],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

fn ingest_files(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> =
        files.iter().map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec())).collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest")
}

fn ingest() -> roundhouse::App {
    ingest_files(&FILES)
}

fn instance_methods(app: &roundhouse::App, class: &str) -> Vec<String> {
    let mut names: Vec<String> = app
        .library_classes
        .iter()
        .filter(|lc| lc.name.0.as_str() == class)
        .flat_map(|lc| lc.methods.iter())
        .filter(|m| m.receiver == roundhouse::dialect::MethodReceiver::Instance)
        .map(|m| m.name.as_str().to_string())
        .collect();
    names.sort();
    names
}

/// Each subclass has the accessors its own calls define, and only those;
/// a redefined key keeps the later macro's method.
#[test]
#[ignore = "fork spec: postgresql"]
fn each_subclass_gets_its_own_accessors() {
    let app = ingest();
    assert_eq!(instance_methods(&app, "Staged::AddItem"), ["label", "price", "price=", "quantity", "quantity=", "sku", "sku="]);
    assert_eq!(instance_methods(&app, "Staged::Reverse"), ["create_reference_id", "write_off_id", "write_off_id="]);
    assert_eq!(
        instance_methods(&app, "Staged::Discount"),
        ["fixed_amount", "fixed_amount=", "reason", "reason=", "taxable=", "taxable?"]
    );
    let discount = app.library_classes.iter().find(|lc| lc.name.0.as_str() == "Staged::Discount").unwrap();
    let writer = discount.methods.iter().find(|m| m.name.as_str() == "fixed_amount=").unwrap();
    assert_eq!(writer.params[0].name.as_str(), "money", "money_data_accessor's writer, which runs last");
    assert_eq!(discount.unknown_calls.len(), 3, "the calls stay, for the Ruby emit to replay");
}

/// Analysis dispatches the union's delegates; one no member answers
/// (an authored mistake, which native raises NoMethodError for when
/// called) is the only failure, at its `delegate` name.
#[test]
#[ignore = "fork spec: postgresql"]
fn the_union_delegates_dispatch_and_a_missing_one_has_a_span() {
    let change = CHANGE.replace(
        "  delegate :price=, to: :staged_change_helper\n",
        "  delegate :price=, to: :staged_change_helper\n  delegate :refund_id, to: :staged_change_helper\n",
    );
    let files: Vec<(&str, &str)> =
        FILES.iter().map(|&(p, c)| if p == "app/models/change.rb" { (p, change.as_str()) } else { (p, c) }).collect();
    let mut app = ingest_files(&files);
    roundhouse::session::analyze_and_lower(&mut app);
    let failures: Vec<String> = diagnose(&app)
        .into_iter()
        .filter_map(|d| match &d.kind {
            DiagnosticKind::SendDispatchFailed { method, recv_ty } => {
                Some(format!("file{}@{} {}#{}", d.span.file.0, d.span.start, roundhouse::ide::render_ty(recv_ty), method.as_str()))
            }
            _ => None,
        })
        .collect();
    let at = change.find(":refund_id").unwrap();
    let [failure] = failures.as_slice() else { panic!("{failures:?}") };
    assert!(
        failure.ends_with(&format!("@{at} Staged::AddItem | Staged::Reverse#refund_id")) && !failure.starts_with("file0@"),
        "{failures:?}"
    );
}

/// The emitted subclass replays its macro calls and carries no copies of
/// the methods they define.
#[test]
#[ignore = "fork spec: postgresql"]
fn the_emitted_subclass_replays_its_calls_without_copies() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let item = std::fs::read_to_string(emitted.join("app/models/staged/add_item.rb")).expect("add_item.rb");
    assert!(item.contains("data_accessor(:sku, :quantity)") && item.contains("money_data_accessor(:price)"), "{item}");
    assert!(!item.contains("def sku") && !item.contains("def price"), "{item}");
    let discount = std::fs::read_to_string(emitted.join("app/models/staged/discount.rb")).expect("discount.rb");
    assert!(!discount.contains("def "), "{discount}");
}
