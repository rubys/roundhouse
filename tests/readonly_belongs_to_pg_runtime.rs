//! PostgreSQL: readonly belongs to, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE shops (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);

CREATE TABLE products (
    id bigserial NOT NULL,
    shop_id bigint,
    title character varying,
    CONSTRAINT products_pkey PRIMARY KEY (id)
);

CREATE TABLE variants (
    id bigserial NOT NULL,
    shop_id bigint,
    CONSTRAINT variants_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_readonly_belongs_to
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true

  include Component::ReadOnlyAssociations
  extend AcceleratedBelongsTo
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

const ACCELERATED: &str = r###"module AcceleratedBelongsTo

  def accelerated_belongs_to(record, *args, **kwargs)
    # let rails do its normal belongs to.
    belongs_to_class = self #: as untyped
    belongs_to_class.belongs_to(record, *args, **kwargs)

    reflection = reflect_on_association(record)

    # Not yet adding Array Foreign Key support for Polymorphic.
    # Since all Postgres/Yugabyte Foreign Keys are an array even when only 1 long
    # this may need to be added in the future.
    array_fk = reflection.foreign_key.is_a?(Array)
    uses_array_foreign_keys = array_fk && !kwargs[:polymorphic]

    current_match_condition = if uses_array_foreign_keys
      # Don't call reflection.association_primary_key here — it triggers eager class
      # resolution via compute_type, which fails for associations with string class_name
      # values when the target class hasn't been loaded yet. Instead, resolve PK columns
      # lazily at runtime via _pk_columns (set in current_snippet below).
      fk_columns = Array(reflection.foreign_key)
      fk_columns.map.with_index { |fk, i|
        "#{fk} == current.read_attribute(_pk_columns[#{i}])"
      }.join(" && ")
    else
      "#{reflection.foreign_key} == current.id"
    end

    current_snippet = if kwargs[:polymorphic]
      <<-EOS
        reflection = self.class.reflect_on_association(:#{record})

        klass = reflection.foreign_type&.then do |foreign_type|
          send(foreign_type)&.constantize
        end

        current = klass.current if klass.respond_to?(:current)
      EOS
    elsif uses_array_foreign_keys
      <<-EOS
        _reflection = self.class.reflect_on_association(:#{record})
        current = _reflection.klass.current
        _pk_columns = current ? Array(_reflection.association_primary_key) : nil
      EOS
    else
      <<-EOS
        current = self.class.reflect_on_association(:#{record}).klass.current
      EOS
    end

    class_eval(<<-EOS, -__FILE__, __LINE__ + 1)
      def #{record}(*)
        #{current_snippet}

        if current && #{current_match_condition}
          current
        else
          super
        end
      end
    EOS
  end

  def accelerated_readonly_belongs_to(name, scope = nil, **options)
    accelerated_belongs_to(name, scope, **options.merge(autosave: false))

    alias_method(:"mutable_#{name}", name)

    class_eval(<<~RUBY, __FILE__, __LINE__ + 1)
      def #{name}(...)
        @_readonly_#{name} ||= begin
          if (record = mutable_#{name}(...))
            record = record.clone
            record.readonly!
          end
          record
        end
      end
    RUBY

    mutating_methods = mutating_singular_methods(name, allow_assignment: true)
    raise_on_mutating_methods!(mutating_methods)
  end
end
"###;

const SHOP: &str = r#"class Shop < ApplicationRecord
  class << self
    attr_accessor :current
  end
end
"#;

const PRODUCT: &str = r#"class Product < ApplicationRecord
  readonly_belongs_to :shop
end
"#;

const VARIANT: &str = r#"class Variant < ApplicationRecord
  accelerated_readonly_belongs_to :shop
end
"#;

const PROBE: &str = r##"class ReadonlyBelongsToProbe
  def self.lines
    out = []
    s1 = Shop.create!(name: "one")
    p1 = Product.create!(shop: s1, title: "a")
    v1 = Variant.create!(shop: s1)
    queries = 0
    ActiveSupport::Notifications.subscribe("sql.active_record") { |*| queries += 1 }
    counted = lambda do |&block|
      queries = 0
      value = block.call
      [value, queries]
    end
    p = Product.find(p1.id)
    shop = p.shop
    out << "readonly: #{shop.name} #{shop.readonly?} #{p.shop.equal?(shop)} #{Shop.find(s1.id).readonly?} #{s1.readonly?}"
    begin
      shop.update!(name: "x")
      out << "save: saved"
    rescue ActiveRecord::ReadOnlyRecord => e
      out << "save: #{e.class} #{e.message} #{Shop.find(s1.id).name}"
    end
    begin
      shop.destroy
      out << "destroy: destroyed"
    rescue ActiveRecord::ReadOnlyRecord => e
      out << "destroy: #{e.message} #{Shop.count}"
    end
    begin
      p.build_shop(name: "b")
      out << "build: built"
    rescue Component::ReadOnlyAssociations::ReadOnlyError => e
      out << "build: #{e.message}"
    end
    p2 = Product.new(title: "n")
    p2.shop = Shop.new(name: "new")
    out << "assign new: #{p2.save} #{p2.shop_id.inspect} #{Shop.count} #{Product.find(p2.id).shop_id.inspect}"
    Shop.current = s1
    v = Variant.find(v1.id)
    shop, n = counted.call { v.shop }
    out << "accelerated readonly: #{shop.name} #{shop.readonly?} #{shop.equal?(s1)} #{s1.readonly?} #{v.shop.equal?(shop)} #{v.mutable_shop.equal?(s1)} #{n}"
    Shop.current = nil
    v = Variant.find(v1.id)
    shop, n = counted.call { v.shop }
    out << "accelerated readonly no current: #{shop.name} #{shop.readonly?} #{n}"
    begin
      v.create_shop!(name: "c")
      out << "create!: created"
    rescue Component::ReadOnlyAssociations::ReadOnlyError => e
      out << "create!: #{e.message} #{Shop.count}"
    end
    out
  end
end
"##;

const EXPECTED: [&str; 8] = [
    "readonly: one true true false false",
    "save: ActiveRecord::ReadOnlyRecord Shop is marked as readonly one",
    "destroy: Shop is marked as readonly 1",
    "build: Mutating methods are disabled for read-only associations",
    "assign new: true nil 1 nil",
    "accelerated readonly: one true false false true true 0",
    "accelerated readonly no current: one true 1",
    "create!: Mutating methods are disabled for read-only associations 1",
];

/// What native Rails requires, in order.
const LOADS: [&str; 7] = [
    "app/models/concerns/component/read_only_associations",
    "app/models/concerns/accelerated_belongs_to",
    "app/models/application_record",
    "app/models/shop",
    "app/models/product",
    "app/models/variant",
    "app/models/readonly_belongs_to_probe",
];

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/concerns/component/read_only_associations.rb", READ_ONLY_ASSOCIATIONS),
    ("app/models/concerns/accelerated_belongs_to.rb", ACCELERATED),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/product.rb", PRODUCT),
    ("app/models/variant.rb", VARIANT),
    ("app/models/readonly_belongs_to_probe.rb", PROBE),
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
fn readonly_belongs_to_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "readonly-belongs-to",
        &FILES,
        &emitted,
        "rh_spec_readonly_belongs_to",
        "ReadonlyBelongsToProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
