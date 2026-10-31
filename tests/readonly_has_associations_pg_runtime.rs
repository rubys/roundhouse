//! PostgreSQL: readonly has associations, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE batches (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT batches_pkey PRIMARY KEY (id)
);

CREATE TABLE events (
    id bigserial NOT NULL,
    subject_id bigint,
    subject_type character varying,
    verb character varying,
    CONSTRAINT events_pkey PRIMARY KEY (id)
);

CREATE TABLE stamps (
    id bigserial NOT NULL,
    event_id bigint,
    label character varying,
    CONSTRAINT stamps_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_readonly_has_associations
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true

  include Component::ReadOnlyAssociations
end
"#;

const READ_ONLY_ASSOCIATIONS: &str = r###"
module Component
  # Provides read-only versions of Active Record associations to use across components as an initial
  # step towards removing these associations entirely.
  #
  # To use, simply prefix the Active Record association macros `has_many`, `has_one` and `belongs_to`
  # with `readonly_` (e.g. `readonly_has_many`).
  #
  # The read only version of the associations do the following:
  # * All mutating methods on an association will now raise a `ReadOnlyError` when called.
  #   Specifically, these methods are
  #   * `build`
  #   * `create`
  #   * `create!`
  #   * `<<`
  #   * `push`
  #   * `concat`
  #   * `delete`
  #   * `delete_all`
  #   * `destroy`
  #   * `destroy_all`
  #   * `build_#{association}`
  #   * `create_#{association}`,
  #   * `create_#{association}!`
  #   * `#{association}=`
  #   * `#{association.to_s.singularize}_ids=`
  # * Any records returned by the association will be marked as read only. That is, calling `readonly?`
  #   on the record will return `true`.
  #
  # The point of using read only associations is to prevent the creation or mutation of records from
  # outside the component when these should instead be done through a component entrypoint. To find
  # existing areas where associations are mutated, you can change an association to read only and
  # then use CI to find the areas you need to migrate to an entrypoint. Once you've fully migrated,
  # the read only association now serves to guard against future regressions.
  #
  module ReadOnlyAssociations
    extend ActiveSupport::Concern

    ReadOnlyError = Class.new(StandardError)

    module ReadOnlyAssociationExtension
      private

      def raise_on_mutating_methods!(...)
        raise ReadOnlyError, "Mutating methods are disabled for read-only associations"
      end

      [:build, :create, :create!, :<<, :push, :concat, :delete, :delete_all, :destroy, :destroy_all].each do |method|
        alias_method(method, :raise_on_mutating_methods!)
        public method
      end
    end

    module ClassMethods
      def readonly_has_many(name, scope = nil, **options, &extension)
        readonly_scope = build_readonly_scope(scope)
        extend_option = [*options[:extend], ReadOnlyAssociationExtension]
        has_many(name, readonly_scope, **options.merge(extend: extend_option, autosave: false), &extension)

        mutating_methods = mutating_collection_methods(name)
        raise_on_mutating_methods!(mutating_methods)
      end

      def readonly_has_one(name, scope = nil, **options)
        has_one(name, scope, **options.merge(autosave: false))

        define_readonly_decorator(name)

        mutating_methods = mutating_singular_methods(name)
        raise_on_mutating_methods!(mutating_methods)
      end

      def readonly_belongs_to(name, scope = nil, **options)
        belongs_to(name, scope, **options.merge(autosave: false))

        define_readonly_decorator(name)

        mutating_methods = mutating_singular_methods(name, allow_assignment: true)
        raise_on_mutating_methods!(mutating_methods)
      end

      private

      def inherited(subclass)
        super
        subclass.class_eval do
          @generated_decorator = nil
        end
      end

      def define_readonly_decorator(name, clone: false)
        @generated_decorator ||= Module.new.tap(&method(:prepend))

        if clone
          # Clone and cache the record to prevent side effects
          @generated_decorator.module_eval(<<~RUBY, __FILE__, __LINE__ + 1)
            def #{name}
              @_readonly_#{name} ||= begin
                if (record = super()) && !record.readonly?
                  record = record.clone
                  record.readonly!
                end
                record
              end
            end
          RUBY
        else
          @generated_decorator.send(:define_method, name) do
            if (record = super())
              record.readonly!
            end
            record
          end
        end
      end

      def build_readonly_scope(scope)
        if scope && scope.arity > 0
          ->(*args) {
            #: self as ActiveRecord::Relation
            readonly.instance_exec(*args, &scope)
          }
        elsif scope
          -> {
            #: self as ActiveRecord::Relation
            readonly.instance_exec(&scope)
          }
        else
          -> {
            #: self as ActiveRecord::Relation
            readonly
          }
        end
      end

      def raise_on_mutating_methods!(mutating_methods)
        mutating_methods.each do |method|
          alias_method(method, :raise_on_mutating_methods!)
          public(method)
        end
      end

      def mutating_collection_methods(name)
        ["#{name}=", "#{name.to_s.singularize}_ids="]
      end

      def mutating_singular_methods(name, allow_assignment: false)
        methods = ["build_#{name}", "create_#{name}", "create_#{name}!"]
        methods << "#{name}=" unless allow_assignment
        methods
      end
    end

    private

    def raise_on_mutating_methods!(...)
      raise ReadOnlyError, "Mutating methods are disabled for read-only associations"
    end
  end
end
"###;

const BATCH: &str = r#"class Batch < ApplicationRecord
  readonly_has_many :events, -> { order("id DESC") }, as: "subject", inverse_of: :subject, primary_key: :id, foreign_key: :subject_id

  readonly_has_one :timeline_comment,
    -> { where(verb: "comment") },
    class_name: "Event",
    as: "subject",
    inverse_of: "subject", primary_key: :id, foreign_key: :subject_id

  readonly_has_many :stamps,
    class_name: "Stamp",
    through: :events,
    source: :stamps
end
"#;

const EVENT: &str = r#"class Event < ApplicationRecord
  belongs_to :subject, polymorphic: true
  has_many :stamps
end
"#;

const STAMP: &str = r#"class Stamp < ApplicationRecord
  belongs_to :event
end
"#;

const PROBE: &str = r##"class ReadonlyHasAssociationsProbe
  def self.lines
    out = []
    b1 = Batch.create!(name: "one")
    b2 = Batch.create!(name: "two")
    Event.create!(subject: b1, verb: "created")
    Event.create!(subject: b1, verb: "comment")
    Event.create!(subject: b1, verb: "closed")
    Event.create!(subject_id: b1.id, subject_type: "Other", verb: "comment")
    Event.create!(subject: b2, verb: "created")
    r = Batch.reflect_on_association(:timeline_comment)
    out << "reflection: #{r.macro} #{r.class_name} #{r.options[:autosave].inspect} #{r.options[:as].inspect}"
    r = Batch.reflect_on_association(:stamps)
    out << "through reflection: #{r.macro} #{r.class_name} #{r.options[:through].inspect} #{r.options[:source].inspect} #{r.options[:autosave].inspect}"
    r = Batch.reflect_on_association(:events)
    out << "events reflection: #{r.macro} #{r.collection?} #{r.options[:autosave].inspect} #{r.options[:extend].inspect}"
    b = Batch.find(b1.id)
    events = b.events.to_a
    out << "events: #{events.map(&:verb).join(",")} #{events.map(&:readonly?).uniq.inspect} #{Batch.find(b2.id).events.map(&:verb).inspect}"
    comment = b.timeline_comment
    out << "has_one: #{comment.verb} #{comment.subject_type} #{comment.readonly?} #{Event.find(comment.id).readonly?}"
    out << "none: #{Batch.find(b2.id).timeline_comment.inspect}"
    begin
      comment.update!(verb: "x")
      out << "save: saved"
    rescue ActiveRecord::ReadOnlyRecord => e
      out << "save: #{e.message} #{Event.find(comment.id).verb}"
    end
    begin
      events.first.destroy
      out << "destroy: destroyed"
    rescue ActiveRecord::ReadOnlyRecord => e
      out << "destroy: #{e.message} #{Event.count}"
    end
    ["build_timeline_comment", "create_timeline_comment", "create_timeline_comment!", "timeline_comment=", "events=", "event_ids="].each do |m|
      begin
        b.public_send(m, m.end_with?("=") ? [] : {})
        out << "#{m}: ran"
      rescue Component::ReadOnlyAssociations::ReadOnlyError => e
        out << "#{m}: #{e.message}"
      end
    end
    out
  end
end
"##;

const EXPECTED: [&str; 14] = [
    "reflection: has_one Event false \"subject\"",
    "through reflection: has_many Stamp :events :stamps false",
    "events reflection: has_many true false [Component::ReadOnlyAssociations::ReadOnlyAssociationExtension]",
    "events: closed,comment,created [true] [\"created\"]",
    "has_one: comment Batch true false",
    "none: nil",
    "save: Event is marked as readonly comment",
    "destroy: Event is marked as readonly 5",
    "build_timeline_comment: Mutating methods are disabled for read-only associations",
    "create_timeline_comment: Mutating methods are disabled for read-only associations",
    "create_timeline_comment!: Mutating methods are disabled for read-only associations",
    "timeline_comment=: Mutating methods are disabled for read-only associations",
    "events=: Mutating methods are disabled for read-only associations",
    "event_ids=: Mutating methods are disabled for read-only associations",
];

/// What native Rails requires, in order.
const LOADS: [&str; 6] = [
    "app/models/concerns/component/read_only_associations",
    "app/models/application_record",
    "app/models/batch",
    "app/models/event",
    "app/models/stamp",
    "app/models/readonly_has_associations_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/concerns/component/read_only_associations.rb", READ_ONLY_ASSOCIATIONS),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/batch.rb", BATCH),
    ("app/models/event.rb", EVENT),
    ("app/models/stamp.rb", STAMP),
    ("app/models/readonly_has_associations_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

const KNOWN_CONCERN_GAPS: [&str; 5] = [
    "a call on a literal list in a class or module body is not carried; Ruby runs it when the body loads: [:build, :create, :create!, :<<, :push, :concat, :delete, :delete_all, :destroy, :destroy_all].each",
    "no known method `module_eval` on Module?",
    "no known method `send` on Module?",
    "no known method `arity` on nil",
    "declared type not supported (all targets): ActiveRecord::Relation has no indexed declaration or modeled dependency",
];

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries and the known concern gaps.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .filter(|e| !KNOWN_CONCERN_GAPS.iter().any(|gap| e.contains(gap)))
        .collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn readonly_has_one_and_has_many_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "readonly-has-associations",
        &FILES,
        &emitted,
        "rh_spec_readonly_has_associations",
        "ReadonlyHasAssociationsProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
