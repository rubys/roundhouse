//! PostgreSQL: relation unscope lock, on the Ruby target against native Rails.
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

CREATE TABLE tasks (
    id bigserial NOT NULL,
    shop_id bigint,
    title character varying,
    is_not_deleted boolean,
    CONSTRAINT tasks_pkey PRIMARY KEY (id)
);

CREATE TABLE items (
    id bigserial NOT NULL,
    task_id bigint,
    label character varying,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_relation_unscope_lock
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :tasks
  has_many :items, through: :tasks
end
"#;

const TASK: &str = r#"class Task < ApplicationRecord
  belongs_to :shop
  has_many :items

  default_scope { where(is_not_deleted: true) }

  scope :ignoring_soft_deletion, -> { unscope(where: :is_not_deleted) }
  scope :unordered, -> { unscope(:order) }
end
"#;

const ITEM: &str = r#"class Item < ApplicationRecord
  belongs_to :task
end
"#;

const REFLECTION_PATCH: &str = r#"module ReflectionWritableForeignKey
  def writable_foreign_key
    return @writable_foreign_key if defined?(@writable_foreign_key)

    case foreign_key
    when String then [foreign_key.to_sym]
    when Symbol then [foreign_key]
    when Array then foreign_key.map(&:to_sym)
    else []
    end
  end

  attr_writer :writable_foreign_key
end

ActiveSupport.on_load(:active_record) do
  ActiveRecord::Reflection::AssociationReflection.prepend(ReflectionWritableForeignKey)
  ActiveRecord::Reflection::ThroughReflection.prepend(ReflectionWritableForeignKey)
end
"#;

const PROBE: &str = r##"class RelationApiProbe
  def self.sql(relation)
    # The relation parenthesizes each condition; Rails writes a bare conjunction.
    relation.to_sql.gsub('"', "").delete("()")
  end

  def self.lines
    out = []
    shop = Shop.create!(name: "s")
    live = Task.create!(shop_id: shop.id, title: "live", is_not_deleted: true)
    gone = Task.create!(shop_id: shop.id, title: "gone", is_not_deleted: false)
    Item.create!(task_id: live.id, label: "i")

    out << "default scope: #{Task.pluck(:title).sort.inspect}"
    out << "unscope where: #{Task.unscope(where: :is_not_deleted).pluck(:title).sort.inspect}"
    out << "unscope where string: #{Task.unscope(where: "tasks.is_not_deleted").pluck(:title).sort.inspect}"
    out << "scope keeps the rest: #{Task.where(title: "gone").ignoring_soft_deletion.pluck(:title).inspect} #{Task.where(title: "live").ignoring_soft_deletion.pluck(:title).inspect}"
    out << "unscope one of two: #{sql(Task.where(title: "x", is_not_deleted: false).unscope(where: :title))}"
    out << "unscope not: #{sql(Task.unscope(where: :is_not_deleted).where.not(title: "x").unscope(where: :title))}"
    out << "unscope other column kept: #{sql(Task.unscope(where: :title))}"
    out << "unscope symbols: #{sql(Task.order(:title).limit(1).offset(1).unscope(:order, :limit, :offset))}"
    out << "unscope where all: #{sql(Task.unscope(:where))}"
    out << "unscope left_joins: #{sql(Task.left_joins(:shop).unscope(:left_joins))}"
    out << "unscope hash array: #{sql(Task.where(title: "a", shop_id: 1).unscope(where: [:title, :shop_id]))}"
    out << "unscope invalid: #{(Task.unscope(:bogus) rescue $!.message)}"
    out << "unscope hash key: #{(Task.unscope(order: :title) rescue $!.message)}"
    out << "unscope other: #{(Task.unscope(1) rescue $!.message)}"
    out << "unscope returns relation: #{Task.unscope(:order).class.name}"

    out << "lock: #{sql(Task.lock)}"
    out << "lock string: #{sql(Task.lock("FOR SHARE"))}"
    out << "lock false: #{sql(Task.lock.lock(false))}"
    out << "lock after limit: #{sql(Task.where(title: "live").limit(1).lock)}"
    out << "unscope lock: #{sql(Task.lock.unscope(:lock))}"
    found = Task.transaction { Task.lock.find_by(title: "live") }
    out << "lock find_by: #{found.title}"
    out << "lock find: #{Task.transaction { Task.lock.find(live.id).title }}"
    out << "lock scoped: #{Task.transaction { Task.where(title: "live").lock.pluck(:title).inspect }}"

    reflection = Task.reflect_on_association(:shop)
    out << "writable fk belongs_to: #{reflection.writable_foreign_key.inspect}"
    out << "writable fk has_many: #{Shop.reflect_on_association(:tasks).writable_foreign_key.inspect}"
    out << "writable fk through: #{Shop.reflect_on_association(:items).writable_foreign_key.inspect}"
    reflection.writable_foreign_key = [:other_id]
    out << "writable fk set: #{Task.reflect_on_association(:shop).writable_foreign_key.inspect} #{Task.reflect_on_association(:shop).foreign_key.inspect}"
    out
  end
end
"##;

const NATIVE_BOOT: &str = r#"require_relative "lib/patches/active_record/reflection_writable_foreign_key"
"#;

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("config/application.rb", "Bundler.require(*Rails.groups)\n"),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("lib/patches/active_record/reflection_writable_foreign_key.rb", REFLECTION_PATCH),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/task.rb", TASK),
    ("app/models/item.rb", ITEM),
    ("app/models/relation_api_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn unscope_lock_and_a_reflection_patch_answer_as_natively_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let mut files = FILES.to_vec();
    files.push(("native_boot.rb", NATIVE_BOOT));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "relation-unscope-lock",
        &files,
        &emitted,
        "rh_spec_relation_unscope_lock",
        "RelationApiProbe",
        &[
            "native_boot",
            "app/models/application_record",
            "app/models/shop",
            "app/models/task",
            "app/models/item",
            "app/models/relation_api_probe",
        ],
    ) else { return };
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
