//! PostgreSQL: macro replay visibility and reopens, on the Ruby target against native Rails.
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
    owner_id bigint,
    title character varying,
    CONSTRAINT products_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_macro_replay_order
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
  extend AcceleratedBelongsTo
end
"#;

const ACCELERATED: &str = r##"module AcceleratedBelongsTo
  def accelerated_belongs_to(record, *args, **kwargs)
    belongs_to_class = self #: as untyped
    belongs_to_class.belongs_to(record, *args, **kwargs)
    reflection = reflect_on_association(record)
    current_match_condition = "#{reflection.foreign_key} == current.id"
    class_eval(<<-EOS, -__FILE__, __LINE__ + 1)
      def #{record}(*)
        current = self.class.reflect_on_association(:#{record}).klass.current
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
  end
end
"##;

/// MetadataAccessors' shape: a concern class method that records the
/// name and defines a writer and a reader, plus an association macro.
const META: &str = r##"module MetaAccessors
  extend ActiveSupport::Concern

  def write_meta(key, value)
    @meta ||= {}
    @meta[key.to_s] = value
  end

  def read_meta(key, default = nil)
    return default if @meta.nil?

    @meta.fetch(key.to_s, default)
  end

  class_methods do
    def meta_accessor(method_name, default: nil)
      @meta_names ||= []
      @meta_names << method_name.to_s
      define_method("#{method_name}=") { |val| write_meta(method_name, val) }
      define_method(method_name) { read_meta(method_name, default) }
    end

    def meta_names
      @meta_names.to_a
    end

    def fast_belongs_to(name, scope = nil, **options)
      belongs_to(name, scope, **options)
    end
  end
end
"##;

const SHOP: &str = r#"class Shop < ApplicationRecord
  class << self
    attr_accessor :current
  end
end
"#;

/// The wrapper sorts before product.rb, as shipping_line/common.rb does.
const PRODUCT_COMMON: &str = r##"class Product < ApplicationRecord
  module Common
    def common_title = "c:#{title}"
  end
end
"##;

const PRODUCT: &str = r##"class Product < ApplicationRecord
  include Product::Common
  include MetaAccessors

  accelerated_readonly_belongs_to :shop, primary_key: :id, foreign_key: :shop_id, inverse_of: false
  meta_accessor :secret_token
  fast_belongs_to :owner, class_name: "Shop", foreign_key: :owner_id, optional: true

  def assign_secret(value)
    self.secret_token = value
  end

  def shown_secret = "secret=#{secret_token}"

  def described = "#{title}/#{shop.name}"

  private :secret_token=
end
"##;

const PROBE: &str = r##"class MacroReplayOrderProbe
  def self.lines
    out = []
    s1 = Shop.create!(name: "one")
    s2 = Shop.create!(name: "two")
    p1 = Product.create!(shop: s1, owner: s2, title: "a")
    p1 = Product.find(p1.id)
    out << "assoc: #{p1.shop.name} #{p1.owner.name} #{p1.described}"
    Shop.current = s1
    out << "current: #{Product.find(p1.id).shop.equal?(s1)}"
    Shop.current = nil
    out << "private: #{p1.respond_to?(:secret_token=)} #{Product.private_method_defined?(:secret_token=)} #{p1.respond_to?(:secret_token)}"
    begin
      p1.secret_token = "x"
      out << "public call: allowed"
    rescue NoMethodError
      out << "public call: NoMethodError"
    end
    p1.assign_secret("tok")
    out << "#{p1.shown_secret} #{Product.meta_names.inspect}"
    out << "common: #{p1.common_title}"
    out << "reflection: #{Product.reflect_on_association(:owner).foreign_key} #{Product.reflect_on_association(:owner).klass.name} #{Product.reflect_on_association(:shop).foreign_key}"
    out << "owner set: #{Product.create!(shop: s2, title: "b").tap { |p| p.update!(owner: s1) }.reload.owner.name}"
    out
  end
end
"##;

const EXPECTED: [&str; 8] = [
    "assoc: one two a/one",
    "current: true",
    "private: false true true",
    "public call: NoMethodError",
    "secret=tok [\"secret_token\"]",
    "common: c:a",
    "reflection: owner_id Shop shop_id",
    "owner set: one",
];

/// What native Rails requires, in order.
const LOADS: [&str; 7] = [
    "app/models/concerns/accelerated_belongs_to",
    "app/models/concerns/meta_accessors",
    "app/models/application_record",
    "app/models/shop",
    "app/models/product/common",
    "app/models/product",
    "app/models/macro_replay_order_probe",
];

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/concerns/accelerated_belongs_to.rb", ACCELERATED),
    ("app/models/concerns/meta_accessors.rb", META),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/product/common.rb", PRODUCT_COMMON),
    ("app/models/product.rb", PRODUCT),
    ("app/models/macro_replay_order_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn emit(app: emit_and_run::Overlay) -> (emit_and_run::Emitted, Vec<String>) {
    roundhouse::ingest::survey::activate();
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    let gaps: Vec<String> = roundhouse::ingest::survey::drain().iter().map(ToString::to_string).collect();
    assert!(
        matches!(gaps.as_slice(), [gap] if gap.contains("model macro `meta_accessor` not expanded")),
        "{gaps:#?}"
    );
    (emitted, errors)
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .collect()
}

/// The 0-based line of the one line in `text` equal to `line` (trimmed).
fn line_of(text: &str, line: &str) -> usize {
    let hits: Vec<usize> =
        text.lines().enumerate().filter(|(_, l)| l.trim() == line).map(|(i, _)| i).collect();
    assert_eq!(hits.len(), 1, "expected one {line:?}, found {hits:?}:\n{text}");
    hits[0]
}

/// The emitted class body replays the macros in source order, with the
/// named visibility after the macro defining its method; the wrapper's
/// reopen does not replace the model's own declarations.
fn assert_product_body(product: &str) {
    let accelerated = line_of(
        product,
        "accelerated_readonly_belongs_to :shop, primary_key: :id, foreign_key: :shop_id, inverse_of: false",
    );
    let accessor = line_of(product, "meta_accessor :secret_token");
    let fast = line_of(
        product,
        "fast_belongs_to :owner, class_name: \"Shop\", foreign_key: :owner_id, optional: true",
    );
    let private = line_of(product, "private :secret_token=");
    assert!(accelerated < accessor && accessor < fast && fast < private, "source order:\n{product}");
    // Both macro-declared associations are the compiled class's, which its
    // class-side `belongs_to` answers from.
    assert!(product.contains("def self.reflect_on_association(association)"), "{product}");
    for reflection in [
        "shop: ::ActiveRecord::Reflection::BelongsToReflection.new(:shop, \"shop_id\", \"Shop\"",
        "owner: ::ActiveRecord::Reflection::BelongsToReflection.new(:owner, \"owner_id\", \"Shop\"",
    ] {
        assert!(product.contains(reflection), "lacks {reflection:?}:\n{product}");
    }
    // The wrapper's module is included, and its method is still defined.
    assert!(product.contains("include Product::Common") || product.contains("include ::Product::Common"), "{product}");
    assert!(line_of(product, "def described") < private, "{product}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn visibility_follows_the_replayed_macro_and_wrappers_reopen_the_model() {
    let (emitted, errors) = emit(overlay());
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let product = std::fs::read_to_string(emitted.join("app/models/product.rb")).expect("product.rb");
    assert_product_body(&product);
}

/// A reopen that sorts after the model's own file (here under a sibling
/// directory) is merged the same way: the declaration in product.rb stays
/// the model, its associations and replayed macros intact, and the
/// reopen's method joins it.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_reopen_sorting_after_the_model_file_also_merges_into_it() {
    let reopen = "class Product < ApplicationRecord\n  def reopened = \"r:#{title}\"\nend\n";
    let (emitted, errors) = emit(overlay().write("app/models/products/reopen.rb", reopen));
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let product = std::fs::read_to_string(emitted.join("app/models/product.rb")).expect("product.rb");
    assert_product_body(&product);
    assert!(product.contains("def reopened"), "{product}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn macro_replay_order_and_wrapped_associations_match_native_rails_on_postgresql() {
    let (emitted, errors) = emit(overlay());
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "macro-replay-order",
        &FILES,
        &emitted,
        "rh_spec_macro_replay_order",
        "MacroReplayOrderProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
