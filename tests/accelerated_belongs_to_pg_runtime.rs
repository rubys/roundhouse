//! PostgreSQL: accelerated belongs to, on the Ruby target against native Rails.
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

CREATE TABLE notes (
    id bigserial NOT NULL,
    owner_type character varying,
    owner_id bigint,
    CONSTRAINT notes_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_accelerated_belongs_to
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
  extend AcceleratedBelongsTo
end
"#;

const CONCERN: &str = r##"module AcceleratedBelongsTo

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
end
"##;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :notes, as: :owner

  class << self
    attr_accessor :current
  end
end
"#;

const PRODUCT: &str = r#"class Product < ApplicationRecord
  accelerated_belongs_to :shop
  accelerated_belongs_to :my_shop, class_name: "Shop", foreign_key: :shop_id
  has_many :notes, as: :owner
end
"#;

/// A polymorphic one: the reader asks the record's type for `current`.
const NOTE: &str = r#"class Note < ApplicationRecord
  accelerated_belongs_to :owner, polymorphic: true, optional: true
end
"#;

const PROBE: &str = r##"class AcceleratedBelongsToProbe
  def self.lines
    out = []
    s1 = Shop.create!(name: "one")
    s2 = Shop.create!(name: "two")
    p1 = Product.create!(shop: s1, title: "a")
    p2 = Product.create!(shop_id: s2.id, title: "b")
    queries = 0
    ActiveSupport::Notifications.subscribe("sql.active_record") { |*| queries += 1 }
    counted = lambda do |&block|
      queries = 0
      value = block.call
      [value, queries]
    end
    a = Product.find(p1.id)
    b = Product.find(p2.id)
    Shop.current = nil
    shop, n = counted.call { a.shop }
    out << "no current: #{shop.name} #{n}"
    Shop.current = s1
    a = Product.find(p1.id)
    shop, n = counted.call { a.shop }
    out << "current matches: #{shop.name} #{shop.equal?(s1)} #{n}"
    shop, n = counted.call { b.shop }
    out << "current differs: #{shop.name} #{shop.equal?(s1)} #{n}"
    a = Product.find(p1.id)
    shop, n = counted.call { a.my_shop }
    out << "named: #{shop.name} #{shop.equal?(s1)} #{n}"
    n1 = Note.create!(owner: s1)
    n2 = Note.create!(owner: p1)
    n3 = Note.create!
    n1 = Note.find(n1.id)
    owner, n = counted.call { n1.owner }
    out << "polymorphic current: #{owner.name} #{owner.equal?(s1)} #{n}"
    n2 = Note.find(n2.id)
    owner, n = counted.call { n2.owner }
    out << "polymorphic other: #{owner.title} #{n} #{Note.find(n3.id).owner.inspect}"
    Shop.current = nil
    out << "polymorphic reflection: #{Note.reflect_on_association(:owner).foreign_type} #{Note.reflect_on_association(:owner).polymorphic?}"
    out << "reflection: #{Product.reflect_on_association(:my_shop).foreign_key} #{Product.reflect_on_association(:shop).klass.name}"
    out
  end
end
"##;

const EXPECTED: [&str; 8] = [
    "no current: one 1",
    "current matches: one true 0",
    "current differs: two false 1",
    "named: one true 0",
    "polymorphic current: one true 0",
    "polymorphic other: a 1 nil",
    "polymorphic reflection: owner_type true",
    "reflection: shop_id Shop",
];

/// What native Rails requires, in order.
const LOADS: [&str; 6] = [
    "app/models/concerns/accelerated_belongs_to",
    "app/models/application_record",
    "app/models/shop",
    "app/models/product",
    "app/models/note",
    "app/models/accelerated_belongs_to_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/concerns/accelerated_belongs_to.rb", CONCERN),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/product.rb", PRODUCT),
    ("app/models/note.rb", NOTE),
    ("app/models/accelerated_belongs_to_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn accelerated_belongs_to_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "accelerated-belongs-to",
        &FILES,
        &emitted,
        "rh_spec_accelerated_belongs_to",
        "AcceleratedBelongsToProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
